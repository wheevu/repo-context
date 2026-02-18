//! Export command implementation

use anyhow::Result;
use clap::Args;
use serde_json::json;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

use super::utils::parse_csv;
use crate::chunk::{chunk_content, coalesce_small_chunks_with_max};
use crate::config::{load_config, merge_cli_with_config, CliOverrides};
use crate::domain::{Chunk, OutputMode, RedactionMode};
use crate::fetch::fetch_repository;
use crate::rank::rank_files_with_manifest;
use crate::redact::Redactor;
use crate::render::{render_context_pack, render_jsonl, write_report};
use crate::scan::scanner::FileScanner;
use crate::scan::tree::generate_tree;
use crate::utils::read_file_safe;

#[derive(Args)]
pub struct ExportArgs {
    /// Local directory path to export
    #[arg(short, long, value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// GitHub repository URL to clone and export
    #[arg(short = 'r', long, value_name = "URL")]
    pub repo: Option<String>,

    /// Git ref (branch/tag/SHA) when using --repo
    #[arg(long, value_name = "REF")]
    pub ref_: Option<String>,

    /// Path to config file (repo-to-prompt.toml or .r2p.yml)
    #[arg(short = 'c', long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Include only these extensions (comma-separated, e.g., '.py,.ts')
    #[arg(short = 'i', long, value_name = "EXTS")]
    pub include_ext: Option<String>,

    /// Exclude paths matching these globs (comma-separated)
    #[arg(short = 'e', long, value_name = "GLOBS")]
    pub exclude_glob: Option<String>,

    /// Skip files larger than this (bytes)
    #[arg(long, value_name = "BYTES")]
    pub max_file_bytes: Option<u64>,

    /// Stop after exporting this many bytes total
    #[arg(long, value_name = "BYTES")]
    pub max_total_bytes: Option<u64>,

    /// Ignore .gitignore rules
    #[arg(long)]
    pub no_gitignore: bool,

    /// Follow symbolic links when scanning
    #[arg(long)]
    pub follow_symlinks: bool,

    /// Include minified/bundled files
    #[arg(long)]
    pub include_minified: bool,

    /// Maximum tokens in output
    #[arg(short = 't', long, value_name = "TOKENS")]
    pub max_tokens: Option<usize>,

    /// Target tokens per chunk
    #[arg(long, value_name = "TOKENS")]
    pub chunk_tokens: Option<usize>,

    /// Overlap tokens between adjacent chunks
    #[arg(long, value_name = "TOKENS")]
    pub chunk_overlap: Option<usize>,

    /// Coalesce chunks smaller than this
    #[arg(long, value_name = "TOKENS")]
    pub min_chunk_tokens: Option<usize>,

    /// Output format: 'prompt' (Markdown), 'rag' (JSONL), or 'both'
    #[arg(short = 'm', long, value_name = "MODE")]
    pub mode: Option<String>,

    /// Directory for output files
    #[arg(short = 'o', long, value_name = "DIR")]
    pub output_dir: Option<PathBuf>,

    /// Omit timestamps for reproducible diffs
    #[arg(long)]
    pub no_timestamp: bool,

    /// Max depth for directory tree in output
    #[arg(long, value_name = "DEPTH")]
    pub tree_depth: Option<usize>,

    /// Disable automatic secret/credential redaction
    #[arg(long)]
    pub no_redact: bool,

    /// Redaction mode: fast|standard|paranoid|structure-safe
    #[arg(long, value_name = "MODE")]
    pub redaction_mode: Option<String>,
}

pub fn run(args: ExportArgs) -> Result<()> {
    let start_time = Instant::now();

    if args.path.is_some() && args.repo.is_some() {
        anyhow::bail!("Cannot specify both --path and --repo");
    }

    let cwd = std::env::current_dir()?;
    let config_anchor = match args.path.as_ref() {
        Some(path) => {
            if path.exists() {
                path.canonicalize().unwrap_or_else(|_| cwd.clone())
            } else {
                cwd.clone()
            }
        }
        None => cwd.clone(),
    };

    let file_config = load_config(&config_anchor, args.config.as_deref())?;
    let include_ext = parse_csv(&args.include_ext).map(|v| v.into_iter().collect());
    let exclude_glob = parse_csv(&args.exclude_glob).map(|v| v.into_iter().collect());
    let mode = if args.mode.is_some() { Some(parse_mode(args.mode.as_deref())?) } else { None };
    let redaction_mode = if args.redaction_mode.is_some() {
        Some(parse_redaction_mode(args.redaction_mode.as_deref())?)
    } else {
        None
    };

    let cli_overrides = CliOverrides {
        path: args.path.clone(),
        repo_url: args.repo.clone(),
        ref_: args.ref_.clone(),
        include_extensions: include_ext,
        exclude_globs: exclude_glob,
        max_file_bytes: args.max_file_bytes,
        max_total_bytes: args.max_total_bytes,
        respect_gitignore: if args.no_gitignore { Some(false) } else { None },
        follow_symlinks: if args.follow_symlinks { Some(true) } else { None },
        skip_minified: if args.include_minified { Some(false) } else { None },
        max_tokens: args.max_tokens,
        chunk_tokens: args.chunk_tokens,
        chunk_overlap: args.chunk_overlap,
        min_chunk_tokens: args.min_chunk_tokens,
        mode,
        output_dir: args.output_dir.clone(),
        tree_depth: args.tree_depth,
        redact_secrets: if args.no_redact { Some(false) } else { None },
        redaction_mode,
    };

    let merged = merge_cli_with_config(file_config, cli_overrides);

    if merged.path.is_none() && merged.repo_url.is_none() {
        anyhow::bail!("Either --path or --repo must be specified");
    }

    let repo_ctx = fetch_repository(
        merged.path.as_deref(),
        merged.repo_url.as_deref(),
        merged.ref_.as_deref(),
    )?;
    let root_path = repo_ctx.root_path.clone();

    let mut scanner = FileScanner::new(root_path.clone())
        .max_file_bytes(merged.max_file_bytes)
        .respect_gitignore(merged.respect_gitignore)
        .follow_symlinks(merged.follow_symlinks)
        .skip_minified(merged.skip_minified)
        .include_extensions(merged.include_extensions.iter().cloned().collect())
        .exclude_globs(merged.exclude_globs.iter().cloned().collect());

    let scanned_files = scanner.scan()?;
    let mut stats = scanner.stats().clone();

    let (ranked_files, manifest_info) =
        rank_files_with_manifest(&root_path, scanned_files, merged.ranking_weights.clone())?;
    stats.top_ranked_files = ranked_files
        .iter()
        .take(20)
        .map(|f| {
            std::collections::HashMap::from([
                ("path".to_string(), json!(f.relative_path)),
                ("priority".to_string(), json!(f.priority)),
            ])
        })
        .collect();

    let mut selected_files =
        apply_byte_budget(ranked_files, Some(merged.max_total_bytes), &mut stats);

    let chunk_tokens = merged.chunk_tokens;
    let chunk_overlap = merged.chunk_overlap;
    let redactor = if merged.redact_secrets {
        Some(build_redactor(merged.redaction_mode, &merged.redaction))
    } else {
        None
    };
    let mut chunks: Vec<Chunk> = Vec::new();
    // Track token budget at file granularity (matching Python behaviour).
    let mut total_tokens_so_far: usize = 0;

    for file in &mut selected_files {
        // Read the full file content once.
        let (content, _enc) = match read_file_safe(&file.path, None, None) {
            Ok(r) => r,
            Err(_) => continue,
        };

        // Redact the full content before chunking so multi-line secrets (e.g. PEM keys)
        // that would straddle chunk boundaries are always caught.
        // Skip redaction entirely if this file matches the allowlist.
        let redacted_content = if let Some(ref r) = redactor {
            let filename = file.path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if r.is_file_allowlisted(filename, &file.relative_path) {
                content
            } else {
                use std::collections::{BTreeMap, HashSet};
                let outcome = r.redact_with_language_report(
                    &content,
                    &file.language,
                    &file.extension,
                    filename,
                    &file.relative_path,
                );
                if outcome.content != content {
                    // Update redaction stats at file granularity.
                    let mut rule_file_sets: BTreeMap<String, HashSet<String>> = BTreeMap::new();
                    for (rule, count) in &outcome.counts {
                        *stats.redaction_counts.entry(rule.clone()).or_insert(0) += count;
                        rule_file_sets
                            .entry(rule.clone())
                            .or_default()
                            .insert(file.relative_path.clone());
                    }
                    stats.redacted_files += 1;
                    for (rule, file_set) in rule_file_sets {
                        *stats.redaction_file_counts.entry(rule).or_insert(0) += file_set.len();
                    }
                    outcome.content
                } else {
                    content
                }
            }
        } else {
            content
        };

        let file_chunks = chunk_content(file, &redacted_content, chunk_tokens, chunk_overlap)?;
        let file_tokens: usize = file_chunks.iter().map(|c| c.token_estimate).sum();
        file.token_estimate = file_tokens;

        // Token budget check at file granularity — matches Python cli.py:530-539.
        if let Some(max_tokens) = merged.max_tokens {
            if total_tokens_so_far + file_tokens > max_tokens {
                stats.files_dropped_budget += 1;
                stats.dropped_files.push(std::collections::HashMap::from([
                    ("path".to_string(), json!(file.relative_path)),
                    ("reason".to_string(), json!("token_budget")),
                    ("priority".to_string(), json!((file.priority * 1000.0).round() / 1000.0)),
                    ("tokens".to_string(), json!(file_tokens)),
                ]));
                continue;
            }
        }
        total_tokens_so_far += file_tokens;

        // Tag redacted chunks and update per-chunk redaction stats.
        if redactor.is_some() {
            for mut chunk in file_chunks {
                if chunk.content.contains("[REDACTED") || chunk.content.contains("_REDACTED]") {
                    chunk.tags.insert("redacted".to_string());
                    stats.redacted_chunks += 1;
                }
                chunks.push(chunk);
            }
        } else {
            chunks.extend(file_chunks);
        }
    }

    let min_chunk_tokens = merged.min_chunk_tokens;
    chunks = coalesce_small_chunks_with_max(chunks, min_chunk_tokens, chunk_tokens);

    stats.chunks_created = chunks.len();
    stats.total_tokens_estimated = chunks.iter().map(|c| c.token_estimate).sum();

    let output_dir = resolve_output_dir(&merged.output_dir, &root_path);
    fs::create_dir_all(&output_dir)?;

    let highlight: HashSet<String> = selected_files
        .iter()
        .filter(|f| f.priority >= 0.8)
        .map(|f| f.relative_path.clone())
        .collect();
    let tree = generate_tree(&root_path, merged.tree_depth, true, &highlight)?;

    let context_pack = render_context_pack(
        &root_path,
        &selected_files,
        &chunks,
        &stats,
        &tree,
        &manifest_info,
        !args.no_timestamp,
    );
    let jsonl = render_jsonl(&chunks);

    let mut output_files = Vec::new();
    if matches!(merged.mode, OutputMode::Prompt | OutputMode::Both) {
        let p = output_dir.join("context_pack.md");
        fs::write(&p, context_pack)?;
        output_files.push(p.display().to_string());
    }
    if matches!(merged.mode, OutputMode::Rag | OutputMode::Both) {
        let p = output_dir.join("chunks.jsonl");
        fs::write(&p, jsonl)?;
        output_files.push(p.display().to_string());
    }

    let report_path = output_dir.join("report.json");
    // Record processing time before writing the report so the value is correct in report.json.
    stats.processing_time_seconds = start_time.elapsed().as_secs_f64();

    // Build curated 14-key config dict matching Python's report schema exactly.
    let config_dict = {
        let exclude_globs_val = if merged.exclude_globs.is_empty() {
            serde_json::Value::Null
        } else {
            let mut v: Vec<&String> = merged.exclude_globs.iter().collect();
            v.sort();
            serde_json::to_value(v)?
        };
        let include_extensions_val = if merged.include_extensions.is_empty() {
            serde_json::Value::Null
        } else {
            let mut v: Vec<&String> = merged.include_extensions.iter().collect();
            v.sort();
            serde_json::to_value(v)?
        };
        let path_val = merged
            .path
            .as_ref()
            .map(|p| serde_json::Value::String(p.to_string_lossy().to_string()))
            .unwrap_or(serde_json::Value::Null);
        let mode_val = serde_json::to_value(&merged.mode)?;
        json!({
            "chunk_overlap":        merged.chunk_overlap,
            "chunk_tokens":         merged.chunk_tokens,
            "exclude_globs":        exclude_globs_val,
            "follow_symlinks":      merged.follow_symlinks,
            "include_extensions":   include_extensions_val,
            "max_file_bytes":       merged.max_file_bytes,
            "max_tokens":           merged.max_tokens,
            "max_total_bytes":      merged.max_total_bytes,
            "mode":                 mode_val,
            "path":                 path_val,
            "redact_secrets":       merged.redact_secrets,
            "ref":                  merged.ref_.clone(),
            "repo":                 merged.repo_url.clone(),
            "skip_minified":        merged.skip_minified,
            "tree_depth":           merged.tree_depth,
        })
    };

    write_report(
        &report_path,
        &root_path,
        &stats,
        &selected_files,
        &output_files,
        &config_dict,
        !args.no_timestamp,
    )?;
    output_files.push(report_path.display().to_string());

    // --- Print export summary ---
    println!();
    println!("Export complete!");
    println!();
    println!("Statistics:");
    println!("  Repository:      {}", root_path.display());
    println!("  Files scanned:   {}", stats.files_scanned);
    println!("  Files included:  {}", stats.files_included);

    // Per-category skip breakdown
    let any_skipped = stats.files_skipped_size > 0
        || stats.files_skipped_binary > 0
        || stats.files_skipped_extension > 0
        || stats.files_skipped_gitignore > 0
        || stats.files_skipped_glob > 0;
    if any_skipped {
        println!("  Files skipped:");
        if stats.files_skipped_size > 0 {
            println!("    size limit:  {}", stats.files_skipped_size);
        }
        if stats.files_skipped_binary > 0 {
            println!("    binary:      {}", stats.files_skipped_binary);
        }
        if stats.files_skipped_extension > 0 {
            println!("    extension:   {}", stats.files_skipped_extension);
        }
        if stats.files_skipped_gitignore > 0 {
            println!("    gitignore:   {}", stats.files_skipped_gitignore);
        }
        if stats.files_skipped_glob > 0 {
            println!("    glob/minify: {}", stats.files_skipped_glob);
        }
    }

    if stats.files_dropped_budget > 0 {
        println!("  Files dropped (budget): {}", stats.files_dropped_budget);
    }

    println!("  Chunks created:  {}", stats.chunks_created);
    println!("  Total bytes:     {}", stats.total_bytes_included);
    println!("  Total tokens:    ~{}", stats.total_tokens_estimated);
    println!("  Processing time: {:.2}s", stats.processing_time_seconds);

    println!();
    println!("Output files:");
    for out in &output_files {
        println!("  {out}");
    }

    // Redaction counts (top 5)
    if !stats.redaction_counts.is_empty() {
        println!();
        println!("Redactions applied:");
        for (name, count) in stats.redaction_counts.iter().take(5) {
            println!("  {name}: {count}");
        }
    }

    // Dropped files list (up to 5)
    if !stats.dropped_files.is_empty() {
        println!();
        println!("Dropped {} file(s) due to budget constraints:", stats.dropped_files.len());
        for df in stats.dropped_files.iter().take(5) {
            let path = df.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            let reason = df.get("reason").and_then(|v| v.as_str()).unwrap_or("?");
            println!("  {path} ({reason})");
        }
        if stats.dropped_files.len() > 5 {
            println!("  ... and {} more (see report.json)", stats.dropped_files.len() - 5);
        }
    }

    Ok(())
}

fn resolve_output_dir(config_output: &Path, root_path: &Path) -> PathBuf {
    let repo_name = root_path.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let normalized = config_output.to_string_lossy().replace('\\', "/");

    let base = if normalized.is_empty() || normalized == "./out" || normalized == "out" {
        PathBuf::from("out")
    } else {
        config_output.to_path_buf()
    };

    // Always namespace by repo name unless the path already ends with it
    // (matches Python's get_repo_output_dir in cli.py:93-109).
    if base.file_name().and_then(|n| n.to_str()) == Some(repo_name) {
        base
    } else {
        base.join(repo_name)
    }
}

fn parse_mode(mode: Option<&str>) -> Result<OutputMode> {
    match mode.unwrap_or("both").to_ascii_lowercase().as_str() {
        "prompt" => Ok(OutputMode::Prompt),
        "rag" => Ok(OutputMode::Rag),
        "both" => Ok(OutputMode::Both),
        invalid => anyhow::bail!("Invalid mode '{invalid}'. Use: prompt|rag|both"),
    }
}

fn parse_redaction_mode(mode: Option<&str>) -> Result<RedactionMode> {
    match mode.unwrap_or("standard").to_ascii_lowercase().as_str() {
        "fast" => Ok(RedactionMode::Fast),
        "standard" => Ok(RedactionMode::Standard),
        "paranoid" => Ok(RedactionMode::Paranoid),
        "structure-safe" | "structure_safe" | "structuresafe" => Ok(RedactionMode::StructureSafe),
        invalid => anyhow::bail!(
            "Invalid redaction mode '{invalid}'. Use: fast|standard|paranoid|structure-safe"
        ),
    }
}

fn build_redactor(mode: RedactionMode, cfg: &crate::domain::RedactionConfig) -> Redactor {
    match mode {
        RedactionMode::Fast => Redactor::from_config(false, false, false, cfg),
        RedactionMode::Standard => Redactor::from_config(true, false, false, cfg),
        RedactionMode::Paranoid => Redactor::from_config(true, true, false, cfg),
        RedactionMode::StructureSafe => Redactor::from_config(true, false, true, cfg),
    }
}

fn apply_byte_budget(
    ranked_files: Vec<crate::domain::FileInfo>,
    max_total_bytes: Option<u64>,
    stats: &mut crate::domain::ScanStats,
) -> Vec<crate::domain::FileInfo> {
    let Some(limit) = max_total_bytes else {
        return ranked_files;
    };

    let mut selected = Vec::new();
    let mut total = 0_u64;
    for (idx, file) in ranked_files.iter().enumerate() {
        // Python checks >= BEFORE adding the current file (cumulative of already-accepted bytes)
        if total >= limit {
            // Bulk-drop this file and all remaining files
            for remaining in &ranked_files[idx..] {
                stats.files_dropped_budget += 1;
                stats.dropped_files.push(std::collections::HashMap::from([
                    ("path".to_string(), json!(remaining.relative_path)),
                    ("reason".to_string(), json!("bytes_limit")),
                    ("priority".to_string(), json!(remaining.priority)),
                ]));
            }
            break;
        }
        total += file.size_bytes;
        selected.push(file.clone());
    }
    stats.total_bytes_included = total;
    selected
}

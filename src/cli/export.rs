//! Export command implementation.

use anyhow::{Context, Result};
use clap::Args;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::utils::parse_csv;
use crate::chunk::{chunk_content, coalesce_small_chunks_with_max};
use crate::config::{load_config, merge_cli_with_config, CliOverrides};
use crate::domain::{Chunk, FileInfo, OutputMode, RedactionMode, ScanStats};
use crate::fetch::fetch_repository;
use crate::rank::rank_files_with_manifest;
use crate::redact::Redactor;
use crate::render::{render_context_pack, render_jsonl, write_report, ReportOptions};
use crate::scan::scanner::FileScanner;
use crate::scan::tree::generate_tree;
use crate::utils::{estimate_tokens, read_file_safe};

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

    /// Path to config file (repo-context.toml or .r2p.yml)
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

    /// Output format: 'prompt', 'rag', or 'both'
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
    if args.path.is_some() && args.repo.is_some() {
        anyhow::bail!("Cannot specify both --path and --repo");
    }

    let cwd = std::env::current_dir()?;
    let config_anchor = match args.path.as_ref() {
        Some(path) if path.exists() => path.canonicalize().unwrap_or_else(|_| cwd.clone()),
        _ => cwd.clone(),
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
        ..CliOverrides::default()
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
    let selected_files = apply_file_byte_budget(ranked_files, merged.max_total_bytes, &mut stats);

    let effective_mode = merged.mode;

    let redactor = if merged.redact_secrets {
        Some(build_redactor(merged.redaction_mode, &merged.redaction))
    } else {
        None
    };

    let mut all_chunks = Vec::new();
    let mut file_tokens: HashMap<String, usize> = HashMap::new();
    for file in &selected_files {
        let (content, _) = read_file_safe(&file.path, None, None)
            .with_context(|| format!("Failed to read {}", file.relative_path))?;

        let raw_chunks = chunk_content(file, &content, merged.chunk_tokens, merged.chunk_overlap)?;
        let mut chunks = coalesce_small_chunks_with_max(
            raw_chunks,
            merged.min_chunk_tokens,
            merged.chunk_tokens,
        );

        let mut file_redacted = false;
        let mut file_token_total = 0usize;
        let file_name =
            Path::new(&file.relative_path).file_name().and_then(|name| name.to_str()).unwrap_or("");
        let file_allowlisted = redactor
            .as_ref()
            .map(|redactor| redactor.is_file_allowlisted(file_name, &file.relative_path))
            .unwrap_or(false);

        for chunk in &mut chunks {
            if let Some(redactor) = &redactor {
                if file_allowlisted {
                    chunk.token_estimate = estimate_tokens(&chunk.content);
                    file_token_total += chunk.token_estimate;
                    continue;
                }

                let outcome = redactor.redact_with_language_report(
                    &chunk.content,
                    &chunk.language,
                    &file.extension,
                    &file.relative_path,
                    &file.relative_path,
                );

                if outcome.content != chunk.content {
                    stats.redacted_chunks += 1;
                    file_redacted = true;
                }

                for (rule, count) in outcome.counts {
                    *stats.redaction_counts.entry(rule).or_insert(0) += count;
                }

                chunk.content = outcome.content;
            }

            chunk.token_estimate = estimate_tokens(&chunk.content);
            file_token_total += chunk.token_estimate;
        }

        if file_redacted {
            stats.redacted_files += 1;
        }

        file_tokens.insert(file.relative_path.clone(), file_token_total);
        all_chunks.extend(chunks);
    }

    let chunks = apply_chunk_token_budget(all_chunks, merged.max_tokens);

    stats.files_included = selected_files.len();
    stats.chunks_created = chunks.len();
    stats.total_tokens_estimated = chunks.iter().map(|c| c.token_estimate).sum();

    let highlights: HashSet<String> =
        selected_files.iter().take(10).map(|f| f.relative_path.clone()).collect();
    let tree = generate_tree(&root_path, merged.tree_depth, true, &highlights)?;

    let output_dir = resolve_output_dir(&merged.output_dir, &root_path, merged.repo_url.as_deref());
    fs::create_dir_all(&output_dir)?;

    let repo_name = repo_name_for_output(&root_path, merged.repo_url.as_deref());
    let context_path = output_dir.join(format!("{}_context_pack.md", repo_name));
    let jsonl_path = output_dir.join(format!("{}_chunks.jsonl", repo_name));
    let report_path = output_dir.join(format!("{}_report.json", repo_name));

    let mut output_files = Vec::new();

    if matches!(effective_mode, OutputMode::Prompt | OutputMode::Both) {
        let content = render_context_pack(
            &root_path,
            &selected_files,
            &chunks,
            &stats,
            &tree,
            &manifest_info,
            merged.task_query.as_deref(),
            !args.no_timestamp,
        );
        fs::write(&context_path, content)?;
        output_files.push(context_path.display().to_string());
    }

    if matches!(effective_mode, OutputMode::Rag | OutputMode::Both) {
        let jsonl = render_jsonl(&chunks);
        fs::write(&jsonl_path, jsonl)?;
        output_files.push(jsonl_path.display().to_string());
    }

    let config_json = build_config_json(&merged);
    let provenance = json!({
        "path": root_path.display().to_string(),
        "repo": merged.repo_url,
        "ref": merged.ref_,
        "tool_version": env!("CARGO_PKG_VERSION"),
        "note": "Report includes deterministic stats and explicit heuristic limits only.",
    });

    write_report(
        &report_path,
        &stats,
        &selected_files_with_tokens(selected_files, &file_tokens),
        &output_files,
        &config_json,
        ReportOptions {
            include_timestamp: !args.no_timestamp,
            provenance: Some(&provenance),
            coverage: None,
        },
    )?;
    output_files.push(report_path.display().to_string());

    println!("Export complete:");
    println!("  root: {}", root_path.display());
    println!("  files: {}", stats.files_included);
    println!("  chunks: {}", stats.chunks_created);
    println!("  tokens: {}", stats.total_tokens_estimated);
    for file in output_files {
        println!("  wrote: {}", file);
    }

    Ok(())
}

fn parse_mode(mode: Option<&str>) -> Result<OutputMode> {
    match mode.unwrap_or("both").trim().to_ascii_lowercase().as_str() {
        "prompt" => Ok(OutputMode::Prompt),
        "rag" => Ok(OutputMode::Rag),
        "both" => Ok(OutputMode::Both),
        other => anyhow::bail!("Invalid mode '{other}'. Expected one of: prompt, rag, both"),
    }
}

fn parse_redaction_mode(mode: Option<&str>) -> Result<RedactionMode> {
    match mode
        .unwrap_or("standard")
        .trim()
        .to_ascii_lowercase()
        .replace('_', "-")
        .as_str()
    {
        "fast" => Ok(RedactionMode::Fast),
        "standard" => Ok(RedactionMode::Standard),
        "paranoid" => Ok(RedactionMode::Paranoid),
        "structure-safe" => Ok(RedactionMode::StructureSafe),
        other => anyhow::bail!(
            "Invalid redaction mode '{other}'. Expected one of: fast, standard, paranoid, structure-safe"
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

fn apply_file_byte_budget(
    ranked_files: Vec<FileInfo>,
    max_total_bytes: u64,
    stats: &mut ScanStats,
) -> Vec<FileInfo> {
    if max_total_bytes == 0 {
        return Vec::new();
    }

    let mut selected = Vec::new();
    let mut total = 0_u64;
    for (idx, file) in ranked_files.iter().enumerate() {
        if total >= max_total_bytes {
            for remaining in &ranked_files[idx..] {
                stats.files_dropped_budget += 1;
                stats.dropped_files.push(HashMap::from([
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

fn apply_chunk_token_budget(mut chunks: Vec<Chunk>, max_tokens: Option<usize>) -> Vec<Chunk> {
    chunks.sort_by(|a, b| {
        b.priority
            .partial_cmp(&a.priority)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.start_line.cmp(&b.start_line))
            .then_with(|| a.id.cmp(&b.id))
    });

    let Some(limit) = max_tokens else {
        return chunks;
    };

    let mut kept = Vec::new();
    let mut used = 0usize;
    for chunk in chunks {
        if used >= limit {
            break;
        }
        if used + chunk.token_estimate > limit {
            break;
        }
        used += chunk.token_estimate;
        kept.push(chunk);
    }
    kept
}

fn resolve_output_dir(config_output: &Path, root_path: &Path, repo_url: Option<&str>) -> PathBuf {
    let repo_name = repo_name_for_output(root_path, repo_url);
    if config_output.file_name().and_then(|n| n.to_str()) == Some(repo_name.as_str()) {
        config_output.to_path_buf()
    } else {
        config_output.join(repo_name)
    }
}

fn repo_name_for_output(root_path: &Path, repo_url: Option<&str>) -> String {
    if let Some(url) = repo_url {
        if let Some(name) = repo_name_from_remote_url(url) {
            return name;
        }
    }
    root_path.file_name().and_then(|n| n.to_str()).unwrap_or("repo").to_string()
}

fn repo_name_from_remote_url(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let last = trimmed.rsplit('/').next()?;
    let cleaned = last.strip_suffix(".git").unwrap_or(last);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

fn selected_files_with_tokens(
    mut files: Vec<FileInfo>,
    token_map: &HashMap<String, usize>,
) -> Vec<FileInfo> {
    for file in &mut files {
        if let Some(tokens) = token_map.get(&file.relative_path) {
            file.token_estimate = *tokens;
        }
    }
    files
}

fn build_config_json(config: &crate::domain::Config) -> serde_json::Value {
    let mut include_extensions: Vec<String> = config.include_extensions.iter().cloned().collect();
    include_extensions.sort();
    let mut exclude_globs: Vec<String> = config.exclude_globs.iter().cloned().collect();
    exclude_globs.sort();

    let mode = match config.mode {
        OutputMode::Prompt => "prompt",
        OutputMode::Rag => "rag",
        OutputMode::Both => "both",
    };

    let redaction_mode = match config.redaction_mode {
        RedactionMode::Fast => "fast",
        RedactionMode::Standard => "standard",
        RedactionMode::Paranoid => "paranoid",
        RedactionMode::StructureSafe => "structure-safe",
    };

    json!({
        "path": config.path,
        "repo": config.repo_url,
        "ref": config.ref_,
        "include_extensions": include_extensions,
        "exclude_globs": exclude_globs,
        "max_file_bytes": config.max_file_bytes,
        "max_total_bytes": config.max_total_bytes,
        "respect_gitignore": config.respect_gitignore,
        "follow_symlinks": config.follow_symlinks,
        "skip_minified": config.skip_minified,
        "max_tokens": config.max_tokens,
        "chunk_tokens": config.chunk_tokens,
        "chunk_overlap": config.chunk_overlap,
        "min_chunk_tokens": config.min_chunk_tokens,
        "mode": mode,
        "output_dir": config.output_dir,
        "tree_depth": config.tree_depth,
        "redact_secrets": config.redact_secrets,
        "redaction_mode": redaction_mode,
        "task_query": config.task_query,
        "heuristics": {
            "semantic_rerank": "disabled",
            "coverage": "disabled",
            "graph": "disabled"
        }
    })
}

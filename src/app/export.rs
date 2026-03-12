#![allow(missing_docs)]

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::chunk::{chunk_content, coalesce_small_chunks_with_max};
use crate::domain::{Chunk, Config, FileInfo, OutputMode, RedactionMode, ScanStats};
use crate::fetch::fetch_repository;
use crate::rank::rank_files_with_manifest;
use crate::redact::Redactor;
use crate::render::{render_context_pack, render_jsonl, write_report, ReportOptions};
use crate::scan::scanner::FileScanner;
use crate::scan::tree::generate_tree;
use crate::utils::{estimate_tokens, read_file_safe};

/// Options controlling export runtime behavior.
#[derive(Debug, Clone, Copy)]
pub struct ExportExecutionOptions {
    /// Whether to include timestamp fields in generated artifacts.
    pub include_timestamp: bool,
}

/// Result summary from an export execution.
#[derive(Debug, Clone)]
pub struct ExportOutcome {
    pub root_path: PathBuf,
    pub stats: ScanStats,
    pub output_files: Vec<String>,
}

pub fn execute(config: Config, options: ExportExecutionOptions) -> Result<ExportOutcome> {
    let repo_ctx = fetch_repository(
        config.path.as_deref(),
        config.repo_url.as_deref(),
        config.ref_.as_deref(),
    )?;
    let root_path = repo_ctx.root_path.clone();

    let mut scanner = FileScanner::new(root_path.clone())
        .max_file_bytes(config.max_file_bytes)
        .respect_gitignore(config.respect_gitignore)
        .follow_symlinks(config.follow_symlinks)
        .skip_minified(config.skip_minified)
        .include_extensions(config.include_extensions.iter().cloned().collect())
        .exclude_globs(config.exclude_globs.iter().cloned().collect());

    let scanned_files = scanner.scan()?;
    let mut stats = scanner.stats().clone();

    let (ranked_files, manifest_info) =
        rank_files_with_manifest(&root_path, scanned_files, config.ranking_weights.clone())?;
    let selected_files = apply_file_byte_budget(ranked_files, config.max_total_bytes, &mut stats);

    let redactor = if config.redact_secrets {
        Some(build_redactor(config.redaction_mode, &config.redaction))
    } else {
        None
    };

    let mut all_chunks = Vec::new();
    let mut redaction_counts: BTreeMap<String, usize> = BTreeMap::new();
    for file in &selected_files {
        let processed = process_file(file, redactor.as_ref(), &config)?;
        if processed.redacted {
            stats.redacted_files += 1;
            stats.redacted_chunks += processed.chunks.len();
        }
        for (rule, count) in processed.counts {
            *redaction_counts.entry(rule).or_insert(0) += count;
        }
        all_chunks.extend(processed.chunks);
    }
    stats.redaction_counts = redaction_counts;

    let chunks = apply_chunk_token_budget(all_chunks, config.max_tokens, &mut stats);

    let file_tokens = file_token_totals(&chunks);
    let included_files = selected_files_with_tokens(selected_files, &file_tokens);

    stats.files_included = included_files.len();
    stats.chunks_created = chunks.len();
    stats.total_tokens_estimated = chunks.iter().map(|c| c.token_estimate).sum();

    let highlights: HashSet<String> =
        included_files.iter().take(10).map(|f| f.relative_path.clone()).collect();
    let tree = generate_tree(&root_path, config.tree_depth, true, &highlights)?;

    let output_dir = resolve_output_dir(&config.output_dir, &root_path, config.repo_url.as_deref());
    fs::create_dir_all(&output_dir)?;

    let repo_name = repo_name_for_output(&root_path, config.repo_url.as_deref());
    let context_path = output_dir.join(format!("{}_context_pack.md", repo_name));
    let jsonl_path = output_dir.join(format!("{}_chunks.jsonl", repo_name));
    let report_path = output_dir.join(format!("{}_report.json", repo_name));

    let mut output_files = Vec::new();

    match config.mode {
        OutputMode::Prompt => {
            let content = render_context_pack(
                &root_path,
                &included_files,
                &chunks,
                &stats,
                &tree,
                &manifest_info,
                options.include_timestamp,
            );
            fs::write(&context_path, content)?;
            output_files.push(context_path.display().to_string());
        }
        OutputMode::Rag => {
            let jsonl = render_jsonl(&chunks);
            fs::write(&jsonl_path, jsonl)?;
            output_files.push(jsonl_path.display().to_string());
        }
        OutputMode::Both => {
            let content = render_context_pack(
                &root_path,
                &included_files,
                &chunks,
                &stats,
                &tree,
                &manifest_info,
                options.include_timestamp,
            );
            fs::write(&context_path, content)?;
            output_files.push(context_path.display().to_string());

            let jsonl = render_jsonl(&chunks);
            fs::write(&jsonl_path, jsonl)?;
            output_files.push(jsonl_path.display().to_string());
        }
    }

    let config_json = build_config_json(&config);
    let provenance = json!({
        "path": root_path.display().to_string(),
        "repo": config.repo_url,
        "ref": config.ref_,
        "tool_version": env!("CARGO_PKG_VERSION"),
        "note": "Report includes deterministic stats and explicit supported fields only.",
    });

    write_report(
        &report_path,
        &stats,
        &included_files,
        &output_files,
        &config_json,
        ReportOptions {
            include_timestamp: options.include_timestamp,
            provenance: Some(&provenance),
        },
    )?;
    output_files.push(report_path.display().to_string());

    Ok(ExportOutcome { root_path, stats, output_files })
}

struct ProcessedFile {
    chunks: Vec<Chunk>,
    redacted: bool,
    counts: BTreeMap<String, usize>,
}

fn process_file(
    file: &FileInfo,
    redactor: Option<&Redactor>,
    config: &Config,
) -> Result<ProcessedFile> {
    let (raw_content, _) = read_file_safe(&file.path, None, None)
        .with_context(|| format!("Failed to read {}", file.relative_path))?;

    let file_name =
        Path::new(&file.relative_path).file_name().and_then(|name| name.to_str()).unwrap_or("");

    let (content, counts) = if let Some(redactor) = redactor {
        if redactor.is_file_allowlisted(file_name, &file.relative_path) {
            (raw_content, BTreeMap::new())
        } else {
            let outcome = redactor.redact_with_language_report(
                &raw_content,
                &file.language,
                &file.extension,
                file_name,
                &file.relative_path,
            );
            (outcome.content, outcome.counts)
        }
    } else {
        (raw_content, BTreeMap::new())
    };

    let redacted = !counts.is_empty();

    let raw_chunks = chunk_content(file, &content, config.chunk_tokens, config.chunk_overlap)?;
    let mut chunks =
        coalesce_small_chunks_with_max(raw_chunks, config.min_chunk_tokens, config.chunk_tokens);

    for chunk in &mut chunks {
        chunk.token_estimate = estimate_tokens(&chunk.content);
    }

    Ok(ProcessedFile { chunks, redacted, counts })
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
        if total + file.size_bytes > max_total_bytes {
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

fn apply_chunk_token_budget(
    mut chunks: Vec<Chunk>,
    max_tokens: Option<usize>,
    stats: &mut ScanStats,
) -> Vec<Chunk> {
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
    let mut dropped_paths: HashSet<String> = HashSet::new();

    for (idx, chunk) in chunks.iter().enumerate() {
        if used + chunk.token_estimate > limit {
            for dropped in &chunks[idx..] {
                dropped_paths.insert(dropped.path.clone());
            }
            break;
        }
        used += chunk.token_estimate;
        kept.push(chunk.clone());
    }

    for path in dropped_paths {
        stats.dropped_files.push(HashMap::from([
            ("path".to_string(), json!(path)),
            ("reason".to_string(), json!("token_limit")),
        ]));
    }

    kept
}

fn file_token_totals(chunks: &[Chunk]) -> HashMap<String, usize> {
    let mut totals = HashMap::new();
    for chunk in chunks {
        *totals.entry(chunk.path.clone()).or_insert(0) += chunk.token_estimate;
    }
    totals
}

fn selected_files_with_tokens(
    files: Vec<FileInfo>,
    token_map: &HashMap<String, usize>,
) -> Vec<FileInfo> {
    let mut selected = Vec::new();
    for mut file in files {
        if let Some(tokens) = token_map.get(&file.relative_path) {
            file.token_estimate = *tokens;
            selected.push(file);
        }
    }
    selected.sort_by(|a, b| {
        b.priority
            .partial_cmp(&a.priority)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.relative_path.cmp(&b.relative_path))
    });
    selected
}

fn build_redactor(mode: RedactionMode, cfg: &crate::domain::RedactionConfig) -> Redactor {
    match mode {
        RedactionMode::Fast => Redactor::from_config(false, false, false, cfg),
        RedactionMode::Standard => Redactor::from_config(true, false, false, cfg),
        RedactionMode::Paranoid => Redactor::from_config(true, true, false, cfg),
        RedactionMode::StructureSafe => Redactor::from_config(true, false, true, cfg),
    }
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

fn build_config_json(config: &Config) -> Value {
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
    })
}

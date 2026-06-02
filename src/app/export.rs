#![allow(missing_docs)]

use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::chunk::{chunk_content, coalesce_small_chunks_with_max, enrich_chunks};
use crate::domain::{
    Chunk, Config, FileDisposition, FileDispositionReason, FileInfo, OutputMode, RedactionMode,
    ScanStats,
};
use crate::fetch::fetch_repository;
use crate::module::picker::ScanMode;
use crate::rank::rank_files_with_manifest;
use crate::redact::Redactor;
use crate::render::{render_context_pack, render_jsonl, write_report, ReportOptions};
use crate::scan::scanner::FileScanner;
use crate::scan::tree::generate_tree;
use crate::utils::{estimate_tokens, read_file_safe};

/// Options controlling export runtime behavior.
#[derive(Debug, Clone)]
pub struct ExportExecutionOptions {
    /// Whether to include timestamp fields in generated artifacts.
    pub include_timestamp: bool,
    /// Optional explicit config path for remote-repo config reload.
    pub explicit_config_path: Option<PathBuf>,
}

/// Result summary from an export execution.
#[derive(Debug, Clone)]
pub struct ExportOutcome {
    pub root_path: PathBuf,
    pub stats: ScanStats,
    pub output_files: Vec<String>,
}

pub fn execute(mut config: Config, options: ExportExecutionOptions) -> Result<ExportOutcome> {
    let started = Instant::now();
    let was_remote = config.repo_url.is_some();
    let repo_ctx = fetch_repository(
        config.path.as_deref(),
        config.repo_url.as_deref(),
        config.ref_.as_deref(),
    )?;
    let root_path = repo_ctx.root_path.clone();

    // When the export target is a remote repository, reload the repo's own
    // config (e.g. repo-context.toml) from the fetched root.  Values set via
    // CLI flags or the caller's own config file are preserved.
    if was_remote {
        crate::config::merge_repo_config(
            &mut config,
            &root_path,
            options.explicit_config_path.as_deref(),
        );
    }

    let mut scanner = FileScanner::new(root_path.clone())
        .max_file_bytes(config.max_file_bytes)
        .respect_gitignore(config.respect_gitignore)
        .follow_symlinks(config.follow_symlinks)
        .skip_minified(config.skip_minified)
        .include_extensions(config.include_extensions.iter().cloned().collect())
        .exclude_globs(config.exclude_globs.iter().cloned().collect());

    let scanned_files = scanner.scan()?;
    let mut stats = scanner.stats().clone();
    let mut dispositions = scanner.dispositions().to_vec();

    let scan_mode = if std::io::stdout().is_terminal() {
        crate::module::picker::pick_scan_mode()?
    } else {
        ScanMode::Full
    };

    let (ranked_files, manifest_info) =
        rank_files_with_manifest(&root_path, scanned_files, config.ranking_weights.clone())?;
    let module_run = if matches!(scan_mode, ScanMode::Module) {
        Some(crate::module::run(&root_path, &ranked_files, &config)?)
    } else {
        None
    };
    update_dispositions_from_files(&mut dispositions, &ranked_files);
    let selected_source =
        module_run.as_ref().map(|module| module.files.clone()).unwrap_or(ranked_files);
    let selected_files = apply_file_byte_budget(
        selected_source,
        config.max_total_bytes,
        &mut stats,
        &mut dispositions,
    );

    let redactor = if config.redact_secrets {
        Some(build_redactor(config.redaction_mode, &config.redaction))
    } else {
        None
    };

    let mut all_chunks = Vec::new();
    let mut redaction_counts: BTreeMap<String, usize> = BTreeMap::new();
    let content_overrides = module_run.as_ref().map(|module| &module.content_overrides);
    for file in &selected_files {
        let processed = process_file(file, redactor.as_ref(), &config, content_overrides)?;
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
    let included_files = selected_files_with_tokens(selected_files.clone(), &file_tokens);

    stats.files_included = included_files.len();
    stats.chunks_created = chunks.len();
    stats.total_tokens_estimated = chunks.iter().map(|c| c.token_estimate).sum();
    stats.total_tokens_estimated_rag = stats.total_tokens_estimated;
    stats.rag_chunks_rendered =
        if matches!(config.mode, OutputMode::Rag | OutputMode::Both) { chunks.len() } else { 0 };
    stats.files_selected_rag = if matches!(config.mode, OutputMode::Rag | OutputMode::Both) {
        included_files.len()
    } else {
        0
    };
    stats.files_selected_prompt = if matches!(config.mode, OutputMode::Prompt | OutputMode::Both) {
        included_files.len()
    } else {
        0
    };
    update_dispositions_for_outputs(&mut dispositions, &included_files, &chunks, config.mode);
    mark_token_dropped(&mut dispositions, &selected_files, &included_files);

    let highlights: HashSet<String> =
        included_files.iter().take(10).map(|f| f.relative_path.clone()).collect();
    let tree = generate_tree(&root_path, config.tree_depth, true, &highlights)?;

    let repo_name = repo_name_for_output(&root_path, config.repo_url.as_deref());
    let module_basename = module_run.as_ref().map(|module| module.entry_basename.as_str());
    let output_dir = resolve_output_dir(&config.output_dir, &repo_name, module_basename);
    fs::create_dir_all(&output_dir)?;

    let output_prefix = module_basename
        .map(|entry| format!("{repo_name}_module_{entry}"))
        .unwrap_or_else(|| repo_name.clone());
    let context_path = output_dir.join(format!("{}_context_pack.md", output_prefix));
    let jsonl_path = output_dir.join(format!("{}_chunks.jsonl", output_prefix));
    let report_path = output_dir.join(format!("{}_report.json", output_prefix));

    let mut output_files = Vec::new();

    match config.mode {
        OutputMode::Prompt => {
            stats.prompt_chunks_rendered = chunks.len();
            let mut content = render_context_pack(
                &root_path,
                &included_files,
                &chunks,
                &stats,
                &tree,
                &manifest_info,
                &dispositions,
                config.full_inventory,
                options.include_timestamp,
            );
            if let Some(module) = &module_run {
                content = format!("{}{}", module.header, content);
            }
            stats.total_tokens_estimated_prompt = estimate_tokens(&content);
            fs::write(&context_path, content)?;
            output_files.push(context_path.display().to_string());
        }
        OutputMode::Rag => {
            let jsonl = render_jsonl(&chunks);
            fs::write(&jsonl_path, jsonl)?;
            output_files.push(jsonl_path.display().to_string());
        }
        OutputMode::Both => {
            stats.prompt_chunks_rendered = chunks.len();
            let mut content = render_context_pack(
                &root_path,
                &included_files,
                &chunks,
                &stats,
                &tree,
                &manifest_info,
                &dispositions,
                config.full_inventory,
                options.include_timestamp,
            );
            if let Some(module) = &module_run {
                content = format!("{}{}", module.header, content);
            }
            stats.total_tokens_estimated_prompt = estimate_tokens(&content);
            fs::write(&context_path, content)?;
            output_files.push(context_path.display().to_string());

            let jsonl = render_jsonl(&chunks);
            fs::write(&jsonl_path, jsonl)?;
            output_files.push(jsonl_path.display().to_string());
        }
    }

    stats.processing_time_seconds =
        if options.include_timestamp { started.elapsed().as_secs_f64() } else { 0.0 };
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
        &dispositions,
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
    content_overrides: Option<&HashMap<PathBuf, String>>,
) -> Result<ProcessedFile> {
    let canonical_path = file.path.canonicalize().unwrap_or_else(|_| file.path.clone());
    let raw_content = if let Some(content) =
        content_overrides.and_then(|m| m.get(&file.path).or_else(|| m.get(&canonical_path)))
    {
        content.clone()
    } else {
        read_file_safe(&file.path, None, None)
            .with_context(|| format!("Failed to read {}", file.relative_path))?
            .0
    };

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

    let raw_chunks = if should_prompt_summary_only(file) {
        vec![summary_chunk(file, &content)]
    } else {
        chunk_content(file, &content, config.chunk_tokens, config.chunk_overlap)?
    };
    let mut chunks =
        coalesce_small_chunks_with_max(raw_chunks, config.min_chunk_tokens, config.chunk_tokens);

    // Re-enrich after coalescing to correct chunk_index, chunks_in_file,
    // byte offsets, content_sha256, file_sha256, and file_id.
    if !chunks.is_empty() && !should_prompt_summary_only(file) {
        enrich_chunks(&mut chunks, file, &content);
    }

    for chunk in &mut chunks {
        chunk.token_estimate = estimate_tokens(&chunk.content);
    }

    Ok(ProcessedFile { chunks, redacted, counts })
}

fn apply_file_byte_budget(
    ranked_files: Vec<FileInfo>,
    max_total_bytes: u64,
    stats: &mut ScanStats,
    dispositions: &mut [FileDisposition],
) -> Vec<FileInfo> {
    if max_total_bytes == 0 {
        return Vec::new();
    }

    let mut selected = Vec::new();
    let mut total = 0_u64;

    for (idx, file) in ranked_files.iter().enumerate() {
        let next_total = total.saturating_add(file.size_bytes);
        if next_total > max_total_bytes {
            for remaining in &ranked_files[idx..] {
                stats.files_dropped_budget += 1;
                set_disposition_reason(
                    dispositions,
                    &remaining.relative_path,
                    FileDispositionReason::DroppedByteBudget,
                );
                stats.dropped_files.push(HashMap::from([
                    ("path".to_string(), json!(remaining.relative_path)),
                    ("reason".to_string(), json!("bytes_limit")),
                    ("priority".to_string(), json!(remaining.priority)),
                ]));
            }
            break;
        }

        total = next_total;
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

    let mut deferred = Vec::new();
    for chunk in chunks.iter().filter(|c| is_mandatory_chunk(c)) {
        if used.saturating_add(chunk.token_estimate) > limit {
            continue;
        }
        used += chunk.token_estimate;
        kept.push(chunk.clone());
    }

    let kept_ids: HashSet<String> = kept.iter().map(|c| c.id.clone()).collect();
    let mut seen_paths: HashSet<String> = kept.iter().map(|c| c.path.clone()).collect();
    for chunk in chunks.iter().filter(|c| !kept_ids.contains(&c.id)) {
        if !seen_paths.contains(&chunk.path) {
            deferred.push(chunk.clone());
        }
    }
    for chunk in chunks.iter().filter(|c| !kept_ids.contains(&c.id)) {
        if seen_paths.contains(&chunk.path) {
            deferred.push(chunk.clone());
        }
    }
    for chunk in deferred {
        if used.saturating_add(chunk.token_estimate) > limit {
            dropped_paths.insert(chunk.path.clone());
            continue;
        }
        used += chunk.token_estimate;
        seen_paths.insert(chunk.path.clone());
        kept.push(chunk);
    }

    for path in dropped_paths {
        stats.dropped_files.push(HashMap::from([
            ("path".to_string(), json!(path)),
            ("reason".to_string(), json!("token_limit")),
        ]));
    }

    kept
}

fn is_mandatory_chunk(chunk: &Chunk) -> bool {
    chunk.tags.iter().any(|t| matches!(t.as_str(), "readme" | "config" | "entrypoint"))
}

fn should_prompt_summary_only(file: &FileInfo) -> bool {
    file.tags.contains("lock-file")
}

fn summary_chunk(file: &FileInfo, content: &str) -> Chunk {
    let summary = format!(
        "Summary only: {}\nlanguage: {}\nbytes: {}\ntokens_estimate: {}\nrole/tags: {}\n",
        file.relative_path,
        file.language,
        file.size_bytes,
        estimate_tokens(content),
        file.tags.iter().cloned().collect::<Vec<_>>().join(",")
    );
    let id = crate::utils::stable_hash(&summary, &file.relative_path, 1, 1);
    let content_sha256 = format!("{:x}", Sha256::digest(summary.as_bytes()));
    let file_sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));
    Chunk {
        id,
        path: file.relative_path.clone(),
        language: file.language.clone(),
        start_line: 1,
        end_line: 1,
        content: summary,
        priority: file.priority,
        tags: file.tags.clone(),
        token_estimate: 64,
        file_id: file.id.clone(),
        chunk_index: 0,
        chunks_in_file: 1,
        byte_start: Some(0),
        byte_end: Some(0),
        content_sha256,
        file_sha256,
    }
}

fn update_dispositions_from_files(dispositions: &mut [FileDisposition], files: &[FileInfo]) {
    for file in files {
        if let Some(d) = dispositions.iter_mut().find(|d| d.path == file.relative_path) {
            d.priority = Some(file.priority);
            d.token_estimate = Some(file.token_estimate);
            d.notes = Some(file.tags.iter().cloned().collect::<Vec<_>>().join(","));
        }
    }
}

fn update_dispositions_for_outputs(
    dispositions: &mut [FileDisposition],
    files: &[FileInfo],
    chunks: &[Chunk],
    mode: OutputMode,
) {
    let chunk_paths: HashSet<&str> = chunks.iter().map(|c| c.path.as_str()).collect();
    for file in files {
        if let Some(d) = dispositions.iter_mut().find(|d| d.path == file.relative_path) {
            d.priority = Some(file.priority);
            d.token_estimate = Some(file.token_estimate);
            d.included_in_prompt = matches!(mode, OutputMode::Prompt | OutputMode::Both)
                && chunk_paths.contains(file.relative_path.as_str());
            d.included_in_rag = matches!(mode, OutputMode::Rag | OutputMode::Both)
                && chunk_paths.contains(file.relative_path.as_str());
            d.reason = if should_prompt_summary_only(file) {
                FileDispositionReason::IncludedSummaryOnly
            } else if chunks.iter().filter(|c| c.path == file.relative_path).count() > 1 {
                FileDispositionReason::IncludedChunked
            } else {
                FileDispositionReason::IncludedFull
            };
        }
    }
}

fn set_disposition_reason(
    dispositions: &mut [FileDisposition],
    path: &str,
    reason: FileDispositionReason,
) {
    if let Some(d) = dispositions.iter_mut().find(|d| d.path == path) {
        d.reason = reason;
        d.included_in_prompt = false;
        d.included_in_rag = false;
    }
}

fn mark_token_dropped(
    dispositions: &mut [FileDisposition],
    selected_files: &[FileInfo],
    included_files: &[FileInfo],
) {
    let included: HashSet<&str> = included_files.iter().map(|f| f.relative_path.as_str()).collect();
    for file in selected_files {
        if !included.contains(file.relative_path.as_str()) {
            set_disposition_reason(
                dispositions,
                &file.relative_path,
                FileDispositionReason::DroppedTokenBudget,
            );
        }
    }
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

fn resolve_output_dir(base_dir: &Path, repo_name: &str, module_basename: Option<&str>) -> PathBuf {
    let repo_dir = base_dir.join(repo_name);
    module_basename.map(|entry| repo_dir.join(format!("module_{entry}"))).unwrap_or(repo_dir)
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

    let coverage_strategy = if config.max_tokens.is_some() { "budget" } else { "full" };

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
        "coverage_strategy": coverage_strategy,
        "mode": mode,
        "output_dir": config.output_dir,
        "tree_depth": config.tree_depth,
        "redact_secrets": config.redact_secrets,
        "redaction_mode": redaction_mode,
        "module": {
            "module_roots": &config.module.module_roots,
            "css_files": &config.module.css_files,
        },
    })
}

#![allow(missing_docs)]

use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::chunk::{chunk_content, coalesce_small_chunks_with_max, enrich_chunks};
use crate::domain::{
    is_programming_language, Chunk, Config, FileDisposition, FileDispositionReason, FileInfo,
    OutputMode, RedactionMode, ScanStats,
};
use crate::fetch::fetch_repository;
use crate::godot::{analyze as analyze_godot, resolve_profile};
use crate::index::{default_index_path, IndexStore};
use crate::module::focus_picker::ScanMode;
use crate::module::FocusResult;
use crate::rank::rank_files_with_manifest;
use crate::redact::redactor::RedactionOccurrence;
use crate::redact::Redactor;
use crate::render::{
    render_jsonl, render_jsonl_with_evidence, write_report_with_retrieval, ContextPackCtx,
    ReportOptions,
};
use crate::retrieve::{self, RetrievalPlan};
use crate::scan::scanner::FileScanner;
use crate::scan::tree::generate_tree;
use crate::utils::{estimate_tokens, read_file_safe, redact_url_credentials, write_atomic};

/// Options controlling export runtime behavior.
#[derive(Debug, Clone)]
pub struct ExportExecutionOptions {
    /// Whether to include timestamp fields in generated artifacts.
    pub include_timestamp: bool,
    /// Optional explicit config path for remote-repo config reload.
    pub explicit_config_path: Option<PathBuf>,
    /// Override scan mode (None = interactive on TTY, full on pipe).
    pub scan_mode: Option<ScanMode>,
    /// For focused mode: pre-select this file or module entry (non-interactive).
    pub focus_path: Option<PathBuf>,
}

/// Optional task retrieval behavior for an export.
#[derive(Debug, Clone, Default)]
pub struct TaskExecutionOptions {
    /// Optional task text used to order or select context.
    pub task: Option<String>,
    /// Explicit persistent index path for task retrieval.
    pub index_db: Option<PathBuf>,
    /// Disable persistent index access.
    pub no_index: bool,
}

/// Result summary from an export execution.
#[derive(Debug, Clone)]
pub struct ExportOutcome {
    pub root_path: PathBuf,
    pub stats: ScanStats,
    pub output_files: Vec<String>,
}

/// Build a local redacted index without writing export artifacts.
pub fn build_index(mut config: Config, index_path: &Path) -> Result<crate::index::IndexRefresh> {
    config.validate()?;
    if config.repo_url.is_some() {
        anyhow::bail!("the index command accepts local repositories only");
    }
    let repo_ctx = fetch_repository(config.path.as_deref(), None, None)?;
    let root_path = repo_ctx.root_path.clone();
    let _ = resolve_profile(&mut config, &root_path);
    let mut scanner = FileScanner::from_config(root_path.clone(), &config);
    let scanned_files = scanner.scan()?;
    let (ranked_files, _) =
        rank_files_with_manifest(&root_path, scanned_files, config.ranking_weights.clone())?;
    let redactor = build_redactor(config.redaction_mode, &config.redaction);
    let graph = crate::module::graph::build(&ranked_files);
    let mut store = IndexStore::open(
        index_path,
        &root_path,
        &crate::index::config_fingerprint(&config),
        &crate::index::redaction_fingerprint(&config),
    )?;
    let stale_paths = store.paths_needing_refresh(&ranked_files)?;
    let current_paths: HashSet<&str> =
        ranked_files.iter().map(|file| file.relative_path.as_str()).collect();
    let mut chunks: Vec<Chunk> = store
        .load_chunks()?
        .into_iter()
        .filter(|chunk| {
            current_paths.contains(chunk.path.as_str()) && !stale_paths.contains(&chunk.path)
        })
        .collect();
    for file in ranked_files.iter().filter(|file| stale_paths.contains(&file.relative_path)) {
        chunks.extend(process_file(file, Some(&redactor), &config, None)?.chunks);
    }
    store.refresh(&ranked_files, &chunks, &graph, &root_path)
}

pub fn execute(config: Config, options: ExportExecutionOptions) -> Result<ExportOutcome> {
    execute_with_task(config, options, TaskExecutionOptions::default())
}

pub fn execute_with_task(
    mut config: Config,
    options: ExportExecutionOptions,
    task_options: TaskExecutionOptions,
) -> Result<ExportOutcome> {
    config.validate()?;
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

    config.validate()?;

    let godot_detection = resolve_profile(&mut config, &root_path);

    let mut scanner = FileScanner::from_config(root_path.clone(), &config);

    let scanned_files = scanner.scan()?;
    let mut stats = scanner.stats().clone();
    let mut dispositions = scanner.dispositions().to_vec();

    let scan_mode = match options.scan_mode {
        Some(mode) => mode,
        None => {
            if std::io::stdout().is_terminal() {
                crate::module::focus_picker::pick_scan_mode()?
            } else {
                ScanMode::Full
            }
        }
    };

    let (ranked_files, manifest_info) =
        rank_files_with_manifest(&root_path, scanned_files, config.ranking_weights.clone())?;
    let full_scan_paths =
        ranked_files.iter().map(|file| file.relative_path.clone()).collect::<HashSet<_>>();
    let module_run = if matches!(scan_mode, ScanMode::Focused) {
        if let Some(ref focus_path) = options.focus_path {
            // Non-interactive: use the provided focus path.
            let focus_abs = root_path.join(focus_path);
            Some(crate::module::run_focused_with_file(
                &root_path,
                &ranked_files,
                &config,
                &focus_abs,
            )?)
        } else {
            // Interactive focused flow.
            match crate::module::run_focused(&root_path, &ranked_files, &config)? {
                FocusResult::Export(m) => Some(m),
                FocusResult::FullContext => None,
                FocusResult::Cancelled => {
                    return Err(anyhow::anyhow!("Export cancelled"));
                }
            }
        }
    } else {
        None
    };
    update_dispositions_from_files(&mut dispositions, &ranked_files);
    let godot_summary = godot_detection.active.then(|| {
        analyze_godot(&root_path, &ranked_files, &dispositions, godot_detection.signals.clone())
    });
    let focus_paths = module_run.as_ref().map(|module| {
        module.files.iter().map(|file| file.relative_path.clone()).collect::<HashSet<_>>()
    });
    if let Some(paths) = &focus_paths {
        mark_focus_excluded(&mut dispositions, paths);
    }
    let selected_source =
        module_run.as_ref().map(|module| module.files.clone()).unwrap_or(ranked_files);
    let selected_files = apply_file_byte_budget(
        selected_source,
        config.max_total_bytes,
        &mut stats,
        &mut dispositions,
    );
    let index_scope_is_full = module_run.is_none()
        && selected_files.len() == full_scan_paths.len()
        && selected_files.iter().all(|file| full_scan_paths.contains(&file.relative_path));

    let redactor = if config.redact_secrets {
        Some(build_redactor(config.redaction_mode, &config.redaction))
    } else {
        None
    };

    // Redact manifest strings (package.json scripts, etc.) before rendering,
    // since they bypass the per-file redaction in process_file.
    let manifest_info = if let Some(ref r) = redactor {
        redact_manifest_info(manifest_info, r)
    } else {
        manifest_info
    };

    let mut all_chunks = Vec::new();
    let mut redactions_by_path: HashMap<String, Vec<RedactionOccurrence>> = HashMap::new();
    let content_overrides = module_run.as_ref().map(|module| &module.content_overrides);
    for file in &selected_files {
        let processed = process_file(file, redactor.as_ref(), &config, content_overrides)?;
        if !processed.redactions.is_empty() {
            redactions_by_path.insert(file.relative_path.clone(), processed.redactions);
        }
        all_chunks.extend(processed.chunks);
    }

    let mut retrieval_plan = if let Some(task) = task_options.task.as_deref() {
        let graph = crate::module::graph::build(&selected_files);
        let mut indexed_chunks = None;
        if !task_options.no_index && config.redact_secrets && !was_remote && index_scope_is_full {
            let index_path =
                task_options.index_db.clone().or_else(|| default_index_path(&root_path));
            if let Some(index_path) = index_path {
                let index_result = IndexStore::open(
                    &index_path,
                    &root_path,
                    &crate::index::config_fingerprint(&config),
                    &crate::index::redaction_fingerprint(&config),
                )
                .and_then(|mut store| {
                    store.refresh(&selected_files, &all_chunks, &graph, &root_path)?;
                    store.load_chunks()
                });
                match index_result {
                    Ok(chunks) if !chunks.is_empty() => indexed_chunks = Some(chunks),
                    Ok(_) => {}
                    Err(error) if task_options.index_db.is_some() => {
                        return Err(error.context("failed to refresh explicit task index"));
                    }
                    Err(error) => {
                        tracing::warn!(
                            "task index unavailable; using in-memory retrieval: {error}"
                        );
                    }
                }
            } else {
                tracing::warn!("no user cache directory; using in-memory task retrieval");
            }
        } else if task_options.index_db.is_some() && !index_scope_is_full {
            tracing::info!(
                "focused or byte-budgeted export uses in-memory retrieval; persistent index was left unchanged"
            );
        }
        let retrieval_chunks = indexed_chunks.as_deref().unwrap_or(&all_chunks);
        Some(retrieve::build_plan(task, retrieval_chunks, &selected_files, &graph))
    } else {
        None
    };

    let mut chunks = if let Some(plan) = retrieval_plan.as_ref() {
        apply_chunk_token_budget_for_task(all_chunks, config.max_tokens, &mut stats, plan)
    } else {
        apply_chunk_token_budget(all_chunks, config.max_tokens, &mut stats)
    };
    if let Some(plan) = retrieval_plan.as_ref() {
        reorder_chunks_for_task(&mut chunks, plan);
    }
    if matches!(config.mode, OutputMode::Rag | OutputMode::Both) {
        if let Some(limit) = config.max_tokens {
            if let Some(plan) = retrieval_plan.as_ref() {
                trim_chunks_to_rag_budget_for_task(&mut chunks, limit, plan);
            } else {
                trim_chunks_to_rag_budget(&mut chunks, limit);
            }
        }
    }
    let accounting_stats = stats.clone();
    let accounting_dispositions = dispositions.clone();
    let mut included_files = refresh_output_accounting(
        &mut stats,
        &mut dispositions,
        &selected_files,
        &chunks,
        config.mode,
        &redactions_by_path,
    );

    let mut highlights: HashSet<String> =
        included_files.iter().take(10).map(|f| f.relative_path.clone()).collect();
    let mut tree = generate_tree(&root_path, config.tree_depth, true, &highlights)?;

    let mut prompt_content = None;
    if matches!(config.mode, OutputMode::Prompt | OutputMode::Both) {
        let mut task_header = task_options
            .task
            .as_deref()
            .map(|task| task_header(task, retrieval_plan.as_ref(), redactor.as_ref()))
            .unwrap_or_default();
        if let Some(limit) = config.max_tokens {
            task_header = cap_to_tokens(&task_header, limit);
        }
        let mut compact = false;
        let full_dispositions =
            context_dispositions(&dispositions, &included_files, module_run.is_some());
        let mut content = render_prompt(
            ContextPackCtx {
                root_path: &root_path,
                files: &included_files,
                chunks: &chunks,
                stats: &stats,
                tree: &tree,
                manifest_info: &manifest_info,
                dispositions: &full_dispositions,
                full_inventory: config.full_inventory,
                compact,
                include_timestamp: options.include_timestamp,
                godot: godot_summary.as_ref(),
            },
            module_run.as_ref().map(|module| module.header.as_str()),
        );

        if let Some(limit) = config.max_tokens {
            if estimate_prefixed_tokens(&task_header, &content) > limit {
                compact = true;
                loop {
                    content = render_prompt(
                        ContextPackCtx {
                            root_path: &root_path,
                            files: &included_files,
                            chunks: &chunks,
                            stats: &stats,
                            tree: &tree,
                            manifest_info: &manifest_info,
                            dispositions: &[],
                            full_inventory: false,
                            compact,
                            include_timestamp: false,
                            godot: None,
                        },
                        None,
                    );
                    if estimate_prefixed_tokens(&task_header, &content) <= limit
                        || chunks.is_empty()
                    {
                        break;
                    }
                    let remove_at = retrieval_plan
                        .as_ref()
                        .map(|plan| lowest_value_chunk_index_for_task(&chunks, plan))
                        .unwrap_or_else(|| lowest_value_chunk_index(&chunks));
                    chunks.remove(remove_at);
                }

                stats = accounting_stats.clone();
                dispositions = accounting_dispositions.clone();
                included_files = refresh_output_accounting(
                    &mut stats,
                    &mut dispositions,
                    &selected_files,
                    &chunks,
                    config.mode,
                    &redactions_by_path,
                );
                highlights =
                    included_files.iter().take(10).map(|f| f.relative_path.clone()).collect();
                tree = generate_tree(&root_path, config.tree_depth, true, &highlights)?;
                content = render_prompt(
                    ContextPackCtx {
                        root_path: &root_path,
                        files: &included_files,
                        chunks: &chunks,
                        stats: &stats,
                        tree: &tree,
                        manifest_info: &manifest_info,
                        dispositions: &[],
                        full_inventory: false,
                        compact,
                        include_timestamp: false,
                        godot: None,
                    },
                    None,
                );
                if estimate_prefixed_tokens(&task_header, &content) > limit {
                    content.clear();
                }
            }
        }
        stats.total_tokens_estimated_prompt = estimate_prefixed_tokens(&task_header, &content);
        prompt_content = Some(format!("{task_header}{content}"));
    }
    if let Some(plan) = retrieval_plan.as_ref() {
        if matches!(config.mode, OutputMode::Rag | OutputMode::Both) {
            stats.total_tokens_estimated_rag =
                estimate_tokens(&render_jsonl_with_evidence(&chunks, Some(plan)));
        }
    }
    if let Some(plan) = retrieval_plan.as_mut() {
        plan.selected_chunks = chunks.len();
        plan.selected_files =
            chunks.iter().map(|chunk| chunk.path.as_str()).collect::<HashSet<_>>().len();
    }

    let repo_name = repo_name_for_output(&root_path, config.repo_url.as_deref());
    let module_basename = module_run
        .as_ref()
        .map(|module| sanitize_output_component(&module.entry_basename, "module"));
    let output_dir = resolve_output_dir(&config.output_dir, &repo_name, module_basename.as_deref());
    fs::create_dir_all(&output_dir)?;

    let output_prefix = module_basename
        .as_deref()
        .map(|entry| format!("{repo_name}_focus_{entry}"))
        .unwrap_or_else(|| repo_name.clone());
    let context_path = output_dir.join(format!("{}_context_pack.md", output_prefix));
    let jsonl_path = output_dir.join(format!("{}_chunks.jsonl", output_prefix));
    let report_path = output_dir.join(format!("{}_report.json", output_prefix));

    let mut output_files = Vec::new();
    if matches!(config.mode, OutputMode::Prompt | OutputMode::Both) {
        output_files.push(context_path.display().to_string());
    }
    if matches!(config.mode, OutputMode::Rag | OutputMode::Both) {
        output_files.push(jsonl_path.display().to_string());
    }
    output_files.push(report_path.display().to_string());

    match config.mode {
        OutputMode::Prompt => {
            write_atomic(&context_path, prompt_content.as_deref().unwrap_or_default().as_bytes())?;
        }
        OutputMode::Rag => {
            let jsonl = render_jsonl_with_evidence(&chunks, retrieval_plan.as_ref());
            write_atomic(&jsonl_path, jsonl.as_bytes())?;
        }
        OutputMode::Both => {
            write_atomic(&context_path, prompt_content.as_deref().unwrap_or_default().as_bytes())?;

            let jsonl = render_jsonl_with_evidence(&chunks, retrieval_plan.as_ref());
            write_atomic(&jsonl_path, jsonl.as_bytes())?;
        }
    }

    stats.processing_time_seconds =
        if options.include_timestamp { started.elapsed().as_secs_f64() } else { 0.0 };
    let config_json = build_config_json(&config);
    let provenance = json!({
        "path": root_path.display().to_string(),
        "repo": config.repo_url.as_ref().map(|u| redact_url_credentials(u)),
        "ref": config.ref_,
        "tool_version": env!("CARGO_PKG_VERSION"),
        "note": "Report includes deterministic stats and explicit supported fields only.",
        "profile_signals": godot_detection.signals,
    });

    // Build focus metadata for the report when in focused mode.
    let included_paths: HashSet<&str> =
        included_files.iter().map(|file| file.relative_path.as_str()).collect();
    let focus_json = module_run.as_ref().and_then(|m| m.focus_scope.as_ref()).map(|scope| {
        let kind = match scope.kind {
            crate::module::FocusKind::File => "file",
            crate::module::FocusKind::Module => "module",
        };
        let selected_rel = scope
            .selected
            .strip_prefix(&root_path)
            .unwrap_or(&scope.selected)
            .to_string_lossy()
            .replace('\\', "/");
        let included_reasons: serde_json::Map<String, Value> = scope
            .files
            .iter()
            .filter(|(file, _)| included_paths.contains(file.relative_path.as_str()))
            .map(|(f, reason)| {
                let reason_str = match reason {
                    crate::module::InclusionReason::Selected => "selected",
                    crate::module::InclusionReason::OutboundDependency => "outbound_dependency",
                    crate::module::InclusionReason::Caller => "caller",
                    crate::module::InclusionReason::RelatedTest => "related_test",
                    crate::module::InclusionReason::EntryPath => "entry_path",
                    crate::module::InclusionReason::CrateFallback => "crate_fallback",
                    crate::module::InclusionReason::RuntimeModule => "runtime_module",
                    crate::module::InclusionReason::CssScope => "css_scope",
                };
                (f.relative_path.clone(), Value::String(reason_str.to_string()))
            })
            .collect();
        json!({
            "kind": kind,
            "selected": selected_rel,
            "included_reasons": included_reasons,
        })
    });

    write_report_with_retrieval(
        &report_path,
        &stats,
        &included_files,
        &output_files,
        &config_json,
        &dispositions,
        ReportOptions {
            include_timestamp: options.include_timestamp,
            provenance: Some(&provenance),
            focus: focus_json.as_ref(),
            godot: godot_summary.as_ref(),
        },
        retrieval_plan.as_ref(),
    )?;
    Ok(ExportOutcome { root_path, stats, output_files })
}

struct ProcessedFile {
    chunks: Vec<Chunk>,
    redactions: Vec<RedactionOccurrence>,
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

    let (content, redactions) = if let Some(redactor) = redactor {
        if redactor.is_file_allowlisted(file_name, &file.relative_path) {
            (raw_content, Vec::new())
        } else {
            let outcome = redactor.redact_with_language_report(
                &raw_content,
                &file.language,
                &file.extension,
                file_name,
                &file.relative_path,
            );
            (outcome.content, outcome.occurrences)
        }
    } else {
        (raw_content, Vec::new())
    };

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

    Ok(ProcessedFile { chunks, redactions })
}

fn render_prompt(ctx: ContextPackCtx<'_>, module_header: Option<&str>) -> String {
    let rendered = ctx.render();
    module_header.map(|header| format!("{header}{rendered}")).unwrap_or(rendered)
}

fn context_dispositions(
    dispositions: &[FileDisposition],
    included_files: &[FileInfo],
    focused: bool,
) -> Vec<FileDisposition> {
    if !focused {
        return dispositions.to_vec();
    }
    let paths: HashSet<&str> =
        included_files.iter().map(|file| file.relative_path.as_str()).collect();
    dispositions.iter().filter(|item| paths.contains(item.path.as_str())).cloned().collect()
}

fn refresh_output_accounting(
    stats: &mut ScanStats,
    dispositions: &mut [FileDisposition],
    selected_files: &[FileInfo],
    chunks: &[Chunk],
    mode: OutputMode,
    redactions_by_path: &HashMap<String, Vec<RedactionOccurrence>>,
) -> Vec<FileInfo> {
    let emitted_chunks: Vec<Chunk> = chunks
        .iter()
        .filter(|chunk| !matches!(mode, OutputMode::Prompt) || !chunk.tags.contains("rag-only"))
        .cloned()
        .collect();
    let file_tokens = file_token_totals(&emitted_chunks);
    let included_files = selected_files_with_tokens(selected_files.to_vec(), &file_tokens);

    stats.files_included = included_files.len();
    stats.total_bytes_included = included_chunk_bytes(&emitted_chunks);
    stats.chunks_created = emitted_chunks.len();
    stats.total_tokens_estimated = emitted_chunks.iter().map(|chunk| chunk.token_estimate).sum();
    stats.source_tokens_selected = emitted_chunks
        .iter()
        .filter(|chunk| is_source_chunk(chunk))
        .map(|chunk| chunk.token_estimate)
        .sum();
    stats.context_tokens_selected = emitted_chunks
        .iter()
        .filter(|chunk| !is_source_chunk(chunk))
        .map(|chunk| chunk.token_estimate)
        .sum();
    stats.total_tokens_estimated_prompt = 0;
    stats.total_tokens_estimated_rag = if matches!(mode, OutputMode::Rag | OutputMode::Both) {
        estimate_tokens(&render_jsonl(chunks))
    } else {
        0
    };
    let prompt_chunks = chunks.iter().filter(|chunk| !chunk.tags.contains("rag-only")).count();
    stats.rag_chunks_rendered =
        if matches!(mode, OutputMode::Rag | OutputMode::Both) { chunks.len() } else { 0 };
    stats.prompt_chunks_rendered =
        if matches!(mode, OutputMode::Prompt | OutputMode::Both) { prompt_chunks } else { 0 };
    stats.files_selected_rag = if matches!(mode, OutputMode::Rag | OutputMode::Both) {
        unique_chunk_paths(chunks, false)
    } else {
        0
    };
    stats.files_selected_prompt = if matches!(mode, OutputMode::Prompt | OutputMode::Both) {
        unique_chunk_paths(chunks, true)
    } else {
        0
    };

    update_dispositions_for_outputs(dispositions, &included_files, &emitted_chunks, mode);
    mark_token_dropped(dispositions, selected_files, &included_files);
    stats.files_dropped_budget = dispositions
        .iter()
        .filter(|item| {
            matches!(
                item.reason,
                FileDispositionReason::DroppedByteBudget
                    | FileDispositionReason::DroppedTokenBudget
            )
        })
        .count();
    stats.dropped_files = dispositions
        .iter()
        .filter_map(|item| {
            let reason = match item.reason {
                FileDispositionReason::DroppedByteBudget => "bytes_limit",
                FileDispositionReason::DroppedTokenBudget => "token_limit",
                _ => return None,
            };
            let mut record = HashMap::from([
                ("path".to_string(), json!(item.path)),
                ("reason".to_string(), json!(reason)),
            ]);
            if let Some(priority) = item.priority {
                record.insert("priority".to_string(), json!(priority));
            }
            Some(record)
        })
        .collect();
    stats.dropped_files.sort_by(|left, right| {
        left.get("path").and_then(Value::as_str).cmp(&right.get("path").and_then(Value::as_str))
    });

    let mut redacted_paths = HashSet::new();
    let mut redacted_chunk_ids = HashSet::new();
    let effective_spans =
        effective_chunk_spans(&emitted_chunks, matches!(mode, OutputMode::Prompt));
    stats.redaction_counts.clear();
    for (path, occurrences) in redactions_by_path {
        let path_chunks: Vec<&Chunk> =
            emitted_chunks.iter().filter(|chunk| chunk.path == *path).collect();
        let mut derived_occurrences: HashMap<(String, String), usize> = HashMap::new();
        for occurrence in occurrences.iter().filter(|item| !item.replacement.is_empty()) {
            derived_occurrences
                .entry((occurrence.rule.clone(), occurrence.replacement.clone()))
                .or_insert_with(|| {
                    path_chunks
                        .iter()
                        .filter(|chunk| chunk.byte_start.is_none())
                        .map(|chunk| chunk.content.matches(&occurrence.replacement).count())
                        .sum()
                });
        }
        for occurrence in occurrences {
            let derived_match = derived_occurrences
                .get_mut(&(occurrence.rule.clone(), occurrence.replacement.clone()))
                .is_some_and(|remaining| {
                    if *remaining == 0 {
                        false
                    } else {
                        *remaining -= 1;
                        true
                    }
                });
            let matching_chunks: Vec<&&Chunk> = path_chunks
                .iter()
                .filter(|chunk| {
                    effective_spans
                        .get(chunk.id.as_str())
                        .is_some_and(|span| span_contains_occurrence(*span, occurrence))
                        || (derived_match
                            && chunk.byte_start.is_none()
                            && chunk.content.contains(&occurrence.replacement))
                })
                .collect();
            if matching_chunks.is_empty() {
                continue;
            }
            redacted_paths.insert(path.as_str());
            *stats.redaction_counts.entry(occurrence.rule.clone()).or_insert(0) += 1;
            for chunk in matching_chunks {
                redacted_chunk_ids.insert(chunk.id.as_str());
            }
        }
    }
    stats.redacted_files = redacted_paths.len();
    stats.redacted_chunks = redacted_chunk_ids.len();

    included_files
}

fn span_contains_occurrence(
    (start, end): (usize, usize),
    occurrence: &RedactionOccurrence,
) -> bool {
    if start >= end {
        return false;
    }
    if occurrence.start == occurrence.end {
        start <= occurrence.start && occurrence.start <= end
    } else {
        start < occurrence.end && end > occurrence.start
    }
}

fn effective_chunk_spans(
    chunks: &[Chunk],
    dedupe_prompt_overlap: bool,
) -> HashMap<&str, (usize, usize)> {
    let mut ordered: Vec<&Chunk> = chunks.iter().collect();
    ordered.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.start_line.cmp(&right.start_line))
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut next_line_by_path = HashMap::new();
    let mut spans = HashMap::new();
    for chunk in ordered {
        let (Some(start), Some(end)) = (chunk.byte_start, chunk.byte_end) else { continue };
        if !dedupe_prompt_overlap {
            spans.insert(chunk.id.as_str(), (start, end));
            continue;
        }
        let next_line = next_line_by_path.get(chunk.path.as_str()).copied().unwrap_or(1usize);
        let skip_lines = next_line.saturating_sub(chunk.start_line);
        let skipped_bytes: usize =
            chunk.content.split_inclusive('\n').take(skip_lines).map(str::len).sum();
        let effective_start = start.saturating_add(skipped_bytes).min(end);
        let remaining = chunk.content.get(skipped_bytes..).unwrap_or_default();
        if !remaining.trim().is_empty() {
            next_line_by_path.insert(chunk.path.as_str(), chunk.end_line.saturating_add(1));
            spans.insert(chunk.id.as_str(), (effective_start, end));
        } else {
            spans.insert(chunk.id.as_str(), (end, end));
        }
    }
    spans
}

fn included_chunk_bytes(chunks: &[Chunk]) -> u64 {
    let mut spans_by_path: HashMap<&str, Vec<(usize, usize)>> = HashMap::new();
    let mut generated_bytes = 0u64;
    for chunk in chunks {
        match (chunk.byte_start, chunk.byte_end) {
            (Some(start), Some(end)) if end > start => {
                spans_by_path.entry(chunk.path.as_str()).or_default().push((start, end));
            }
            _ => generated_bytes = generated_bytes.saturating_add(chunk.content.len() as u64),
        }
    }

    let mut total = generated_bytes;
    for spans in spans_by_path.values_mut() {
        spans.sort_unstable();
        let mut current: Option<(usize, usize)> = None;
        for &(start, end) in spans.iter() {
            match current {
                Some((current_start, current_end)) if start <= current_end => {
                    current = Some((current_start, current_end.max(end)));
                }
                Some((current_start, current_end)) => {
                    total = total.saturating_add((current_end - current_start) as u64);
                    current = Some((start, end));
                }
                None => current = Some((start, end)),
            }
        }
        if let Some((start, end)) = current {
            total = total.saturating_add((end - start) as u64);
        }
    }
    total
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
    let len = apply_chunk_token_budget_inner(&mut chunks, max_tokens, stats, None);
    chunks.truncate(len);
    chunks
}

fn apply_chunk_token_budget_for_task(
    mut chunks: Vec<Chunk>,
    max_tokens: Option<usize>,
    stats: &mut ScanStats,
    plan: &RetrievalPlan,
) -> Vec<Chunk> {
    let len = apply_chunk_token_budget_inner(&mut chunks, max_tokens, stats, Some(plan));
    chunks.truncate(len);
    chunks
}

fn apply_chunk_token_budget_inner(
    chunks: &mut [Chunk],
    max_tokens: Option<usize>,
    stats: &mut ScanStats,
    retrieval: Option<&RetrievalPlan>,
) -> usize {
    chunks.sort_by(|a, b| {
        let score_order = if let Some(plan) = retrieval {
            let left = plan.score_for(a).unwrap_or(-1.0);
            let right = plan.score_for(b).unwrap_or(-1.0);
            right.partial_cmp(&left).unwrap_or(std::cmp::Ordering::Equal)
        } else {
            b.priority.partial_cmp(&a.priority).unwrap_or(std::cmp::Ordering::Equal)
        };
        score_order
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.start_line.cmp(&b.start_line))
            .then_with(|| a.id.cmp(&b.id))
    });

    let Some(limit) = max_tokens else {
        return chunks.len();
    };

    let source_limit = limit.saturating_mul(40) / 100;
    let context_limit = limit.saturating_sub(source_limit);
    let mut kept = Vec::new();
    let mut kept_ids = HashSet::new();

    select_pool_chunks(
        chunks.iter().filter(|chunk| is_source_chunk(chunk)),
        source_limit,
        &mut kept,
        &mut kept_ids,
    );
    if !chunks.iter().any(|chunk| kept_ids.contains(&chunk.id) && is_source_chunk(chunk)) {
        if let Some(chunk) =
            chunks.iter().find(|chunk| is_source_chunk(chunk) && chunk.token_estimate <= limit)
        {
            kept_ids.insert(chunk.id.clone());
            kept.push(chunk.clone());
        }
    }

    let source_used: usize =
        kept.iter().filter(|chunk| is_source_chunk(chunk)).map(|chunk| chunk.token_estimate).sum();
    let available_context = context_limit.min(limit.saturating_sub(source_used));
    let mut reserved_context = 0usize;
    for tag in ["readme", "config"] {
        if let Some(chunk) = chunks.iter().find(|chunk| {
            !is_source_chunk(chunk)
                && chunk.tags.contains(tag)
                && !kept_ids.contains(&chunk.id)
                && reserved_context.saturating_add(chunk.token_estimate) <= available_context
        }) {
            reserved_context += chunk.token_estimate;
            kept_ids.insert(chunk.id.clone());
            kept.push(chunk.clone());
        }
    }
    select_pool_chunks(
        chunks.iter().filter(|chunk| !is_source_chunk(chunk)),
        available_context.saturating_sub(reserved_context),
        &mut kept,
        &mut kept_ids,
    );

    let mut used: usize = kept.iter().map(|chunk| chunk.token_estimate).sum();
    let remaining = limit.saturating_sub(used);
    select_pool_chunks(chunks.iter(), remaining, &mut kept, &mut kept_ids);
    used = kept.iter().map(|chunk| chunk.token_estimate).sum();
    debug_assert!(used <= limit);

    let kept_paths: HashSet<&str> = kept.iter().map(|chunk| chunk.path.as_str()).collect();
    let dropped_paths: HashSet<&str> = chunks
        .iter()
        .filter(|chunk| !kept_ids.contains(&chunk.id) && !kept_paths.contains(chunk.path.as_str()))
        .map(|chunk| chunk.path.as_str())
        .collect();
    for path in dropped_paths {
        stats.dropped_files.push(HashMap::from([
            ("path".to_string(), json!(path)),
            ("reason".to_string(), json!("token_limit")),
        ]));
    }

    let kept_len = kept.len();
    chunks[..kept_len].clone_from_slice(&kept);
    kept_len
}

fn reorder_chunks_for_task(chunks: &mut [Chunk], plan: &RetrievalPlan) {
    let rank: HashMap<&str, usize> =
        plan.ordered_chunk_ids.iter().enumerate().map(|(index, id)| (id.as_str(), index)).collect();
    chunks.sort_by(|left, right| {
        rank.get(left.id.as_str())
            .copied()
            .unwrap_or(usize::MAX)
            .cmp(&rank.get(right.id.as_str()).copied().unwrap_or(usize::MAX))
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.start_line.cmp(&right.start_line))
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn trim_chunks_to_rag_budget(chunks: &mut Vec<Chunk>, limit: usize) {
    let mut rendered_bytes: usize =
        chunks.iter().map(|chunk| render_jsonl(std::slice::from_ref(chunk)).len()).sum();
    while rendered_bytes / 4 > limit && !chunks.is_empty() {
        let remove_at = lowest_value_chunk_index(chunks);
        rendered_bytes = rendered_bytes
            .saturating_sub(render_jsonl(std::slice::from_ref(&chunks[remove_at])).len());
        chunks.remove(remove_at);
    }
}

fn trim_chunks_to_rag_budget_for_task(chunks: &mut Vec<Chunk>, limit: usize, plan: &RetrievalPlan) {
    let mut rendered_bytes: usize = chunks
        .iter()
        .map(|chunk| render_jsonl_with_evidence(std::slice::from_ref(chunk), Some(plan)).len())
        .sum();
    while rendered_bytes / 4 > limit && !chunks.is_empty() {
        let remove_at = chunks
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                plan.score_for(left)
                    .unwrap_or(-1.0)
                    .partial_cmp(&plan.score_for(right).unwrap_or(-1.0))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        left.priority
                            .partial_cmp(&right.priority)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .then_with(|| right.path.cmp(&left.path))
                    .then_with(|| right.id.cmp(&left.id))
            })
            .map(|(index, _)| index)
            .unwrap_or(0);
        rendered_bytes = rendered_bytes.saturating_sub(
            render_jsonl_with_evidence(std::slice::from_ref(&chunks[remove_at]), Some(plan)).len(),
        );
        chunks.remove(remove_at);
    }
}

fn redact_task_for_markdown(task: &str, redactor: Option<&Redactor>) -> String {
    redactor
        .map(|redactor| redactor.redact_with_language_report(task, "", "", "", "").content)
        .unwrap_or_else(|| task.to_string())
}

fn task_header(
    task: &str,
    retrieval: Option<&RetrievalPlan>,
    redactor: Option<&Redactor>,
) -> String {
    let task = redact_task_for_markdown(task, redactor);
    let retrieval_note = retrieval
        .map(|plan| {
            format!(
                "> Retrieval: {} seed chunks, {} candidate files, {} candidate chunks\n\n",
                plan.seed_chunks, plan.candidate_files, plan.candidate_chunks
            )
        })
        .unwrap_or_default();
    format!("> Task: `{}`\n{}", task.replace('`', "'").replace(['\r', '\n'], " "), retrieval_note)
}

fn estimate_prefixed_tokens(prefix: &str, content: &str) -> usize {
    estimate_tokens(&format!("{prefix}{content}"))
}

fn cap_to_tokens(text: &str, limit: usize) -> String {
    let max_bytes = limit.saturating_mul(4);
    if text.len() <= max_bytes {
        return text.to_string();
    }
    text.char_indices().take_while(|(index, _)| *index < max_bytes).map(|(_, ch)| ch).collect()
}

fn select_pool_chunks<'a>(
    chunks: impl Iterator<Item = &'a Chunk>,
    limit: usize,
    kept: &mut Vec<Chunk>,
    kept_ids: &mut HashSet<String>,
) {
    let chunks: Vec<&Chunk> = chunks.collect();
    let mut used = 0usize;
    let mut seen_paths = HashSet::new();
    for first_per_file in [true, false] {
        for chunk in &chunks {
            if kept_ids.contains(&chunk.id)
                || first_per_file == seen_paths.contains(chunk.path.as_str())
                || used.saturating_add(chunk.token_estimate) > limit
            {
                continue;
            }
            used += chunk.token_estimate;
            seen_paths.insert(chunk.path.as_str());
            kept_ids.insert(chunk.id.clone());
            kept.push((*chunk).clone());
        }
    }
}

fn is_source_chunk(chunk: &Chunk) -> bool {
    is_programming_language(&chunk.language)
}

fn lowest_value_chunk_index(chunks: &[Chunk]) -> usize {
    let mut path_counts: HashMap<&str, usize> = HashMap::new();
    for chunk in chunks {
        *path_counts.entry(chunk.path.as_str()).or_insert(0) += 1;
    }
    let source_count = chunks.iter().filter(|chunk| is_source_chunk(chunk)).count();
    let has_context = chunks.iter().any(|chunk| !is_source_chunk(chunk));
    chunks
        .iter()
        .enumerate()
        .filter(|(_, chunk)| !(has_context && source_count == 1 && is_source_chunk(chunk)))
        .min_by(|(_, left), (_, right)| {
            let left_duplicate = path_counts.get(left.path.as_str()).copied().unwrap_or(0) > 1;
            let right_duplicate = path_counts.get(right.path.as_str()).copied().unwrap_or(0) > 1;
            right_duplicate
                .cmp(&left_duplicate)
                .then_with(|| {
                    left.priority.partial_cmp(&right.priority).unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| right.start_line.cmp(&left.start_line))
                .then_with(|| right.id.cmp(&left.id))
        })
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn lowest_value_chunk_index_for_task(chunks: &[Chunk], plan: &RetrievalPlan) -> usize {
    let mut path_counts: HashMap<&str, usize> = HashMap::new();
    for chunk in chunks {
        *path_counts.entry(chunk.path.as_str()).or_insert(0) += 1;
    }
    let source_count = chunks.iter().filter(|chunk| is_source_chunk(chunk)).count();
    let has_context = chunks.iter().any(|chunk| !is_source_chunk(chunk));
    chunks
        .iter()
        .enumerate()
        .filter(|(_, chunk)| !(has_context && source_count == 1 && is_source_chunk(chunk)))
        .min_by(|(_, left), (_, right)| {
            let left_duplicate = path_counts.get(left.path.as_str()).copied().unwrap_or(0) > 1;
            let right_duplicate = path_counts.get(right.path.as_str()).copied().unwrap_or(0) > 1;
            right_duplicate
                .cmp(&left_duplicate)
                .then_with(|| {
                    plan.score_for(left)
                        .unwrap_or(-1.0)
                        .partial_cmp(&plan.score_for(right).unwrap_or(-1.0))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| {
                    left.priority.partial_cmp(&right.priority).unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| right.start_line.cmp(&left.start_line))
                .then_with(|| right.id.cmp(&left.id))
        })
        .map(|(index, _)| index)
        .unwrap_or(0)
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
    let rag_paths: HashSet<&str> = chunks.iter().map(|c| c.path.as_str()).collect();
    let prompt_paths: HashSet<&str> = chunks
        .iter()
        .filter(|chunk| !chunk.tags.contains("rag-only"))
        .map(|chunk| chunk.path.as_str())
        .collect();
    for file in files {
        if let Some(d) = dispositions.iter_mut().find(|d| d.path == file.relative_path) {
            d.priority = Some(file.priority);
            d.token_estimate = Some(file.token_estimate);
            d.included_in_prompt = matches!(mode, OutputMode::Prompt | OutputMode::Both)
                && prompt_paths.contains(file.relative_path.as_str());
            d.included_in_rag = matches!(mode, OutputMode::Rag | OutputMode::Both)
                && rag_paths.contains(file.relative_path.as_str());
            d.reason = if should_prompt_summary_only(file) {
                FileDispositionReason::IncludedSummaryOnly
            } else if chunks
                .iter()
                .filter(|chunk| chunk.path == file.relative_path)
                .any(|chunk| chunk.chunks_in_file > 1)
            {
                FileDispositionReason::IncludedChunked
            } else {
                FileDispositionReason::IncludedFull
            };
        }
    }
}

fn unique_chunk_paths(chunks: &[Chunk], prompt_only: bool) -> usize {
    chunks
        .iter()
        .filter(|chunk| !prompt_only || !chunk.tags.contains("rag-only"))
        .map(|chunk| chunk.path.as_str())
        .collect::<HashSet<_>>()
        .len()
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

fn mark_focus_excluded(dispositions: &mut [FileDisposition], focus_paths: &HashSet<String>) {
    for item in dispositions {
        if item.reason == FileDispositionReason::IncludedFull && !focus_paths.contains(&item.path) {
            item.reason = FileDispositionReason::ExcludedFocus;
            item.included_in_prompt = false;
            item.included_in_rag = false;
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
    let repo_dir = base_dir.join(sanitize_output_component(repo_name, "repo"));
    module_basename
        .map(|entry| repo_dir.join(format!("focus_{}", sanitize_output_component(entry, "module"))))
        .unwrap_or(repo_dir)
}

fn repo_name_for_output(root_path: &Path, repo_url: Option<&str>) -> String {
    if let Some(url) = repo_url {
        if let Some(name) = repo_name_from_remote_url(url) {
            return name;
        }
    }
    sanitize_output_component(
        root_path.file_name().and_then(|n| n.to_str()).unwrap_or("repo"),
        "repo",
    )
}

fn repo_name_from_remote_url(url: &str) -> Option<String> {
    let trimmed = url.trim().split(['?', '#']).next().unwrap_or_default().trim_end_matches('/');
    let path = trimmed
        .strip_prefix("git@github.com:")
        .or_else(|| trimmed.rsplit_once('/').map(|(_, path)| path))?;
    let last = path.rsplit('/').next()?;
    let cleaned = last.strip_suffix(".git").unwrap_or(last);
    if cleaned.is_empty() {
        None
    } else {
        Some(sanitize_output_component(cleaned, "repo"))
    }
}

fn sanitize_output_component(value: &str, fallback: &str) -> String {
    let mut sanitized = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            sanitized.push(character);
        } else {
            sanitized.push('_');
        }
    }
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        fallback.to_string()
    } else {
        sanitized.to_string()
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

    let mut value = json!({
        "path": config.path,
        "repo": config.repo_url.as_ref().map(|u| redact_url_credentials(u)),
        "ref": config.ref_,
        "profile": match config.profile {
            crate::domain::ProjectProfile::Auto => "auto",
            crate::domain::ProjectProfile::Generic => "generic",
            crate::domain::ProjectProfile::Godot => "godot",
        },
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
    });
    if config.max_tokens.is_some() {
        value["token_budget_allocation"] = json!({
            "source_percent": 40,
            "context_percent": 60,
            "policy": "soft_reservation_with_borrowing",
        });
    }
    value
}

/// Redact secrets from manifest info (package.json scripts, etc.) that bypass
/// per-file redaction.
fn redact_manifest_info(
    mut info: HashMap<String, Value>,
    redactor: &Redactor,
) -> HashMap<String, Value> {
    // Redact string values recursively.
    fn redact_value(v: &mut Value, redactor: &Redactor) {
        match v {
            Value::String(s) => {
                let outcome = redactor.redact_with_language_report(s, "", "", "", "");
                *s = outcome.content;
            }
            Value::Object(map) => {
                for val in map.values_mut() {
                    redact_value(val, redactor);
                }
            }
            Value::Array(arr) => {
                for val in arr.iter_mut() {
                    redact_value(val, redactor);
                }
            }
            _ => {}
        }
    }

    for val in info.values_mut() {
        redact_value(val, redactor);
    }
    info
}

#[cfg(test)]
mod tests {
    use super::{apply_chunk_token_budget, refresh_output_accounting};
    use crate::domain::{
        Chunk, FileDisposition, FileDispositionReason, FileInfo, OutputMode, ScanStats,
    };
    use crate::redact::redactor::RedactionOccurrence;
    use std::collections::{BTreeSet, HashMap};
    use std::path::PathBuf;

    fn chunk(id: &str, path: &str, language: &str, tokens: usize) -> Chunk {
        Chunk {
            id: id.to_string(),
            path: path.to_string(),
            language: language.to_string(),
            start_line: 1,
            end_line: 1,
            content: "x".repeat(tokens * 4),
            priority: 1.0,
            tags: BTreeSet::new(),
            token_estimate: tokens,
            file_id: format!("file-{id}"),
            chunk_index: 0,
            chunks_in_file: 1,
            byte_start: Some(0),
            byte_end: Some(tokens * 4),
            content_sha256: String::new(),
            file_sha256: String::new(),
        }
    }

    fn file(path: &str, size_bytes: u64) -> FileInfo {
        FileInfo {
            path: PathBuf::from(path),
            relative_path: path.to_string(),
            size_bytes,
            extension: ".rs".to_string(),
            language: "rust".to_string(),
            id: format!("file-{path}"),
            priority: 1.0,
            token_estimate: 0,
            tags: BTreeSet::new(),
            is_readme: false,
            is_config: false,
            is_doc: false,
        }
    }

    #[test]
    fn context_chunks_borrow_an_unused_source_reservation() {
        let mut stats = ScanStats::default();
        let kept = apply_chunk_token_budget(
            vec![chunk("a", "README.md", "markdown", 55), chunk("b", "guide.md", "markdown", 40)],
            Some(100),
            &mut stats,
        );

        assert_eq!(kept.iter().map(|chunk| chunk.token_estimate).sum::<usize>(), 95);
    }

    #[test]
    fn oversized_source_share_still_keeps_source_before_context() {
        let mut stats = ScanStats::default();
        let kept = apply_chunk_token_budget(
            vec![
                chunk("source", "src/lib.rs", "rust", 45),
                chunk("context", "README.md", "markdown", 55),
            ],
            Some(100),
            &mut stats,
        );

        assert!(kept.iter().any(|chunk| chunk.id == "source"));
        assert_eq!(
            kept.iter()
                .filter(|chunk| chunk.language == "markdown")
                .map(|chunk| chunk.token_estimate)
                .sum::<usize>(),
            55
        );
    }

    #[test]
    fn partial_file_accounting_uses_emitted_span_and_original_chunk_count() {
        let selected = vec![file("src/lib.rs", 100)];
        let mut emitted = chunk("kept", "src/lib.rs", "rust", 5);
        emitted.chunks_in_file = 2;
        emitted.byte_end = Some(20);
        let mut stats = ScanStats::default();
        let mut dispositions = vec![FileDisposition::new(
            "src/lib.rs".to_string(),
            FileDispositionReason::IncludedFull,
        )];
        let redactions = HashMap::from([(
            "src/lib.rs".to_string(),
            vec![RedactionOccurrence {
                rule: "openai_key".to_string(),
                start: 50,
                end: 60,
                replacement: "[REDACTED_OPENAI_KEY]".to_string(),
            }],
        )]);

        refresh_output_accounting(
            &mut stats,
            &mut dispositions,
            &selected,
            &[emitted],
            OutputMode::Rag,
            &redactions,
        );

        assert_eq!(stats.total_bytes_included, 20);
        assert_eq!(stats.redacted_files, 0);
        assert!(stats.redaction_counts.is_empty());
        assert_eq!(dispositions[0].reason, FileDispositionReason::IncludedChunked);
    }

    #[test]
    fn prompt_accounting_excludes_rag_only_chunks() {
        let selected = vec![file("scene.tscn", 20)];
        let mut rag_only = chunk("rag", "scene.tscn", "godot_scene", 5);
        rag_only.tags.insert("rag-only".to_string());
        let redactions = HashMap::from([(
            "scene.tscn".to_string(),
            vec![RedactionOccurrence {
                rule: "openai_key".to_string(),
                start: 0,
                end: 5,
                replacement: "[REDACTED_OPENAI_KEY]".to_string(),
            }],
        )]);

        let mut prompt_stats = ScanStats::default();
        let mut prompt_dispositions = vec![FileDisposition::new(
            "scene.tscn".to_string(),
            FileDispositionReason::IncludedFull,
        )];
        let prompt_files = refresh_output_accounting(
            &mut prompt_stats,
            &mut prompt_dispositions,
            &selected,
            std::slice::from_ref(&rag_only),
            OutputMode::Prompt,
            &redactions,
        );

        assert!(prompt_files.is_empty());
        assert_eq!(prompt_stats.total_bytes_included, 0);
        assert_eq!(prompt_stats.redacted_chunks, 0);

        let mut rag_stats = ScanStats::default();
        let mut rag_dispositions = vec![FileDisposition::new(
            "scene.tscn".to_string(),
            FileDispositionReason::IncludedFull,
        )];
        refresh_output_accounting(
            &mut rag_stats,
            &mut rag_dispositions,
            &selected,
            &[rag_only],
            OutputMode::Rag,
            &redactions,
        );
        assert_eq!(rag_stats.total_bytes_included, 20);
        assert_eq!(rag_stats.redacted_chunks, 1);
    }

    #[test]
    fn overlapping_chunks_count_one_source_redaction_occurrence() {
        let selected = vec![file("src/lib.rs", 40)];
        let first = chunk("first", "src/lib.rs", "rust", 5);
        let mut second = chunk("second", "src/lib.rs", "rust", 5);
        second.chunk_index = 1;
        second.chunks_in_file = 2;
        let redactions = HashMap::from([(
            "src/lib.rs".to_string(),
            vec![RedactionOccurrence {
                rule: "openai_key".to_string(),
                start: 4,
                end: 12,
                replacement: "[REDACTED_OPENAI_KEY]".to_string(),
            }],
        )]);
        let mut stats = ScanStats::default();
        let mut dispositions = vec![FileDisposition::new(
            "src/lib.rs".to_string(),
            FileDispositionReason::IncludedFull,
        )];

        refresh_output_accounting(
            &mut stats,
            &mut dispositions,
            &selected,
            &[first, second],
            OutputMode::Prompt,
            &redactions,
        );

        assert_eq!(stats.redaction_counts["openai_key"], 1);
        assert_eq!(stats.redacted_chunks, 1);
    }

    #[test]
    fn one_derived_marker_does_not_count_two_source_occurrences() {
        let selected = vec![file("data.json", 40)];
        let mut derived = chunk("derived", "data.json", "json", 5);
        derived.byte_start = None;
        derived.byte_end = None;
        derived.content = "[CUSTOM_REDACTED]".to_string();
        let occurrence = |start| RedactionOccurrence {
            rule: "custom".to_string(),
            start,
            end: start + 5,
            replacement: "[CUSTOM_REDACTED]".to_string(),
        };
        let redactions =
            HashMap::from([("data.json".to_string(), vec![occurrence(5), occurrence(25)])]);
        let mut stats = ScanStats::default();
        let mut dispositions = vec![FileDisposition::new(
            "data.json".to_string(),
            FileDispositionReason::IncludedFull,
        )];

        refresh_output_accounting(
            &mut stats,
            &mut dispositions,
            &selected,
            &[derived],
            OutputMode::Rag,
            &redactions,
        );

        assert_eq!(stats.redaction_counts["custom"], 1);
        assert_eq!(stats.redacted_chunks, 1);
    }
}

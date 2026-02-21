//! Export command implementation

use anyhow::{Context, Result};
use clap::Args;
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::IsTerminal;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

use super::cache::remote_index_cache_db_path;
use super::guided::{choose_guided_plan, GuidedPlan};
use super::utils::{parse_csv, parse_csv_multi};
use crate::analysis::async_boundary::detect_async_boundaries;
use crate::analysis::pr::build_pr_context;
use crate::chunk::{chunk_content, coalesce_small_chunks_with_max};
use crate::config::{load_config, merge_cli_with_config, CliOverrides};
use crate::domain::{Chunk, OutputMode, RedactionMode};
use crate::fetch::fetch_repository;
use crate::graph::{lazy_loader::LazyChunkLoader, persist::persist_graph, schema::open_or_create};
use crate::rank::{
    dependency_graph, rank_files_with_manifest, rerank_chunks_by_task, stitch_thread_bundles,
    symbol_definitions, StitchTier,
};
use crate::redact::Redactor;
use crate::render::{render_context_pack, render_jsonl, write_report};
use crate::rerank::{build_reranker, normalize_scores};
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

    /// Allow always-include files to exceed max token budget
    #[arg(long)]
    pub allow_over_budget: bool,

    /// Fail when protected pins exceed max_tokens instead of pinned-only fallback
    #[arg(long)]
    pub strict_budget: bool,

    /// Always-include repository-relative paths (repeatable or comma-separated)
    #[arg(long, value_name = "PATHS", value_delimiter = ',', num_args = 1..)]
    pub always_include_path: Vec<String>,

    /// Always-include glob patterns (repeatable or comma-separated)
    #[arg(long, value_name = "GLOBS", value_delimiter = ',', num_args = 1..)]
    pub always_include_glob: Vec<String>,

    /// Replace invariant discovery keywords (repeatable or comma-separated)
    #[arg(long, value_name = "WORDS", value_delimiter = ',', num_args = 1..)]
    pub invariant_keywords: Vec<String>,

    /// Append invariant discovery keywords (repeatable or comma-separated)
    #[arg(long, value_name = "WORDS", value_delimiter = ',', num_args = 1..)]
    pub invariant_keywords_add: Vec<String>,

    /// Task description for retrieval-driven reranking
    #[arg(long, value_name = "TEXT")]
    pub task: Option<String>,

    /// Disable second-stage semantic reranking
    #[arg(long)]
    pub no_semantic_rerank: bool,

    /// Semantic model identifier
    #[arg(long, value_name = "MODEL")]
    pub semantic_model: Option<String>,

    /// Number of chunks to semantic-rerank
    #[arg(long, value_name = "N")]
    pub rerank_top_k: Option<usize>,

    /// Fraction of max tokens reserved for stitched context
    #[arg(long, value_name = "FLOAT")]
    pub stitch_budget_fraction: Option<f64>,

    /// Number of top-ranked chunks used as stitching seeds
    #[arg(long, value_name = "N")]
    pub stitch_top_n: Option<usize>,

    /// Target tokens per chunk
    #[arg(long, value_name = "TOKENS")]
    pub chunk_tokens: Option<usize>,

    /// Overlap tokens between adjacent chunks
    #[arg(long, value_name = "TOKENS")]
    pub chunk_overlap: Option<usize>,

    /// Coalesce chunks smaller than this
    #[arg(long, value_name = "TOKENS")]
    pub min_chunk_tokens: Option<usize>,

    /// Output format: 'prompt' (Markdown), 'rag' (JSONL), 'contribution', 'pr-context', or 'both'
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

    /// Skip writing persisted graph database
    #[arg(long)]
    pub no_graph: bool,

    /// Skip interactive guided mode and run quick export defaults
    #[arg(long)]
    pub quick: bool,

    /// Prefer loading files/chunks from local index when it is fresh
    #[arg(long)]
    pub from_index: bool,

    /// Require a fresh local index when using --from-index
    #[arg(long)]
    pub require_fresh_index: bool,
}

pub fn run(args: ExportArgs) -> Result<()> {
    let start_time = Instant::now();

    let interactive_terminal = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let guided_enabled = !args.quick && interactive_terminal;
    if !args.quick && !interactive_terminal {
        eprintln!(
            "info: non-interactive session detected; using quick export defaults (same as --quick)"
        );
    }

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
        task_query: args.task.clone(),
        semantic_rerank: if args.no_semantic_rerank { Some(false) } else { None },
        rerank_top_k: args.rerank_top_k,
        semantic_model: args.semantic_model.clone(),
        stitch_budget_fraction: args.stitch_budget_fraction,
        stitch_top_n: args.stitch_top_n,
        chunk_tokens: args.chunk_tokens,
        chunk_overlap: args.chunk_overlap,
        min_chunk_tokens: args.min_chunk_tokens,
        mode,
        output_dir: args.output_dir.clone(),
        tree_depth: args.tree_depth,
        redact_secrets: if args.no_redact { Some(false) } else { None },
        redaction_mode,
        always_include_patterns: None,
        always_include_paths: None,
        invariant_keywords: None,
    };

    let mut merged = merge_cli_with_config(file_config, cli_overrides);

    let cli_pin_paths = parse_csv_multi(&args.always_include_path);
    for path in cli_pin_paths {
        if !merged.always_include_paths.contains(&path) {
            merged.always_include_paths.push(path);
        }
    }

    let cli_pin_globs = parse_csv_multi(&args.always_include_glob);
    for pattern in cli_pin_globs {
        if !merged.always_include_patterns.contains(&pattern) {
            merged.always_include_patterns.push(pattern);
        }
    }

    let cli_keywords = parse_csv_multi(&args.invariant_keywords);
    if !cli_keywords.is_empty() {
        merged.invariant_keywords = cli_keywords;
    }
    let cli_keywords_add = parse_csv_multi(&args.invariant_keywords_add);
    for keyword in cli_keywords_add {
        if !merged.invariant_keywords.contains(&keyword) {
            merged.invariant_keywords.push(keyword);
        }
    }

    let contribution_mode = matches!(merged.mode, OutputMode::Contribution | OutputMode::PrContext);
    if contribution_mode {
        for pattern in default_contribution_globs() {
            if !merged.always_include_patterns.contains(&pattern) {
                merged.always_include_patterns.push(pattern);
            }
        }
        for path in default_contribution_paths() {
            if !merged.always_include_paths.contains(&path) {
                merged.always_include_paths.push(path);
            }
        }
    }

    if merged.path.is_none() && merged.repo_url.is_none() {
        anyhow::bail!("Either --path or --repo must be specified");
    }

    let repo_ctx = fetch_repository(
        merged.path.as_deref(),
        merged.repo_url.as_deref(),
        merged.ref_.as_deref(),
    )?;
    let root_path = repo_ctx.root_path.clone();
    let index_db_path = resolve_index_db_path(&root_path, &merged);
    let lazy_loader = index_db_path.as_deref().map(LazyChunkLoader::new);

    let index_state = evaluate_index_state(index_db_path.as_deref(), &root_path, &merged);
    let mut used_index_dataset = false;
    let (mut stats, ranked_files, manifest_info) = if args.from_index {
        match index_state.kind {
            IndexFreshness::Fresh | IndexFreshness::Stale => {
                if index_state.kind == IndexFreshness::Stale {
                    if args.require_fresh_index {
                        anyhow::bail!(
                            "fresh index required but unavailable: {}",
                            index_state.reason.as_deref().unwrap_or("unknown")
                        );
                    }
                    if let Some(reason) = index_state.reason.as_deref() {
                        eprintln!("info: using stale index dataset ({reason})");
                    }
                }
                let db_path = index_state
                    .db_path
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("index state missing db path"))?;
                println!("info: using index dataset from {}", db_path.display());
                let (stats, files) = load_files_and_stats_from_index(db_path, &root_path)?;
                used_index_dataset = true;
                let (ranked_files, manifest_info) =
                    rank_files_with_manifest(&root_path, files, merged.ranking_weights.clone())?;
                (stats, ranked_files, manifest_info)
            }
            _ => {
                if args.require_fresh_index {
                    anyhow::bail!(
                        "fresh index required but unavailable: {}",
                        index_state.reason.as_deref().unwrap_or("unknown")
                    );
                }
                if let Some(reason) = index_state.reason.as_deref() {
                    eprintln!("info: index not fresh ({reason}); falling back to scan export");
                }
                collect_scan_inputs(&root_path, &merged)?
            }
        }
    } else {
        collect_scan_inputs(&root_path, &merged)?
    };
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

    if guided_enabled {
        let plan = choose_guided_plan(&root_path, &stats, &ranked_files)?;
        apply_guided_plan(&mut merged, &args, &plan);
    }

    let pin_plan = if contribution_mode {
        Some(build_pin_plan(
            &root_path,
            &ranked_files,
            &merged.always_include_paths,
            &merged.always_include_patterns,
            &merged.invariant_keywords,
        )?)
    } else {
        None
    };

    let protected_paths = pin_plan.as_ref().map(|plan| plan.protected_paths()).unwrap_or_default();

    let mut selected_files =
        apply_byte_budget(ranked_files, Some(merged.max_total_bytes), &mut stats, &protected_paths);

    if let Some(plan) = pin_plan.as_ref() {
        stats.pinned_files = selected_files
            .iter()
            .filter_map(|file| {
                let tier = plan.tier_for(&file.relative_path)?;
                Some(HashMap::from([
                    ("path".to_string(), json!(file.relative_path)),
                    ("tier".to_string(), json!(tier.as_str())),
                    (
                        "reason".to_string(),
                        json!(plan.reason_for(&file.relative_path).unwrap_or("protected")),
                    ),
                ]))
            })
            .collect();
    }

    let chunk_tokens = merged.chunk_tokens;
    let chunk_overlap = merged.chunk_overlap;
    let redactor = if merged.redact_secrets {
        Some(build_redactor(merged.redaction_mode, &merged.redaction))
    } else {
        None
    };
    let always_include =
        if contribution_mode { None } else { build_globset(&merged.always_include_patterns)? };
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut always_indices = Vec::new();
    let mut advisory_indices = Vec::new();
    let mut normal_indices = Vec::new();
    for (idx, file) in selected_files.iter().enumerate() {
        let pin_tier = pin_plan.as_ref().and_then(|plan| plan.tier_for(&file.relative_path));
        if matches!(pin_tier, Some(PinTier::Tier0 | PinTier::Tier1)) {
            always_indices.push(idx);
        } else if matches!(pin_tier, Some(PinTier::Tier2)) {
            advisory_indices.push(idx);
        } else if always_include.as_ref().map(|g| g.is_match(&file.relative_path)).unwrap_or(false)
        {
            always_indices.push(idx);
        } else {
            normal_indices.push(idx);
        }
    }

    let mut always_tokens = 0usize;
    for idx in always_indices {
        if let Some(file_chunks) = process_file_for_export(
            &mut selected_files[idx],
            used_index_dataset,
            lazy_loader.as_ref(),
            redactor.as_ref(),
            chunk_tokens,
            chunk_overlap,
            &mut stats,
        )? {
            let file_tokens: usize = file_chunks.iter().map(|c| c.token_estimate).sum();
            always_tokens += file_tokens;
            chunks.extend(file_chunks);
        }
    }

    let mut pinned_only_mode = false;
    if let Some(max_tokens) = merged.max_tokens {
        if always_tokens > max_tokens {
            let overflow = always_tokens.saturating_sub(max_tokens);
            stats.pinned_overflow_tokens = overflow;
            if contribution_mode {
                if args.strict_budget {
                    anyhow::bail!(
                        "protected pin files require {always_tokens} tokens but max_tokens={max_tokens}; increase budget or remove --strict-budget"
                    );
                }
                pinned_only_mode = true;
                stats.pinned_only_mode = true;
                eprintln!(
                    "info: protected pins exceed max_tokens by {overflow}; writing pinned-only contribution pack"
                );
            } else if !args.allow_over_budget {
                anyhow::bail!(
                    "always-include files require {always_tokens} tokens but max_tokens={max_tokens}; use --allow-over-budget to proceed"
                );
            }
        }
    }

    let mut normal_tokens = 0usize;
    let mut remaining_budget = merged.max_tokens.map(|max| max.saturating_sub(always_tokens));
    if let (Some(max_tokens), Some(rest)) = (merged.max_tokens, remaining_budget) {
        if always_tokens > max_tokens {
            eprintln!(
                "Warning: always-include files use {} tokens above max_tokens={} (remaining budget: {})",
                always_tokens.saturating_sub(max_tokens),
                max_tokens,
                rest
            );
            remaining_budget = Some(0);
        }
    }

    let mut budgeted_indices = Vec::new();
    if !pinned_only_mode {
        budgeted_indices.extend(advisory_indices);
        budgeted_indices.extend(normal_indices);
    }

    for idx in budgeted_indices {
        let Some(file_chunks) = process_file_for_export(
            &mut selected_files[idx],
            used_index_dataset,
            lazy_loader.as_ref(),
            redactor.as_ref(),
            chunk_tokens,
            chunk_overlap,
            &mut stats,
        )?
        else {
            continue;
        };

        let file_tokens: usize = file_chunks.iter().map(|c| c.token_estimate).sum();
        if let Some(budget) = remaining_budget {
            if normal_tokens + file_tokens > budget {
                stats.files_dropped_budget += 1;
                stats.dropped_files.push(std::collections::HashMap::from([
                    ("path".to_string(), json!(selected_files[idx].relative_path)),
                    (
                        "reason".to_string(),
                        json!(if pin_plan
                            .as_ref()
                            .and_then(|plan| plan.tier_for(&selected_files[idx].relative_path))
                            == Some(PinTier::Tier2)
                        {
                            "token_budget_tier2"
                        } else {
                            "token_budget"
                        }),
                    ),
                    (
                        "priority".to_string(),
                        json!((selected_files[idx].priority * 1000.0).round() / 1000.0),
                    ),
                    ("tokens".to_string(), json!(file_tokens)),
                    ("chunks".to_string(), json!(file_chunks.len())),
                ]));
                continue;
            }
        }
        normal_tokens += file_tokens;
        chunks.extend(file_chunks);
    }

    let min_chunk_tokens = merged.min_chunk_tokens;
    chunks = coalesce_small_chunks_with_max(chunks, min_chunk_tokens, chunk_tokens);
    let workspace_members = extract_workspace_members(&manifest_info);

    let mut reranking_mode: Option<String> = None;
    let mut stitched_unavailable_chunks: usize = 0;
    if let Some(task_query) = merged.task_query.as_deref() {
        let file_scores = rerank_chunks_by_task(&mut chunks, task_query, 0.4);
        reranking_mode = Some("bm25+deps".to_string());
        chunks.sort_by(|a, b| {
            b.priority
                .partial_cmp(&a.priority)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.start_line.cmp(&b.start_line))
                .then_with(|| a.id.cmp(&b.id))
        });

        for (idx, chunk) in chunks.iter_mut().enumerate() {
            chunk.tags.insert(format!("reason:bm25(rank={})", idx + 1));
        }

        if merged.semantic_rerank {
            let reranker = build_reranker(merged.semantic_model.as_deref());
            let top_k = merged.rerank_top_k.min(chunks.len());
            let semantic_scores = reranker.rerank(task_query, &chunks[..top_k])?;
            let normalized = normalize_scores(&semantic_scores);
            for (chunk, score) in chunks[..top_k].iter_mut().zip(normalized.into_iter()) {
                chunk.priority =
                    (((chunk.priority * 0.6) + (score * 0.4)) * 1000.0).round() / 1000.0;
                chunk.tags.insert(format!("reason:semantic(score={:.3})", score));
            }
            chunks.sort_by(|a, b| {
                b.priority
                    .partial_cmp(&a.priority)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.path.cmp(&b.path))
                    .then_with(|| a.start_line.cmp(&b.start_line))
                    .then_with(|| a.id.cmp(&b.id))
            });
            reranking_mode = Some(format!("bm25+{}", reranker.name()));
        }

        if let Some(max_tokens) = merged.max_tokens {
            let effective_tokens = max_tokens.saturating_sub(always_tokens);
            let budget =
                ((effective_tokens as f64) * merged.stitch_budget_fraction).round() as usize;
            let stitch = stitch_thread_bundles(
                &chunks,
                merged.stitch_top_n,
                budget,
                lazy_loader.as_ref(),
                &workspace_members,
            );
            if !stitch.lazy_chunks.is_empty() {
                chunks.extend(stitch.lazy_chunks.iter().cloned());
            }
            for chunk in &mut chunks {
                if let Some(tier) = stitch.stitched.get(&chunk.id) {
                    chunk.tags.insert(format!("stitch:{}", tier.as_str()));
                    chunk.tags.insert(format!("reason:stitched({})", tier.as_str()));
                }
            }
            stats.stitched_chunks = stitch.stitched.len();
            let dropped_chunks: usize = stats
                .dropped_files
                .iter()
                .filter(|dropped| {
                    dropped.get("reason").and_then(|v| v.as_str()) == Some("token_budget")
                })
                .map(|dropped| dropped.get("chunks").and_then(|v| v.as_u64()).unwrap_or(0) as usize)
                .sum();
            stitched_unavailable_chunks = dropped_chunks.saturating_sub(stitch.lazy_chunks.len());

            sort_chunks_for_stitch_story(&mut chunks, &stitch.seed_ids, &stitch.stitched);

            if stats.stitched_chunks > 0 {
                println!(
                    "  Thread stitching: {} chunks (~{} tokens reserved)",
                    stats.stitched_chunks, stitch.tokens_used
                );
            }
        }

        for file in &mut selected_files {
            if let Some(task_score) = file_scores.get(&file.relative_path) {
                file.priority =
                    (((file.priority * 0.6) + (task_score * 0.4)) * 1000.0).round() / 1000.0;
            }
        }
        selected_files.sort_by(|a, b| {
            b.priority
                .partial_cmp(&a.priority)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.relative_path.cmp(&b.relative_path))
        });

        stats.top_ranked_files = selected_files
            .iter()
            .take(20)
            .map(|f| {
                std::collections::HashMap::from([
                    ("path".to_string(), json!(f.relative_path)),
                    ("priority".to_string(), json!(f.priority)),
                ])
            })
            .collect();
    }

    for boundary in detect_async_boundaries(&chunks) {
        if let Some(chunk) = chunks.iter_mut().find(|c| c.id == boundary.chunk_id) {
            for pattern in boundary.patterns {
                chunk.tags.insert(pattern.tag().to_string());
            }
        }
    }

    stats.chunks_created = chunks.len();
    stats.total_tokens_estimated = chunks.iter().map(|c| c.token_estimate).sum();

    let output_dir = resolve_output_dir(&merged.output_dir, &root_path, merged.repo_url.as_deref());
    let repo_name = repo_name_for_output(&root_path, merged.repo_url.as_deref());
    fs::create_dir_all(&output_dir)?;
    let mut graph_written: Option<(PathBuf, usize, usize)> = None;
    if !args.no_graph {
        if let Some(index_db) = index_db_path.as_ref() {
            if let Some((symbols, edges)) = query_graph_stats(index_db) {
                println!(
                    "info: using index.sqlite graph ({} symbols, {} import edges)",
                    symbols, edges
                );
                graph_written = Some((index_db.clone(), symbols, edges));
            } else {
                println!(
                    "info: index.sqlite exists but graph tables are missing; using pack-only graph."
                );
                let graph_path =
                    output_dir.join(prefixed_output_file_name(&repo_name, "symbol_graph.db"));
                match open_or_create(&graph_path) {
                    Ok(mut conn) => match persist_graph(&mut conn, &chunks) {
                        Ok((symbols, edges)) => {
                            graph_written = Some((graph_path, symbols, edges));
                        }
                        Err(err) => {
                            eprintln!("[graph] Warning: failed to persist graph: {err}");
                        }
                    },
                    Err(err) => {
                        eprintln!("[graph] Warning: failed to open graph DB: {err}");
                    }
                }
            }
        } else {
            println!(
                "info: no index.sqlite found — using pack-only graph. Run 'repo-context index' for full graph + better stitching."
            );
            let graph_path =
                output_dir.join(prefixed_output_file_name(&repo_name, "symbol_graph.db"));
            match open_or_create(&graph_path) {
                Ok(mut conn) => match persist_graph(&mut conn, &chunks) {
                    Ok((symbols, edges)) => {
                        graph_written = Some((graph_path, symbols, edges));
                    }
                    Err(err) => {
                        eprintln!("[graph] Warning: failed to persist graph: {err}");
                    }
                },
                Err(err) => {
                    eprintln!("[graph] Warning: failed to open graph DB: {err}");
                }
            }
        }
    }

    let highlight: HashSet<String> = selected_files
        .iter()
        .filter(|f| f.priority >= 0.8)
        .map(|f| f.relative_path.clone())
        .collect();
    let tree = generate_tree(&root_path, merged.tree_depth, true, &highlight)?;

    let pr_report = if matches!(merged.mode, OutputMode::PrContext) {
        Some(build_pr_context(
            &selected_files,
            &chunks,
            merged.task_query.as_deref(),
            graph_written.is_some(),
        ))
    } else {
        None
    };

    let context_pack = render_context_pack(
        &root_path,
        &selected_files,
        &chunks,
        &stats,
        &tree,
        &manifest_info,
        merged.task_query.as_deref(),
        pr_report.as_ref(),
        !args.no_timestamp,
    );
    let jsonl = render_jsonl(&chunks);

    let mut output_files = Vec::new();
    if matches!(
        merged.mode,
        OutputMode::Prompt | OutputMode::Both | OutputMode::Contribution | OutputMode::PrContext
    ) {
        let p = output_dir.join(prefixed_output_file_name(&repo_name, "context_pack.md"));
        fs::write(&p, context_pack)?;
        output_files.push(p.display().to_string());
    }
    if matches!(
        merged.mode,
        OutputMode::Rag | OutputMode::Both | OutputMode::Contribution | OutputMode::PrContext
    ) {
        let p = output_dir.join(prefixed_output_file_name(&repo_name, "chunks.jsonl"));
        fs::write(&p, jsonl)?;
        output_files.push(p.display().to_string());
    }
    if let Some((graph_path, symbols, edges)) = &graph_written {
        println!("[graph] {}: {symbols} symbols, {edges} import edges", graph_path.display());
        output_files.push(graph_path.display().to_string());
    }

    let report_path = output_dir.join(prefixed_output_file_name(&repo_name, "report.json"));
    // Record processing time before writing the report so the value is correct in report.json.
    stats.processing_time_seconds = start_time.elapsed().as_secs_f64();

    // Build curated config dict for report.json.
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
        let mode_val = serde_json::to_value(merged.mode)?;
        let task_val = merged.task_query.clone();
        let mut always_include_patterns = merged.always_include_patterns.clone();
        always_include_patterns.sort();
        let mut always_include_paths = merged.always_include_paths.clone();
        always_include_paths.sort();
        let mut invariant_keywords = merged.invariant_keywords.clone();
        invariant_keywords.sort();
        json!({
            "chunk_overlap":        merged.chunk_overlap,
            "chunk_tokens":         merged.chunk_tokens,
            "stitch_budget_fraction": merged.stitch_budget_fraction,
            "stitch_top_n":         merged.stitch_top_n,
            "exclude_globs":        exclude_globs_val,
            "follow_symlinks":      merged.follow_symlinks,
            "include_extensions":   include_extensions_val,
            "max_file_bytes":       merged.max_file_bytes,
            "max_tokens":           merged.max_tokens,
            "allow_over_budget":    args.allow_over_budget,
            "strict_budget":        args.strict_budget,
            "max_total_bytes":      merged.max_total_bytes,
            "semantic_rerank":      merged.semantic_rerank,
            "semantic_model":       merged.semantic_model,
            "rerank_top_k":         merged.rerank_top_k,
            "mode":                 mode_val,
            "path":                 path_val,
            "task_query":           task_val,
            "reranking":            reranking_mode,
            "redact_secrets":       merged.redact_secrets,
            "ref":                  merged.ref_.clone(),
            "repo":                 merged.repo_url.clone(),
            "skip_minified":        merged.skip_minified,
            "tree_depth":           merged.tree_depth,
            "always_include_patterns": always_include_patterns,
            "always_include_paths": always_include_paths,
            "invariant_keywords":   invariant_keywords,
            "pinned_only_mode":     stats.pinned_only_mode,
            "from_index":           args.from_index,
            "require_fresh_index":  args.require_fresh_index,
        })
    };

    let provenance =
        build_provenance(&root_path, &merged, &config_dict, &index_state, used_index_dataset);
    let coverage = build_coverage_report(
        &root_path,
        &selected_files,
        &chunks,
        &stats,
        &provenance,
        index_db_path.as_deref(),
    );

    write_report(
        &report_path,
        &root_path,
        &stats,
        &selected_files,
        &output_files,
        &config_dict,
        !args.no_timestamp,
        Some(&provenance),
        Some(&coverage),
    )?;
    output_files.push(report_path.display().to_string());

    // --- Print export summary ---
    println!();
    println!("Export complete!");
    println!();
    println!("Statistics:");
    println!("  Repository:      {}", root_path.display());
    println!(
        "  Index status:    {}{}",
        index_state.kind.as_str(),
        if used_index_dataset { " (used)" } else { "" }
    );
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
        if stitched_unavailable_chunks > 0 {
            println!(
                "  {} stitched chunks unavailable (file dropped pre-budget)",
                stitched_unavailable_chunks
            );
        }
    }
    println!("  Chunks created:  {}", stats.chunks_created);
    println!("  Total bytes:     {}", stats.total_bytes_included);
    println!("  Total tokens:    ~{}", stats.total_tokens_estimated);
    if let Some(task_query) = merged.task_query.as_deref() {
        if let Some(mode) = reranking_mode.as_deref() {
            println!("  Task reranking:  {mode} ({task_query})");
        } else {
            println!("  Task reranking:  bm25+deps ({task_query})");
        }
    }
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

fn resolve_output_dir(config_output: &Path, root_path: &Path, repo_url: Option<&str>) -> PathBuf {
    let repo_name = repo_name_for_output(root_path, repo_url);
    let normalized = config_output.to_string_lossy().replace('\\', "/");

    let base = if normalized.is_empty() || normalized == "./out" || normalized == "out" {
        PathBuf::from("out")
    } else {
        config_output.to_path_buf()
    };

    // Always namespace by repo name unless the path already ends with it
    // (matches Python's get_repo_output_dir in cli.py:93-109).
    if base.file_name().and_then(|n| n.to_str()) == Some(repo_name.as_str()) {
        base
    } else {
        base.join(repo_name)
    }
}

fn repo_name_for_output(root_path: &Path, repo_url: Option<&str>) -> String {
    if let Some(url) = repo_url {
        if let Some(repo_name) = repo_name_from_remote_url(url) {
            return repo_name;
        }
    }

    root_path.file_name().and_then(|n| n.to_str()).unwrap_or("repo").to_string()
}

fn repo_name_from_remote_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }

    let without_query = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
    let without_tree =
        without_query.rsplit_once("/tree/").map(|(prefix, _)| prefix).unwrap_or(without_query);
    let base = without_tree.trim_end_matches('/');
    if base.is_empty() {
        return None;
    }

    let repo_segment = base.rsplit('/').next().unwrap_or(base);
    let repo_name = repo_segment.strip_suffix(".git").unwrap_or(repo_segment);
    let repo_name = repo_name.trim();

    if repo_name.is_empty() {
        None
    } else {
        Some(repo_name.to_string())
    }
}

fn apply_guided_plan(merged: &mut crate::domain::Config, args: &ExportArgs, plan: &GuidedPlan) {
    if args.mode.is_none() {
        if let Some(mode) = plan.mode {
            merged.mode = mode;
        }
    }

    if args.max_tokens.is_none() {
        if let Some(max_tokens) = plan.max_tokens {
            merged.max_tokens = Some(max_tokens);
        }
    }

    if args.task.is_none() {
        if let Some(task_query) = plan.task_query.as_ref() {
            merged.task_query = Some(task_query.clone());
        }
    }

    if args.stitch_budget_fraction.is_none() {
        if let Some(stitch_budget_fraction) = plan.stitch_budget_fraction {
            merged.stitch_budget_fraction = stitch_budget_fraction;
        }
    }

    if args.stitch_top_n.is_none() {
        if let Some(stitch_top_n) = plan.stitch_top_n {
            merged.stitch_top_n = stitch_top_n;
        }
    }

    if args.rerank_top_k.is_none() {
        if let Some(rerank_top_k) = plan.rerank_top_k {
            merged.rerank_top_k = rerank_top_k;
        }
    }
}

fn prefixed_output_file_name(repo_name: &str, base_name: &str) -> String {
    format!("{repo_name}_{base_name}")
}

fn build_provenance(
    root_path: &Path,
    merged: &crate::domain::Config,
    config: &serde_json::Value,
    index_state: &IndexState,
    used_index_dataset: bool,
) -> serde_json::Value {
    let mut config_for_hash = config.clone();
    if let Some(obj) = config_for_hash.as_object_mut() {
        obj.remove("path");
        obj.remove("output_dir");
    }
    let config_hash = stable_json_hash(&config_for_hash);
    let git = git2::Repository::discover(root_path).ok();
    let commit = git
        .as_ref()
        .and_then(|repo| repo.head().ok())
        .and_then(|head| head.target())
        .map(|oid| oid.to_string());
    let branch = git
        .as_ref()
        .and_then(|repo| repo.head().ok())
        .and_then(|head| head.shorthand().map(|name| name.to_string()));
    let repo_identity = merged
        .repo_url
        .clone()
        .or_else(|| {
            git.as_ref().and_then(|repo| {
                repo.find_remote("origin")
                    .ok()
                    .and_then(|remote| remote.url().map(|url| url.to_string()))
            })
        })
        .unwrap_or_else(|| repo_name_for_output(root_path, merged.repo_url.as_deref()));
    let mut hasher = Sha256::new();
    hasher.update(&repo_identity);
    if let Some(ref_) = merged.ref_.as_ref() {
        hasher.update(ref_);
    }
    if let Some(commit) = commit.as_ref() {
        hasher.update(commit);
    }
    hasher.update(&config_hash);
    hasher.update(env!("CARGO_PKG_VERSION"));
    let fingerprint = format!("{:x}", hasher.finalize());

    json!({
        "repo": merged.repo_url.clone().or(Some(repo_identity)),
        "path": root_path.display().to_string(),
        "ref": merged.ref_.clone(),
        "git_branch": branch,
        "git_commit": commit,
        "config_hash": config_hash,
        "tool_version": env!("CARGO_PKG_VERSION"),
        "fingerprint": fingerprint,
        "index": {
            "status": index_state.kind.as_str(),
            "reason": index_state.reason.clone(),
            "db_path": index_state.db_path.as_ref().map(|p| p.display().to_string()),
            "used_for_export": used_index_dataset,
        }
    })
}

fn stable_json_hash(value: &serde_json::Value) -> String {
    let canonical = serde_json::to_vec(value).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(&canonical);
    format!("{:x}", hasher.finalize())
}

fn build_coverage_report(
    root_path: &Path,
    selected_files: &[crate::domain::FileInfo],
    chunks: &[Chunk],
    stats: &crate::domain::ScanStats,
    provenance: &serde_json::Value,
    index_db_path: Option<&Path>,
) -> serde_json::Value {
    let dropped_paths: Vec<String> = stats
        .dropped_files
        .iter()
        .filter_map(|entry| entry.get("path").and_then(|v| v.as_str()).map(|p| p.to_string()))
        .collect();
    let included_paths: HashSet<String> =
        selected_files.iter().map(|f| f.relative_path.clone()).collect();
    let most_imported_not_included = most_imported_not_included(
        index_db_path,
        &dropped_paths,
        &included_paths,
        &stats.dropped_files,
    );

    let public_api = public_api_coverage(root_path, selected_files, stats);
    let hot_paths = hot_paths_from_tests_examples(chunks);
    let mut missing_context_todos = Vec::new();
    for entry in stats.dropped_files.iter().take(15) {
        if let Some(path) = entry.get("path").and_then(|v| v.as_str()) {
            let reason = entry.get("reason").and_then(|v| v.as_str()).unwrap_or("unknown");
            missing_context_todos.push(json!({"path": path, "reason": reason}));
        }
    }

    if stats.pinned_only_mode {
        missing_context_todos.push(json!({
            "path": "<export>",
            "reason": "pinned-only fallback activated; increase --max-tokens for broader coverage"
        }));
    }

    json!({
        "most_imported_not_included": most_imported_not_included,
        "public_api_surface_coverage": public_api,
        "hot_paths_from_tests_examples": hot_paths,
        "missing_context_todos": missing_context_todos,
        "fingerprint": provenance.get("fingerprint").cloned().unwrap_or(json!(null)),
    })
}

fn most_imported_not_included(
    index_db_path: Option<&Path>,
    dropped_paths: &[String],
    included_paths: &HashSet<String>,
    dropped_entries: &[HashMap<String, serde_json::Value>],
) -> Vec<serde_json::Value> {
    if dropped_paths.is_empty() {
        return Vec::new();
    }

    let dropped_set: HashSet<String> = dropped_paths.iter().cloned().collect();
    let mut dropped_reason: HashMap<String, String> = HashMap::new();
    let mut dropped_priority: HashMap<String, serde_json::Value> = HashMap::new();
    for entry in dropped_entries {
        let Some(path) = entry.get("path").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(reason) = entry.get("reason").and_then(|v| v.as_str()) {
            dropped_reason.insert(path.to_string(), reason.to_string());
        }
        if let Some(priority) = entry.get("priority") {
            dropped_priority.insert(path.to_string(), priority.clone());
        }
    }

    let mut inbound_all: HashMap<String, usize> = HashMap::new();
    let mut inbound_from_included: HashMap<String, usize> = HashMap::new();

    if let Some(db_path) = index_db_path {
        if let Ok(conn) = rusqlite::Connection::open(db_path) {
            if let Ok(mut stmt) = conn.prepare("SELECT source_path, target_path FROM file_imports")
            {
                if let Ok(rows) = stmt.query_map([], |row| {
                    let source: String = row.get(0)?;
                    let target: String = row.get(1)?;
                    Ok((source, target))
                }) {
                    for (source, target) in rows.flatten() {
                        if !dropped_set.contains(&target) {
                            continue;
                        }
                        *inbound_all.entry(target.clone()).or_insert(0) += 1;
                        if included_paths.contains(&source) {
                            *inbound_from_included.entry(target).or_insert(0) += 1;
                        }
                    }
                }
            }
        }
    }

    let mut ranked: Vec<(String, usize, usize)> = dropped_set
        .iter()
        .map(|path| {
            (
                path.clone(),
                *inbound_all.get(path).unwrap_or(&0),
                *inbound_from_included.get(path).unwrap_or(&0),
            )
        })
        .collect();
    ranked.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| b.1.cmp(&a.1)).then_with(|| a.0.cmp(&b.0)));

    ranked
        .into_iter()
        .take(10)
        .map(|(path, inbound, inbound_included)| {
            let reason =
                dropped_reason.get(&path).cloned().unwrap_or_else(|| "unknown".to_string());
            let priority = dropped_priority.get(&path).cloned().unwrap_or(json!(null));
            json!({
                "path": path,
                "reason": reason,
                "priority": priority,
                "incoming_edges": inbound,
                "incoming_edges_from_included": inbound_included,
            })
        })
        .collect()
}

fn public_api_coverage(
    root_path: &Path,
    files: &[crate::domain::FileInfo],
    stats: &crate::domain::ScanStats,
) -> serde_json::Value {
    let included_pub_items = count_pub_items_in_files(files);

    let mut estimated_total_pub_items = included_pub_items;
    let mut dropped_rs_files = 0usize;
    for dropped in &stats.dropped_files {
        let Some(path) = dropped.get("path").and_then(|v| v.as_str()) else {
            continue;
        };
        if !path.ends_with(".rs") {
            continue;
        }
        dropped_rs_files += 1;
        if let Ok((content, _)) = read_file_safe(&root_path.join(path), None, None) {
            estimated_total_pub_items += count_pub_items_in_content(&content);
        }
    }

    let coverage_ratio = if estimated_total_pub_items == 0 {
        1.0
    } else {
        (included_pub_items as f64) / (estimated_total_pub_items as f64)
    };

    json!({
        "language": "rust",
        "included_pub_items": included_pub_items,
        "estimated_total_pub_items": estimated_total_pub_items,
        "coverage_ratio": ((coverage_ratio * 1000.0).round() / 1000.0),
        "dropped_rust_files_considered": dropped_rs_files,
        "notes": "Coverage counts Rust public items in included files and estimates dropped Rust files from disk when readable.",
    })
}

fn count_pub_items_in_files(files: &[crate::domain::FileInfo]) -> usize {
    let mut included_pub_items = 0usize;
    for file in files {
        if file.extension != ".rs" {
            continue;
        }
        if let Ok((content, _)) = read_file_safe(&file.path, None, None) {
            included_pub_items += count_pub_items_in_content(&content);
        }
    }
    included_pub_items
}

fn count_pub_items_in_content(content: &str) -> usize {
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("pub ")
                || trimmed.starts_with("pub(")
                || trimmed.starts_with("pub(crate)")
        })
        .count()
}

fn hot_paths_from_tests_examples(chunks: &[Chunk]) -> Vec<serde_json::Value> {
    let known_files: HashSet<String> = chunks.iter().map(|c| c.path.clone()).collect();
    if known_files.is_empty() {
        return Vec::new();
    }
    let defs = symbol_definitions(chunks);
    let graph = dependency_graph(chunks, &known_files, &defs);

    let mut counts: HashMap<String, usize> = HashMap::new();
    for (source, targets) in &graph {
        if !is_test_or_example_path(source) {
            continue;
        }
        for target in targets {
            if is_test_or_example_path(target) {
                continue;
            }
            *counts.entry(target.clone()).or_insert(0) += 1;
        }
    }

    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
        .into_iter()
        .take(10)
        .map(|(path, refs)| json!({"path": path, "incoming_from_tests_examples": refs}))
        .collect()
}

fn is_test_or_example_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("tests/")
        || lower.contains("/tests/")
        || lower.starts_with("examples/")
        || lower.contains("/examples/")
        || lower.contains("_test")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndexFreshness {
    Fresh,
    Stale,
    Missing,
    Error,
}

impl IndexFreshness {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Missing => "missing",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
struct IndexState {
    kind: IndexFreshness,
    db_path: Option<PathBuf>,
    reason: Option<String>,
}

fn collect_scan_inputs(
    root_path: &Path,
    merged: &crate::domain::Config,
) -> Result<(
    crate::domain::ScanStats,
    Vec<crate::domain::FileInfo>,
    HashMap<String, serde_json::Value>,
)> {
    let mut scanner = FileScanner::new(root_path.to_path_buf())
        .max_file_bytes(merged.max_file_bytes)
        .respect_gitignore(merged.respect_gitignore)
        .follow_symlinks(merged.follow_symlinks)
        .skip_minified(merged.skip_minified)
        .include_extensions(merged.include_extensions.iter().cloned().collect())
        .exclude_globs(merged.exclude_globs.iter().cloned().collect());

    let scanned_files = scanner.scan()?;
    let stats = scanner.stats().clone();
    let (ranked_files, manifest_info) =
        rank_files_with_manifest(root_path, scanned_files, merged.ranking_weights.clone())?;

    Ok((stats, ranked_files, manifest_info))
}

fn evaluate_index_state(
    index_db_path: Option<&Path>,
    root_path: &Path,
    merged: &crate::domain::Config,
) -> IndexState {
    let Some(db_path) = index_db_path else {
        return IndexState {
            kind: IndexFreshness::Missing,
            db_path: None,
            reason: Some("index.sqlite not found".to_string()),
        };
    };

    let conn = match rusqlite::Connection::open(db_path) {
        Ok(conn) => conn,
        Err(err) => {
            return IndexState {
                kind: IndexFreshness::Error,
                db_path: Some(db_path.to_path_buf()),
                reason: Some(format!("failed opening index db: {err}")),
            };
        }
    };

    let metadata = load_index_metadata_map(&conn);
    let expected_hash = export_index_config_hash(merged);
    let stored_hash = metadata.get("config_hash").cloned().unwrap_or_default();
    if stored_hash != expected_hash {
        return IndexState {
            kind: IndexFreshness::Stale,
            db_path: Some(db_path.to_path_buf()),
            reason: Some("config hash mismatch".to_string()),
        };
    }

    let current_commit = git2::Repository::discover(root_path).ok().and_then(|repo| {
        let head = repo.head().ok()?;
        head.target().map(|oid| oid.to_string())
    });
    let stored_commit =
        metadata.get("git_commit").cloned().unwrap_or_else(|| "unknown".to_string());
    if stored_commit != "unknown" && current_commit.as_deref() != Some(stored_commit.as_str()) {
        return IndexState {
            kind: IndexFreshness::Stale,
            db_path: Some(db_path.to_path_buf()),
            reason: Some("git commit mismatch".to_string()),
        };
    }

    IndexState {
        kind: IndexFreshness::Fresh,
        db_path: Some(db_path.to_path_buf()),
        reason: Some("metadata matches current config and commit".to_string()),
    }
}

fn load_files_and_stats_from_index(
    db_path: &Path,
    root_path: &Path,
) -> Result<(crate::domain::ScanStats, Vec<crate::domain::FileInfo>)> {
    let conn = rusqlite::Connection::open(db_path)
        .with_context(|| format!("Failed to open index database at {}", db_path.display()))?;
    let metadata = load_index_metadata_map(&conn);

    let mut stmt = conn.prepare(
        "SELECT path, language, extension, size_bytes, priority, token_estimate FROM files",
    )?;
    let rows = stmt.query_map([], |row| {
        let rel_path: String = row.get(0)?;
        let language: String = row.get(1)?;
        let extension: String = row.get(2)?;
        let size_bytes: i64 = row.get(3)?;
        let priority: f64 = row.get(4)?;
        let token_estimate: i64 = row.get(5)?;
        Ok((rel_path, language, extension, size_bytes, priority, token_estimate))
    })?;

    let mut files = Vec::new();
    let mut languages_detected: HashMap<String, usize> = HashMap::new();
    let mut total_bytes_included = 0_u64;
    for row in rows {
        let (relative_path, language, extension, size_bytes, priority, token_estimate) = row?;
        total_bytes_included = total_bytes_included.saturating_add(size_bytes.max(0) as u64);
        *languages_detected.entry(language.clone()).or_insert(0) += 1;
        files.push(crate::domain::FileInfo {
            path: root_path.join(&relative_path),
            relative_path: relative_path.clone(),
            size_bytes: size_bytes.max(0) as u64,
            extension,
            language,
            id: format!("idx:{relative_path}"),
            priority,
            token_estimate: token_estimate.max(0) as usize,
            tags: BTreeSet::new(),
            is_readme: false,
            is_config: false,
            is_doc: false,
        });
    }

    let files_scanned =
        metadata.get("files_scanned").and_then(|v| v.parse::<usize>().ok()).unwrap_or(files.len());

    let stats = crate::domain::ScanStats {
        files_scanned,
        files_included: files.len(),
        total_bytes_scanned: total_bytes_included,
        total_bytes_included,
        languages_detected,
        ..crate::domain::ScanStats::default()
    };
    Ok((stats, files))
}

fn load_index_metadata_map(conn: &rusqlite::Connection) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Ok(mut stmt) = conn.prepare("SELECT key, value FROM metadata") {
        if let Ok(rows) = stmt.query_map([], |row| {
            let key: String = row.get(0)?;
            let value: String = row.get(1)?;
            Ok((key, value))
        }) {
            for (key, value) in rows.flatten() {
                out.insert(key, value);
            }
        }
    }
    out
}

fn export_index_config_hash(config: &crate::domain::Config) -> String {
    let payload = json!({
        "include_extensions": config.include_extensions,
        "exclude_globs": config.exclude_globs,
        "max_file_bytes": config.max_file_bytes,
        "max_total_bytes": config.max_total_bytes,
        "respect_gitignore": config.respect_gitignore,
        "follow_symlinks": config.follow_symlinks,
        "skip_minified": config.skip_minified,
        "chunk_tokens": config.chunk_tokens,
        "chunk_overlap": config.chunk_overlap,
        "min_chunk_tokens": config.min_chunk_tokens,
    });
    stable_json_hash(&payload)
}

fn resolve_index_db_path(root_path: &Path, merged: &crate::domain::Config) -> Option<PathBuf> {
    let local = find_index_db(root_path);
    let cached = remote_index_cache_db_path(
        merged.repo_url.as_deref(),
        merged.ref_.as_deref(),
        &export_index_config_hash(merged),
    )
    .filter(|path| path.exists());

    match (local, cached) {
        (Some(local_path), Some(cached_path)) => {
            let local_meta = fs::metadata(&local_path).and_then(|m| m.modified()).ok();
            let cached_meta = fs::metadata(&cached_path).and_then(|m| m.modified()).ok();
            if cached_meta.is_some() && cached_meta >= local_meta {
                Some(cached_path)
            } else {
                Some(local_path)
            }
        }
        (Some(local_path), None) => Some(local_path),
        (None, Some(cached_path)) => Some(cached_path),
        (None, None) => None,
    }
}

fn find_index_db(root_path: &Path) -> Option<PathBuf> {
    let candidate = root_path.join(".repo-context").join("index.sqlite");
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

fn query_graph_stats(db_path: &Path) -> Option<(usize, usize)> {
    let conn = rusqlite::Connection::open(db_path).ok()?;
    let symbols = conn
        .query_row("SELECT COUNT(*) FROM symbol_chunks", [], |row| row.get::<_, i64>(0))
        .ok()? as usize;
    let edges = conn
        .query_row("SELECT COUNT(*) FROM file_imports", [], |row| row.get::<_, i64>(0))
        .ok()? as usize;
    Some((symbols, edges))
}

fn extract_workspace_members(
    manifest_info: &std::collections::HashMap<String, serde_json::Value>,
) -> Vec<String> {
    let Some(value) = manifest_info.get("cargo_workspace_members") else {
        return Vec::new();
    };
    let mut members: Vec<String> = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(ToString::to_string)
        .collect();
    members.sort();
    members.dedup();
    members
}

fn process_file_for_export(
    file: &mut crate::domain::FileInfo,
    use_index_first: bool,
    lazy_loader: Option<&LazyChunkLoader>,
    redactor: Option<&Redactor>,
    chunk_tokens: usize,
    chunk_overlap: usize,
    stats: &mut crate::domain::ScanStats,
) -> Result<Option<Vec<Chunk>>> {
    if use_index_first {
        if let Some(index_chunks) =
            process_export_file_from_index(file, lazy_loader, redactor, stats)?
        {
            return Ok(Some(index_chunks));
        }
    }

    process_export_file(file, redactor, chunk_tokens, chunk_overlap, stats)
}

fn process_export_file_from_index(
    file: &mut crate::domain::FileInfo,
    lazy_loader: Option<&LazyChunkLoader>,
    redactor: Option<&Redactor>,
    stats: &mut crate::domain::ScanStats,
) -> Result<Option<Vec<Chunk>>> {
    let Some(loader) = lazy_loader else {
        return Ok(None);
    };
    let mut file_chunks = loader.load_chunks_for_file(&file.relative_path);
    if file_chunks.is_empty() {
        return Ok(None);
    }

    if let Some(r) = redactor {
        let filename = file.path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !r.is_file_allowlisted(filename, &file.relative_path) {
            let mut rule_file_sets: BTreeMap<String, HashSet<String>> = BTreeMap::new();
            for chunk in &mut file_chunks {
                let original = chunk.content.clone();
                let outcome = r.redact_with_language_report(
                    &chunk.content,
                    &file.language,
                    &file.extension,
                    filename,
                    &file.relative_path,
                );
                if outcome.content != original {
                    chunk.content = outcome.content;
                    chunk.tags.insert("redacted".to_string());
                    stats.redacted_chunks += 1;
                    for (rule, count) in &outcome.counts {
                        *stats.redaction_counts.entry(rule.clone()).or_insert(0) += count;
                        rule_file_sets
                            .entry(rule.clone())
                            .or_default()
                            .insert(file.relative_path.clone());
                    }
                }
            }
            if !rule_file_sets.is_empty() {
                stats.redacted_files += 1;
                for (rule, file_set) in rule_file_sets {
                    *stats.redaction_file_counts.entry(rule).or_insert(0) += file_set.len();
                }
            }
        }
    }

    file.token_estimate = file_chunks.iter().map(|c| c.token_estimate).sum();
    Ok(Some(file_chunks))
}

fn process_export_file(
    file: &mut crate::domain::FileInfo,
    redactor: Option<&Redactor>,
    chunk_tokens: usize,
    chunk_overlap: usize,
    stats: &mut crate::domain::ScanStats,
) -> Result<Option<Vec<Chunk>>> {
    let (content, _enc) = match read_file_safe(&file.path, None, None) {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };

    let redacted_content = if let Some(r) = redactor {
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

    let mut file_chunks = chunk_content(file, &redacted_content, chunk_tokens, chunk_overlap)?;
    let file_tokens: usize = file_chunks.iter().map(|c| c.token_estimate).sum();
    file.token_estimate = file_tokens;

    if redactor.is_some() {
        for chunk in &mut file_chunks {
            if chunk.content.contains("[REDACTED") || chunk.content.contains("_REDACTED]") {
                chunk.tags.insert("redacted".to_string());
                stats.redacted_chunks += 1;
            }
        }
    }

    Ok(Some(file_chunks))
}

fn sort_group(
    chunk: &Chunk,
    seed_ids: &std::collections::BTreeSet<String>,
    stitched: &std::collections::HashMap<String, StitchTier>,
) -> u8 {
    if seed_ids.contains(&chunk.id) {
        return 0;
    }
    match stitched.get(&chunk.id) {
        Some(StitchTier::Definition) => 1,
        Some(StitchTier::Callee) => 2,
        Some(StitchTier::Caller) => 3,
        Some(StitchTier::CrossCrate) => 4,
        None => 5,
    }
}

fn sort_chunks_for_stitch_story(
    chunks: &mut [Chunk],
    seed_ids: &std::collections::BTreeSet<String>,
    stitched: &std::collections::HashMap<String, StitchTier>,
) {
    chunks.sort_by(|a, b| {
        let a_key = sort_group(a, seed_ids, stitched);
        let b_key = sort_group(b, seed_ids, stitched);
        a_key
            .cmp(&b_key)
            .then_with(|| b.priority.partial_cmp(&a.priority).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.start_line.cmp(&b.start_line))
            .then_with(|| a.id.cmp(&b.id))
    });
}

fn parse_mode(mode: Option<&str>) -> Result<OutputMode> {
    match mode.unwrap_or("both").to_ascii_lowercase().as_str() {
        "prompt" => Ok(OutputMode::Prompt),
        "rag" => Ok(OutputMode::Rag),
        "contribution" => Ok(OutputMode::Contribution),
        "pr-context" | "pr_context" | "prcontext" => Ok(OutputMode::PrContext),
        "both" => Ok(OutputMode::Both),
        invalid => {
            anyhow::bail!("Invalid mode '{invalid}'. Use: prompt|rag|contribution|pr-context|both")
        }
    }
}

fn default_contribution_globs() -> Vec<String> {
    [
        "examples/**",
        ".github/PULL_REQUEST_TEMPLATE*",
        ".github/ISSUE_TEMPLATE/**",
        ".github/workflows/**",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn default_contribution_paths() -> Vec<String> {
    [
        "README.md",
        "CONTRIBUTING.md",
        "CONTRIBUTING.rst",
        "SECURITY.md",
        "CODE_OF_CONDUCT.md",
        "Cargo.toml",
        "pyproject.toml",
        "package.json",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PinTier {
    Tier0,
    Tier1,
    Tier2,
}

impl PinTier {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tier0 => "tier0",
            Self::Tier1 => "tier1",
            Self::Tier2 => "tier2",
        }
    }
}

#[derive(Debug, Clone)]
struct PinPlan {
    tiers: HashMap<String, PinTier>,
    reasons: HashMap<String, String>,
}

impl PinPlan {
    fn protected_paths(&self) -> HashSet<String> {
        self.tiers
            .iter()
            .filter_map(|(path, tier)| {
                if matches!(tier, PinTier::Tier0 | PinTier::Tier1) {
                    Some(path.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    fn tier_for(&self, path: &str) -> Option<PinTier> {
        self.tiers.get(path).copied()
    }

    fn reason_for(&self, path: &str) -> Option<&str> {
        self.reasons.get(path).map(String::as_str)
    }

    fn promote(&mut self, path: &str, tier: PinTier, reason: String) {
        let should_update = match self.tiers.get(path) {
            None => true,
            Some(existing) => tier_rank(tier) < tier_rank(*existing),
        };
        if should_update {
            self.tiers.insert(path.to_string(), tier);
            self.reasons.insert(path.to_string(), reason);
        }
    }
}

fn tier_rank(tier: PinTier) -> u8 {
    match tier {
        PinTier::Tier0 => 0,
        PinTier::Tier1 => 1,
        PinTier::Tier2 => 2,
    }
}

fn build_pin_plan(
    root_path: &Path,
    ranked_files: &[crate::domain::FileInfo],
    explicit_paths: &[String],
    explicit_globs: &[String],
    invariant_keywords: &[String],
) -> Result<PinPlan> {
    const MAX_GLOB_PROTECTED: usize = 80;

    let mut plan = PinPlan { tiers: HashMap::new(), reasons: HashMap::new() };
    let normalized_paths: HashSet<String> =
        explicit_paths.iter().map(|p| normalize_rel_path(p)).filter(|p| !p.is_empty()).collect();

    for file in ranked_files {
        let path = file.relative_path.as_str();
        if normalized_paths.contains(path) {
            plan.promote(path, PinTier::Tier0, "explicit_path".to_string());
        }
        if is_tier0_contract_path(path) {
            plan.promote(path, PinTier::Tier0, "auto_contract_path".to_string());
        }
    }

    let globset = build_globset(explicit_globs)?;
    if let Some(globset) = globset {
        let mut matches: Vec<&crate::domain::FileInfo> =
            ranked_files.iter().filter(|f| globset.is_match(&f.relative_path)).collect();
        matches.sort_by(|a, b| {
            b.priority
                .partial_cmp(&a.priority)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.relative_path.cmp(&b.relative_path))
        });

        for (idx, file) in matches.into_iter().enumerate() {
            if idx < MAX_GLOB_PROTECTED {
                plan.promote(&file.relative_path, PinTier::Tier1, "explicit_glob".to_string());
            } else {
                plan.promote(
                    &file.relative_path,
                    PinTier::Tier2,
                    "explicit_glob_rate_limited".to_string(),
                );
            }
        }
    }

    for file in ranked_files {
        if plan.tier_for(&file.relative_path) == Some(PinTier::Tier0) {
            continue;
        }
        let Some((score, evidence)) = invariant_score(root_path, file, invariant_keywords) else {
            continue;
        };
        if score >= 9 {
            plan.promote(
                &file.relative_path,
                PinTier::Tier1,
                format!("invariant_score:{score} ({evidence})"),
            );
        } else if score >= 6 {
            plan.promote(
                &file.relative_path,
                PinTier::Tier2,
                format!("invariant_score:{score} ({evidence})"),
            );
        }
    }

    Ok(plan)
}

fn normalize_rel_path(path: &str) -> String {
    path.trim().trim_start_matches("./").replace('\\', "/")
}

fn is_tier0_contract_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    if ["readme.md", "contributing.md", "security.md", "code_of_conduct.md"].contains(&file) {
        return true;
    }
    if lower == "cargo.toml" || lower.ends_with("/cargo.toml") {
        return true;
    }
    lower.ends_with("schema.json")
        || lower.ends_with("schema.yaml")
        || lower.ends_with("schema.yml")
}

fn invariant_score(
    root_path: &Path,
    file: &crate::domain::FileInfo,
    keywords: &[String],
) -> Option<(usize, String)> {
    let mut score = 0usize;
    let mut evidence = Vec::new();
    let path_lower = file.relative_path.to_ascii_lowercase();

    if file.is_readme {
        score += 2;
        evidence.push("readme");
    }
    if file.is_config {
        score += 3;
        evidence.push("config");
    }
    if file.is_doc {
        score += 2;
        evidence.push("doc");
    }
    if file.tags.contains("entrypoint") {
        score += 2;
        evidence.push("entrypoint");
    }

    for (needle, weight) in [
        ("schema", 3usize),
        ("contract", 3),
        ("invariant", 3),
        ("api", 2),
        ("error", 2),
        ("architecture", 3),
        ("example", 2),
        ("safety", 3),
    ] {
        if path_lower.contains(needle) {
            score += weight;
            evidence.push(needle);
        }
    }

    if !(file.is_doc || file.is_config || path_lower.contains("test")) {
        if score < 6 {
            return None;
        }
    }

    if file.size_bytes <= 256_000 {
        if let Ok((content, _)) = read_file_safe(&root_path.join(&file.relative_path), None, None) {
            let lower = content.to_ascii_lowercase();
            let mut keyword_hits = 0usize;
            for keyword in keywords {
                let k = keyword.to_ascii_lowercase();
                if k.is_empty() {
                    continue;
                }
                if lower.contains(&k) {
                    keyword_hits += 1;
                }
            }
            if keyword_hits > 0 {
                score += keyword_hits.min(4);
                evidence.push("keywords");
            }
        }
    }

    if score == 0 {
        None
    } else {
        Some((score, evidence.join("+")))
    }
}

fn build_globset(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern)?);
    }
    Ok(Some(builder.build()?))
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
    protected_paths: &HashSet<String>,
) -> Vec<crate::domain::FileInfo> {
    let Some(limit) = max_total_bytes else {
        return ranked_files;
    };

    let mut selected = Vec::new();
    let mut total = 0_u64;
    for (idx, file) in ranked_files.iter().enumerate() {
        if protected_paths.contains(&file.relative_path) {
            total += file.size_bytes;
            selected.push(file.clone());
            continue;
        }
        // Python checks >= BEFORE adding the current file (cumulative of already-accepted bytes)
        if total >= limit {
            // Bulk-drop this file and all remaining files
            for remaining in &ranked_files[idx..] {
                if protected_paths.contains(&remaining.relative_path) {
                    total += remaining.size_bytes;
                    selected.push(remaining.clone());
                    continue;
                }
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

#[cfg(test)]
mod tests {
    use super::{
        apply_guided_plan, build_pin_plan, most_imported_not_included, repo_name_for_output,
        repo_name_from_remote_url, sort_chunks_for_stitch_story, ExportArgs, GuidedPlan, PinTier,
    };
    use crate::domain::{Chunk, Config, OutputMode};
    use crate::rank::StitchTier;
    use rusqlite::Connection;
    use std::collections::{BTreeSet, HashMap};
    use std::path::Path;

    fn mk_chunk(id: &str, priority: f64, path: &str, start_line: usize) -> Chunk {
        Chunk {
            id: id.to_string(),
            path: path.to_string(),
            language: "rust".to_string(),
            start_line,
            end_line: start_line,
            content: "fn x() {}".to_string(),
            priority,
            tags: BTreeSet::new(),
            token_estimate: 10,
        }
    }

    #[test]
    fn stitch_story_sort_orders_seed_then_tiers_then_rest() {
        let mut chunks = vec![
            mk_chunk("rest", 0.99, "z.rs", 1),
            mk_chunk("callee_hi", 0.95, "b.rs", 2),
            mk_chunk("seed_hi", 0.70, "a.rs", 2),
            mk_chunk("caller", 0.90, "c.rs", 1),
            mk_chunk("def_lo", 0.60, "d.rs", 10),
            mk_chunk("seed_lo", 0.20, "a.rs", 1),
            mk_chunk("def_hi", 0.85, "d.rs", 1),
            mk_chunk("callee_lo", 0.30, "b.rs", 1),
        ];

        let seed_ids = BTreeSet::from(["seed_hi".to_string(), "seed_lo".to_string()]);
        let stitched = HashMap::from([
            ("def_hi".to_string(), StitchTier::Definition),
            ("def_lo".to_string(), StitchTier::Definition),
            ("callee_hi".to_string(), StitchTier::Callee),
            ("callee_lo".to_string(), StitchTier::Callee),
            ("caller".to_string(), StitchTier::Caller),
        ]);

        sort_chunks_for_stitch_story(&mut chunks, &seed_ids, &stitched);
        let ordered: Vec<&str> = chunks.iter().map(|c| c.id.as_str()).collect();

        assert_eq!(
            ordered,
            vec![
                "seed_hi",
                "seed_lo",
                "def_hi",
                "def_lo",
                "callee_hi",
                "callee_lo",
                "caller",
                "rest"
            ]
        );
    }

    #[test]
    fn repo_name_from_remote_url_extracts_repo_segment() {
        assert_eq!(
            repo_name_from_remote_url("https://github.com/owner/repo"),
            Some("repo".to_string())
        );
        assert_eq!(
            repo_name_from_remote_url("https://github.com/owner/repo.git"),
            Some("repo".to_string())
        );
        assert_eq!(
            repo_name_from_remote_url("https://github.com/owner/repo/tree/main"),
            Some("repo".to_string())
        );
        assert_eq!(
            repo_name_from_remote_url("https://huggingface.co/spaces/gradio/demo/tree/main"),
            Some("demo".to_string())
        );
    }

    #[test]
    fn repo_name_for_output_prefers_remote_name_for_temp_clone_paths() {
        let temp_clone_root = Path::new("/tmp/repo-context-123456789");
        let repo_name =
            repo_name_for_output(temp_clone_root, Some("https://github.com/acme/important-repo"));

        assert_eq!(repo_name, "important-repo");
    }

    fn default_args() -> ExportArgs {
        ExportArgs {
            path: None,
            repo: None,
            ref_: None,
            config: None,
            include_ext: None,
            exclude_glob: None,
            max_file_bytes: None,
            max_total_bytes: None,
            no_gitignore: false,
            follow_symlinks: false,
            include_minified: false,
            max_tokens: None,
            allow_over_budget: false,
            strict_budget: false,
            always_include_path: Vec::new(),
            always_include_glob: Vec::new(),
            invariant_keywords: Vec::new(),
            invariant_keywords_add: Vec::new(),
            task: None,
            no_semantic_rerank: false,
            semantic_model: None,
            rerank_top_k: None,
            stitch_budget_fraction: None,
            stitch_top_n: None,
            chunk_tokens: None,
            chunk_overlap: None,
            min_chunk_tokens: None,
            mode: None,
            output_dir: None,
            no_timestamp: false,
            tree_depth: None,
            no_redact: false,
            redaction_mode: None,
            no_graph: false,
            quick: false,
            from_index: false,
            require_fresh_index: false,
        }
    }

    #[test]
    fn guided_plan_applies_defaults_when_cli_not_explicit() {
        let mut cfg = Config::default();
        let args = default_args();
        let plan = GuidedPlan {
            mode: Some(OutputMode::Both),
            max_tokens: Some(140_000),
            task_query: Some("architecture and dependencies".to_string()),
            stitch_budget_fraction: Some(0.45),
            stitch_top_n: Some(40),
            rerank_top_k: Some(300),
        };

        apply_guided_plan(&mut cfg, &args, &plan);

        assert_eq!(cfg.mode, OutputMode::Both);
        assert_eq!(cfg.max_tokens, Some(140_000));
        assert_eq!(cfg.task_query.as_deref(), Some("architecture and dependencies"));
        assert_eq!(cfg.stitch_budget_fraction, 0.45);
        assert_eq!(cfg.stitch_top_n, 40);
        assert_eq!(cfg.rerank_top_k, 300);
    }

    #[test]
    fn guided_plan_does_not_override_explicit_cli_flags() {
        let mut cfg = Config {
            mode: OutputMode::Prompt,
            max_tokens: Some(50_000),
            task_query: Some("explicit task".to_string()),
            stitch_budget_fraction: 0.2,
            stitch_top_n: 10,
            rerank_top_k: 42,
            ..Config::default()
        };
        let mut args = default_args();
        args.mode = Some("prompt".to_string());
        args.max_tokens = Some(50_000);
        args.task = Some("explicit task".to_string());
        args.stitch_budget_fraction = Some(0.2);
        args.stitch_top_n = Some(10);
        args.rerank_top_k = Some(42);

        let plan = GuidedPlan {
            mode: Some(OutputMode::Both),
            max_tokens: Some(140_000),
            task_query: Some("architecture and dependencies".to_string()),
            stitch_budget_fraction: Some(0.45),
            stitch_top_n: Some(40),
            rerank_top_k: Some(300),
        };

        apply_guided_plan(&mut cfg, &args, &plan);

        assert_eq!(cfg.mode, OutputMode::Prompt);
        assert_eq!(cfg.max_tokens, Some(50_000));
        assert_eq!(cfg.task_query.as_deref(), Some("explicit task"));
        assert_eq!(cfg.stitch_budget_fraction, 0.2);
        assert_eq!(cfg.stitch_top_n, 10);
        assert_eq!(cfg.rerank_top_k, 42);
    }

    #[test]
    fn pin_plan_marks_explicit_paths_as_tier0() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        std::fs::write(tmp.path().join("README.md"), "# Readme\n").expect("write readme");
        let file = crate::domain::FileInfo {
            path: tmp.path().join("README.md"),
            relative_path: "README.md".to_string(),
            size_bytes: 8,
            extension: ".md".to_string(),
            language: "markdown".to_string(),
            id: "id1".to_string(),
            priority: 0.9,
            token_estimate: 4,
            tags: BTreeSet::new(),
            is_readme: true,
            is_config: false,
            is_doc: true,
        };
        let plan = build_pin_plan(
            tmp.path(),
            &[file],
            &["README.md".to_string()],
            &[],
            &["must".to_string()],
        )
        .expect("pin plan");

        assert_eq!(plan.tier_for("README.md"), Some(PinTier::Tier0));
    }

    #[test]
    fn most_imported_not_included_prefers_incoming_edges_from_included() {
        let tmp = tempfile::TempDir::new().expect("tmp");
        let db = tmp.path().join("index.sqlite");
        let conn = Connection::open(&db).expect("open db");
        conn.execute(
            "CREATE TABLE file_imports (source_path TEXT NOT NULL, target_path TEXT NOT NULL)",
            [],
        )
        .expect("create table");
        conn.execute(
            "INSERT INTO file_imports (source_path, target_path) VALUES ('src/a.rs','src/x.rs')",
            [],
        )
        .expect("insert edge 1");
        conn.execute(
            "INSERT INTO file_imports (source_path, target_path) VALUES ('src/b.rs','src/x.rs')",
            [],
        )
        .expect("insert edge 2");
        conn.execute(
            "INSERT INTO file_imports (source_path, target_path) VALUES ('tests/t.rs','src/y.rs')",
            [],
        )
        .expect("insert edge 3");

        let dropped_paths = vec!["src/x.rs".to_string(), "src/y.rs".to_string()];
        let included_paths = std::collections::HashSet::from(["src/a.rs".to_string()]);
        let dropped_entries = vec![
            HashMap::from([
                ("path".to_string(), serde_json::json!("src/x.rs")),
                ("reason".to_string(), serde_json::json!("token_budget")),
            ]),
            HashMap::from([
                ("path".to_string(), serde_json::json!("src/y.rs")),
                ("reason".to_string(), serde_json::json!("token_budget")),
            ]),
        ];

        let rows = most_imported_not_included(
            Some(&db),
            &dropped_paths,
            &included_paths,
            &dropped_entries,
        );

        assert_eq!(rows[0]["path"], serde_json::json!("src/x.rs"));
        assert_eq!(rows[0]["incoming_edges_from_included"], serde_json::json!(1));
    }
}

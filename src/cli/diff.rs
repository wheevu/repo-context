//! Diff command for comparing two export outputs.

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct DiffArgs {
    /// Path to older export output directory
    #[arg(value_name = "BEFORE")]
    pub before: PathBuf,

    /// Path to newer export output directory
    #[arg(value_name = "AFTER")]
    pub after: PathBuf,

    /// Output format: text, markdown, or json
    #[arg(long, value_enum, default_value = "text")]
    pub format: DiffFormat,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum DiffFormat {
    Text,
    Markdown,
    Json,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ReportFile {
    id: String,
    path: String,
    priority: f64,
    tokens: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct ReportDoc {
    #[serde(default)]
    files: Vec<ReportFile>,
}

#[derive(Debug, Clone, Deserialize)]
struct ChunkRow {
    id: String,
    path: String,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ModifiedFile {
    path: String,
    before_priority: f64,
    after_priority: f64,
    before_tokens: usize,
    after_tokens: usize,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct GraphDelta {
    added_symbols: usize,
    removed_symbols: usize,
    added_imports: usize,
    removed_imports: usize,
}

#[derive(Debug, Clone, Serialize)]
struct DiffSummary {
    before: String,
    after: String,
    files_added: usize,
    files_removed: usize,
    files_modified: usize,
    tokens_before: usize,
    tokens_after: usize,
    tokens_delta: isize,
    chunks_before: usize,
    chunks_after: usize,
    chunks_delta: isize,
    changed_chunk_tags: usize,
    moved_chunks: usize,
    added_files: Vec<ReportFile>,
    removed_files: Vec<ReportFile>,
    modified_files: Vec<ModifiedFile>,
    graph: Option<GraphDelta>,
}

pub fn run(args: DiffArgs) -> Result<()> {
    let before_report = read_report(&args.before)?;
    let after_report = read_report(&args.after)?;

    let before_chunks = read_chunks(&args.before)?;
    let after_chunks = read_chunks(&args.after)?;

    let before_by_path: HashMap<String, ReportFile> =
        before_report.files.into_iter().map(|f| (f.path.clone(), f)).collect();
    let after_by_path: HashMap<String, ReportFile> =
        after_report.files.into_iter().map(|f| (f.path.clone(), f)).collect();

    let mut added_files: Vec<ReportFile> =
        after_by_path.values().filter(|f| !before_by_path.contains_key(&f.path)).cloned().collect();
    let mut removed_files: Vec<ReportFile> =
        before_by_path.values().filter(|f| !after_by_path.contains_key(&f.path)).cloned().collect();

    let mut modified_files = Vec::new();
    for (path, before) in &before_by_path {
        if let Some(after) = after_by_path.get(path) {
            if before.id != after.id
                || (before.priority - after.priority).abs() >= 0.001
                || before.tokens != after.tokens
            {
                modified_files.push(ModifiedFile {
                    path: before.path.clone(),
                    before_priority: before.priority,
                    after_priority: after.priority,
                    before_tokens: before.tokens,
                    after_tokens: after.tokens,
                });
            }
        }
    }

    added_files.sort_by(|a, b| a.path.cmp(&b.path));
    removed_files.sort_by(|a, b| a.path.cmp(&b.path));
    modified_files.sort_by(|a, b| a.path.cmp(&b.path));

    let before_total_tokens: usize = before_by_path.values().map(|f| f.tokens).sum();
    let after_total_tokens: usize = after_by_path.values().map(|f| f.tokens).sum();

    let before_chunk_count = before_chunks.len();
    let after_chunk_count = after_chunks.len();

    let before_chunks_by_id: HashMap<&str, &ChunkRow> =
        before_chunks.iter().map(|row| (row.id.as_str(), row)).collect();
    let after_chunks_by_id: HashMap<&str, &ChunkRow> =
        after_chunks.iter().map(|row| (row.id.as_str(), row)).collect();

    let mut tag_changes = 0usize;
    let mut moved_chunks = 0usize;
    for (id, before) in &before_chunks_by_id {
        if let Some(after) = after_chunks_by_id.get(id) {
            let a: BTreeSet<&str> = before.tags.iter().map(String::as_str).collect();
            let b: BTreeSet<&str> = after.tags.iter().map(String::as_str).collect();
            if a != b {
                tag_changes += 1;
            }
            if before.path != after.path {
                moved_chunks += 1;
            }
        }
    }

    let summary = DiffSummary {
        before: args.before.display().to_string(),
        after: args.after.display().to_string(),
        files_added: added_files.len(),
        files_removed: removed_files.len(),
        files_modified: modified_files.len(),
        tokens_before: before_total_tokens,
        tokens_after: after_total_tokens,
        tokens_delta: after_total_tokens as isize - before_total_tokens as isize,
        chunks_before: before_chunk_count,
        chunks_after: after_chunk_count,
        chunks_delta: after_chunk_count as isize - before_chunk_count as isize,
        changed_chunk_tags: tag_changes,
        moved_chunks,
        added_files,
        removed_files,
        modified_files,
        graph: compare_graphs(&args.before, &args.after),
    };

    match args.format {
        DiffFormat::Text => render_text(&summary),
        DiffFormat::Markdown => render_markdown(&summary),
        DiffFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&summary)?);
        }
    }

    Ok(())
}

fn render_text(summary: &DiffSummary) {
    println!("Context Diff: {} -> {}", summary.before, summary.after);
    println!();
    println!(
        "Files: +{} added, -{} removed, {} modified",
        summary.files_added, summary.files_removed, summary.files_modified
    );
    println!(
        "Tokens: {} -> {} ({:+})",
        summary.tokens_before, summary.tokens_after, summary.tokens_delta
    );
    println!(
        "Chunks: {} -> {} ({:+})",
        summary.chunks_before, summary.chunks_after, summary.chunks_delta
    );
    println!("Changed chunk tags: {}", summary.changed_chunk_tags);
    println!("Moved chunks: {}", summary.moved_chunks);

    if !summary.added_files.is_empty() {
        println!();
        println!("Added files:");
        for file in summary.added_files.iter().take(10) {
            println!("  + {} (priority {:.3}, {} tokens)", file.path, file.priority, file.tokens);
        }
    }
    if !summary.removed_files.is_empty() {
        println!();
        println!("Removed files:");
        for file in summary.removed_files.iter().take(10) {
            println!("  - {}", file.path);
        }
    }
    if !summary.modified_files.is_empty() {
        println!();
        println!("Modified files:");
        for file in summary.modified_files.iter().take(12) {
            println!(
                "  * {} (priority {:.3}->{:.3}, tokens {}->{})",
                file.path,
                file.before_priority,
                file.after_priority,
                file.before_tokens,
                file.after_tokens
            );
        }
    }

    if let Some(graph) = summary.graph {
        println!();
        println!(
            "Graph changes: symbols +{} / -{}, imports +{} / -{}",
            graph.added_symbols, graph.removed_symbols, graph.added_imports, graph.removed_imports
        );
    }
}

fn render_markdown(summary: &DiffSummary) {
    println!("## Context Diff");
    println!();
    println!("`{}` -> `{}`", summary.before, summary.after);
    println!();
    println!(
        "- Files: +{} / -{} / ~{}",
        summary.files_added, summary.files_removed, summary.files_modified
    );
    println!(
        "- Tokens: {} -> {} ({:+})",
        summary.tokens_before, summary.tokens_after, summary.tokens_delta
    );
    println!(
        "- Chunks: {} -> {} ({:+})",
        summary.chunks_before, summary.chunks_after, summary.chunks_delta
    );
    println!("- Changed chunk tags: {}", summary.changed_chunk_tags);
    println!("- Moved chunks: {}", summary.moved_chunks);

    if !summary.added_files.is_empty() {
        println!();
        println!("### Added Files");
        for file in summary.added_files.iter().take(10) {
            println!("- `{}` ({:.3}, {} tokens)", file.path, file.priority, file.tokens);
        }
    }
    if !summary.removed_files.is_empty() {
        println!();
        println!("### Removed Files");
        for file in summary.removed_files.iter().take(10) {
            println!("- `{}`", file.path);
        }
    }
    if !summary.modified_files.is_empty() {
        println!();
        println!("### Modified Files");
        for file in summary.modified_files.iter().take(12) {
            println!(
                "- `{}` (priority {:.3}->{:.3}, tokens {}->{})",
                file.path,
                file.before_priority,
                file.after_priority,
                file.before_tokens,
                file.after_tokens
            );
        }
    }
}

fn read_report(dir: &Path) -> Result<ReportDoc> {
    let path = dir.join("report.json");
    let data = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read report.json at {}", path.display()))?;
    serde_json::from_str::<ReportDoc>(&data)
        .with_context(|| format!("Failed to parse JSON at {}", path.display()))
}

fn read_chunks(dir: &Path) -> Result<Vec<ChunkRow>> {
    let path = dir.join("chunks.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read chunks.jsonl at {}", path.display()))?;
    let mut rows = Vec::new();
    for line in content.lines().filter(|line| !line.trim().is_empty()) {
        if let Ok(row) = serde_json::from_str::<ChunkRow>(line) {
            rows.push(row);
        }
    }
    Ok(rows)
}

fn compare_graphs(before_dir: &Path, after_dir: &Path) -> Option<GraphDelta> {
    let before_db = resolve_graph_db(before_dir)?;
    let after_db = resolve_graph_db(after_dir)?;

    let before_symbols = load_symbol_pairs(&before_db)?;
    let after_symbols = load_symbol_pairs(&after_db)?;
    let before_imports = load_import_pairs(&before_db)?;
    let after_imports = load_import_pairs(&after_db)?;

    let added_symbols = after_symbols.difference(&before_symbols).count();
    let removed_symbols = before_symbols.difference(&after_symbols).count();
    let added_imports = after_imports.difference(&before_imports).count();
    let removed_imports = before_imports.difference(&after_imports).count();

    Some(GraphDelta { added_symbols, removed_symbols, added_imports, removed_imports })
}

fn resolve_graph_db(dir: &Path) -> Option<PathBuf> {
    let symbol_graph = dir.join("symbol_graph.db");
    if symbol_graph.exists() {
        return Some(symbol_graph);
    }
    let index_db = dir.join(".repo-context").join("index.sqlite");
    if index_db.exists() {
        return Some(index_db);
    }
    None
}

fn load_symbol_pairs(path: &Path) -> Option<HashSet<(String, String)>> {
    let conn = Connection::open(path).ok()?;
    let mut stmt = conn.prepare("SELECT symbol, chunk_id FROM symbol_chunks").ok()?;
    let rows =
        stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))).ok()?;
    let mut out = HashSet::new();
    for row in rows {
        let Ok(v) = row else {
            continue;
        };
        out.insert(v);
    }
    Some(out)
}

fn load_import_pairs(path: &Path) -> Option<HashSet<(String, String)>> {
    let conn = Connection::open(path).ok()?;
    let mut stmt = conn.prepare("SELECT source_path, target_path FROM file_imports").ok()?;
    let rows =
        stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))).ok()?;
    let mut out = HashSet::new();
    for row in rows {
        let Ok(v) = row else {
            continue;
        };
        out.insert(v);
    }
    Some(out)
}

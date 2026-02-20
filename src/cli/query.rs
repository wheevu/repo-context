//! Query command implementation

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use rusqlite::{params, Connection, OptionalExtension};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use crate::lsp::rust_analyzer;

#[derive(Args)]
pub struct QueryArgs {
    /// SQLite index database path
    #[arg(long, value_name = "FILE", default_value = ".repo-context/index.sqlite")]
    pub db: PathBuf,

    /// Task query text
    #[arg(long, value_name = "TEXT")]
    pub task: String,

    /// Max results to display
    #[arg(short = 'n', long, value_name = "COUNT", default_value_t = 20)]
    pub limit: usize,

    /// Optional LSP backend for Rust symbol discovery
    #[arg(long, value_name = "MODE", default_value = "auto")]
    pub lsp_backend: LspBackend,

    /// Expand results into definition/callers/tests/docs sections
    #[arg(long)]
    pub expand: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LspBackend {
    Off,
    Auto,
    RustAnalyzer,
}

pub fn run(args: QueryArgs) -> Result<()> {
    let conn = Connection::open(&args.db)
        .with_context(|| format!("Failed to open SQLite database at {}", args.db.display()))?;

    let has_chunks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'chunks'",
        [],
        |row| row.get(0),
    )?;
    if has_chunks == 0 {
        anyhow::bail!(
            "Index schema not found in {}. Run `repo-context index` first.",
            args.db.display()
        );
    }

    let tokens = tokenize(&args.task);
    if tokens.is_empty() {
        anyhow::bail!("Task query is empty after tokenization");
    }

    let fts_query = tokens.join(" ");
    let search_limit = (args.limit.max(1) * 5) as i64;

    let mut scored: HashMap<String, SearchRow> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "
            SELECT c.id, c.file_path, c.start_line, c.end_line, c.content, bm25(chunk_fts) AS rank
            FROM chunk_fts
            JOIN chunks c ON c.id = chunk_fts.chunk_id
            WHERE chunk_fts MATCH ?1
            ORDER BY rank
            LIMIT ?2
            ",
        )?;

        let rows = stmt.query_map(params![fts_query, search_limit], |row| {
            Ok(SearchRow {
                chunk_id: row.get(0)?,
                path: row.get(1)?,
                start_line: row.get::<_, i64>(2)? as usize,
                end_line: row.get::<_, i64>(3)? as usize,
                content: row.get(4)?,
                score: bm25_to_score(row.get::<_, f64>(5)?),
            })
        })?;

        for row in rows {
            let row = row?;
            scored.insert(row.chunk_id.clone(), row);
        }
    }

    let mut symbol_hits = HashSet::new();
    for token in &tokens {
        let mut stmt = conn.prepare("SELECT DISTINCT chunk_id FROM symbols WHERE symbol = ?1")?;
        let ids = stmt.query_map(params![token], |row| row.get::<_, String>(0))?;
        for id in ids {
            symbol_hits.insert(id?);
        }
    }

    for chunk_id in symbol_hits {
        if let Some(existing) = scored.get_mut(&chunk_id) {
            existing.score = (existing.score + 0.25).min(1.0);
            continue;
        }

        let mut stmt = conn.prepare(
            "SELECT id, file_path, start_line, end_line, content FROM chunks WHERE id = ?1",
        )?;
        let fetched = stmt
            .query_row(params![chunk_id], |row| {
                Ok(SearchRow {
                    chunk_id: row.get(0)?,
                    path: row.get(1)?,
                    start_line: row.get::<_, i64>(2)? as usize,
                    end_line: row.get::<_, i64>(3)? as usize,
                    content: row.get(4)?,
                    score: 0.5,
                })
            })
            .optional()?;

        if let Some(row) = fetched {
            scored.insert(row.chunk_id.clone(), row);
        }
    }

    let mut related_test_paths = BTreeSet::new();
    if args.lsp_backend != LspBackend::Off {
        let outcome =
            apply_lsp_boosts(&conn, &mut scored, &args.task, args.limit, args.lsp_backend)?;
        related_test_paths = outcome.related_test_paths;
    }

    let mut rows: Vec<SearchRow> = scored.into_values().collect();
    rows.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.start_line.cmp(&b.start_line))
    });
    rows.truncate(args.limit.max(1));

    if rows.is_empty() {
        println!("No matches found. Try broadening the query.");
        return Ok(());
    }

    if args.expand {
        let expanded = expand_symbol_context(&conn, &tokens, &rows, args.limit)?;
        print_expanded_results(&args.task, &expanded);
        return Ok(());
    }

    println!("Top matches for task: {}", args.task);
    for row in rows {
        println!("- {}:{}-{} (score {:.3})", row.path, row.start_line, row.end_line, row.score);
        println!("  {}", summarize(&row.content));
    }
    if !related_test_paths.is_empty() {
        println!("Related tests:");
        for path in related_test_paths.into_iter().take(args.limit.max(1)) {
            println!("- {}", path);
        }
    }

    Ok(())
}

fn apply_lsp_boosts(
    conn: &Connection,
    scored: &mut HashMap<String, SearchRow>,
    task: &str,
    limit: usize,
    backend: LspBackend,
) -> Result<LspBoostOutcome> {
    let root = match metadata_value(conn, "repo_root")? {
        Some(value) => PathBuf::from(value),
        None => return Ok(LspBoostOutcome::default()),
    };
    if !root.is_dir() {
        return Ok(LspBoostOutcome::default());
    }

    let has_rust: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files WHERE language = 'rust' OR extension = '.rs'",
        [],
        |row| row.get(0),
    )?;
    if has_rust == 0 {
        return Ok(LspBoostOutcome::default());
    }

    let use_ra = match backend {
        LspBackend::Off => false,
        LspBackend::RustAnalyzer => true,
        LspBackend::Auto => rust_analyzer::is_available(),
    };
    if !use_ra {
        return Ok(LspBoostOutcome::default());
    }

    let analysis = match rust_analyzer::analyze_workspace_symbols(&root, task, limit.max(1)) {
        Ok(analysis) => analysis,
        Err(err) => {
            eprintln!("warning: rust-analyzer enrichment unavailable: {err}");
            return Ok(LspBoostOutcome::default());
        }
    };
    let lsp_symbols = analysis.symbols;
    if lsp_symbols.is_empty() {
        return Ok(LspBoostOutcome::default());
    }

    let symbol_paths: Vec<String> = lsp_symbols.iter().map(|s| s.path.clone()).collect();
    let reference_paths = analysis.reference_paths;
    let symbol_terms = symbol_query_terms(&lsp_symbols);

    let symbol_path_set: HashSet<&str> = symbol_paths.iter().map(String::as_str).collect();
    let reference_path_set: HashSet<&str> = reference_paths.iter().map(String::as_str).collect();
    for row in scored.values_mut() {
        if symbol_path_set.contains(row.path.as_str()) {
            row.score = (row.score + 0.2).min(1.0);
        } else if reference_path_set.contains(row.path.as_str()) {
            row.score = (row.score + 0.15).min(1.0);
        }
    }

    for path in symbol_paths.into_iter().chain(reference_paths.into_iter()) {
        if scored.values().any(|row| row.path == path) {
            continue;
        }
        if let Some(row) = fetch_top_chunk_for_path(conn, &path)? {
            scored.insert(row.chunk_id.clone(), row);
        }
    }

    let mut related_test_paths = BTreeSet::new();
    for row in related_test_chunks(conn, &symbol_terms, limit.max(1) * 4)? {
        related_test_paths.insert(row.path.clone());
        scored.entry(row.chunk_id.clone()).or_insert(row);
    }

    Ok(LspBoostOutcome { related_test_paths })
}

#[derive(Default)]
struct LspBoostOutcome {
    related_test_paths: BTreeSet<String>,
}

fn symbol_query_terms(symbols: &[rust_analyzer::WorkspaceSymbol]) -> HashSet<String> {
    let mut terms = HashSet::new();
    for symbol in symbols {
        for token in tokenize(&symbol.name) {
            terms.insert(token);
        }
        if let Some(stem) = symbol.path.rsplit('/').next().and_then(|name| name.strip_suffix(".rs"))
        {
            for token in tokenize(stem) {
                terms.insert(token);
            }
        }
    }
    terms
}

fn related_test_chunks(
    conn: &Connection,
    terms: &HashSet<String>,
    limit: usize,
) -> Result<Vec<SearchRow>> {
    if terms.is_empty() {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        "
        SELECT id, file_path, start_line, end_line, content, priority
        FROM chunks
        WHERE file_path LIKE 'tests/%'
           OR file_path LIKE '%/tests/%'
           OR file_path LIKE '%_test.rs'
           OR file_path LIKE 'test/%'
        ORDER BY priority DESC, start_line ASC
        LIMIT 500
        ",
    )?;

    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let content: String = row.get(4)?;
        let tokens: HashSet<String> = tokenize(&content).into_iter().collect();
        if terms.intersection(&tokens).next().is_none() {
            continue;
        }
        out.push(SearchRow {
            chunk_id: row.get(0)?,
            path: row.get(1)?,
            start_line: row.get::<_, i64>(2)? as usize,
            end_line: row.get::<_, i64>(3)? as usize,
            content,
            score: 0.58_f64.max(row.get::<_, f64>(5)? * 0.9),
        });
    }
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.start_line.cmp(&b.start_line))
    });
    out.truncate(limit);
    Ok(out)
}

fn metadata_value(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM metadata WHERE key = ?1 LIMIT 1")?;
    let value = stmt.query_row(params![key], |row| row.get::<_, String>(0)).optional()?;
    Ok(value)
}

fn fetch_top_chunk_for_path(conn: &Connection, path: &str) -> Result<Option<SearchRow>> {
    let mut stmt = conn.prepare(
        "
        SELECT id, file_path, start_line, end_line, content, priority
        FROM chunks
        WHERE file_path = ?1
        ORDER BY priority DESC, start_line ASC
        LIMIT 1
        ",
    )?;
    let row = stmt
        .query_row(params![path], |row| {
            Ok(SearchRow {
                chunk_id: row.get(0)?,
                path: row.get(1)?,
                start_line: row.get::<_, i64>(2)? as usize,
                end_line: row.get::<_, i64>(3)? as usize,
                content: row.get(4)?,
                score: 0.55_f64.max(row.get::<_, f64>(5)? * 0.8),
            })
        })
        .optional()?;
    Ok(row)
}

#[derive(Clone)]
struct SearchRow {
    chunk_id: String,
    path: String,
    start_line: usize,
    end_line: usize,
    content: String,
    score: f64,
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter_map(|t| {
            let v = t.trim().to_ascii_lowercase();
            if v.len() >= 2 {
                Some(v)
            } else {
                None
            }
        })
        .collect()
}

fn bm25_to_score(rank: f64) -> f64 {
    let positive = rank.abs();
    (1.0 / (1.0 + positive)).clamp(0.0, 1.0)
}

fn summarize(content: &str) -> String {
    let first_line = content.lines().find(|line| !line.trim().is_empty()).unwrap_or("").trim();
    let mut out = first_line.to_string();
    if out.len() > 120 {
        out.truncate(120);
        out.push_str("...");
    }
    out
}

#[derive(Default)]
struct ExpandedContext {
    definitions: Vec<SearchRow>,
    callers: Vec<SearchRow>,
    tests: Vec<SearchRow>,
    config_docs: Vec<SearchRow>,
}

fn expand_symbol_context(
    conn: &Connection,
    tokens: &[String],
    ranked_rows: &[SearchRow],
    limit: usize,
) -> Result<ExpandedContext> {
    let mut out = ExpandedContext::default();
    let symbols = matched_symbols(conn, tokens)?;
    if symbols.is_empty() {
        out.definitions = ranked_rows.iter().take(limit.max(1)).cloned().collect();
        return Ok(out);
    }

    let definition_rows = fetch_definition_chunks(conn, &symbols, limit.max(1))?;
    let def_chunk_ids: Vec<String> = definition_rows.iter().map(|r| r.chunk_id.clone()).collect();
    out.definitions = definition_rows;

    let edges_available = table_exists(conn, "symbol_edges")?;
    if edges_available && !def_chunk_ids.is_empty() {
        out.callers =
            fetch_edge_chunks(conn, &def_chunk_ids, &["ref", "call", "import"], limit.max(1))?;
        out.tests = fetch_edge_chunks(conn, &def_chunk_ids, &["test"], limit.max(1))?;
    }

    if out.callers.is_empty() {
        out.callers = ranked_rows.iter().take(limit.max(1)).cloned().collect();
    }

    if out.tests.is_empty() {
        let terms: HashSet<String> = symbols.into_iter().collect();
        out.tests = related_test_chunks(conn, &terms, limit.max(1))?;
    }

    out.config_docs = fetch_config_doc_chunks(conn, limit.max(1))?;

    if out.definitions.is_empty() {
        out.definitions = ranked_rows.iter().take(limit.max(1)).cloned().collect();
    }

    Ok(out)
}

fn print_expanded_results(task: &str, expanded: &ExpandedContext) {
    println!("Expanded matches for task: {task}");
    print_section("Definition", &expanded.definitions);
    print_section("Top Callers", &expanded.callers);
    print_section("Related Tests", &expanded.tests);
    print_section("Config / Docs", &expanded.config_docs);
}

fn print_section(title: &str, rows: &[SearchRow]) {
    println!("\n== {title} ==");
    if rows.is_empty() {
        println!("- none");
        return;
    }

    for row in rows {
        println!("- {}:{}-{} (score {:.3})", row.path, row.start_line, row.end_line, row.score);
        println!("  {}", summarize(&row.content));
    }
}

fn matched_symbols(conn: &Connection, tokens: &[String]) -> Result<Vec<String>> {
    if tokens.is_empty() {
        return Ok(Vec::new());
    }

    let mut symbols = BTreeSet::new();
    let mut stmt = conn.prepare("SELECT DISTINCT symbol FROM symbols WHERE symbol = ?1")?;
    for token in tokens {
        let rows = stmt.query_map(params![token], |row| row.get::<_, String>(0))?;
        for row in rows {
            symbols.insert(row?);
        }
    }

    Ok(symbols.into_iter().collect())
}

fn fetch_definition_chunks(
    conn: &Connection,
    symbols: &[String],
    limit: usize,
) -> Result<Vec<SearchRow>> {
    if symbols.is_empty() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut stmt = conn.prepare(
        "
        SELECT c.id, c.file_path, c.start_line, c.end_line, c.content, c.priority
        FROM symbols s
        JOIN chunks c ON c.id = s.chunk_id
        WHERE s.symbol = ?1 AND s.kind = 'def'
        ORDER BY c.priority DESC, c.start_line ASC
        LIMIT ?2
        ",
    )?;

    for symbol in symbols {
        let rows = stmt.query_map(params![symbol, limit as i64], |row| {
            Ok(SearchRow {
                chunk_id: row.get(0)?,
                path: row.get(1)?,
                start_line: row.get::<_, i64>(2)? as usize,
                end_line: row.get::<_, i64>(3)? as usize,
                content: row.get(4)?,
                score: row.get(5)?,
            })
        })?;

        for row in rows {
            let row = row?;
            if seen.insert(row.chunk_id.clone()) {
                out.push(row);
            }
        }
    }

    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.start_line.cmp(&b.start_line))
    });
    out.truncate(limit);
    Ok(out)
}

fn fetch_edge_chunks(
    conn: &Connection,
    def_chunk_ids: &[String],
    kinds: &[&str],
    limit: usize,
) -> Result<Vec<SearchRow>> {
    if def_chunk_ids.is_empty() || kinds.is_empty() {
        return Ok(Vec::new());
    }

    let mut rows_out = Vec::new();
    let mut seen = HashSet::new();
    let mut stmt = conn.prepare(
        "
        SELECT c.id, c.file_path, c.start_line, c.end_line, c.content, c.priority
        FROM symbol_edges e
        JOIN chunks c ON c.id = e.to_chunk_id
        WHERE e.from_chunk_id = ?1 AND e.kind = ?2
        ORDER BY c.priority DESC, c.start_line ASC
        LIMIT ?3
        ",
    )?;

    for def_id in def_chunk_ids {
        for kind in kinds {
            let rows = stmt.query_map(params![def_id, kind, limit as i64], |row| {
                Ok(SearchRow {
                    chunk_id: row.get(0)?,
                    path: row.get(1)?,
                    start_line: row.get::<_, i64>(2)? as usize,
                    end_line: row.get::<_, i64>(3)? as usize,
                    content: row.get(4)?,
                    score: 0.6_f64.max(row.get::<_, f64>(5)? * 0.9),
                })
            })?;

            for row in rows {
                let row = row?;
                if seen.insert(row.chunk_id.clone()) {
                    rows_out.push(row);
                }
            }
        }
    }

    rows_out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.start_line.cmp(&b.start_line))
    });
    rows_out.truncate(limit);
    Ok(rows_out)
}

fn fetch_config_doc_chunks(conn: &Connection, limit: usize) -> Result<Vec<SearchRow>> {
    let mut stmt = conn.prepare(
        "
        SELECT c.id, c.file_path, c.start_line, c.end_line, c.content, c.priority
        FROM chunks c
        JOIN files f ON f.path = c.file_path
        WHERE f.priority >= 0.9
           OR c.file_path LIKE 'README%'
           OR c.file_path LIKE 'docs/%'
           OR c.file_path LIKE '%.md'
           OR c.file_path LIKE '%.toml'
           OR c.file_path LIKE '.github/%'
        ORDER BY c.priority DESC, c.start_line ASC
        LIMIT ?1
        ",
    )?;

    let rows = stmt.query_map(params![limit as i64], |row| {
        Ok(SearchRow {
            chunk_id: row.get(0)?,
            path: row.get(1)?,
            start_line: row.get::<_, i64>(2)? as usize,
            end_line: row.get::<_, i64>(3)? as usize,
            content: row.get(4)?,
            score: row.get(5)?,
        })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![name],
        |row| row.get(0),
    )?;
    Ok(exists > 0)
}

#[cfg(test)]
mod tests {
    use super::{expand_symbol_context, symbol_query_terms, SearchRow};
    use crate::lsp::rust_analyzer::WorkspaceSymbol;
    use rusqlite::Connection;
    use std::collections::HashSet;

    #[test]
    fn symbol_query_terms_include_symbol_and_file_tokens() {
        let symbols = vec![WorkspaceSymbol {
            name: "refresh_token".to_string(),
            path: "src/auth/session_manager.rs".to_string(),
            uri: "file:///tmp/repo/src/auth/session_manager.rs".to_string(),
            line: 10,
            character: 4,
        }];
        let terms = symbol_query_terms(&symbols);
        let expected: HashSet<String> =
            ["refresh_token", "session_manager"].iter().map(|s| s.to_string()).collect();
        for term in expected {
            assert!(terms.contains(&term));
        }
    }

    #[test]
    fn expand_context_uses_definition_and_edges() {
        let conn = Connection::open_in_memory().expect("sqlite in-memory db");
        conn.execute_batch(
            "
            CREATE TABLE files (
                path TEXT PRIMARY KEY,
                language TEXT NOT NULL,
                extension TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                priority REAL NOT NULL,
                token_estimate INTEGER NOT NULL,
                file_hash TEXT NOT NULL,
                indexed_at TEXT NOT NULL,
                mtime INTEGER
            );
            CREATE TABLE chunks (
                id TEXT PRIMARY KEY,
                file_path TEXT NOT NULL,
                start_line INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                language TEXT NOT NULL,
                priority REAL NOT NULL,
                token_estimate INTEGER NOT NULL,
                tags_json TEXT NOT NULL,
                content TEXT NOT NULL
            );
            CREATE TABLE symbols (
                symbol TEXT NOT NULL,
                kind TEXT NOT NULL,
                file_path TEXT NOT NULL,
                chunk_id TEXT NOT NULL
            );
            CREATE TABLE symbol_edges (
                from_chunk_id TEXT NOT NULL,
                to_chunk_id TEXT NOT NULL,
                kind TEXT NOT NULL
            );
            ",
        )
        .expect("schema");

        conn.execute(
            "INSERT INTO files (path, language, extension, size_bytes, priority, token_estimate, file_hash, indexed_at, mtime)
             VALUES (?1, 'rust', '.rs', 10, 0.9, 5, 'a', 'now', 0)",
            ["src/auth.rs"],
        )
        .expect("insert file");
        conn.execute(
            "INSERT INTO files (path, language, extension, size_bytes, priority, token_estimate, file_hash, indexed_at, mtime)
             VALUES (?1, 'rust', '.rs', 10, 0.5, 5, 'b', 'now', 0)",
            ["src/handler.rs"],
        )
        .expect("insert file 2");
        conn.execute(
            "INSERT INTO chunks (id, file_path, start_line, end_line, language, priority, token_estimate, tags_json, content)
             VALUES ('def1', 'src/auth.rs', 1, 20, 'rust', 0.95, 20, '{}', 'fn refresh_token() {}')",
            [],
        )
        .expect("insert def chunk");
        conn.execute(
            "INSERT INTO chunks (id, file_path, start_line, end_line, language, priority, token_estimate, tags_json, content)
             VALUES ('call1', 'src/handler.rs', 1, 20, 'rust', 0.60, 20, '{}', 'refresh_token();')",
            [],
        )
        .expect("insert caller chunk");
        conn.execute(
            "INSERT INTO symbols (symbol, kind, file_path, chunk_id)
             VALUES ('refresh_token', 'def', 'src/auth.rs', 'def1')",
            [],
        )
        .expect("insert symbol");
        conn.execute(
            "INSERT INTO symbol_edges (from_chunk_id, to_chunk_id, kind)
             VALUES ('def1', 'call1', 'call')",
            [],
        )
        .expect("insert edge");

        let tokens = vec!["refresh_token".to_string()];
        let ranked = vec![SearchRow {
            chunk_id: "fallback".to_string(),
            path: "src/fallback.rs".to_string(),
            start_line: 1,
            end_line: 1,
            content: "fallback".to_string(),
            score: 0.1,
        }];

        let expanded = expand_symbol_context(&conn, &tokens, &ranked, 5).expect("expanded");
        assert_eq!(expanded.definitions.len(), 1);
        assert_eq!(expanded.definitions[0].chunk_id, "def1");
        assert_eq!(expanded.callers.len(), 1);
        assert_eq!(expanded.callers[0].chunk_id, "call1");
    }
}

//! Index command implementation

use anyhow::{Context, Result};
use clap::Args;
use git2::Repository;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::cache::remote_index_cache_db_path;
use super::utils::parse_csv;
use crate::chunk::{chunk_content, coalesce_small_chunks_with_max};
use crate::config::{load_config, merge_cli_with_config, CliOverrides};
use crate::domain::{Chunk, FileInfo, ScanStats};
use crate::fetch::fetch_repository;
use crate::graph::persist::persist_graph;
use crate::lsp::rust_analyzer;
use crate::rank::rank_files;
use crate::scan::scanner::FileScanner;
use crate::utils::read_file_safe;

#[derive(Args)]
pub struct IndexArgs {
    /// Local directory path to index
    #[arg(short, long, value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// GitHub repository URL to clone and index
    #[arg(short = 'r', long, value_name = "URL")]
    pub repo: Option<String>,

    /// Git ref (branch/tag/SHA) when using --repo
    #[arg(long, value_name = "REF")]
    pub ref_: Option<String>,

    /// Path to config file (repo-context.toml or .r2p.yml)
    #[arg(short = 'c', long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// SQLite path for the index database
    #[arg(long, value_name = "FILE", default_value = ".repo-context/index.sqlite")]
    pub db: PathBuf,

    /// Include only these extensions (comma-separated)
    #[arg(short = 'i', long, value_name = "EXTS")]
    pub include_ext: Option<String>,

    /// Exclude paths matching these globs (comma-separated)
    #[arg(short = 'e', long, value_name = "GLOBS")]
    pub exclude_glob: Option<String>,

    /// Skip files larger than this (bytes)
    #[arg(long, value_name = "BYTES")]
    pub max_file_bytes: Option<u64>,

    /// Stop after indexing this many bytes total
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

    /// Target tokens per chunk
    #[arg(long, value_name = "TOKENS")]
    pub chunk_tokens: Option<usize>,

    /// Overlap tokens between adjacent chunks
    #[arg(long, value_name = "TOKENS")]
    pub chunk_overlap: Option<usize>,

    /// Coalesce chunks smaller than this
    #[arg(long, value_name = "TOKENS")]
    pub min_chunk_tokens: Option<usize>,

    /// Enrich index with rust-analyzer symbol references
    #[arg(long)]
    pub lsp: bool,
}

pub fn run(args: IndexArgs) -> Result<()> {
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
        chunk_tokens: args.chunk_tokens,
        chunk_overlap: args.chunk_overlap,
        min_chunk_tokens: args.min_chunk_tokens,
        ..CliOverrides::default()
    };
    let merged = merge_cli_with_config(file_config, cli_overrides);
    let config_hash = index_config_hash(&merged);
    let default_db = PathBuf::from(".repo-context/index.sqlite");
    let mut db_path = args.db.clone();
    if merged.repo_url.is_some() && args.db == default_db {
        if let Some(cache_db) = remote_index_cache_db_path(
            merged.repo_url.as_deref(),
            merged.ref_.as_deref(),
            &config_hash,
        ) {
            db_path = cache_db;
            println!("info: using remote index cache at {}", db_path.display());
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

    let mut scanner = FileScanner::new(root_path.clone())
        .max_file_bytes(merged.max_file_bytes)
        .respect_gitignore(merged.respect_gitignore)
        .follow_symlinks(merged.follow_symlinks)
        .skip_minified(merged.skip_minified)
        .include_extensions(merged.include_extensions.iter().cloned().collect())
        .exclude_globs(merged.exclude_globs.iter().cloned().collect());

    let scanned_files = scanner.scan()?;
    let mut stats = scanner.stats().clone();
    let ranked_files = rank_files(&root_path, scanned_files)?;
    let selected_files = apply_byte_budget(ranked_files, Some(merged.max_total_bytes), &mut stats);

    let summary = write_index(
        &db_path,
        &root_path,
        &selected_files,
        &stats,
        IndexMetadata {
            repo: merged.repo_url.clone(),
            ref_: merged.ref_.clone(),
            git_commit: discover_git_commit(&root_path),
            config_hash,
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
        },
        IndexBuildOptions {
            chunk_tokens: merged.chunk_tokens,
            chunk_overlap: merged.chunk_overlap,
            min_chunk_tokens: merged.min_chunk_tokens,
            lsp_enabled: args.lsp,
        },
    )?;

    println!("Index created at {}", db_path.display());
    println!("  files indexed: {}", summary.files_indexed);
    println!("  chunks indexed: {}", summary.chunks_indexed);
    println!("  files reindexed: {}", summary.files_reindexed);
    println!("  files reused: {}", summary.files_reused);
    println!("  files removed: {}", summary.files_removed);
    if summary.files_unreadable > 0 {
        println!("  files unreadable: {}", summary.files_unreadable);
    }
    if args.lsp {
        println!("  lsp edges indexed: {}", summary.symbol_edges_indexed);
    }
    println!(
        "  graph symbols/import edges: {}/{}",
        summary.graph_symbols_indexed, summary.graph_import_edges_indexed
    );

    Ok(())
}

fn write_index(
    db_path: &Path,
    root_path: &Path,
    files: &[FileInfo],
    stats: &ScanStats,
    metadata_ctx: IndexMetadata,
    build: IndexBuildOptions,
) -> Result<IndexSummary> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut conn = Connection::open(db_path)
        .with_context(|| format!("Failed to open SQLite database at {}", db_path.display()))?;

    ensure_schema(&conn)?;

    let tx = conn.transaction()?;

    let existing_index = {
        let mut stmt = tx.prepare("SELECT path, file_hash, mtime FROM files")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                ExistingFileRecord {
                    file_hash: row.get::<_, String>(1)?,
                    mtime: row.get::<_, Option<i64>>(2)?,
                },
            ))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (path, record) = row?;
            map.insert(path, record);
        }
        map
    };

    let selected_paths: HashSet<String> = files.iter().map(|f| f.relative_path.clone()).collect();
    let existing_paths: HashSet<String> = existing_index.keys().cloned().collect();
    let stale_paths: Vec<String> = existing_paths.difference(&selected_paths).cloned().collect();
    for path in &stale_paths {
        tx.execute("DELETE FROM chunk_fts WHERE path = ?1", params![path])?;
        tx.execute("DELETE FROM files WHERE path = ?1", params![path])?;
    }

    let mut files_reindexed = 0usize;
    let mut files_reused = 0usize;
    let mut files_unreadable = 0usize;
    let indexed_at = chrono::Utc::now().to_rfc3339();

    for file in files {
        let path = &file.relative_path;
        let current_mtime = file_mtime_seconds(&file.path);
        let existing = existing_index.get(path);

        if existing.and_then(|record| record.mtime) == current_mtime {
            files_reused += 1;
            tx.execute(
                "
                UPDATE files
                SET language = ?2, extension = ?3, size_bytes = ?4, priority = ?5, indexed_at = ?6
                WHERE path = ?1
                ",
                params![
                    path,
                    &file.language,
                    &file.extension,
                    file.size_bytes as i64,
                    file.priority,
                    &indexed_at,
                ],
            )?;
            continue;
        }

        let (content, _encoding) = match read_file_safe(&file.path, None, None) {
            Ok(value) => value,
            Err(_) => {
                files_unreadable += 1;
                continue;
            }
        };

        let content_hash = sha256_hex(&content);
        let was_same = existing.is_some_and(|record| record.file_hash == content_hash);

        if was_same {
            files_reused += 1;
            tx.execute(
                "
                UPDATE files
                SET language = ?2, extension = ?3, size_bytes = ?4, priority = ?5, indexed_at = ?6,
                    file_hash = ?7, mtime = ?8
                WHERE path = ?1
                ",
                params![
                    path,
                    &file.language,
                    &file.extension,
                    file.size_bytes as i64,
                    file.priority,
                    &indexed_at,
                    &content_hash,
                    current_mtime,
                ],
            )?;
            continue;
        }

        files_reindexed += 1;
        tx.execute("DELETE FROM chunk_fts WHERE path = ?1", params![path])?;
        tx.execute("DELETE FROM symbol_edges WHERE from_chunk_id IN (SELECT id FROM chunks WHERE file_path = ?1)", params![path])?;
        tx.execute("DELETE FROM symbol_edges WHERE to_chunk_id IN (SELECT id FROM chunks WHERE file_path = ?1)", params![path])?;
        tx.execute("DELETE FROM files WHERE path = ?1", params![path])?;

        let raw_chunks = chunk_content(file, &content, build.chunk_tokens, build.chunk_overlap)?;
        let file_chunks =
            coalesce_small_chunks_with_max(raw_chunks, build.min_chunk_tokens, build.chunk_tokens);
        let file_tokens = file_chunks.iter().map(|c| c.token_estimate).sum::<usize>();

        tx.execute(
            "
            INSERT INTO files
                (path, language, extension, size_bytes, priority, token_estimate, file_hash, mtime,
                 indexed_at)
            VALUES
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ",
            params![
                path,
                &file.language,
                &file.extension,
                file.size_bytes as i64,
                file.priority,
                file_tokens as i64,
                &content_hash,
                current_mtime,
                &indexed_at,
            ],
        )?;

        for chunk in &file_chunks {
            insert_chunk(&tx, chunk)?;
        }
    }

    let files_indexed: usize =
        tx.query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, i64>(0))? as usize;
    let chunks_indexed: usize =
        tx.query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get::<_, i64>(0))? as usize;

    tx.execute("DELETE FROM metadata", [])?;

    let metadata = [
        ("repo_root".to_string(), root_path.to_string_lossy().to_string()),
        ("files_scanned".to_string(), stats.files_scanned.to_string()),
        ("files_indexed".to_string(), files_indexed.to_string()),
        ("chunks_indexed".to_string(), chunks_indexed.to_string()),
        ("languages".to_string(), json!(stats.languages_detected).to_string()),
        (
            "repo_url".to_string(),
            metadata_ctx.repo.clone().unwrap_or_else(|| root_path.to_string_lossy().to_string()),
        ),
        (
            "repo_ref".to_string(),
            metadata_ctx.ref_.clone().unwrap_or_else(|| "unknown".to_string()),
        ),
        (
            "git_commit".to_string(),
            metadata_ctx.git_commit.clone().unwrap_or_else(|| "unknown".to_string()),
        ),
        ("config_hash".to_string(), metadata_ctx.config_hash),
        ("tool_version".to_string(), metadata_ctx.tool_version),
    ];
    for (key, value) in metadata {
        tx.execute("INSERT INTO metadata (key, value) VALUES (?1, ?2)", params![key, value])?;
    }

    tx.commit()?;

    let mut symbol_edges_indexed = 0usize;
    let mut graph_symbols_indexed = 0usize;
    let mut graph_import_edges_indexed = 0usize;
    let all_chunks = load_all_chunks(&conn)?;
    if let Ok((symbols, edges)) = persist_graph(&mut conn, &all_chunks) {
        graph_symbols_indexed = symbols;
        graph_import_edges_indexed = edges;
    }
    if build.lsp_enabled {
        symbol_edges_indexed = enrich_symbol_edges_with_lsp(db_path, root_path)?;
    }

    Ok(IndexSummary {
        files_indexed,
        chunks_indexed,
        files_reindexed,
        files_reused,
        files_removed: stale_paths.len(),
        files_unreadable,
        symbol_edges_indexed,
        graph_symbols_indexed,
        graph_import_edges_indexed,
    })
}

fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS files (
            path TEXT PRIMARY KEY,
            language TEXT NOT NULL,
            extension TEXT NOT NULL,
            size_bytes INTEGER NOT NULL,
            priority REAL NOT NULL,
            token_estimate INTEGER NOT NULL,
            file_hash TEXT NOT NULL,
            mtime INTEGER,
            indexed_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS chunks (
            id TEXT PRIMARY KEY,
            file_path TEXT NOT NULL,
            start_line INTEGER NOT NULL,
            end_line INTEGER NOT NULL,
            language TEXT NOT NULL,
            priority REAL NOT NULL,
            token_estimate INTEGER NOT NULL,
            tags_json TEXT NOT NULL,
            content TEXT NOT NULL,
            FOREIGN KEY(file_path) REFERENCES files(path) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS symbols (
            symbol TEXT NOT NULL,
            kind TEXT NOT NULL,
            file_path TEXT NOT NULL,
            chunk_id TEXT NOT NULL,
            PRIMARY KEY(symbol, kind, chunk_id),
            FOREIGN KEY(file_path) REFERENCES files(path) ON DELETE CASCADE,
            FOREIGN KEY(chunk_id) REFERENCES chunks(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS symbol_edges (
            from_chunk_id TEXT NOT NULL,
            to_chunk_id TEXT NOT NULL,
            kind TEXT NOT NULL CHECK(kind IN ('ref', 'call', 'test', 'import')),
            PRIMARY KEY(from_chunk_id, to_chunk_id, kind),
            FOREIGN KEY(from_chunk_id) REFERENCES chunks(id) ON DELETE CASCADE,
            FOREIGN KEY(to_chunk_id) REFERENCES chunks(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS symbol_chunks (
            symbol TEXT NOT NULL,
            chunk_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            path TEXT NOT NULL,
            PRIMARY KEY (symbol, chunk_id)
        );

        CREATE TABLE IF NOT EXISTS file_imports (
            source_path TEXT NOT NULL,
            target_path TEXT NOT NULL,
            PRIMARY KEY (source_path, target_path)
        );

        CREATE TABLE IF NOT EXISTS chunk_meta (
            chunk_id TEXT PRIMARY KEY,
            path TEXT NOT NULL,
            start_line INTEGER NOT NULL,
            end_line INTEGER NOT NULL,
            priority REAL NOT NULL
        );

        CREATE TABLE IF NOT EXISTS symbol_refs (
            symbol TEXT NOT NULL,
            chunk_id TEXT NOT NULL,
            ref_kind TEXT NOT NULL DEFAULT 'ref',
            PRIMARY KEY (symbol, chunk_id, ref_kind)
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS chunk_fts USING fts5(
            chunk_id UNINDEXED,
            path UNINDEXED,
            content
        );

        CREATE INDEX IF NOT EXISTS idx_chunks_file_path ON chunks(file_path);
        CREATE INDEX IF NOT EXISTS idx_symbols_symbol ON symbols(symbol);
        CREATE INDEX IF NOT EXISTS idx_symbols_file_path ON symbols(file_path);
        CREATE INDEX IF NOT EXISTS idx_symbol_edges_from ON symbol_edges(from_chunk_id);
        CREATE INDEX IF NOT EXISTS idx_symbol_edges_to ON symbol_edges(to_chunk_id);
        CREATE INDEX IF NOT EXISTS idx_symbol_refs_symbol ON symbol_refs(symbol);
        CREATE INDEX IF NOT EXISTS idx_symbol_refs_chunk ON symbol_refs(chunk_id);
        ",
    )?;
    ensure_files_mtime_column(conn)?;
    Ok(())
}

fn insert_chunk(tx: &rusqlite::Transaction<'_>, chunk: &Chunk) -> Result<()> {
    let tags = serde_json::to_string(&chunk.tags)?;

    tx.execute(
        "
        INSERT INTO chunks
            (id, file_path, start_line, end_line, language, priority, token_estimate, tags_json, content)
        VALUES
            (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ",
        params![
            &chunk.id,
            &chunk.path,
            chunk.start_line as i64,
            chunk.end_line as i64,
            &chunk.language,
            chunk.priority,
            chunk.token_estimate as i64,
            tags,
            &chunk.content,
        ],
    )?;

    tx.execute(
        "INSERT INTO chunk_fts (chunk_id, path, content) VALUES (?1, ?2, ?3)",
        params![&chunk.id, &chunk.path, &chunk.content],
    )?;

    for tag in &chunk.tags {
        if let Some((kind, symbol)) = tag.split_once(':') {
            if matches!(kind, "def" | "type" | "impl") && !symbol.trim().is_empty() {
                tx.execute(
                    "
                    INSERT OR IGNORE INTO symbols (symbol, kind, file_path, chunk_id)
                    VALUES (?1, ?2, ?3, ?4)
                    ",
                    params![symbol.to_ascii_lowercase(), kind, &chunk.path, &chunk.id],
                )?;
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct ExistingFileRecord {
    file_hash: String,
    mtime: Option<i64>,
}

#[derive(Debug)]
struct IndexSummary {
    files_indexed: usize,
    chunks_indexed: usize,
    files_reindexed: usize,
    files_reused: usize,
    files_removed: usize,
    files_unreadable: usize,
    symbol_edges_indexed: usize,
    graph_symbols_indexed: usize,
    graph_import_edges_indexed: usize,
}

#[derive(Debug, Copy, Clone)]
struct IndexBuildOptions {
    chunk_tokens: usize,
    chunk_overlap: usize,
    min_chunk_tokens: usize,
    lsp_enabled: bool,
}

#[derive(Debug, Clone)]
struct IndexMetadata {
    repo: Option<String>,
    ref_: Option<String>,
    git_commit: Option<String>,
    config_hash: String,
    tool_version: String,
}

fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    format!("{digest:x}")
}

fn discover_git_commit(root_path: &Path) -> Option<String> {
    Repository::discover(root_path).ok()?.head().ok()?.target().map(|oid| oid.to_string())
}

fn index_config_hash(config: &crate::domain::Config) -> String {
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
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&payload).unwrap_or_default());
    format!("{:x}", hasher.finalize())
}

fn apply_byte_budget(
    ranked_files: Vec<FileInfo>,
    max_total_bytes: Option<u64>,
    stats: &mut ScanStats,
) -> Vec<FileInfo> {
    let Some(limit) = max_total_bytes else {
        return ranked_files;
    };

    let mut selected = Vec::new();
    let mut total = 0_u64;
    for (idx, file) in ranked_files.iter().enumerate() {
        if total >= limit {
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

fn load_all_chunks(conn: &Connection) -> Result<Vec<Chunk>> {
    let mut stmt = conn.prepare(
        "
        SELECT id, file_path, start_line, end_line, language, priority, token_estimate, tags_json,
               content
        FROM chunks
        ORDER BY file_path, start_line, id
        ",
    )?;

    let rows = stmt.query_map([], |row| {
        let tags_json: String = row.get(7)?;
        let tags: BTreeSet<String> = serde_json::from_str(&tags_json).unwrap_or_default();
        Ok(Chunk {
            id: row.get(0)?,
            path: row.get(1)?,
            start_line: row.get::<_, i64>(2)? as usize,
            end_line: row.get::<_, i64>(3)? as usize,
            language: row.get(4)?,
            priority: row.get(5)?,
            token_estimate: row.get::<_, i64>(6)? as usize,
            tags,
            content: row.get(8)?,
        })
    })?;

    let mut chunks = Vec::new();
    for row in rows {
        chunks.push(row?);
    }
    Ok(chunks)
}

#[derive(Debug, Clone)]
struct SymbolSeed {
    chunk_id: String,
    symbol: String,
    path: String,
    line: u32,
}

#[derive(Debug, Clone)]
struct ChunkLocation {
    id: String,
    path: String,
    content: String,
}

fn ensure_files_mtime_column(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(files)")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut has_mtime = false;
    for row in rows {
        if row? == "mtime" {
            has_mtime = true;
            break;
        }
    }

    if !has_mtime {
        conn.execute("ALTER TABLE files ADD COLUMN mtime INTEGER", [])?;
    }
    Ok(())
}

fn file_mtime_seconds(path: &Path) -> Option<i64> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let duration = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(duration.as_secs() as i64)
}

fn enrich_symbol_edges_with_lsp(db_path: &Path, root_path: &Path) -> Result<usize> {
    if !rust_analyzer::is_available() {
        return Ok(0);
    }

    let mut conn = Connection::open(db_path)
        .with_context(|| format!("Failed to open SQLite database at {}", db_path.display()))?;
    let has_rust: i64 = conn.query_row(
        "SELECT COUNT(*) FROM files WHERE language = 'rust' OR extension = '.rs'",
        [],
        |row| row.get(0),
    )?;
    if has_rust == 0 {
        return Ok(0);
    }

    let seeds = collect_symbol_seeds(&conn)?;
    if seeds.is_empty() {
        return Ok(0);
    }

    let refs =
        match rust_analyzer::analyze_symbol_references(root_path, &to_lsp_seeds(&seeds), 600, 80) {
            Ok(items) => items,
            Err(err) => {
                eprintln!("warning: rust-analyzer edge enrichment unavailable: {err}");
                return Ok(0);
            }
        };
    if refs.is_empty() {
        return Ok(0);
    }

    let tx = conn.transaction()?;
    tx.execute("DELETE FROM symbol_edges", [])?;

    let mut inserted = 0usize;
    for reference in refs {
        let line_1based = reference.line as usize + 1;
        let Some(target) = find_chunk_for_reference(&tx, &reference.path, line_1based)? else {
            continue;
        };
        if target.id == reference.from_chunk_id {
            continue;
        }

        let kind = classify_edge_kind(&reference.symbol, &target.path, &target.content);
        inserted += tx.execute(
            "
            INSERT OR IGNORE INTO symbol_edges (from_chunk_id, to_chunk_id, kind)
            VALUES (?1, ?2, ?3)
            ",
            params![reference.from_chunk_id, target.id, kind],
        )?;
    }

    tx.commit()?;
    Ok(inserted)
}

fn collect_symbol_seeds(conn: &Connection) -> Result<Vec<SymbolSeed>> {
    let mut stmt = conn.prepare(
        "
        SELECT s.chunk_id, s.symbol, s.file_path, c.start_line
        FROM symbols s
        JOIN chunks c ON c.id = s.chunk_id
        WHERE c.language = 'rust'
          AND s.kind IN ('def', 'type', 'impl')
        ORDER BY c.priority DESC, c.start_line ASC
        LIMIT 1200
        ",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(SymbolSeed {
            chunk_id: row.get(0)?,
            symbol: row.get::<_, String>(1)?,
            path: row.get(2)?,
            line: row.get::<_, i64>(3)? as u32,
        })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn to_lsp_seeds(seeds: &[SymbolSeed]) -> Vec<rust_analyzer::SymbolSeed> {
    seeds
        .iter()
        .map(|seed| rust_analyzer::SymbolSeed {
            chunk_id: seed.chunk_id.clone(),
            symbol: seed.symbol.clone(),
            path: seed.path.clone(),
            line: seed.line.saturating_sub(1),
            character: 0,
        })
        .collect()
}

fn find_chunk_for_reference(
    tx: &rusqlite::Transaction<'_>,
    path: &str,
    line_1based: usize,
) -> Result<Option<ChunkLocation>> {
    let mut stmt = tx.prepare(
        "
        SELECT id, file_path, content
        FROM chunks
        WHERE file_path = ?1
          AND start_line <= ?2
          AND end_line >= ?2
        ORDER BY priority DESC, start_line ASC
        LIMIT 1
        ",
    )?;

    let row = stmt
        .query_row(params![path, line_1based as i64], |row| {
            Ok(ChunkLocation { id: row.get(0)?, path: row.get(1)?, content: row.get(2)? })
        })
        .optional()?;
    Ok(row)
}

fn classify_edge_kind(symbol: &str, path: &str, content: &str) -> &'static str {
    let lower_path = path.to_ascii_lowercase();
    if lower_path.contains("/tests/")
        || lower_path.starts_with("tests/")
        || lower_path.contains("_test.rs")
        || lower_path.contains(".test.")
    {
        return "test";
    }

    let lower = content.to_ascii_lowercase();
    if lower.contains(&format!("use {symbol}"))
        || lower.contains(&format!("mod {symbol}"))
        || lower.contains(&format!("::{symbol}"))
    {
        return "import";
    }
    if lower.contains(&format!("{symbol}(")) {
        return "call";
    }
    "ref"
}

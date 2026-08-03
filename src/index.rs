//! Redacted, incremental SQLite index for local task retrieval.
//!
//! The index is an advisory cache. Source files remain the authority: export
//! refreshes the cache from the current scan and renders current chunks after
//! retrieval. Only relative paths, metadata, and redacted chunk content are
//! persisted.

#![allow(missing_docs)]

use crate::domain::{Chunk, Config, FileInfo};
use crate::module::graph::ImportGraph;
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub const INDEX_SCHEMA_VERSION: &str = "1";
pub const CHUNK_SCHEMA_FINGERPRINT: &str = "chunk-v1";

pub struct IndexStore {
    path: PathBuf,
    connection: Connection,
    config_changed: bool,
    config_fingerprint: String,
    redaction_fingerprint: String,
}

#[derive(Debug, Clone, Default)]
pub struct IndexRefresh {
    pub reused_files: usize,
    pub updated_files: usize,
    pub removed_files: usize,
    pub indexed_chunks: usize,
}

impl IndexStore {
    /// Open or create an index, validating its repository and schema identity.
    pub fn open(
        path: &Path,
        root_path: &Path,
        config_fingerprint: &str,
        redaction_fingerprint: &str,
    ) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create index directory {}", parent.display())
            })?;
        }
        let connection = Connection::open(path)
            .with_context(|| format!("failed to open index database {}", path.display()))?;
        connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        initialize_schema(&connection)?;

        let root_key = root_fingerprint(root_path);
        if let Some(existing) = metadata(&connection, "root_key")? {
            if existing != root_key {
                anyhow::bail!(
                    "index database {} belongs to a different repository",
                    path.display()
                );
            }
        }
        let config_changed = metadata(&connection, "config_fingerprint")?
            .is_some_and(|existing| existing != config_fingerprint)
            || metadata(&connection, "redaction_fingerprint")?
                .is_some_and(|existing| existing != redaction_fingerprint)
            || metadata(&connection, "chunk_fingerprint")?
                .is_some_and(|existing| existing != CHUNK_SCHEMA_FINGERPRINT);
        if let Some(redacted) = metadata(&connection, "redacted")? {
            if redacted != "true" {
                anyhow::bail!("index database is not marked as redacted");
            }
        }
        Ok(Self {
            path: path.to_path_buf(),
            connection,
            config_changed,
            config_fingerprint: config_fingerprint.to_string(),
            redaction_fingerprint: redaction_fingerprint.to_string(),
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return relative paths whose source content or index configuration is stale.
    pub fn paths_needing_refresh(&self, files: &[FileInfo]) -> Result<HashSet<String>> {
        let mut stale = HashSet::new();
        for file in files {
            if self.config_changed {
                stale.insert(file.relative_path.clone());
                continue;
            }
            let current_hash = hash_file(&file.path)
                .with_context(|| format!("failed to hash {} for index", file.relative_path))?;
            let previous_hash: Option<String> = self
                .connection
                .query_row(
                    "SELECT content_hash FROM files WHERE path = ?1",
                    params![file.relative_path],
                    |row| row.get(0),
                )
                .optional()?;
            if previous_hash.as_deref() != Some(current_hash.as_str()) {
                stale.insert(file.relative_path.clone());
            }
        }
        Ok(stale)
    }

    /// Refresh the index atomically from an already-redacted corpus.
    pub fn refresh(
        &mut self,
        files: &[FileInfo],
        chunks: &[Chunk],
        graph: &ImportGraph,
        root_path: &Path,
    ) -> Result<IndexRefresh> {
        let tx = self.connection.transaction()?;
        let mut refresh = IndexRefresh::default();
        let current_paths: HashSet<&str> =
            files.iter().map(|file| file.relative_path.as_str()).collect();

        let existing_paths: Vec<String> = {
            let mut statement = tx.prepare("SELECT path FROM files ORDER BY path")?;
            let paths = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            paths
        };
        for path in existing_paths {
            if !current_paths.contains(path.as_str()) {
                tx.execute("DELETE FROM files WHERE path = ?1", params![path])?;
                refresh.removed_files += 1;
            }
        }

        let mut chunks_by_file: HashMap<&str, Vec<&Chunk>> = HashMap::new();
        for chunk in chunks {
            chunks_by_file.entry(chunk.path.as_str()).or_default().push(chunk);
        }

        for file in files {
            let raw_hash = hash_file(&file.path)
                .with_context(|| format!("failed to hash {} for index", file.relative_path))?;
            let previous_hash: Option<String> = tx
                .query_row(
                    "SELECT content_hash FROM files WHERE path = ?1",
                    params![file.relative_path],
                    |row| row.get(0),
                )
                .optional()?;
            let unchanged =
                !self.config_changed && previous_hash.as_deref() == Some(raw_hash.as_str());
            let tags = serde_json::to_string(&file.tags.iter().collect::<Vec<_>>())?;
            tx.execute(
                "INSERT INTO files (path, file_id, content_hash, size_bytes, language, extension, priority, tags, classification)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(path) DO UPDATE SET
                   file_id = excluded.file_id,
                   content_hash = excluded.content_hash,
                   size_bytes = excluded.size_bytes,
                   language = excluded.language,
                   extension = excluded.extension,
                   priority = excluded.priority,
                   tags = excluded.tags,
                   classification = excluded.classification",
                params![
                    file.relative_path,
                    file.id,
                    raw_hash,
                    file.size_bytes,
                    file.language,
                    file.extension,
                    file.priority,
                    tags,
                    classification(file),
                ],
            )?;

            if unchanged {
                refresh.reused_files += 1;
                continue;
            }

            refresh.updated_files += 1;
            tx.execute("DELETE FROM chunks WHERE file_path = ?1", params![file.relative_path])?;
            if let Some(file_chunks) = chunks_by_file.get(file.relative_path.as_str()) {
                for chunk in file_chunks {
                    insert_chunk(&tx, chunk, &file.relative_path)?;
                }
            }
        }

        tx.execute("DELETE FROM imports", [])?;
        let relative_by_absolute: HashMap<PathBuf, &str> = files
            .iter()
            .map(|file| (normalize_abs(&file.path), file.relative_path.as_str()))
            .collect();
        let mut import_rows = Vec::new();
        for (source, targets) in &graph.edges {
            let Some(source) = relative_by_absolute.get(&normalize_abs(source)) else { continue };
            for target in targets {
                let Some(target) = relative_by_absolute.get(&normalize_abs(target)) else {
                    continue;
                };
                import_rows.push(((*source).to_string(), (*target).to_string()));
            }
        }
        import_rows.sort();
        import_rows.dedup();
        for (source, target) in import_rows {
            tx.execute(
                "INSERT INTO imports (source_path, target_path) VALUES (?1, ?2)",
                params![source, target],
            )?;
        }

        set_metadata_tx(&tx, "schema_version", INDEX_SCHEMA_VERSION)?;
        set_metadata_tx(&tx, "root_key", &root_fingerprint(root_path))?;
        set_metadata_tx(&tx, "config_fingerprint", &self.config_fingerprint)?;
        set_metadata_tx(&tx, "redaction_fingerprint", &self.redaction_fingerprint)?;
        set_metadata_tx(&tx, "chunk_fingerprint", CHUNK_SCHEMA_FINGERPRINT)?;
        set_metadata_tx(&tx, "redacted", "true")?;
        refresh.indexed_chunks =
            tx.query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
        tx.commit()?;
        self.config_changed = false;
        Ok(refresh)
    }

    /// Load the current redacted corpus in deterministic order.
    pub fn load_chunks(&self) -> Result<Vec<Chunk>> {
        let mut statement = self.connection.prepare(
            "SELECT id, file_path, language, start_line, end_line, content, priority, tags,
                    token_estimate, file_id, chunk_index, chunks_in_file, byte_start,
                    byte_end, content_sha256, file_sha256
             FROM chunks ORDER BY file_path, start_line, end_line, id",
        )?;
        let rows = statement.query_map([], |row| {
            let tags_json: String = row.get(7)?;
            let tags = serde_json::from_str::<Vec<String>>(&tags_json)
                .unwrap_or_default()
                .into_iter()
                .collect::<BTreeSet<_>>();
            Ok(Chunk {
                id: row.get(0)?,
                path: row.get(1)?,
                language: row.get(2)?,
                start_line: row.get(3)?,
                end_line: row.get(4)?,
                content: row.get(5)?,
                priority: row.get(6)?,
                tags,
                token_estimate: row.get(8)?,
                file_id: row.get(9)?,
                chunk_index: row.get(10)?,
                chunks_in_file: row.get(11)?,
                byte_start: row.get(12)?,
                byte_end: row.get(13)?,
                content_sha256: row.get(14)?,
                file_sha256: row.get(15)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

/// Resolve the platform cache location for a repository index.
#[must_use]
pub fn default_index_path(root_path: &Path) -> Option<PathBuf> {
    let base = if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(PathBuf::from).map(|home| home.join("Library/Caches"))
    } else if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).or_else(|| {
            std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .map(|home| home.join("AppData/Local"))
        })
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from).map(|home| home.join(".cache")))
    }?;
    Some(base.join("repo-context/indexes").join(root_key(root_path)).join("index.sqlite"))
}

/// Stable hash of configuration fields that affect indexed content.
#[must_use]
pub fn config_fingerprint(config: &Config) -> String {
    let mut value = serde_json::to_value(config).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut value {
        for key in ["include_extensions", "exclude_globs"] {
            if let Some(Value::Array(values)) = map.get_mut(key) {
                values.sort_by_key(Value::to_string);
            }
        }
    }
    let serialized = serde_json::to_vec(&value).unwrap_or_default();
    format!("{:x}", Sha256::digest(serialized))
}

/// Stable hash of redaction settings stored alongside the index metadata.
#[must_use]
pub fn redaction_fingerprint(config: &Config) -> String {
    let serialized =
        serde_json::to_vec(&(config.redact_secrets, config.redaction_mode, &config.redaction))
            .unwrap_or_default();
    format!("{:x}", Sha256::digest(serialized))
}

fn initialize_schema(connection: &Connection) -> Result<()> {
    let metadata_exists = table_exists(connection, "metadata")?;
    let data_exists = table_exists(connection, "files")?
        || table_exists(connection, "chunks")?
        || table_exists(connection, "imports")?
        || table_exists(connection, "symbols")?;
    if data_exists && !metadata_exists {
        anyhow::bail!("unsupported index database: missing metadata table");
    }
    if metadata_exists && metadata(connection, "schema_version")?.is_none() {
        anyhow::bail!("unsupported index database: missing schema version");
    }
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS metadata (
             key TEXT PRIMARY KEY NOT NULL,
             value TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS files (
             path TEXT PRIMARY KEY NOT NULL,
             file_id TEXT NOT NULL,
             content_hash TEXT NOT NULL,
             size_bytes INTEGER NOT NULL,
             language TEXT NOT NULL,
             extension TEXT NOT NULL,
             priority REAL NOT NULL,
             tags TEXT NOT NULL,
             classification TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS chunks (
             id TEXT PRIMARY KEY NOT NULL,
             file_path TEXT NOT NULL REFERENCES files(path) ON DELETE CASCADE,
             language TEXT NOT NULL,
             start_line INTEGER NOT NULL,
             end_line INTEGER NOT NULL,
             content TEXT NOT NULL,
             priority REAL NOT NULL,
             tags TEXT NOT NULL,
             token_estimate INTEGER NOT NULL,
             file_id TEXT NOT NULL,
             chunk_index INTEGER NOT NULL,
             chunks_in_file INTEGER NOT NULL,
             byte_start INTEGER,
             byte_end INTEGER,
             content_sha256 TEXT NOT NULL,
             file_sha256 TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS imports (
             source_path TEXT NOT NULL REFERENCES files(path) ON DELETE CASCADE,
             target_path TEXT NOT NULL REFERENCES files(path) ON DELETE CASCADE,
             PRIMARY KEY (source_path, target_path)
         );
         CREATE TABLE IF NOT EXISTS symbols (
             name TEXT NOT NULL,
             kind TEXT NOT NULL,
             chunk_id TEXT NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
             PRIMARY KEY (name, kind, chunk_id)
         );
         CREATE INDEX IF NOT EXISTS chunks_file_path_idx ON chunks(file_path);
         CREATE INDEX IF NOT EXISTS chunks_language_idx ON chunks(language);
         CREATE INDEX IF NOT EXISTS symbols_name_idx ON symbols(name);
         CREATE INDEX IF NOT EXISTS imports_target_idx ON imports(target_path);",
    )?;
    if let Some(schema_version) = metadata(connection, "schema_version")? {
        if schema_version != INDEX_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported index schema version '{schema_version}' (expected {INDEX_SCHEMA_VERSION})"
            );
        }
    }
    Ok(())
}

fn insert_chunk(
    transaction: &rusqlite::Transaction<'_>,
    chunk: &Chunk,
    file_path: &str,
) -> Result<()> {
    let tags = serde_json::to_string(&chunk.tags.iter().collect::<Vec<_>>())?;
    transaction.execute(
        "INSERT INTO chunks
         (id, file_path, language, start_line, end_line, content, priority, tags,
          token_estimate, file_id, chunk_index, chunks_in_file, byte_start, byte_end,
          content_sha256, file_sha256)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            chunk.id,
            file_path,
            chunk.language,
            chunk.start_line,
            chunk.end_line,
            chunk.content,
            chunk.priority,
            tags,
            chunk.token_estimate,
            chunk.file_id,
            chunk.chunk_index,
            chunk.chunks_in_file,
            chunk.byte_start,
            chunk.byte_end,
            chunk.content_sha256,
            chunk.file_sha256,
        ],
    )?;
    for tag in &chunk.tags {
        let Some((kind, name)) = tag.split_once(':') else { continue };
        if matches!(kind, "def" | "type" | "impl" | "class" | "method" | "function") {
            transaction.execute(
                "INSERT INTO symbols (name, kind, chunk_id) VALUES (?1, ?2, ?3)",
                params![name, kind, chunk.id],
            )?;
        }
    }
    Ok(())
}

fn metadata(connection: &Connection, key: &str) -> Result<Option<String>> {
    Ok(connection
        .query_row("SELECT value FROM metadata WHERE key = ?1", params![key], |row| row.get(0))
        .optional()?)
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        params![table],
        |row| row.get(0),
    )?)
}

fn set_metadata_tx(transaction: &rusqlite::Transaction<'_>, key: &str, value: &str) -> Result<()> {
    transaction.execute(
        "INSERT INTO metadata (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn classification(file: &FileInfo) -> &'static str {
    if file.is_readme {
        "readme"
    } else if file.is_config {
        "config"
    } else if file.is_doc {
        "documentation"
    } else if file.tags.contains("test") {
        "test"
    } else {
        "source"
    }
}

fn hash_file(path: &Path) -> Result<String> {
    let content = crate::utils::read_file_safe(path, None, None)?.0;
    Ok(format!("{:x}", Sha256::digest(content.as_bytes())))
}

fn normalize_abs(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn root_fingerprint(root_path: &Path) -> String {
    let canonical = normalize_abs(root_path);
    let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
    format!("{:x}", digest)[..16].to_string()
}

fn root_key(root_path: &Path) -> String {
    root_fingerprint(root_path)
}

#[cfg(test)]
mod tests {
    use super::{config_fingerprint, IndexStore};
    use crate::domain::{Chunk, Config, FileInfo};
    use crate::module::graph;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn file(root: &TempDir, path: &str) -> FileInfo {
        let absolute = root.path().join(path);
        FileInfo {
            path: absolute,
            relative_path: path.to_string(),
            size_bytes: 10,
            extension: ".rs".to_string(),
            language: "rust".to_string(),
            id: path.to_string(),
            priority: 0.8,
            token_estimate: 5,
            tags: BTreeSet::new(),
            is_readme: false,
            is_config: false,
            is_doc: false,
        }
    }

    fn chunk(path: &str, content: &str) -> Chunk {
        Chunk {
            id: format!("{path}:1"),
            path: path.to_string(),
            language: "rust".to_string(),
            start_line: 1,
            end_line: 1,
            content: content.to_string(),
            priority: 0.8,
            tags: BTreeSet::new(),
            token_estimate: 2,
            file_id: path.to_string(),
            chunk_index: 0,
            chunks_in_file: 1,
            byte_start: Some(0),
            byte_end: Some(content.len()),
            content_sha256: "chunk".to_string(),
            file_sha256: "file".to_string(),
        }
    }

    #[test]
    fn refresh_reuses_and_removes_rows_without_storing_absolute_paths() {
        let root = TempDir::new().expect("root");
        fs::write(root.path().join("a.rs"), "fn a() {}\n").expect("write a");
        fs::write(root.path().join("b.rs"), "fn b() {}\n").expect("write b");
        let a = file(&root, "a.rs");
        let b = file(&root, "b.rs");
        let db = root.path().join("index.sqlite");
        let config = Config::default();
        let mut store = IndexStore::open(
            &db,
            root.path(),
            &config_fingerprint(&config),
            &super::redaction_fingerprint(&config),
        )
        .expect("open");
        let files = vec![a.clone(), b.clone()];
        let chunks = vec![chunk("a.rs", "redacted a"), chunk("b.rs", "redacted b")];
        let refresh =
            store.refresh(&files, &chunks, &graph::build(&files), root.path()).expect("refresh");
        assert_eq!(refresh.updated_files, 2);
        assert_eq!(store.load_chunks().expect("load").len(), 2);

        let refresh = store
            .refresh(
                std::slice::from_ref(&a),
                std::slice::from_ref(&chunks[0]),
                &graph::build(std::slice::from_ref(&a)),
                root.path(),
            )
            .expect("refresh removal");
        assert_eq!(refresh.removed_files, 1);
        assert_eq!(refresh.reused_files, 1);
        let loaded = store.load_chunks().expect("load");
        assert_eq!(loaded.len(), 1);
        assert!(!loaded[0].content.contains("fn a"));
        assert!(!fs::read_to_string(&db)
            .unwrap_or_default()
            .contains(root.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn unknown_schema_is_rejected() {
        let root = TempDir::new().expect("root");
        let db = root.path().join("index.sqlite");
        let connection = rusqlite::Connection::open(&db).expect("db");
        connection.execute_batch("CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL); INSERT INTO metadata VALUES ('schema_version', '99');").expect("schema");
        drop(connection);
        let error = match IndexStore::open(&db, root.path(), "config", "redaction") {
            Ok(_) => panic!("reject schema"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("unsupported index schema"));
        assert!(db.exists(), "a rejected database must not be deleted");
    }

    #[test]
    fn unredacted_index_marker_is_rejected() {
        let root = TempDir::new().expect("root");
        let db = root.path().join("index.sqlite");
        let connection = rusqlite::Connection::open(&db).expect("db");
        connection
            .execute_batch(
                "CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO metadata VALUES ('schema_version', '1');
                 INSERT INTO metadata VALUES ('redacted', 'false');",
            )
            .expect("schema");
        drop(connection);
        let error = match IndexStore::open(&db, root.path(), "config", "redaction") {
            Ok(_) => panic!("reject unredacted marker"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("not marked as redacted"));
    }

    #[test]
    fn chunk_round_trip_is_deterministic() {
        let root = TempDir::new().expect("root");
        fs::write(root.path().join("a.rs"), "fn a() {}\n").expect("write");
        let file = file(&root, "a.rs");
        let db = root.path().join("index.sqlite");
        let mut store = IndexStore::open(&db, root.path(), "config", "redaction").expect("open");
        store
            .refresh(
                std::slice::from_ref(&file),
                &[chunk("a.rs", "safe")],
                &graph::build(std::slice::from_ref(&file)),
                root.path(),
            )
            .expect("refresh");
        let first = store.load_chunks().expect("first");
        let second = store.load_chunks().expect("second");
        assert_eq!(first[0].id, second[0].id);
        assert_eq!(first[0].content, "safe");
    }

    #[test]
    fn configuration_fingerprint_is_independent_of_set_insertion_order() {
        let first = Config {
            include_extensions: [".rs".to_string(), ".py".to_string()].into_iter().collect(),
            exclude_globs: ["target/**".to_string(), "dist/**".to_string()].into_iter().collect(),
            ..Config::default()
        };
        let second = Config {
            include_extensions: [".py".to_string(), ".rs".to_string()].into_iter().collect(),
            exclude_globs: ["dist/**".to_string(), "target/**".to_string()].into_iter().collect(),
            ..Config::default()
        };
        assert_eq!(config_fingerprint(&first), config_fingerprint(&second));
    }

    #[test]
    fn configuration_change_rebuilds_unchanged_source_rows() {
        let root = TempDir::new().expect("root");
        fs::write(root.path().join("a.rs"), "fn a() {}\n").expect("write");
        let file = file(&root, "a.rs");
        let db = root.path().join("index.sqlite");
        let mut first =
            IndexStore::open(&db, root.path(), "config-a", "redaction-a").expect("open");
        first
            .refresh(
                std::slice::from_ref(&file),
                &[chunk("a.rs", "redaction-a")],
                &graph::build(std::slice::from_ref(&file)),
                root.path(),
            )
            .expect("first refresh");
        drop(first);

        let mut second =
            IndexStore::open(&db, root.path(), "config-b", "redaction-b").expect("reopen");
        let refresh = second
            .refresh(
                std::slice::from_ref(&file),
                &[chunk("a.rs", "redaction-b")],
                &graph::build(std::slice::from_ref(&file)),
                root.path(),
            )
            .expect("second refresh");
        assert_eq!(refresh.updated_files, 1);
        assert_eq!(second.load_chunks().expect("load")[0].content, "redaction-b");
    }

    #[test]
    fn failed_refresh_does_not_commit_new_configuration_metadata() {
        let root = TempDir::new().expect("root");
        fs::write(root.path().join("a.rs"), "fn a() {}\n").expect("write");
        let file = file(&root, "a.rs");
        let db = root.path().join("index.sqlite");
        let mut first =
            IndexStore::open(&db, root.path(), "config-a", "redaction-a").expect("open");
        first
            .refresh(
                std::slice::from_ref(&file),
                &[chunk("a.rs", "safe-a")],
                &graph::build(std::slice::from_ref(&file)),
                root.path(),
            )
            .expect("first refresh");
        drop(first);

        let missing = FileInfo {
            path: root.path().join("missing.rs"),
            relative_path: "missing.rs".to_string(),
            ..file.clone()
        };
        let mut second =
            IndexStore::open(&db, root.path(), "config-b", "redaction-b").expect("reopen");
        assert!(second.refresh(&[missing], &[], &graph::build(&[]), root.path()).is_err());
        drop(second);

        let reopened =
            IndexStore::open(&db, root.path(), "config-b", "redaction-b").expect("reopen");
        assert!(reopened
            .paths_needing_refresh(std::slice::from_ref(&file))
            .expect("stale paths")
            .contains("a.rs"));
    }

    #[allow(dead_code)]
    fn _path_type_is_used(path: PathBuf) -> PathBuf {
        path
    }
}

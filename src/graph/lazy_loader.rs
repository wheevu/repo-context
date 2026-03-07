//! Lazy chunk loading from the index database.
//!
//! Provides functionality to load chunks on-demand from a SQLite database
//! without loading the entire dataset into memory.

use crate::domain::Chunk;
use rusqlite::{params, Connection};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Loader for lazy chunk retrieval from SQLite database.
#[derive(Debug, Clone)]
pub struct LazyChunkLoader {
    /// Path to the SQLite database file
    db_path: PathBuf,
}

impl LazyChunkLoader {
    /// Creates a new LazyChunkLoader.
    ///
    /// # Arguments
    /// * `db_path` - Path to the SQLite database file
    pub fn new(db_path: &Path) -> Self {
        Self { db_path: db_path.to_path_buf() }
    }

    /// Checks if a file exists in the database.
    ///
    /// # Arguments
    /// * `path` - File path to check
    ///
    /// # Returns
    /// true if file exists, false otherwise
    pub fn has_file(&self, path: &str) -> bool {
        let Ok(conn) = Connection::open(&self.db_path) else {
            return false;
        };
        conn.query_row("SELECT 1 FROM files WHERE path = ?1 LIMIT 1", params![path], |row| {
            row.get::<_, i64>(0)
        })
        .is_ok()
    }

    /// Loads all chunks for a given file path.
    ///
    /// # Arguments
    /// * `path` - File path to load chunks for
    ///
    /// # Returns
    /// Vector of chunks for the file, empty if file not found
    pub fn load_chunks_for_file(&self, path: &str) -> Vec<Chunk> {
        let Ok(conn) = Connection::open(&self.db_path) else {
            return Vec::new();
        };
        let Ok(mut stmt) = conn.prepare(
            "
            SELECT id, file_path, start_line, end_line, language, priority, token_estimate,
                   tags_json, content
            FROM chunks
            WHERE file_path = ?1
            ORDER BY start_line, id
            ",
        ) else {
            return Vec::new();
        };
        let rows = stmt.query_map(params![path], |row| {
            let tags_json: String = row.get(7)?;
            let tags: BTreeSet<String> =
                serde_json::from_str(&tags_json).unwrap_or_else(|_| BTreeSet::new());
            Ok(Chunk {
                id: row.get(0)?,
                path: row.get(1)?,
                start_line: row.get(2)?,
                end_line: row.get(3)?,
                language: row.get(4)?,
                priority: row.get(5)?,
                token_estimate: row.get(6)?,
                tags,
                content: row.get(8)?,
            })
        });
        match rows {
            Ok(iter) => iter.filter_map(Result::ok).collect(),
            Err(_) => Vec::new(),
        }
    }
}

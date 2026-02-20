//! Lazy chunk loading from the index database.

use crate::domain::Chunk;
use rusqlite::{params, Connection};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct LazyChunkLoader {
    db_path: PathBuf,
}

impl LazyChunkLoader {
    pub fn new(db_path: &Path) -> Self {
        Self { db_path: db_path.to_path_buf() }
    }

    pub fn has_file(&self, path: &str) -> bool {
        let Ok(conn) = Connection::open(&self.db_path) else {
            return false;
        };
        conn.query_row("SELECT 1 FROM files WHERE path = ?1 LIMIT 1", params![path], |row| {
            row.get::<_, i64>(0)
        })
        .is_ok()
    }

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

        let rows = match stmt.query_map(params![path], |row| {
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
        }) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        rows.filter_map(Result::ok).collect()
    }
}

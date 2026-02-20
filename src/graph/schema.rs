//! SQLite schema for retrieval graph.

use anyhow::{bail, Result};
use rusqlite::Connection;
use std::path::Path;

pub const SCHEMA_VERSION: i64 = 2;

pub fn open_or_create(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER NOT NULL
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
        ",
    )?;

    let current: Option<i64> =
        conn.query_row("SELECT version FROM schema_version LIMIT 1", [], |row| row.get(0)).ok();
    match current {
        None => {
            conn.execute("INSERT INTO schema_version(version) VALUES(?1)", [SCHEMA_VERSION])?;
        }
        Some(version) if version == SCHEMA_VERSION => {}
        Some(1) => {
            migrate_v1_to_v2(&conn)?;
            conn.execute("UPDATE schema_version SET version = ?1", [SCHEMA_VERSION])?;
        }
        Some(version) => {
            bail!("Unsupported symbol_graph schema version {version}; expected {}", SCHEMA_VERSION);
        }
    }
    Ok(conn)
}

fn migrate_v1_to_v2(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        ALTER TABLE symbol_refs RENAME TO symbol_refs_old;
        CREATE TABLE symbol_refs (
            symbol TEXT NOT NULL,
            chunk_id TEXT NOT NULL,
            ref_kind TEXT NOT NULL DEFAULT 'ref',
            PRIMARY KEY (symbol, chunk_id, ref_kind)
        );
        INSERT OR IGNORE INTO symbol_refs(symbol, chunk_id, ref_kind)
            SELECT symbol, chunk_id, 'ref' FROM symbol_refs_old;
        DROP TABLE symbol_refs_old;
        ",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn open_or_create_inserts_schema_version() {
        let tmp = TempDir::new().expect("temp dir");
        let db = tmp.path().join("graph.db");
        let conn = open_or_create(&db).expect("open db");
        let version: i64 = conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| row.get(0))
            .expect("query version");
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn open_or_create_rejects_mismatched_schema_version() {
        let tmp = TempDir::new().expect("temp dir");
        let db = tmp.path().join("graph.db");
        let conn = Connection::open(&db).expect("open db");
        conn.execute_batch(
            "CREATE TABLE schema_version(version INTEGER NOT NULL);\
             INSERT INTO schema_version(version) VALUES(999);",
        )
        .expect("seed schema version");

        let err = open_or_create(&db).expect_err("must fail on mismatched schema version");
        assert!(err.to_string().contains("Unsupported symbol_graph schema version"));
    }
}

//! Graph persistence helpers.

use crate::domain::Chunk;
use crate::graph::symbol_usage::{extract_symbol_usages, UsageKind};
use crate::rank::{extract_import_references, resolve_reference, symbol_definitions};
use anyhow::Result;
use rusqlite::{params, Connection};
use std::collections::HashSet;

pub fn persist_graph(conn: &mut Connection, chunks: &[Chunk]) -> Result<(usize, usize)> {
    let tx = conn.transaction()?;

    tx.execute("DELETE FROM symbol_chunks", [])?;
    tx.execute("DELETE FROM file_imports", [])?;
    tx.execute("DELETE FROM chunk_meta", [])?;
    tx.execute("DELETE FROM symbol_refs", [])?;

    let known_files: HashSet<String> = chunks.iter().map(|c| c.path.clone()).collect();

    let mut symbol_count = 0usize;
    for chunk in chunks {
        for tag in &chunk.tags {
            if let Some((kind, symbol)) = tag.split_once(':') {
                if !matches!(kind, "def" | "type" | "impl") {
                    continue;
                }
                tx.execute(
                    "INSERT OR REPLACE INTO symbol_chunks(symbol, chunk_id, kind, path) VALUES(?1, ?2, ?3, ?4)",
                    params![symbol.to_ascii_lowercase(), chunk.id, kind, chunk.path],
                )?;
                symbol_count += 1;
            }
        }

        tx.execute(
            "INSERT OR REPLACE INTO chunk_meta(chunk_id, path, start_line, end_line, priority) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![chunk.id, chunk.path, chunk.start_line as i64, chunk.end_line as i64, chunk.priority],
        )?;
    }

    let defs = symbol_definitions(chunks);
    let mut edge_count = 0usize;
    for chunk in chunks {
        for reference in extract_import_references(&chunk.content) {
            for target in resolve_reference(&reference, &chunk.path, &known_files) {
                if target == chunk.path {
                    continue;
                }
                tx.execute(
                    "INSERT OR REPLACE INTO file_imports(source_path, target_path) VALUES(?1, ?2)",
                    params![chunk.path, target],
                )?;
                edge_count += 1;
            }
        }

        let mut usages = extract_symbol_usages(&chunk.content, &chunk.language);
        if usages.is_empty() {
            usages = chunk
                .content
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .map(|t| (t.to_ascii_lowercase(), UsageKind::Ref))
                .filter(|(t, _)| t.len() >= 2)
                .collect();
        }
        for (symbol, ref_kind) in usages {
            if defs.contains_key(&symbol) {
                tx.execute(
                    "INSERT OR REPLACE INTO symbol_refs(symbol, chunk_id, ref_kind) VALUES(?1, ?2, ?3)",
                    params![symbol, chunk.id, ref_kind.as_str()],
                )?;
            }
        }
    }

    tx.commit()?;
    Ok((symbol_count, edge_count))
}

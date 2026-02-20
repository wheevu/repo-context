//! Portable code-intel export from local index.

use anyhow::{Context, Result};
use clap::Args;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct CodeIntelArgs {
    /// SQLite index database path
    #[arg(long, value_name = "FILE", default_value = ".repo-context/index.sqlite")]
    pub db: PathBuf,

    /// Output path for portable code-intel JSON
    #[arg(long, value_name = "FILE", default_value = ".repo-context/codeintel.json")]
    pub out: PathBuf,
}

pub fn run(args: CodeIntelArgs) -> Result<()> {
    let conn = Connection::open(&args.db)
        .with_context(|| format!("Failed to open SQLite database at {}", args.db.display()))?;

    let has_symbols: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'symbols'",
        [],
        |row| row.get(0),
    )?;
    if has_symbols == 0 {
        anyhow::bail!(
            "Index schema not found in {}. Run `repo-context index` first.",
            args.db.display()
        );
    }

    let project_root = metadata_value(&conn, "repo_root")?.unwrap_or_default();
    let files = load_files(&conn)?;
    let symbol_export = load_symbols(&conn)?;

    let payload = CodeIntelDocument {
        schema_version: "0.4.0".to_string(),
        format: "scip-lite".to_string(),
        project_root,
        files,
        symbols: symbol_export.symbols,
        occurrences: symbol_export.occurrences,
        relationships: symbol_export.relationships,
        symbol_links: symbol_export.symbol_links,
        stats: CodeIntelStats::default(),
    };
    let payload = payload.with_stats();

    if let Some(parent) = args.out.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&payload)?;
    fs::write(&args.out, json)?;

    println!("Code-intel export written to {}", args.out.display());
    println!("  files: {}", payload.files.len());
    println!("  symbols: {}", payload.symbols.len());
    println!("  occurrences: {}", payload.occurrences.len());
    println!("  symbol_links: {}", payload.symbol_links.len());
    println!("  edge_kinds: {}", payload.stats.edge_kind_counts.len());
    Ok(())
}

#[derive(Debug, Serialize)]
struct CodeIntelDocument {
    schema_version: String,
    format: String,
    project_root: String,
    files: Vec<PortableFile>,
    symbols: Vec<PortableSymbol>,
    occurrences: Vec<PortableOccurrence>,
    relationships: Vec<PortableRelationship>,
    symbol_links: Vec<PortableSymbolLink>,
    stats: CodeIntelStats,
}

impl CodeIntelDocument {
    fn with_stats(mut self) -> Self {
        self.stats = compute_stats(
            self.files.as_slice(),
            &self.symbols,
            &self.occurrences,
            &self.symbol_links,
        );
        self
    }
}

#[derive(Debug, Serialize, Default)]
struct CodeIntelStats {
    file_count: usize,
    symbol_count: usize,
    occurrence_count: usize,
    symbol_link_count: usize,
    symbol_kind_counts: BTreeMap<String, usize>,
    edge_kind_counts: BTreeMap<String, usize>,
    language_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
struct PortableFile {
    path: String,
    language: String,
    file_hash: String,
}

#[derive(Debug, Serialize)]
struct PortableSymbol {
    id: String,
    symbol: String,
    kinds: Vec<String>,
}

#[derive(Debug, Serialize, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct PortableOccurrence {
    id: String,
    symbol_id: String,
    path: String,
    chunk_id: String,
    start_line: usize,
    end_line: usize,
    role: String,
}

#[derive(Debug, Serialize, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct PortableRelationship {
    kind: String,
    from_symbol_id: String,
    to_occurrence_id: String,
}

#[derive(Debug, Serialize, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct PortableSymbolLink {
    kind: String,
    from_symbol_id: String,
    to_symbol_id: String,
}

fn compute_stats(
    files: &[PortableFile],
    symbols: &[PortableSymbol],
    occurrences: &[PortableOccurrence],
    symbol_links: &[PortableSymbolLink],
) -> CodeIntelStats {
    let mut symbol_kind_counts = BTreeMap::new();
    let mut edge_kind_counts = BTreeMap::new();
    let mut language_counts = BTreeMap::new();

    for file in files {
        *language_counts.entry(file.language.clone()).or_insert(0) += 1;
    }
    for symbol in symbols {
        for kind in &symbol.kinds {
            *symbol_kind_counts.entry(kind.clone()).or_insert(0) += 1;
        }
    }
    for link in symbol_links {
        *edge_kind_counts.entry(link.kind.clone()).or_insert(0) += 1;
    }

    CodeIntelStats {
        file_count: files.len(),
        symbol_count: symbols.len(),
        occurrence_count: occurrences.len(),
        symbol_link_count: symbol_links.len(),
        symbol_kind_counts,
        edge_kind_counts,
        language_counts,
    }
}

fn metadata_value(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM metadata WHERE key = ?1 LIMIT 1")?;
    let value = stmt.query_row(params![key], |row| row.get::<_, String>(0)).optional()?;
    Ok(value)
}

fn load_files(conn: &Connection) -> Result<Vec<PortableFile>> {
    let mut stmt = conn.prepare("SELECT path, language, file_hash FROM files ORDER BY path ASC")?;
    let rows = stmt.query_map([], |row| {
        Ok(PortableFile { path: row.get(0)?, language: row.get(1)?, file_hash: row.get(2)? })
    })?;

    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn load_symbols(conn: &Connection) -> Result<SymbolExport> {
    let mut by_symbol: BTreeMap<String, SymbolAccumulator> = BTreeMap::new();

    let mut defs_stmt = conn.prepare(
        "
        SELECT s.symbol, s.kind, c.file_path, c.id, c.start_line, c.end_line
        FROM symbols s
        JOIN chunks c ON c.id = s.chunk_id
        ORDER BY s.symbol ASC, c.file_path ASC, c.start_line ASC
        ",
    )?;
    let def_rows = defs_stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            RawOccurrence {
                path: row.get(2)?,
                chunk_id: row.get(3)?,
                start_line: row.get::<_, i64>(4)? as usize,
                end_line: row.get::<_, i64>(5)? as usize,
            },
        ))
    })?;
    for row in def_rows {
        let (symbol, kind, occ) = row?;
        let entry = by_symbol.entry(symbol).or_default();
        entry.kinds.insert(kind);
        entry.definitions.insert(occ);
    }

    if by_symbol.is_empty() {
        return Ok(SymbolExport::default());
    }

    let symbol_set: HashSet<String> = by_symbol.keys().cloned().collect();

    let mut chunks_stmt =
        conn.prepare("SELECT file_path, id, start_line, end_line, content FROM chunks")?;
    let chunk_rows = chunks_stmt.query_map([], |row| {
        let content: String = row.get(4)?;
        Ok(ChunkRecord {
            path: row.get(0)?,
            chunk_id: row.get(1)?,
            start_line: row.get::<_, i64>(2)? as usize,
            end_line: row.get::<_, i64>(3)? as usize,
            tokens: tokenize(&content).into_iter().collect(),
            import_refs: extract_import_references(&content),
        })
    })?;
    let chunks: Vec<ChunkRecord> = chunk_rows.collect::<rusqlite::Result<Vec<_>>>()?;

    for chunk in &chunks {
        for token in &chunk.tokens {
            if !symbol_set.contains(token) {
                continue;
            }
            if let Some(acc) = by_symbol.get_mut(token) {
                acc.references.insert(RawOccurrence {
                    path: chunk.path.clone(),
                    chunk_id: chunk.chunk_id.clone(),
                    start_line: chunk.start_line,
                    end_line: chunk.end_line,
                });
            }
        }
    }

    let mut symbols = Vec::new();
    let mut occurrences = Vec::new();
    let mut relationships = Vec::new();
    let mut definition_chunks_by_symbol_id: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut definition_symbols_by_file: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut definition_symbols_by_chunk: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut reference_occurrences: Vec<ReferenceOccurrence> = Vec::new();
    for (symbol, acc) in by_symbol {
        let symbol_id = stable_id(&format!("symbol:{symbol}"));
        symbols.push(PortableSymbol {
            id: symbol_id.clone(),
            symbol,
            kinds: acc.kinds.into_iter().collect(),
        });

        for occ in acc.definitions {
            let path = occ.path.clone();
            let chunk_id = occ.chunk_id.clone();
            let occurrence_id = stable_id(&format!(
                "occ:{}:{}:{}:{}:{}:{}",
                symbol_id, "definition", &occ.path, &occ.chunk_id, occ.start_line, occ.end_line
            ));
            occurrences.push(PortableOccurrence {
                id: occurrence_id.clone(),
                symbol_id: symbol_id.clone(),
                path: path.clone(),
                chunk_id: chunk_id.clone(),
                start_line: occ.start_line,
                end_line: occ.end_line,
                role: "definition".to_string(),
            });
            relationships.push(PortableRelationship {
                kind: "defines".to_string(),
                from_symbol_id: symbol_id.clone(),
                to_occurrence_id: occurrence_id,
            });
            definition_chunks_by_symbol_id
                .entry(symbol_id.clone())
                .or_default()
                .insert(chunk_id.clone());
            definition_symbols_by_file.entry(path).or_default().insert(symbol_id.clone());
            definition_symbols_by_chunk.entry(chunk_id).or_default().insert(symbol_id.clone());
        }

        for occ in acc.references {
            let path = occ.path.clone();
            let chunk_id = occ.chunk_id.clone();
            let occurrence_id = stable_id(&format!(
                "occ:{}:{}:{}:{}:{}:{}",
                symbol_id, "reference", &occ.path, &occ.chunk_id, occ.start_line, occ.end_line
            ));
            occurrences.push(PortableOccurrence {
                id: occurrence_id.clone(),
                symbol_id: symbol_id.clone(),
                path: path.clone(),
                chunk_id: chunk_id.clone(),
                start_line: occ.start_line,
                end_line: occ.end_line,
                role: "reference".to_string(),
            });
            relationships.push(PortableRelationship {
                kind: "references".to_string(),
                from_symbol_id: symbol_id.clone(),
                to_occurrence_id: occurrence_id,
            });
            reference_occurrences.push(ReferenceOccurrence {
                target_symbol_id: symbol_id.clone(),
                path,
                chunk_id,
            });
        }
    }

    let symbol_links = infer_symbol_links(
        &definition_chunks_by_symbol_id,
        &definition_symbols_by_file,
        &definition_symbols_by_chunk,
        &reference_occurrences,
        &chunks,
    );

    Ok(SymbolExport { symbols, occurrences, relationships, symbol_links })
}

#[derive(Default)]
struct SymbolAccumulator {
    kinds: BTreeSet<String>,
    definitions: BTreeSet<RawOccurrence>,
    references: BTreeSet<RawOccurrence>,
}

#[derive(Default)]
struct SymbolExport {
    symbols: Vec<PortableSymbol>,
    occurrences: Vec<PortableOccurrence>,
    relationships: Vec<PortableRelationship>,
    symbol_links: Vec<PortableSymbolLink>,
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
struct RawOccurrence {
    path: String,
    chunk_id: String,
    start_line: usize,
    end_line: usize,
}

#[derive(Clone)]
struct ReferenceOccurrence {
    target_symbol_id: String,
    path: String,
    chunk_id: String,
}

struct ChunkRecord {
    path: String,
    chunk_id: String,
    start_line: usize,
    end_line: usize,
    tokens: BTreeSet<String>,
    import_refs: Vec<String>,
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter_map(|token| {
            let lower = token.trim().to_ascii_lowercase();
            if lower.len() >= 2 {
                Some(lower)
            } else {
                None
            }
        })
        .collect()
}

fn stable_id(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    format!("{:x}", digest)[..16].to_string()
}

fn infer_symbol_links(
    definitions_by_symbol: &BTreeMap<String, BTreeSet<String>>,
    symbols_by_file: &BTreeMap<String, BTreeSet<String>>,
    symbols_by_chunk: &BTreeMap<String, BTreeSet<String>>,
    references: &[ReferenceOccurrence],
    chunks: &[ChunkRecord],
) -> Vec<PortableSymbolLink> {
    let mut links = BTreeSet::new();

    for reference in references {
        let source_symbols = symbols_by_chunk
            .get(&reference.chunk_id)
            .or_else(|| symbols_by_file.get(&reference.path));
        if let Some(source_symbols) = source_symbols {
            for source in source_symbols {
                if source != &reference.target_symbol_id {
                    links.insert(PortableSymbolLink {
                        kind: "calls".to_string(),
                        from_symbol_id: source.clone(),
                        to_symbol_id: reference.target_symbol_id.clone(),
                    });
                }
            }
            if is_test_like_file(&reference.path) {
                for source in source_symbols {
                    if source != &reference.target_symbol_id {
                        links.insert(PortableSymbolLink {
                            kind: "tests".to_string(),
                            from_symbol_id: source.clone(),
                            to_symbol_id: reference.target_symbol_id.clone(),
                        });
                    }
                }
            }
        }
    }

    let known_files: HashSet<String> = symbols_by_file.keys().cloned().collect();
    for chunk in chunks {
        let Some(source_symbols) = symbols_by_file.get(&chunk.path) else {
            continue;
        };

        for import_ref in &chunk.import_refs {
            for target_file in resolve_reference(import_ref, &chunk.path, &known_files) {
                if let Some(target_symbols) = symbols_by_file.get(&target_file) {
                    for source in source_symbols {
                        for target in target_symbols {
                            if source != target {
                                links.insert(PortableSymbolLink {
                                    kind: "imports".to_string(),
                                    from_symbol_id: source.clone(),
                                    to_symbol_id: target.clone(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    for (source_symbol_id, definition_chunks) in definitions_by_symbol {
        for reference in references {
            if source_symbol_id != &reference.target_symbol_id
                && definition_chunks.contains(&reference.chunk_id)
            {
                links.insert(PortableSymbolLink {
                    kind: "calls".to_string(),
                    from_symbol_id: source_symbol_id.clone(),
                    to_symbol_id: reference.target_symbol_id.clone(),
                });
            }
        }
    }

    links.into_iter().collect()
}

fn extract_import_references(content: &str) -> Vec<String> {
    let mut refs = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();

        if let Some(rest) = trimmed.strip_prefix("from ") {
            if let Some(module) = rest.split_whitespace().next() {
                refs.push(module.trim_matches('"').trim_matches('\'').to_string());
            }
        }
        if let Some(rest) = trimmed.strip_prefix("import ") {
            for module in rest.split(',') {
                if let Some(module_name) = module.split_whitespace().next() {
                    refs.push(module_name.trim_matches('"').trim_matches('\'').to_string());
                }
            }
        }
        if let Some(rest) = trimmed.strip_prefix("use ") {
            if let Some(module) = rest.split(';').next() {
                refs.push(module.trim().to_string());
            }
        }
        if let Some(rest) = trimmed.strip_prefix("mod ") {
            if let Some(module) = rest.split(';').next() {
                refs.push(module.trim().to_string());
            }
        }
        for marker in [" from '", " from \"", "require('", "require(\""] {
            if let Some(pos) = trimmed.find(marker) {
                let tail = &trimmed[pos + marker.len()..];
                let module = tail.split(['\'', '"', ')']).next().unwrap_or("").trim();
                if !module.is_empty() {
                    refs.push(module.to_string());
                }
            }
        }
    }
    refs
}

fn resolve_reference(
    reference: &str,
    current_file: &str,
    known_files: &HashSet<String>,
) -> Vec<String> {
    let mut out = BTreeSet::new();
    let cleaned = reference
        .trim()
        .trim_start_matches("crate::")
        .trim_start_matches("self::")
        .trim_start_matches("super::")
        .replace("::", "/")
        .replace('.', "/");

    if cleaned.is_empty() {
        return Vec::new();
    }

    let mut candidates = vec![cleaned.clone()];
    candidates.extend(candidate_paths(&cleaned));

    if reference.starts_with("./") || reference.starts_with("../") {
        let base = Path::new(current_file).parent().unwrap_or_else(|| Path::new(""));
        let rel = normalize_path(base.join(reference));
        candidates.push(rel.clone());
        candidates.extend(candidate_paths(&rel));
    }

    let lower_known: BTreeMap<String, String> =
        known_files.iter().map(|file| (file.to_ascii_lowercase(), file.clone())).collect();

    for candidate in candidates {
        let lowered = candidate.to_ascii_lowercase();
        if let Some(exact) = lower_known.get(&lowered) {
            out.insert(exact.clone());
        }
        for (known, original) in &lower_known {
            if known.ends_with(&format!("/{lowered}"))
                || known.ends_with(&format!("/{lowered}.py"))
                || known.ends_with(&format!("/{lowered}.rs"))
                || known.ends_with(&format!("/{lowered}.ts"))
                || known.ends_with(&format!("/{lowered}.js"))
            {
                out.insert(original.clone());
            }
        }
    }

    out.into_iter().collect()
}

fn candidate_paths(module: &str) -> Vec<String> {
    [
        module.to_string(),
        format!("{module}.py"),
        format!("{module}/__init__.py"),
        format!("{module}.rs"),
        format!("{module}/mod.rs"),
        format!("{module}.ts"),
        format!("{module}.tsx"),
        format!("{module}.js"),
        format!("{module}.jsx"),
        format!("{module}.go"),
    ]
    .to_vec()
}

fn normalize_path(path: PathBuf) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn is_test_like_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("tests/")
        || lower.starts_with("test/")
        || lower.contains("/tests/")
        || lower.contains("/test/")
        || lower.contains("_test.")
        || lower.contains(".test.")
        || lower.contains("test_")
}

#[cfg(test)]
mod tests {
    use super::{infer_symbol_links, stable_id, tokenize, ChunkRecord, ReferenceOccurrence};
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn stable_id_is_deterministic() {
        let a = stable_id("symbol:refresh_token");
        let b = stable_id("symbol:refresh_token");
        assert_eq!(a, b);
    }

    #[test]
    fn tokenize_preserves_snake_case_terms() {
        let tokens = tokenize("refresh_token(user_id)");
        assert!(tokens.contains(&"refresh_token".to_string()));
        assert!(tokens.contains(&"user_id".to_string()));
    }

    #[test]
    fn infer_symbol_links_has_specific_edge_kinds() {
        let defs_by_symbol = BTreeMap::from([
            (String::from("a"), BTreeSet::from([String::from("chunk1")])),
            (String::from("b"), BTreeSet::from([String::from("chunk2")])),
        ]);
        let symbols_by_file = BTreeMap::from([
            (String::from("src/a.rs"), BTreeSet::from([String::from("a")])),
            (String::from("src/b.rs"), BTreeSet::from([String::from("b")])),
            (String::from("tests/test_a.rs"), BTreeSet::from([String::from("t")])),
        ]);
        let symbols_by_chunk = BTreeMap::from([
            (String::from("chunk1"), BTreeSet::from([String::from("a")])),
            (String::from("chunk_test"), BTreeSet::from([String::from("t")])),
        ]);
        let references = vec![
            ReferenceOccurrence {
                target_symbol_id: String::from("b"),
                path: String::from("src/a.rs"),
                chunk_id: String::from("chunk1"),
            },
            ReferenceOccurrence {
                target_symbol_id: String::from("a"),
                path: String::from("tests/test_a.rs"),
                chunk_id: String::from("chunk_test"),
            },
        ];
        let chunks = vec![ChunkRecord {
            path: String::from("src/a.rs"),
            chunk_id: String::from("chunk1"),
            start_line: 1,
            end_line: 10,
            tokens: BTreeSet::new(),
            import_refs: vec![String::from("src.b")],
        }];

        let links = infer_symbol_links(
            &defs_by_symbol,
            &symbols_by_file,
            &symbols_by_chunk,
            &references,
            &chunks,
        );
        let kinds: BTreeSet<String> = links.iter().map(|link| link.kind.clone()).collect();
        assert!(kinds.contains("calls"));
        assert!(kinds.contains("tests"));
        assert!(kinds.contains("imports"));
    }
}

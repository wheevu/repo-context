//! PR-oriented context synthesis.

use crate::domain::{Chunk, FileInfo};
use crate::rank::{dependency_graph, symbol_definitions};
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct PrContextReport {
    pub touch_points: Vec<TouchPoint>,
    pub entrypoints: Vec<EntrypointSurface>,
    pub invariants: Vec<Invariant>,
    pub graph_available: bool,
}

#[derive(Debug, Clone)]
pub struct TouchPoint {
    pub path: String,
    pub reason: String,
    pub chunk_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct EntrypointSurface {
    pub kind: &'static str,
    pub path: String,
    pub symbol: String,
    pub evidence: String,
}

#[derive(Debug, Clone)]
pub struct Invariant {
    pub kind: &'static str,
    pub path: String,
    pub symbol: String,
    pub chunk_id: String,
}

pub fn build_pr_context(
    files: &[FileInfo],
    chunks: &[Chunk],
    task_query: Option<&str>,
    graph_available: bool,
) -> PrContextReport {
    let mut touch_points = Vec::new();
    let mut entrypoints = Vec::new();
    let mut invariants = Vec::new();

    let mut ranked_chunks: Vec<&Chunk> = chunks.iter().collect();
    ranked_chunks.sort_by(|a, b| {
        b.priority
            .partial_cmp(&a.priority)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.path.cmp(&b.path))
    });
    let seeds: Vec<&Chunk> = ranked_chunks.into_iter().take(20).collect();
    let seed_files: HashSet<String> = seeds.iter().map(|c| c.path.clone()).collect();

    let known_files: HashSet<String> = chunks.iter().map(|c| c.path.clone()).collect();
    let defs = symbol_definitions(chunks);
    let graph = dependency_graph(chunks, &known_files, &defs);

    let mut touched: HashSet<String> = seed_files.clone();
    for seed in &seed_files {
        if let Some(neighbors) = graph.get(seed) {
            for neighbor in neighbors {
                touched.insert(neighbor.clone());
            }
        }
    }

    let mut by_path: HashMap<String, Vec<&Chunk>> = HashMap::new();
    for chunk in chunks {
        by_path.entry(chunk.path.clone()).or_default().push(chunk);
    }

    for path in touched {
        let reason = if seed_files.contains(&path) {
            "top-ranked task seed".to_string()
        } else {
            "1-hop module stitching".to_string()
        };
        let ids = by_path
            .get(&path)
            .map(|v| v.iter().take(3).map(|c| c.id.clone()).collect::<Vec<_>>())
            .unwrap_or_default();
        touch_points.push(TouchPoint { path, reason, chunk_ids: ids });
    }
    touch_points.sort_by(|a, b| a.path.cmp(&b.path));

    for file in files {
        if file.tags.contains("entrypoint") {
            entrypoints.push(EntrypointSurface {
                kind: "CLI",
                path: file.relative_path.clone(),
                symbol: file.relative_path.clone(),
                evidence: "entrypoint tag".to_string(),
            });
        }
        if file.tags.contains("config") {
            entrypoints.push(EntrypointSurface {
                kind: "Config",
                path: file.relative_path.clone(),
                symbol: file.relative_path.clone(),
                evidence: "config tag".to_string(),
            });
        }
    }

    for chunk in chunks {
        let lower = chunk.content.to_ascii_lowercase();
        if lower.contains("#[test]")
            || lower.contains("def test_")
            || lower.contains("func test")
            || chunk.path.contains("/tests/")
            || chunk.path.starts_with("tests/")
        {
            invariants.push(Invariant {
                kind: "Test",
                path: chunk.path.clone(),
                symbol: "test".to_string(),
                chunk_id: chunk.id.clone(),
            });
        }
        if lower.contains("assert!") || lower.contains("ensure!") || lower.contains("bail!") {
            invariants.push(Invariant {
                kind: "SafetyCheck",
                path: chunk.path.clone(),
                symbol: "assert/ensure/bail".to_string(),
                chunk_id: chunk.id.clone(),
            });
        }
        if lower.contains("derive(error)") || lower.contains("thiserror::error") {
            invariants.push(Invariant {
                kind: "ErrorType",
                path: chunk.path.clone(),
                symbol: "error type".to_string(),
                chunk_id: chunk.id.clone(),
            });
        }
        if lower.contains("cfg(feature") || lower.contains("feature =") {
            invariants.push(Invariant {
                kind: "FeatureFlag",
                path: chunk.path.clone(),
                symbol: "feature flag".to_string(),
                chunk_id: chunk.id.clone(),
            });
        }
    }

    // Keep output compact and deterministic.
    entrypoints.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.kind.cmp(b.kind)));
    entrypoints.dedup_by(|a, b| a.kind == b.kind && a.path == b.path);

    invariants.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.kind.cmp(b.kind)));
    invariants.truncate(30);

    if let Some(query) = task_query {
        let query_tokens: BTreeSet<String> = query
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|t| t.len() >= 2)
            .map(|t| t.to_ascii_lowercase())
            .collect();
        for chunk in chunks.iter().take(50) {
            for token in chunk
                .content
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .filter(|t| t.len() >= 3)
                .map(|t| t.to_ascii_lowercase())
            {
                if query_tokens.contains(&token) {
                    entrypoints.push(EntrypointSurface {
                        kind: "Task",
                        path: chunk.path.clone(),
                        symbol: token,
                        evidence: "query overlap".to_string(),
                    });
                    break;
                }
            }
        }
        entrypoints.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.kind.cmp(b.kind)));
        entrypoints.dedup_by(|a, b| a.kind == b.kind && a.path == b.path && a.symbol == b.symbol);
    }

    PrContextReport { touch_points, entrypoints, invariants, graph_available }
}

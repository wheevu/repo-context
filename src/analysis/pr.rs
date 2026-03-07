//! PR-oriented context synthesis.
//!
//! This module provides analysis capabilities for generating PR-focused context reports,
//! including touch points, entrypoints, invariants, feature flags, trait implementations,
//! and error flow signals.

use crate::domain::{Chunk, FileInfo};
use crate::rank::{dependency_graph, symbol_definitions};
use std::collections::{BTreeSet, HashMap, HashSet};

/// Comprehensive report for PR context analysis.
#[derive(Debug, Clone)]
pub struct PrContextReport {
    /// Points of contact where changes affect the codebase
    pub touch_points: Vec<TouchPoint>,
    /// Entry surfaces (CLI, API, config) that may be affected
    pub entrypoints: Vec<EntrypointSurface>,
    /// Invariants detected in the code (tests, safety checks, error types)
    pub invariants: Vec<Invariant>,
    /// Feature flag boundaries found in the code
    pub feature_flags: Vec<FeatureFlagBoundary>,
    /// Trait implementation edges
    pub trait_impls: Vec<TraitImplEdge>,
    /// Error handling flow signals
    pub error_flows: Vec<ErrorFlowSignal>,
    /// Whether a symbol graph was available for analysis
    pub graph_available: bool,
}

/// Represents a point where changes touch the codebase.
#[derive(Debug, Clone)]
pub struct TouchPoint {
    /// File path where the touch occurs
    pub path: String,
    /// Reason for the touch point classification
    pub reason: String,
    /// IDs of relevant chunks
    pub chunk_ids: Vec<String>,
}

/// Represents an entry surface in the codebase.
#[derive(Debug, Clone)]
pub struct EntrypointSurface {
    /// Type of entry (CLI, Config, API, etc.)
    pub kind: &'static str,
    /// File path of the entrypoint
    pub path: String,
    /// Symbol name
    pub symbol: String,
    /// Evidence for this classification
    pub evidence: String,
}

/// Represents a code invariant detected during analysis.
#[derive(Debug, Clone)]
pub struct Invariant {
    /// Type of invariant (Test, SafetyCheck, ErrorType, FeatureFlag)
    pub kind: &'static str,
    /// File path where the invariant is found
    pub path: String,
    /// Symbol or description of the invariant
    pub symbol: String,
    /// ID of the chunk containing the invariant
    pub chunk_id: String,
}

/// Represents a feature flag boundary.
#[derive(Debug, Clone)]
pub struct FeatureFlagBoundary {
    /// File path where the feature flag is used
    pub path: String,
    /// Name of the feature
    pub feature: String,
    /// ID of the chunk containing the feature flag
    pub chunk_id: String,
}

/// Represents a trait implementation relationship.
#[derive(Debug, Clone)]
pub struct TraitImplEdge {
    /// File path of the implementation
    pub path: String,
    /// Name of the trait being implemented
    pub trait_name: String,
    /// Target type receiving the implementation
    pub target_type: String,
    /// ID of the chunk containing the implementation
    pub chunk_id: String,
}

/// Represents an error flow signal in the code.
#[derive(Debug, Clone)]
pub struct ErrorFlowSignal {
    /// File path where the signal is found
    pub path: String,
    /// Evidence of error handling (e.g., "thiserror type", "anyhow context")
    pub evidence: String,
    /// ID of the chunk containing the signal
    pub chunk_id: String,
}

/// Builds a PR context report from files and chunks.
///
/// Analyzes the codebase to identify touch points, entrypoints, invariants,
/// feature flags, trait implementations, and error flows relevant to a PR.
///
/// # Arguments
/// * `files` - List of file information
/// * `chunks` - List of code chunks to analyze
/// * `task_query` - Optional task query for relevance scoring
/// * `graph_available` - Whether symbol graph data is available
///
/// # Returns
/// A comprehensive PR context report
pub fn build_pr_context(
    files: &[FileInfo],
    chunks: &[Chunk],
    task_query: Option<&str>,
    graph_available: bool,
) -> PrContextReport {
    let mut touch_points = Vec::new();
    let mut entrypoints = Vec::new();
    let mut invariants = Vec::new();
    let mut feature_flags = Vec::new();
    let mut trait_impls = Vec::new();
    let mut error_flows = Vec::new();

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
            for feature in extract_feature_names(&chunk.content) {
                feature_flags.push(FeatureFlagBoundary {
                    path: chunk.path.clone(),
                    feature,
                    chunk_id: chunk.id.clone(),
                });
            }
        }

        for (trait_name, target_type) in extract_trait_impls(&chunk.content) {
            trait_impls.push(TraitImplEdge {
                path: chunk.path.clone(),
                trait_name,
                target_type,
                chunk_id: chunk.id.clone(),
            });
        }

        for evidence in extract_error_flow_signals(&chunk.content) {
            error_flows.push(ErrorFlowSignal {
                path: chunk.path.clone(),
                evidence,
                chunk_id: chunk.id.clone(),
            });
        }
    }

    // Keep output compact and deterministic.
    entrypoints.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.kind.cmp(b.kind)));
    entrypoints.dedup_by(|a, b| a.kind == b.kind && a.path == b.path);

    invariants.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.kind.cmp(b.kind)));
    invariants.truncate(30);

    feature_flags.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.feature.cmp(&b.feature)));
    feature_flags.dedup_by(|a, b| a.path == b.path && a.feature == b.feature);
    feature_flags.truncate(30);

    trait_impls.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.trait_name.cmp(&b.trait_name))
            .then_with(|| a.target_type.cmp(&b.target_type))
    });
    trait_impls.dedup_by(|a, b| {
        a.path == b.path && a.trait_name == b.trait_name && a.target_type == b.target_type
    });
    trait_impls.truncate(30);

    error_flows.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.evidence.cmp(&b.evidence)));
    error_flows.dedup_by(|a, b| a.path == b.path && a.evidence == b.evidence);
    error_flows.truncate(30);

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

    PrContextReport {
        touch_points,
        entrypoints,
        invariants,
        feature_flags,
        trait_impls,
        error_flows,
        graph_available,
    }
}

/// Extracts feature names from cfg attribute lines.
fn extract_feature_names(content: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.contains("feature") {
            continue;
        }
        if let Some(start) = trimmed.find("feature = \"") {
            let tail = &trimmed[start + "feature = \"".len()..];
            if let Some(end) = tail.find('"') {
                let feature = tail[..end].trim();
                if !feature.is_empty() {
                    out.insert(feature.to_string());
                }
            }
        }
    }
    out.into_iter().collect()
}

/// Extracts trait implementations from impl blocks.
fn extract_trait_impls(content: &str) -> Vec<(String, String)> {
    let mut out = BTreeSet::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("impl ") || !trimmed.contains(" for ") {
            continue;
        }
        let rest = trimmed.trim_start_matches("impl ");
        let Some((trait_part, target_part)) = rest.split_once(" for ") else {
            continue;
        };
        let trait_name = trait_part.split('<').next().unwrap_or("").trim();
        let target =
            target_part.split('{').next().unwrap_or("").split('<').next().unwrap_or("").trim();
        if !trait_name.is_empty() && !target.is_empty() {
            out.insert((trait_name.to_string(), target.to_string()));
        }
    }
    out.into_iter().collect()
}

/// Extracts error flow signals from code content.
fn extract_error_flow_signals(content: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    let lower = content.to_ascii_lowercase();
    for (needle, label) in [
        ("thiserror::error", "thiserror type"),
        ("derive(error)", "derive(Error)"),
        ("anyhow::", "anyhow context"),
        ("-> result<", "result return"),
        ("map_err(", "map_err conversion"),
        ("?", "error propagation (?)"),
    ] {
        if lower.contains(needle) {
            out.insert(label.to_string());
        }
    }
    out.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::{extract_error_flow_signals, extract_feature_names, extract_trait_impls};

    #[test]
    fn extracts_feature_flags_from_cfg_lines() {
        let content = "#[cfg(feature = \"simd\")]\nfn run() {}\n";
        let features = extract_feature_names(content);
        assert_eq!(features, vec!["simd".to_string()]);
    }

    #[test]
    fn extracts_trait_impl_edges() {
        let content = "impl Display for ErrorKind { }\nimpl Serialize for Payload<T> {}\n";
        let impls = extract_trait_impls(content);
        assert!(impls.contains(&("Display".to_string(), "ErrorKind".to_string())));
        assert!(impls.contains(&("Serialize".to_string(), "Payload".to_string())));
    }

    #[test]
    fn extracts_error_flow_evidence() {
        let content = "#[derive(Error)]\nfn x() -> Result<()> { anyhow::bail!(\"x\") }";
        let signals = extract_error_flow_signals(content);
        assert!(signals.iter().any(|s| s == "derive(Error)"));
        assert!(signals.iter().any(|s| s == "result return"));
        assert!(signals.iter().any(|s| s == "anyhow context"));
    }
}

//! Deterministic, local task retrieval.
//!
//! Retrieval deliberately stays small: BM25 finds lexical seeds, then the
//! existing static import graph supplies a bounded amount of structural
//! context. There are no embeddings, network calls, or runtime claims here.

#![allow(missing_docs)]

use crate::domain::{Chunk, FileInfo};
use crate::module::graph::ImportGraph;
use crate::rank::bm25::{score_query_against_chunks, tokenize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

pub const STRATEGY: &str = "bm25_static_import_graph";
const MAX_SEEDS: usize = 64;
const MAX_SUPPORT_FILES: usize = 128;
const MAX_SUPPORT_CHUNKS: usize = 128;
const MAX_GRAPH_HOPS: usize = 2;
const DEPENDENCY_DECAY: f64 = 0.65;
const IMPORTER_DECAY: f64 = 0.55;
const TEST_DECAY: f64 = 0.45;
const ANCHOR_DECAY: f64 = 0.30;

#[derive(Debug, Clone)]
pub struct RetrievalEvidence {
    pub score: f64,
    pub depth: usize,
    pub reasons: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub struct RetrievalPlan {
    pub strategy: &'static str,
    pub task_sha256: String,
    pub candidate_files: usize,
    pub candidate_chunks: usize,
    pub seed_chunks: usize,
    pub selected_files: usize,
    pub selected_chunks: usize,
    pub relation_counts: BTreeMap<String, usize>,
    pub evidence_by_chunk: HashMap<String, RetrievalEvidence>,
    pub ordered_chunk_ids: Vec<String>,
}

impl RetrievalPlan {
    #[must_use]
    pub fn score_for(&self, chunk: &Chunk) -> Option<f64> {
        self.evidence_by_chunk.get(&chunk.id).map(|evidence| evidence.score)
    }

    #[must_use]
    pub fn evidence_for(&self, chunk: &Chunk) -> Option<&RetrievalEvidence> {
        self.evidence_by_chunk.get(&chunk.id)
    }

    #[must_use]
    pub fn report_value(&self) -> Value {
        json!({
            "strategy": self.strategy,
            "task_sha256": self.task_sha256,
            "candidate_files": self.candidate_files,
            "candidate_chunks": self.candidate_chunks,
            "seed_chunks": self.seed_chunks,
            "selected_files": self.selected_files,
            "selected_chunks": self.selected_chunks,
            "relation_counts": self.relation_counts,
        })
    }
}

/// Build a deterministic retrieval plan over already-redacted chunks.
#[must_use]
pub fn build_plan(
    task: &str,
    chunks: &[Chunk],
    files: &[FileInfo],
    graph: &ImportGraph,
) -> RetrievalPlan {
    let task_sha256 = hash_text(task);
    let query_terms = tokenize(task);
    let bm25_scores = score_query_against_chunks(chunks, task);
    let max_bm25 = bm25_scores.iter().copied().fold(0.0_f64, f64::max);

    let files_by_relative: HashMap<&str, &FileInfo> =
        files.iter().map(|file| (file.relative_path.as_str(), file)).collect();
    let absolute_by_relative: HashMap<&str, PathBuf> =
        files.iter().map(|file| (file.relative_path.as_str(), normalize_abs(&file.path))).collect();

    let mut evidence_by_chunk = HashMap::new();
    let mut seed_indexes = Vec::new();
    for (index, chunk) in chunks.iter().enumerate() {
        let bm25 = bm25_scores.get(index).copied().unwrap_or(0.0);
        let normalized_bm25 = if max_bm25 > 0.0 { bm25 / max_bm25 } else { 0.0 };
        let path_overlap = overlap_ratio(&query_terms, &tokenize(&chunk.path));
        let symbol_overlap = symbol_overlap(&query_terms, chunk);
        let exact_boost = path_overlap.max(symbol_overlap);
        let priority =
            if chunk.priority.is_finite() { chunk.priority.clamp(0.0, 1.0) } else { 0.0 };
        let score = 0.70 * normalized_bm25 + 0.20 * priority + 0.10 * exact_boost;

        if bm25 > 0.0 || path_overlap > 0.0 || symbol_overlap > 0.0 {
            let mut reasons = BTreeSet::new();
            if bm25 > 0.0 {
                reasons.insert("task_match".to_string());
            }
            if path_overlap > 0.0 {
                reasons.insert("exact_path_match".to_string());
            }
            if symbol_overlap > 0.0 {
                reasons.insert("symbol_definition".to_string());
            }
            evidence_by_chunk
                .insert(chunk.id.clone(), RetrievalEvidence { score, depth: 0, reasons });
            seed_indexes.push(index);
        }
    }

    seed_indexes.sort_by(|left, right| {
        compare_chunk_scores(
            chunks[*right].priority_score(&evidence_by_chunk),
            chunks[*left].priority_score(&evidence_by_chunk),
        )
        .then_with(|| chunks[*left].path.cmp(&chunks[*right].path))
        .then_with(|| chunks[*left].start_line.cmp(&chunks[*right].start_line))
        .then_with(|| chunks[*left].id.cmp(&chunks[*right].id))
    });
    seed_indexes.truncate(MAX_SEEDS);

    let mut support_files: Vec<(String, f64, usize, String)> = Vec::new();
    let mut relation_counts = BTreeMap::new();
    for seed_index in &seed_indexes {
        let seed = &chunks[*seed_index];
        let seed_score = evidence_by_chunk.get(&seed.id).map(|e| e.score).unwrap_or(0.0);
        let Some(seed_abs) = absolute_by_relative.get(seed.path.as_str()) else { continue };

        for (path, depth, reason, decay) in graph_neighbors(graph, seed_abs) {
            let relative = relative_path_for(&path, files_by_relative.values().copied());
            let Some(relative) = relative else { continue };
            let score = seed_score * decay.powi(depth as i32);
            support_files.push((relative, score, depth, reason.to_string()));
        }

        for file in files {
            if is_related_test(seed.path.as_str(), file.relative_path.as_str()) {
                support_files.push((
                    file.relative_path.clone(),
                    seed_score * TEST_DECAY,
                    1,
                    "related_test".to_string(),
                ));
            }
            if is_repository_anchor(file) {
                support_files.push((
                    file.relative_path.clone(),
                    seed_score * ANCHOR_DECAY,
                    1,
                    "repository_anchor".to_string(),
                ));
            }
        }
    }

    support_files.sort_by(|left, right| {
        compare_chunk_scores(right.1, left.1)
            .then_with(|| left.0.cmp(&right.0))
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.3.cmp(&right.3))
    });
    let mut selected_support = HashSet::new();
    let mut support_chunk_count = 0;
    for (relative, score, depth, reason) in support_files {
        if selected_support.len() >= MAX_SUPPORT_FILES && !selected_support.contains(&relative) {
            continue;
        }
        selected_support.insert(relative.clone());
        for chunk in chunks.iter().filter(|chunk| chunk.path == relative) {
            if support_chunk_count >= MAX_SUPPORT_CHUNKS {
                break;
            }
            let evidence = evidence_by_chunk.entry(chunk.id.clone()).or_insert_with(|| {
                RetrievalEvidence { score: 0.0, depth, reasons: BTreeSet::new() }
            });
            evidence.score = evidence.score.max(score);
            evidence.depth = evidence.depth.min(depth);
            evidence.reasons.insert(reason.clone());
            *relation_counts.entry(reason.clone()).or_insert(0) += 1;
            support_chunk_count += 1;
        }
    }

    // A task with no lexical hit still gets the normal ranked corpus. This is
    // deliberately a graceful fallback, not a claim that the task was
    // semantically understood.
    if seed_indexes.is_empty() {
        for chunk in chunks {
            let file_priority = files_by_relative
                .get(chunk.path.as_str())
                .map(|file| file.priority)
                .unwrap_or(chunk.priority);
            let file_priority =
                if file_priority.is_finite() { file_priority.clamp(0.0, 1.0) } else { 0.0 };
            let mut reasons = BTreeSet::new();
            if files_by_relative
                .get(chunk.path.as_str())
                .is_some_and(|file| is_repository_anchor(file))
            {
                reasons.insert("repository_anchor".to_string());
                *relation_counts.entry("repository_anchor".to_string()).or_insert(0) += 1;
            }
            evidence_by_chunk.insert(
                chunk.id.clone(),
                RetrievalEvidence { score: file_priority, depth: 0, reasons },
            );
        }
    }

    let mut ordered: Vec<&Chunk> = chunks.iter().collect();
    ordered.sort_by(|left, right| {
        let left_score = evidence_by_chunk.get(&left.id).map(|e| e.score).unwrap_or(-1.0);
        let right_score = evidence_by_chunk.get(&right.id).map(|e| e.score).unwrap_or(-1.0);
        compare_chunk_scores(right_score, left_score)
            .then_with(|| right.priority.partial_cmp(&left.priority).unwrap_or(Ordering::Equal))
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.start_line.cmp(&right.start_line))
            .then_with(|| left.id.cmp(&right.id))
    });

    let candidate_files = evidence_by_chunk
        .keys()
        .filter_map(|id| {
            chunks.iter().find(|chunk| &chunk.id == id).map(|chunk| chunk.path.as_str())
        })
        .collect::<HashSet<_>>()
        .len();

    RetrievalPlan {
        strategy: STRATEGY,
        task_sha256,
        candidate_files,
        candidate_chunks: evidence_by_chunk.len(),
        seed_chunks: seed_indexes.len(),
        selected_files: candidate_files,
        selected_chunks: evidence_by_chunk.len(),
        relation_counts,
        evidence_by_chunk,
        ordered_chunk_ids: ordered.into_iter().map(|chunk| chunk.id.clone()).collect(),
    }
}

pub fn hash_text(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn graph_neighbors(graph: &ImportGraph, start: &Path) -> Vec<(PathBuf, usize, &'static str, f64)> {
    let start = normalize_abs(start);
    let mut queue = VecDeque::from([(start.clone(), 0usize)]);
    let mut queued = HashSet::from([start]);
    let mut seen_relations: HashSet<(PathBuf, &'static str)> = HashSet::new();
    let mut out = Vec::new();

    while let Some((path, depth)) = queue.pop_front() {
        if depth >= MAX_GRAPH_HOPS {
            continue;
        }
        let next_depth = depth + 1;
        for (neighbor, reason, decay) in graph
            .edges
            .get(&path)
            .into_iter()
            .flat_map(|paths| {
                paths.iter().map(|path| (path, "static_dependency", DEPENDENCY_DECAY))
            })
            .chain(graph.reverse.get(&path).into_iter().flat_map(|paths| {
                paths.iter().map(|path| (path, "static_importer", IMPORTER_DECAY))
            }))
        {
            let neighbor = normalize_abs(neighbor);
            if !graph.files.contains_key(&neighbor) {
                continue;
            }
            if seen_relations.insert((neighbor.clone(), reason)) {
                out.push((neighbor.clone(), next_depth, reason, decay));
            }
            if queued.insert(neighbor.clone()) {
                queue.push_back((neighbor, next_depth));
            }
        }
    }
    out
}

fn relative_path_for<'a>(
    path: &Path,
    mut files: impl Iterator<Item = &'a FileInfo>,
) -> Option<String> {
    files.find(|file| normalize_abs(&file.path) == path).map(|file| file.relative_path.clone())
}

fn is_repository_anchor(file: &FileInfo) -> bool {
    file.is_readme || file.is_config || file.tags.contains("entrypoint")
}

fn is_related_test(seed: &str, candidate: &str) -> bool {
    let seed_is_test = is_test_path(seed);
    let candidate_is_test = is_test_path(candidate);
    if seed_is_test == candidate_is_test {
        return false;
    }
    let seed_stem = path_stem(seed);
    let candidate_stem = path_stem(candidate);
    !seed_stem.is_empty()
        && !candidate_stem.is_empty()
        && (seed_stem == candidate_stem
            || seed_stem.contains(&candidate_stem)
            || candidate_stem.contains(&seed_stem))
}

fn is_test_path(path: &str) -> bool {
    path.split('/')
        .any(|part| part.eq_ignore_ascii_case("test") || part.eq_ignore_ascii_case("tests"))
        || path.to_ascii_lowercase().contains("_test.")
        || path.to_ascii_lowercase().contains(".test.")
}

fn path_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.trim_end_matches("_test").trim_end_matches(".test").to_ascii_lowercase())
        .unwrap_or_default()
}

fn symbol_overlap(query_terms: &[String], chunk: &Chunk) -> f64 {
    let symbols: Vec<String> = chunk
        .tags
        .iter()
        .filter_map(|tag| {
            let (kind, value) = tag.split_once(':')?;
            matches!(kind, "def" | "type" | "impl" | "class" | "method" | "function")
                .then(|| value.to_string())
        })
        .flat_map(|value| tokenize(&value))
        .collect();
    overlap_ratio(query_terms, &symbols)
}

fn overlap_ratio(left: &[String], right: &[String]) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let right: HashSet<&str> = right.iter().map(String::as_str).collect();
    let hits = left.iter().filter(|term| right.contains(term.as_str())).count();
    hits as f64 / left.len() as f64
}

fn compare_chunk_scores(left: f64, right: f64) -> Ordering {
    left.partial_cmp(&right).unwrap_or(Ordering::Equal)
}

fn normalize_abs(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

trait PriorityScore {
    fn priority_score(&self, evidence: &HashMap<String, RetrievalEvidence>) -> f64;
}

impl PriorityScore for Chunk {
    fn priority_score(&self, evidence: &HashMap<String, RetrievalEvidence>) -> f64 {
        evidence.get(&self.id).map(|item| item.score).unwrap_or(-1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{build_plan, hash_text};
    use crate::domain::{Chunk, FileInfo};
    use crate::module::graph;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn file(path: &str, tags: &[&str], priority: f64) -> FileInfo {
        FileInfo {
            path: PathBuf::from(path),
            relative_path: path.to_string(),
            size_bytes: 10,
            extension: ".rs".to_string(),
            language: "rust".to_string(),
            id: path.to_string(),
            priority,
            token_estimate: 10,
            tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
            is_readme: tags.contains(&"readme"),
            is_config: tags.contains(&"config"),
            is_doc: false,
        }
    }

    fn chunk(id: &str, path: &str, content: &str, tags: &[&str], priority: f64) -> Chunk {
        Chunk {
            id: id.to_string(),
            path: path.to_string(),
            language: "rust".to_string(),
            start_line: 1,
            end_line: 2,
            content: content.to_string(),
            priority,
            tags: tags.iter().map(|tag| (*tag).to_string()).collect::<BTreeSet<_>>(),
            token_estimate: 5,
            file_id: id.to_string(),
            chunk_index: 0,
            chunks_in_file: 1,
            byte_start: None,
            byte_end: None,
            content_sha256: String::new(),
            file_sha256: String::new(),
        }
    }

    #[test]
    fn task_hash_is_stable() {
        assert_eq!(hash_text("refresh oauth"), hash_text("refresh oauth"));
        assert_ne!(hash_text("refresh oauth"), hash_text("delete oauth"));
    }

    #[test]
    fn task_match_and_symbol_definition_are_explainable() {
        let files =
            vec![file("src/auth.rs", &["entrypoint"], 0.8), file("README.md", &["readme"], 1.0)];
        let chunks = vec![
            chunk("auth", "src/auth.rs", "fn refresh_token() {}", &["def:refresh_token"], 0.8),
            chunk("readme", "README.md", "Authentication overview", &[], 1.0),
        ];
        let plan = build_plan("refresh_token", &chunks, &files, &graph::build(&files));
        let evidence = plan.evidence_for(&chunks[0]).expect("seed evidence");
        assert!(evidence.reasons.contains("task_match"));
        assert!(evidence.reasons.contains("symbol_definition"));
        assert!(plan.evidence_for(&chunks[1]).is_some());
    }

    #[test]
    fn empty_match_falls_back_to_ranked_corpus() {
        let files = vec![file("src/lib.rs", &[], 0.8), file("README.md", &["readme"], 1.0)];
        let chunks = vec![
            chunk("lib", "src/lib.rs", "fn add() {}", &[], 0.8),
            chunk("readme", "README.md", "Overview", &[], 1.0),
        ];
        let plan = build_plan("something absent", &chunks, &files, &graph::build(&files));
        assert_eq!(plan.seed_chunks, 0);
        assert_eq!(plan.candidate_chunks, 2);
        assert_eq!(plan.ordered_chunk_ids[0], "readme");
    }
}

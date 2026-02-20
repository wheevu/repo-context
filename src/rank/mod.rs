//! File ranking by importance

use crate::domain::{Chunk, FileInfo, RankingWeights};
use crate::graph::lazy_loader::LazyChunkLoader;
use anyhow::Result;
use serde_json::Value as JsonValue;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

pub mod bm25;
pub mod ranker;

pub use ranker::FileRanker;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StitchTier {
    Definition,
    Callee,
    Caller,
    CrossCrate,
}

impl StitchTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Definition => "definition",
            Self::Callee => "callee",
            Self::Caller => "caller",
            Self::CrossCrate => "cross-crate",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Definition => 0,
            Self::Callee => 1,
            Self::Caller => 2,
            Self::CrossCrate => 3,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct StitchedBundle {
    pub seed_ids: BTreeSet<String>,
    pub stitched: HashMap<String, StitchTier>,
    pub tokens_used: usize,
    pub lazy_chunks: Vec<Chunk>,
}

pub fn rerank_chunks_by_task(
    chunks: &mut [Chunk],
    query: &str,
    relevance_weight: f64,
) -> HashMap<String, f64> {
    let weight = relevance_weight.clamp(0.0, 1.0);
    let lexical_scores = bm25::score_query_against_chunks(chunks, query);

    let mut max_score = 0.0_f64;
    for score in &lexical_scores {
        max_score = max_score.max(*score);
    }

    let mut file_scores: HashMap<String, f64> = HashMap::new();
    let mut lexical_by_file: HashMap<String, f64> = HashMap::new();

    for (chunk, lexical) in chunks.iter_mut().zip(lexical_scores.into_iter()) {
        let normalized = if max_score > 0.0 { lexical / max_score } else { 0.0 };
        let blended = (chunk.priority * (1.0 - weight)) + (normalized * weight);
        chunk.priority = (blended * 1000.0).round() / 1000.0;
        file_scores
            .entry(chunk.path.clone())
            .and_modify(|existing| *existing = existing.max(chunk.priority))
            .or_insert(chunk.priority);
        lexical_by_file
            .entry(chunk.path.clone())
            .and_modify(|existing| *existing = existing.max(normalized))
            .or_insert(normalized);
    }

    let expansion = dependency_expansion_scores(chunks, &lexical_by_file);
    for chunk in chunks.iter_mut() {
        if let Some(expanded) = expansion.get(&chunk.path) {
            let boosted = (chunk.priority * 0.8) + (expanded * 0.2);
            chunk.priority = (boosted * 1000.0).round() / 1000.0;
        }
    }

    let symbol_expansion = symbol_expansion_scores(chunks, query);
    for chunk in chunks.iter_mut() {
        if let Some(expanded) = symbol_expansion.get(&chunk.path) {
            let boosted = (chunk.priority * 0.7) + (expanded * 0.3);
            chunk.priority = (boosted * 1000.0).round() / 1000.0;
        }
    }

    file_scores.clear();
    for chunk in chunks.iter() {
        file_scores
            .entry(chunk.path.clone())
            .and_modify(|existing| *existing = existing.max(chunk.priority))
            .or_insert(chunk.priority);
    }

    file_scores
}

pub fn stitch_thread_bundles(
    chunks: &[Chunk],
    top_n_seeds: usize,
    stitch_budget_tokens: usize,
    loader: Option<&LazyChunkLoader>,
    workspace_members: &[String],
) -> StitchedBundle {
    if chunks.is_empty() || stitch_budget_tokens == 0 {
        return StitchedBundle::default();
    }

    let mut ranked: Vec<&Chunk> = chunks.iter().collect();
    ranked.sort_by(|a, b| {
        b.priority
            .partial_cmp(&a.priority)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.start_line.cmp(&b.start_line))
            .then_with(|| a.id.cmp(&b.id))
    });

    let seeds: Vec<&Chunk> = ranked.into_iter().take(top_n_seeds.max(1)).collect();
    let seed_ids: BTreeSet<String> = seeds.iter().map(|c| c.id.clone()).collect();
    let seed_files: HashSet<String> = seeds.iter().map(|c| c.path.clone()).collect();

    let known_files: HashSet<String> = chunks.iter().map(|c| c.path.clone()).collect();
    let symbol_defs = symbol_definitions(chunks);
    let graph = dependency_graph(chunks, &known_files, &symbol_defs);

    let mut symbol_chunk_ids: HashMap<String, BTreeSet<String>> = HashMap::new();
    for chunk in chunks {
        for tag in &chunk.tags {
            if let Some((kind, name)) = tag.split_once(':') {
                if matches!(kind, "def" | "type" | "impl") {
                    symbol_chunk_ids
                        .entry(name.to_ascii_lowercase())
                        .or_default()
                        .insert(chunk.id.clone());
                }
            }
        }
    }

    let mut definition_ids: BTreeSet<String> = BTreeSet::new();
    let mut seed_tag_symbols: HashSet<String> = HashSet::new();
    for seed in &seeds {
        for tag in &seed.tags {
            if let Some((kind, name)) = tag.split_once(':') {
                if matches!(kind, "def" | "type" | "impl") {
                    seed_tag_symbols.insert(name.to_ascii_lowercase());
                }
            }
        }
    }

    for symbol in &seed_tag_symbols {
        if let Some(ids) = symbol_chunk_ids.get(symbol) {
            for id in ids {
                if !seed_ids.contains(id) {
                    definition_ids.insert(id.clone());
                }
            }
        }
    }

    let mut token_hits: HashMap<String, usize> = HashMap::new();
    for seed in &seeds {
        for token in extract_signal_tokens(&seed.content) {
            *token_hits.entry(token).or_insert(0) += 1;
        }
    }
    for (token, count) in token_hits {
        if count < 2 {
            continue;
        }
        if let Some(ids) = symbol_chunk_ids.get(&token) {
            for id in ids {
                if !seed_ids.contains(id) {
                    definition_ids.insert(id.clone());
                }
            }
        }
    }

    let mut callee_files = BTreeSet::new();
    let mut caller_files = BTreeSet::new();
    for seed in &seed_files {
        if let Some(neighbors) = graph.get(seed) {
            for target in neighbors {
                if !seed_files.contains(target) {
                    callee_files.insert(target.clone());
                }
            }
        }
    }
    for (source, neighbors) in &graph {
        for neighbor in neighbors {
            if seed_files.contains(neighbor) && !seed_files.contains(source) {
                caller_files.insert(source.clone());
            }
        }
    }

    let mut chunks_by_file: HashMap<&str, Vec<&Chunk>> = HashMap::new();
    for chunk in chunks {
        chunks_by_file.entry(chunk.path.as_str()).or_default().push(chunk);
    }

    let mut candidates: Vec<(Chunk, StitchTier)> = Vec::new();
    let seed_member_roots: HashSet<String> = seed_files
        .iter()
        .filter_map(|path| {
            workspace_member_for_path(path, workspace_members).map(|s| s.to_string())
        })
        .collect();
    for id in &definition_ids {
        if let Some(chunk) = chunks.iter().find(|c| c.id == *id) {
            candidates.push((chunk.clone(), StitchTier::Definition));
        }
    }
    for file in &callee_files {
        if let Some(file_chunks) = chunks_by_file.get(file.as_str()) {
            for chunk in file_chunks {
                if !seed_ids.contains(&chunk.id) && !definition_ids.contains(&chunk.id) {
                    let tier = if is_cross_crate_candidate(
                        &chunk.path,
                        &seed_member_roots,
                        workspace_members,
                    ) {
                        StitchTier::CrossCrate
                    } else {
                        StitchTier::Callee
                    };
                    candidates.push(((*chunk).clone(), tier));
                }
            }
        }
    }
    for file in &caller_files {
        if let Some(file_chunks) = chunks_by_file.get(file.as_str()) {
            for chunk in file_chunks {
                if !seed_ids.contains(&chunk.id)
                    && !definition_ids.contains(&chunk.id)
                    && !callee_files.contains(file)
                {
                    let tier = if is_cross_crate_candidate(
                        &chunk.path,
                        &seed_member_roots,
                        workspace_members,
                    ) {
                        StitchTier::CrossCrate
                    } else {
                        StitchTier::Caller
                    };
                    candidates.push(((*chunk).clone(), tier));
                }
            }
        }
    }

    if let Some(loader) = loader {
        let mut seen_ids: HashSet<String> = chunks.iter().map(|c| c.id.clone()).collect();

        for file in &callee_files {
            if chunks_by_file.contains_key(file.as_str()) || !loader.has_file(file) {
                continue;
            }
            for mut chunk in loader.load_chunks_for_file(file) {
                if seen_ids.contains(&chunk.id)
                    || seed_ids.contains(&chunk.id)
                    || definition_ids.contains(&chunk.id)
                {
                    continue;
                }
                chunk.tags.insert("stitch:lazy".to_string());
                seen_ids.insert(chunk.id.clone());
                let tier =
                    if is_cross_crate_candidate(&chunk.path, &seed_member_roots, workspace_members)
                    {
                        StitchTier::CrossCrate
                    } else {
                        StitchTier::Callee
                    };
                candidates.push((chunk, tier));
            }
        }
        for file in &caller_files {
            if chunks_by_file.contains_key(file.as_str()) || !loader.has_file(file) {
                continue;
            }
            if callee_files.contains(file) {
                continue;
            }
            for mut chunk in loader.load_chunks_for_file(file) {
                if seen_ids.contains(&chunk.id)
                    || seed_ids.contains(&chunk.id)
                    || definition_ids.contains(&chunk.id)
                {
                    continue;
                }
                chunk.tags.insert("stitch:lazy".to_string());
                seen_ids.insert(chunk.id.clone());
                let tier =
                    if is_cross_crate_candidate(&chunk.path, &seed_member_roots, workspace_members)
                    {
                        StitchTier::CrossCrate
                    } else {
                        StitchTier::Caller
                    };
                candidates.push((chunk, tier));
            }
        }
    }

    candidates.sort_by(|(a, tier_a), (b, tier_b)| {
        tier_a
            .rank()
            .cmp(&tier_b.rank())
            .then_with(|| b.priority.partial_cmp(&a.priority).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.start_line.cmp(&b.start_line))
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut stitched = HashMap::new();
    let mut lazy_chunks = Vec::new();
    let mut tokens_used = 0usize;
    for (chunk, tier) in candidates {
        if stitched.contains_key(&chunk.id) {
            continue;
        }
        if tokens_used + chunk.token_estimate > stitch_budget_tokens {
            continue;
        }
        stitched.insert(chunk.id.clone(), tier);
        if chunk.tags.contains("stitch:lazy") {
            lazy_chunks.push(chunk.clone());
        }
        tokens_used += chunk.token_estimate;
    }

    StitchedBundle { seed_ids, stitched, tokens_used, lazy_chunks }
}

fn dependency_expansion_scores(
    chunks: &[Chunk],
    lexical_by_file: &HashMap<String, f64>,
) -> HashMap<String, f64> {
    let known_files: HashSet<String> = chunks.iter().map(|c| c.path.clone()).collect();
    if known_files.is_empty() {
        return HashMap::new();
    }

    let symbol_defs = symbol_definitions(chunks);
    let graph = dependency_graph(chunks, &known_files, &symbol_defs);

    let mut seeds: Vec<(&String, &f64)> =
        lexical_by_file.iter().filter(|(_, s)| **s > 0.0).collect();
    seeds.sort_by(|a, b| {
        b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(b.0))
    });

    let mut expanded: HashMap<String, f64> = HashMap::new();
    for (seed, score) in seeds.into_iter().take(5) {
        expanded.entry(seed.clone()).and_modify(|v| *v = v.max(*score)).or_insert(*score);

        if let Some(neighbors) = graph.get(seed) {
            for neighbor in neighbors {
                let value = (score * 0.6).min(1.0);
                expanded.entry(neighbor.clone()).and_modify(|v| *v = v.max(value)).or_insert(value);

                if let Some(level2) = graph.get(neighbor) {
                    for neighbor2 in level2 {
                        let value2 = (score * 0.3).min(1.0);
                        expanded
                            .entry(neighbor2.clone())
                            .and_modify(|v| *v = v.max(value2))
                            .or_insert(value2);
                    }
                }
            }
        }
    }

    expanded
}

fn symbol_expansion_scores(chunks: &[Chunk], query: &str) -> HashMap<String, f64> {
    let symbol_defs = symbol_definitions(chunks);
    if symbol_defs.is_empty() {
        return HashMap::new();
    }

    let query_tokens: HashSet<String> = tokenize(query).into_iter().collect();
    if query_tokens.is_empty() {
        return HashMap::new();
    }

    let mut hits: HashSet<String> = HashSet::new();
    for symbol in symbol_defs.keys() {
        if query_matches_symbol(&query_tokens, symbol) {
            hits.insert(symbol.clone());
        }
    }

    if hits.is_empty() {
        return HashMap::new();
    }

    let mut header_by_file: HashMap<&str, usize> = HashMap::new();
    for chunk in chunks {
        header_by_file
            .entry(chunk.path.as_str())
            .and_modify(|line| *line = (*line).min(chunk.start_line))
            .or_insert(chunk.start_line);
    }

    let mut scores: HashMap<String, f64> = HashMap::new();
    for symbol in hits {
        let mut def_files: HashSet<&str> = HashSet::new();
        if let Some(files) = symbol_defs.get(&symbol) {
            for file in files {
                def_files.insert(file.as_str());
                scores.entry(file.clone()).and_modify(|s| *s = s.max(1.0)).or_insert(1.0);
            }
        }

        for chunk in chunks {
            let path = chunk.path.as_str();
            let token_set: HashSet<String> = tokenize(&chunk.content).into_iter().collect();
            if !token_set.contains(&symbol) {
                continue;
            }

            if def_files.contains(path) {
                let header_line = header_by_file.get(path).copied().unwrap_or(chunk.start_line);
                if chunk.start_line == header_line {
                    scores.entry(chunk.path.clone()).and_modify(|s| *s = s.max(1.0)).or_insert(1.0);
                }
                if chunk.tags.iter().any(|t| t.starts_with("type:") || t.starts_with("impl:")) {
                    scores
                        .entry(chunk.path.clone())
                        .and_modify(|s| *s = s.max(0.95))
                        .or_insert(0.95);
                }
                continue;
            }

            let is_test = is_test_like_file(path) || chunk.tags.iter().any(|t| t == "test");
            let caller_score = if is_test { 0.9 } else { 0.8 };
            scores
                .entry(chunk.path.clone())
                .and_modify(|s| *s = s.max(caller_score))
                .or_insert(caller_score);
        }
    }

    scores
}

fn query_matches_symbol(query_tokens: &HashSet<String>, symbol: &str) -> bool {
    if query_tokens.contains(symbol) {
        return true;
    }

    let parts: Vec<String> = symbol
        .split(['_', ':'])
        .map(|part| part.trim().to_ascii_lowercase())
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        return false;
    }

    parts.iter().all(|part| query_tokens.contains(part))
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

pub(crate) fn symbol_definitions(chunks: &[Chunk]) -> HashMap<String, HashSet<String>> {
    let mut defs: HashMap<String, HashSet<String>> = HashMap::new();
    for chunk in chunks {
        for tag in &chunk.tags {
            if let Some((kind, name)) = tag.split_once(':') {
                if matches!(kind, "def" | "type" | "impl") && !name.is_empty() {
                    defs.entry(name.to_ascii_lowercase()).or_default().insert(chunk.path.clone());
                }
            }
        }
    }
    defs
}

pub(crate) fn dependency_graph(
    chunks: &[Chunk],
    known_files: &HashSet<String>,
    symbol_defs: &HashMap<String, HashSet<String>>,
) -> HashMap<String, BTreeSet<String>> {
    let mut graph: HashMap<String, BTreeSet<String>> = HashMap::new();

    for chunk in chunks {
        for reference in extract_import_references(&chunk.content) {
            for target in resolve_reference(&reference, &chunk.path, known_files) {
                if target != chunk.path {
                    graph.entry(chunk.path.clone()).or_default().insert(target.clone());
                    graph.entry(target).or_default().insert(chunk.path.clone());
                }
            }
        }

        for token in tokenize(&chunk.content) {
            if let Some(def_files) = symbol_defs.get(&token) {
                for target in def_files {
                    if target != &chunk.path {
                        graph.entry(chunk.path.clone()).or_default().insert(target.clone());
                        graph.entry(target.clone()).or_default().insert(chunk.path.clone());
                    }
                }
            }
        }
    }

    graph
}

pub(crate) fn extract_import_references(content: &str) -> Vec<String> {
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
                if let Some(m) = module.split_whitespace().next() {
                    refs.push(m.trim_matches('"').trim_matches('\'').to_string());
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

pub(crate) fn resolve_reference(
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

    let lower_known: HashMap<String, String> =
        known_files.iter().map(|f| (f.to_ascii_lowercase(), f.clone())).collect();

    for candidate in candidates {
        let c = candidate.to_ascii_lowercase();
        if let Some(exact) = lower_known.get(&c) {
            out.insert(exact.clone());
        }
        for (lf, original) in &lower_known {
            if lf.ends_with(&format!("/{c}"))
                || lf.ends_with(&format!("/{c}.py"))
                || lf.ends_with(&format!("/{c}.rs"))
                || lf.ends_with(&format!("/{c}.ts"))
                || lf.ends_with(&format!("/{c}.js"))
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

fn extract_signal_tokens(content: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('#') {
            continue;
        }

        if trimmed.starts_with("use ")
            || trimmed.starts_with("impl ")
            || trimmed.starts_with("trait ")
            || trimmed.contains("::")
        {
            tokens.extend(
                trimmed
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .map(|t| t.to_ascii_lowercase())
                    .filter(|t| t.len() >= 3 && !ignored_symbols().contains(&t.as_str())),
            );
        }

        if let Some((prefix, _)) = trimmed.split_once('(') {
            let name = prefix
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .next_back()
                .unwrap_or("")
                .to_ascii_lowercase();
            if name.len() >= 3 && !ignored_symbols().contains(&name.as_str()) {
                tokens.push(name);
            }
        }
    }
    tokens
}

fn ignored_symbols() -> &'static [&'static str] {
    &[
        "result", "ok", "err", "vec", "string", "option", "none", "some", "box", "arc", "mutex",
        "hashmap", "hashset", "self", "super", "crate", "true", "false",
    ]
}

fn workspace_member_for_path<'a>(path: &str, workspace_members: &'a [String]) -> Option<&'a str> {
    workspace_members
        .iter()
        .map(String::as_str)
        .filter(|root| path == *root || path.starts_with(&format!("{root}/")))
        .max_by_key(|root| root.len())
}

fn is_cross_crate_candidate(
    path: &str,
    seed_member_roots: &HashSet<String>,
    workspace_members: &[String],
) -> bool {
    let Some(member) = workspace_member_for_path(path, workspace_members) else {
        return false;
    };
    !seed_member_roots.is_empty() && !seed_member_roots.contains(member)
}

pub fn rank_files(root_path: &Path, files: Vec<FileInfo>) -> Result<Vec<FileInfo>> {
    rank_files_with_weights(root_path, files, RankingWeights::default())
}

pub fn rank_files_with_weights(
    root_path: &Path,
    mut files: Vec<FileInfo>,
    weights: RankingWeights,
) -> Result<Vec<FileInfo>> {
    let scanned_files: HashSet<String> = files.iter().map(|f| f.relative_path.clone()).collect();
    let ranker = FileRanker::with_weights(root_path, scanned_files, weights);
    ranker.rank_files(&mut files);
    Ok(files)
}

/// Same as `rank_files_with_weights` but also returns manifest info extracted during ranking.
/// The manifest info includes `scripts`, `name`, `description` from `package.json` etc.
pub fn rank_files_with_manifest(
    root_path: &Path,
    mut files: Vec<FileInfo>,
    weights: RankingWeights,
) -> Result<(Vec<FileInfo>, HashMap<String, JsonValue>)> {
    let scanned_files: HashSet<String> = files.iter().map(|f| f.relative_path.clone()).collect();
    let ranker = FileRanker::with_weights(root_path, scanned_files, weights);
    ranker.rank_files(&mut files);
    let manifest = ranker.get_manifest_info().clone();
    Ok((files, manifest))
}

#[cfg(test)]
mod tests {
    use super::rerank_chunks_by_task;
    use crate::domain::Chunk;
    use std::collections::BTreeSet;

    #[test]
    fn reranking_expands_to_related_files() {
        let mut chunks = vec![
            Chunk {
                id: "1".to_string(),
                path: "src/auth.py".to_string(),
                language: "python".to_string(),
                start_line: 1,
                end_line: 20,
                content: "def refresh_token():\n    return True\n".to_string(),
                priority: 0.5,
                tags: BTreeSet::from(["def:refresh_token".to_string()]),
                token_estimate: 10,
            },
            Chunk {
                id: "2".to_string(),
                path: "tests/test_auth.py".to_string(),
                language: "python".to_string(),
                start_line: 1,
                end_line: 20,
                content: "from src.auth import login\n\ndef test_login():\n    assert login()\n"
                    .to_string(),
                priority: 0.2,
                tags: BTreeSet::new(),
                token_estimate: 16,
            },
        ];

        let scores = rerank_chunks_by_task(&mut chunks, "refresh token bug", 0.4);
        assert!(scores.contains_key("src/auth.py"));
        assert!(scores.contains_key("tests/test_auth.py"));
        assert!(scores["tests/test_auth.py"] >= 0.12);
    }

    #[test]
    fn reranking_expands_symbol_callers_and_tests() {
        let mut chunks = vec![
            Chunk {
                id: "1".to_string(),
                path: "src/auth.py".to_string(),
                language: "python".to_string(),
                start_line: 1,
                end_line: 20,
                content: "class SessionManager:\n    pass\n\ndef refresh_token(user):\n    return user\n"
                    .to_string(),
                priority: 0.55,
                tags: BTreeSet::from([
                    "type:SessionManager".to_string(),
                    "def:refresh_token".to_string(),
                ]),
                token_estimate: 16,
            },
            Chunk {
                id: "2".to_string(),
                path: "src/handler.py".to_string(),
                language: "python".to_string(),
                start_line: 1,
                end_line: 12,
                content: "from src.auth import refresh_token\n\ndef handle():\n    return refresh_token('u')\n"
                    .to_string(),
                priority: 0.2,
                tags: BTreeSet::new(),
                token_estimate: 12,
            },
            Chunk {
                id: "3".to_string(),
                path: "tests/test_auth.py".to_string(),
                language: "python".to_string(),
                start_line: 1,
                end_line: 12,
                content: "from src.auth import refresh_token\n\ndef test_refresh_token():\n    assert refresh_token('u')\n"
                    .to_string(),
                priority: 0.1,
                tags: BTreeSet::new(),
                token_estimate: 12,
            },
        ];

        let scores = rerank_chunks_by_task(&mut chunks, "fix refresh token bug", 0.4);
        assert!(scores.contains_key("src/auth.py"));
        assert!(scores.contains_key("src/handler.py"));
        assert!(scores.contains_key("tests/test_auth.py"));
        assert!(scores["src/handler.py"] > 0.2);
        assert!(scores["tests/test_auth.py"] > 0.1);
    }
}

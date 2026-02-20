//! Guardrail rendering for context pack outputs.

use crate::domain::{Chunk, ScanStats};
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub struct ClaimEntry {
    pub symbol: String,
    pub kind: String,
    pub file: String,
    pub chunk_id: String,
}

pub struct MissingPiece {
    pub kind: String,
    pub description: String,
    pub heuristic: String,
    pub chunk_ids: Vec<String>,
}

pub fn build_claims(chunks: &[Chunk]) -> Vec<ClaimEntry> {
    let mut claims = Vec::new();
    for chunk in chunks {
        for tag in &chunk.tags {
            if let Some((kind, symbol)) = tag.split_once(':') {
                if !matches!(kind, "def" | "type" | "impl") {
                    continue;
                }
                claims.push(ClaimEntry {
                    symbol: symbol.to_string(),
                    kind: kind.to_string(),
                    file: chunk.path.clone(),
                    chunk_id: chunk.id.clone(),
                });
            }
        }
    }
    claims.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.symbol.cmp(&b.symbol)));
    claims
}

pub fn build_missing_pieces(chunks: &[Chunk], stats: &ScanStats) -> Vec<MissingPiece> {
    let mut missing = Vec::new();

    let mut known_symbols = BTreeSet::new();
    for chunk in chunks {
        for tag in &chunk.tags {
            if let Some((kind, symbol)) = tag.split_once(':') {
                if matches!(kind, "def" | "type" | "impl") {
                    known_symbols.insert(symbol.to_ascii_lowercase());
                }
            }
        }
    }

    let ignored = ignored_symbol_set();
    let mut unresolved: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut dynamic: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for chunk in chunks {
        let content = chunk.content.as_str();
        for token in extract_signal_tokens(content) {
            let lower = token.to_ascii_lowercase();
            if lower.len() < 3 || ignored.contains(lower.as_str()) || known_symbols.contains(&lower)
            {
                continue;
            }
            unresolved.entry(token).or_default().insert(chunk.id.clone());
        }

        for marker in extract_dynamic_dispatch_markers(content) {
            dynamic.entry(marker).or_default().insert(chunk.id.clone());
        }
    }

    for (symbol, chunk_ids) in unresolved {
        if chunk_ids.len() < 3 {
            continue;
        }
        let ids: Vec<String> = chunk_ids.into_iter().collect();
        missing.push(MissingPiece {
            kind: "UnresolvedSymbol".to_string(),
            description: symbol,
            heuristic: format!("referenced_in_{}_chunks_no_def_found", ids.len()),
            chunk_ids: ids,
        });
    }

    for (marker, chunk_ids) in dynamic {
        missing.push(MissingPiece {
            kind: "DynamicDispatch".to_string(),
            description: marker,
            heuristic: "dynamic_dispatch_pattern".to_string(),
            chunk_ids: chunk_ids.into_iter().collect(),
        });
    }

    for dropped in &stats.dropped_files {
        if let Some(path) = dropped.get("path").and_then(|v| v.as_str()) {
            missing.push(MissingPiece {
                kind: "TruncatedFile".to_string(),
                description: path.to_string(),
                heuristic: "budget_dropped".to_string(),
                chunk_ids: Vec::new(),
            });
        }
    }

    missing.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.description.cmp(&b.description)));
    missing
}

pub fn render_guardrails(claims: &[ClaimEntry], missing: &[MissingPiece]) -> String {
    let mut out = String::new();
    if !claims.is_empty() {
        out.push_str("\n---\n\n## 🔍 Claims Index\n\n");
        out.push_str("| Symbol | Kind | File | Chunk ID |\n");
        out.push_str("|--------|------|------|----------|\n");
        for claim in claims.iter().take(200) {
            out.push_str(&format!(
                "| `{}` | {} | `{}` | `{}` |\n",
                claim.symbol, claim.kind, claim.file, claim.chunk_id
            ));
        }
    }

    if !missing.is_empty() {
        out.push_str("\n---\n\n## ⚠️ Missing Pieces\n\n");
        for piece in missing.iter().take(100) {
            let refs = if piece.chunk_ids.is_empty() {
                "none".to_string()
            } else {
                piece.chunk_ids.iter().take(4).cloned().collect::<Vec<_>>().join(", ")
            };
            out.push_str(&format!(
                "- **{}** `{}` — heuristic: `{}`; chunk_ids: {}\n",
                piece.kind, piece.description, piece.heuristic, refs
            ));
        }
    }
    out
}

fn ignored_symbol_set() -> HashSet<&'static str> {
    [
        "result", "ok", "err", "vec", "string", "option", "none", "some", "box", "arc", "mutex",
        "hashmap", "hashset", "btree", "self", "super", "crate", "true", "false",
    ]
    .into_iter()
    .collect()
}

fn extract_signal_tokens(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with("use ") || trimmed.contains("::") || trimmed.contains("impl ") {
            out.extend(
                trimmed
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .filter(|t| t.len() >= 3)
                    .map(|t| t.to_string()),
            );
            continue;
        }
        if let Some((name, _)) = trimmed.split_once('(') {
            let token = name
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .next_back()
                .unwrap_or("")
                .trim();
            if token.len() >= 3 {
                out.push(token.to_string());
            }
        }
    }
    out
}

fn extract_dynamic_dispatch_markers(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.contains("Box<dyn ") || trimmed.contains("Arc<dyn ") || trimmed.contains(" dyn ")
        {
            out.push(trimmed.to_string());
        }
        if trimmed.contains("interface{}") {
            out.push(trimmed.to_string());
        }
    }
    out
}

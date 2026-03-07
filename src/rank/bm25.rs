//! Lightweight BM25 scoring for task-driven chunk reranking.

use crate::domain::Chunk;
use std::collections::{HashMap, HashSet};

const K1: f64 = 1.5;
const B: f64 = 0.75;

/// Scores chunks against a query using BM25 algorithm.
///
/// BM25 is a probabilistic retrieval function that ranks chunks based on
/// query term frequency and document length normalization.
///
/// # Arguments
/// * `chunks` - Chunks to score
/// * `query` - Query string
///
/// # Returns
/// Vector of scores (higher = more relevant)
pub fn score_query_against_chunks(chunks: &[Chunk], query: &str) -> Vec<f64> {
    if chunks.is_empty() {
        return Vec::new();
    }

    let query_terms = tokenize(query);
    if query_terms.is_empty() {
        return vec![0.0; chunks.len()];
    }

    let mut docs: Vec<Vec<String>> = Vec::with_capacity(chunks.len());
    let mut doc_freq: HashMap<String, usize> = HashMap::new();
    let mut total_len = 0usize;

    for chunk in chunks {
        let tokens = tokenize(&chunk.content);
        total_len += tokens.len();

        let unique: HashSet<String> = tokens.iter().cloned().collect();
        for term in unique {
            *doc_freq.entry(term).or_insert(0) += 1;
        }

        docs.push(tokens);
    }

    let avg_doc_len = (total_len as f64 / chunks.len() as f64).max(1.0);
    let total_docs = chunks.len() as f64;

    docs.into_iter()
        .map(|doc_tokens| {
            if doc_tokens.is_empty() {
                return 0.0;
            }

            let mut term_freq: HashMap<&str, usize> = HashMap::new();
            for term in &doc_tokens {
                *term_freq.entry(term.as_str()).or_insert(0) += 1;
            }

            let dl = doc_tokens.len() as f64;
            query_terms.iter().fold(0.0, |acc, term| {
                let tf = *term_freq.get(term.as_str()).unwrap_or(&0) as f64;
                if tf <= 0.0 {
                    return acc;
                }

                let df = *doc_freq.get(term).unwrap_or(&0) as f64;
                let idf = ((total_docs - df + 0.5) / (df + 0.5) + 1.0).ln();
                let denom = tf + K1 * (1.0 - B + B * (dl / avg_doc_len));
                let score = idf * ((tf * (K1 + 1.0)) / denom);
                acc + score
            })
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::score_query_against_chunks;
    use crate::domain::Chunk;
    use std::collections::BTreeSet;

    #[test]
    fn bm25_scores_relevant_chunk_higher() {
        let chunks = vec![
            Chunk {
                id: "1".to_string(),
                path: "src/auth.rs".to_string(),
                language: "rust".to_string(),
                start_line: 1,
                end_line: 10,
                content: "fn refresh_token() { oauth refresh token logic }".to_string(),
                priority: 0.5,
                tags: BTreeSet::new(),
                token_estimate: 20,
            },
            Chunk {
                id: "2".to_string(),
                path: "src/math.rs".to_string(),
                language: "rust".to_string(),
                start_line: 1,
                end_line: 10,
                content: "fn add(a: i32, b: i32) -> i32 { a + b }".to_string(),
                priority: 0.5,
                tags: BTreeSet::new(),
                token_estimate: 20,
            },
        ];

        let scores = score_query_against_chunks(&chunks, "oauth token refresh");
        assert_eq!(scores.len(), 2);
        assert!(scores[0] > scores[1]);
    }
}

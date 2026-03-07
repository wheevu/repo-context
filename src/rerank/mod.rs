//! Second-stage semantic reranking.
//!
//! Provides lightweight embedding-based semantic reranking for chunks
//! based on query similarity.

use crate::domain::Chunk;
use anyhow::Result;

/// Trait for semantic reranking implementations.
pub trait Reranker {
    /// Returns the name of the reranker.
    fn name(&self) -> &'static str;

    /// Reranks chunks based on similarity to the query.
    ///
    /// # Arguments
    /// * `query` - The search query
    /// * `chunks` - Chunks to rerank
    ///
    /// # Returns
    /// Vector of similarity scores (higher = more similar)
    fn rerank(&self, query: &str, chunks: &[Chunk]) -> Result<Vec<f64>>;
}

/// Lightweight embedding reranker using hash-based embeddings.
///
/// Uses FNV-1a hashing to create 256-dimensional embeddings
/// for fast approximate similarity computation.
pub struct LightweightEmbeddingReranker;

impl Reranker for LightweightEmbeddingReranker {
    fn name(&self) -> &'static str {
        "lightweight-embedding"
    }

    fn rerank(&self, query: &str, chunks: &[Chunk]) -> Result<Vec<f64>> {
        let qv = hash_embedding(query);
        let scores = chunks
            .iter()
            .map(|chunk| {
                let dv = hash_embedding(&chunk.content);
                cosine_similarity(&qv, &dv)
            })
            .collect();
        Ok(scores)
    }
}

/// Builds a reranker instance.
///
/// Currently only supports the lightweight embedding reranker.
/// Future versions may support model-based reranking.
pub fn build_reranker(_model_id: Option<&str>) -> Box<dyn Reranker + Send + Sync> {
    Box::new(LightweightEmbeddingReranker)
}

/// Creates a hash-based embedding for text.
fn hash_embedding(text: &str) -> [f64; 256] {
    let mut vec = [0.0_f64; 256];
    for token in tokenize(text) {
        let hash = fnv1a_64(token.as_bytes());
        let idx = (hash % 256) as usize;
        vec[idx] += 1.0;
    }
    normalize(&mut vec);
    vec
}

/// Tokenizes text into alphanumeric tokens.
fn tokenize(text: &str) -> Vec<&str> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|t| t.len() >= 2).collect()
}

/// Normalizes a vector to unit length.
fn normalize(vec: &mut [f64; 256]) {
    let norm = vec.iter().map(|v| v * v).sum::<f64>().sqrt();
    if norm > 0.0 {
        for value in vec.iter_mut() {
            *value /= norm;
        }
    }
}

/// Computes cosine similarity between two vectors.
fn cosine_similarity(a: &[f64; 256], b: &[f64; 256]) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum::<f64>()
}

/// FNV-1a hash function for 64-bit hashes.
fn fnv1a_64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Normalizes scores to [0, 1] range.
///
/// Uses max normalization: score / max_score
pub fn normalize_scores(scores: &[f64]) -> Vec<f64> {
    let max = scores.iter().copied().fold(0.0_f64, f64::max);
    if max <= 0.0 {
        return vec![0.0; scores.len()];
    }
    scores.iter().map(|s| s / max).collect()
}

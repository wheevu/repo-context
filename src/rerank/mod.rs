//! Second-stage semantic reranking.

use crate::domain::Chunk;
use anyhow::Result;

pub trait Reranker {
    fn name(&self) -> &'static str;
    fn rerank(&self, query: &str, chunks: &[Chunk]) -> Result<Vec<f64>>;
}

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

pub fn build_reranker(_model_id: Option<&str>) -> Box<dyn Reranker + Send + Sync> {
    Box::new(LightweightEmbeddingReranker)
}

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

fn tokenize(text: &str) -> Vec<&str> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|t| t.len() >= 2).collect()
}

fn normalize(vec: &mut [f64; 256]) {
    let norm = vec.iter().map(|v| v * v).sum::<f64>().sqrt();
    if norm > 0.0 {
        for value in vec.iter_mut() {
            *value /= norm;
        }
    }
}

fn cosine_similarity(a: &[f64; 256], b: &[f64; 256]) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum::<f64>()
}

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

pub fn normalize_scores(scores: &[f64]) -> Vec<f64> {
    let max = scores.iter().copied().fold(0.0_f64, f64::max);
    if max <= 0.0 {
        return vec![0.0; scores.len()];
    }
    scores.iter().map(|s| s / max).collect()
}

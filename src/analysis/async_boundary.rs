//! Async boundary detection for runtime topology hints.

use crate::domain::Chunk;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AsyncPattern {
    Await,
    TokioSpawn,
    Channel,
    AsyncFn,
    SelectMacro,
    AsyncTrait,
    TokioEntry,
}

impl AsyncPattern {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Await => "async:await",
            Self::TokioSpawn => "async:spawn",
            Self::Channel => "async:channel",
            Self::AsyncFn => "async:fn",
            Self::SelectMacro => "async:select",
            Self::AsyncTrait => "async:trait",
            Self::TokioEntry => "async:entry",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AsyncBoundary {
    pub chunk_id: String,
    pub patterns: BTreeSet<AsyncPattern>,
}

static AWAIT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.await\b").expect("valid await regex"));
static SPAWN_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b(?:tokio::spawn|task::spawn|spawn_blocking)\b").expect("valid spawn regex")
});
static CHANNEL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b(?:mpsc|oneshot|broadcast|watch)::|\bchannel\s*\(").expect("valid channel regex")
});
static ASYNC_FN_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\basync\s+fn\b").expect("valid async fn regex"));
static SELECT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b(?:tokio|futures)::select!").expect("valid select regex"));
static ASYNC_TRAIT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"#\[\s*async_trait").expect("valid async trait regex"));
static TOKIO_ENTRY_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"#\[\s*tokio::(?:main|test)").expect("valid tokio entry regex"));

pub fn detect_async_boundaries(chunks: &[Chunk]) -> Vec<AsyncBoundary> {
    let mut boundaries = Vec::new();
    for chunk in chunks {
        let mut patterns = BTreeSet::new();
        let content = chunk.content.as_str();
        if AWAIT_RE.is_match(content) {
            patterns.insert(AsyncPattern::Await);
        }
        if SPAWN_RE.is_match(content) {
            patterns.insert(AsyncPattern::TokioSpawn);
        }
        if CHANNEL_RE.is_match(content) {
            patterns.insert(AsyncPattern::Channel);
        }
        if ASYNC_FN_RE.is_match(content) {
            patterns.insert(AsyncPattern::AsyncFn);
        }
        if SELECT_RE.is_match(content) {
            patterns.insert(AsyncPattern::SelectMacro);
        }
        if ASYNC_TRAIT_RE.is_match(content) {
            patterns.insert(AsyncPattern::AsyncTrait);
        }
        if TOKIO_ENTRY_RE.is_match(content) {
            patterns.insert(AsyncPattern::TokioEntry);
        }

        if !patterns.is_empty() {
            boundaries.push(AsyncBoundary { chunk_id: chunk.id.clone(), patterns });
        }
    }
    boundaries
}

#[cfg(test)]
mod tests {
    use super::{detect_async_boundaries, AsyncPattern};
    use crate::domain::Chunk;
    use std::collections::BTreeSet;

    #[test]
    fn detects_core_async_patterns() {
        let chunks = vec![Chunk {
            id: "c1".to_string(),
            path: "src/main.rs".to_string(),
            language: "rust".to_string(),
            start_line: 1,
            end_line: 12,
            content: "#[tokio::main]\nasync fn main() {\nlet (tx, rx) = tokio::sync::mpsc::channel(1);\nlet h = tokio::spawn(async move { tx.send(1).await.ok(); });\ntokio::select! { _ = h => {} }\n}\n".to_string(),
            priority: 0.5,
            tags: BTreeSet::new(),
            token_estimate: 10,
        }];

        let found = detect_async_boundaries(&chunks);
        assert_eq!(found.len(), 1);
        let pats = &found[0].patterns;
        assert!(pats.contains(&AsyncPattern::TokioEntry));
        assert!(pats.contains(&AsyncPattern::AsyncFn));
        assert!(pats.contains(&AsyncPattern::Channel));
        assert!(pats.contains(&AsyncPattern::TokioSpawn));
        assert!(pats.contains(&AsyncPattern::Await));
        assert!(pats.contains(&AsyncPattern::SelectMacro));
    }

    #[test]
    fn detects_async_trait_pattern() {
        let chunks = vec![Chunk {
            id: "c2".to_string(),
            path: "src/worker.rs".to_string(),
            language: "rust".to_string(),
            start_line: 1,
            end_line: 5,
            content: "#[async_trait]\npub trait Worker { async fn run(&self); }\n".to_string(),
            priority: 0.5,
            tags: BTreeSet::new(),
            token_estimate: 10,
        }];

        let found = detect_async_boundaries(&chunks);
        assert_eq!(found.len(), 1);
        assert!(found[0].patterns.contains(&AsyncPattern::AsyncTrait));
    }
}

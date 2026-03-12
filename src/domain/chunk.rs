use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A chunk of file content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    /// Unique stable ID for this chunk.
    pub id: String,
    /// Relative path to source file.
    pub path: String,
    /// Programming language.
    pub language: String,
    /// Starting line number (1-indexed).
    pub start_line: usize,
    /// Ending line number (inclusive).
    pub end_line: usize,
    /// Chunk content.
    pub content: String,
    /// Priority score from parent file.
    #[serde(default)]
    pub priority: f64,
    /// Classification tags.
    #[serde(default)]
    pub tags: BTreeSet<String>,
    /// Estimated tokens in chunk.
    #[serde(default)]
    pub token_estimate: usize,
}

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
    /// Stable file identifier for the source file.
    #[serde(default)]
    pub file_id: String,
    /// Zero-based index of this chunk within its source file.
    #[serde(default)]
    pub chunk_index: usize,
    /// Total number of chunks emitted for the source file.
    #[serde(default)]
    pub chunks_in_file: usize,
    /// Byte offset where this chunk starts, if known.
    #[serde(default)]
    pub byte_start: Option<usize>,
    /// Byte offset where this chunk ends, if known.
    #[serde(default)]
    pub byte_end: Option<usize>,
    /// SHA-256 hash of this chunk's content.
    #[serde(default)]
    pub content_sha256: String,
    /// SHA-256 hash of the source file content.
    #[serde(default)]
    pub file_sha256: String,
}

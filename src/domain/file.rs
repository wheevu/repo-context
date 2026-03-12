use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Information about a scanned file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    /// Absolute path to the file.
    pub path: PathBuf,
    /// Path relative to repository root.
    pub relative_path: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// File extension (with leading dot).
    pub extension: String,
    /// Detected programming language.
    pub language: String,
    /// Unique content-based ID.
    pub id: String,
    /// Priority score (0.0 to 1.0, higher = more important).
    #[serde(default)]
    pub priority: f64,
    /// Estimated tokens in file.
    #[serde(default)]
    pub token_estimate: usize,
    /// Classification tags.
    #[serde(default)]
    pub tags: BTreeSet<String>,
    /// Whether this is a README file.
    #[serde(default)]
    pub is_readme: bool,
    /// Whether this is a configuration file.
    #[serde(default)]
    pub is_config: bool,
    /// Whether this is documentation.
    #[serde(default)]
    pub is_doc: bool,
}

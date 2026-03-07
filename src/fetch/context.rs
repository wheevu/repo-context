//! Repository context management
//!
//! Manages repository paths and temporary directory cleanup.

use std::path::PathBuf;

/// Context for a repository being processed.
///
/// Holds the repository path and manages temporary directory cleanup.
#[derive(Debug)]
pub struct RepoContext {
    /// Root path to the repository
    pub root_path: PathBuf,
    /// Whether this is a temporary directory (needs cleanup)
    pub is_temp: bool,
}

impl RepoContext {
    /// Creates a new RepoContext.
    ///
    /// # Arguments
    /// * `root_path` - Path to the repository root
    /// * `is_temp` - Whether this is a temporary directory
    pub fn new(root_path: PathBuf, is_temp: bool) -> Self {
        Self { root_path, is_temp }
    }
}

impl Drop for RepoContext {
    /// Cleans up temporary directory if `is_temp` is true.
    fn drop(&mut self) {
        if self.is_temp {
            if let Err(e) = std::fs::remove_dir_all(&self.root_path) {
                tracing::warn!(
                    "Failed to clean up temp directory {}: {}",
                    self.root_path.display(),
                    e
                );
            }
        }
    }
}

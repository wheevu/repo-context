//! Repository context management

use std::path::PathBuf;

/// Context for a repository being processed
pub struct RepoContext {
    pub root_path: PathBuf,
    pub is_temp: bool,
}

impl RepoContext {
    pub fn new(root_path: PathBuf, is_temp: bool) -> Self {
        Self { root_path, is_temp }
    }
}

impl Drop for RepoContext {
    fn drop(&mut self) {
        if self.is_temp {
            let _ = std::fs::remove_dir_all(&self.root_path);
        }
    }
}

//! Local path validation

use crate::fetch::RepoContext;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Walk up from `start` looking for a `.git` directory.
///
/// - If a `.git` entry is found at an ancestor directory that differs from `start`,
///   prints a notice and returns that ancestor.
/// - If no `.git` is found, prints a notice and returns `start` unchanged.
pub fn find_repo_root(start: &Path) -> PathBuf {
    let mut current = start.to_path_buf();
    loop {
        if current.join(".git").exists() {
            if current != start {
                println!(
                    "Note: using repository root {} (detected from {})",
                    current.display(),
                    start.display()
                );
            }
            return current;
        }
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => break,
        }
    }
    // No .git found anywhere — use the provided path as-is.
    println!("Note: no .git found; using provided path {} as repository root", start.display());
    start.to_path_buf()
}

pub fn validate_local_path(path: &Path) -> Result<RepoContext> {
    let canonical = path.canonicalize()?;

    if !canonical.exists() {
        anyhow::bail!("Path does not exist: {}", path.display());
    }

    if !canonical.is_dir() {
        anyhow::bail!("Path is not a directory: {}", path.display());
    }

    let root = find_repo_root(&canonical);

    Ok(RepoContext::new(root, false))
}

#[cfg(test)]
mod tests {
    use super::find_repo_root;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn find_repo_root_returns_ancestor_with_git() {
        let temp = TempDir::new().expect("tmp");
        let root = temp.path();

        // Create a .git directory at the root
        fs::create_dir(root.join(".git")).expect("mkdir .git");

        // Create a subdirectory
        let subdir = root.join("src").join("lib");
        fs::create_dir_all(&subdir).expect("mkdir subdir");

        let found = find_repo_root(&subdir);
        assert_eq!(found, root, "should walk up to the directory containing .git");
    }

    #[test]
    fn find_repo_root_returns_start_when_no_git() {
        let temp = TempDir::new().expect("tmp");
        let dir = temp.path().join("myproject");
        fs::create_dir_all(&dir).expect("mkdir myproject");

        let found = find_repo_root(&dir);
        assert_eq!(found, dir, "should return the input path when no .git is found");
    }

    #[test]
    fn find_repo_root_returns_start_when_git_is_at_start() {
        let temp = TempDir::new().expect("tmp");
        let root = temp.path();

        // .git is already at the start — no upward walk needed
        fs::create_dir(root.join(".git")).expect("mkdir .git");

        let found = find_repo_root(root);
        assert_eq!(found, root, "should return start when .git is already there");
    }
}

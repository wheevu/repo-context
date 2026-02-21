//! Shared cache path helpers for CLI commands.

use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub fn remote_index_cache_db_path(
    repo_url: Option<&str>,
    repo_ref: Option<&str>,
    config_hash: &str,
) -> Option<PathBuf> {
    let repo_url = repo_url?.trim();
    if repo_url.is_empty() {
        return None;
    }
    let cache_base = cache_root_dir()?;
    let key = remote_index_cache_key(repo_url, repo_ref, config_hash);
    Some(cache_base.join("repo-context").join("index").join(key).join("index.sqlite"))
}

pub fn remote_index_cache_key(repo_url: &str, repo_ref: Option<&str>, config_hash: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(repo_url.trim());
    hasher.update("\n");
    hasher.update(repo_ref.unwrap_or("HEAD"));
    hasher.update("\n");
    hasher.update(config_hash);
    format!("{:x}", hasher.finalize())
}

pub fn cache_root_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
            return Some(PathBuf::from(xdg));
        }
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache"))
    }
}

#[cfg(test)]
mod tests {
    use super::{remote_index_cache_db_path, remote_index_cache_key};

    #[test]
    fn remote_index_key_changes_with_ref() {
        let a = remote_index_cache_key("https://github.com/o/r", Some("main"), "abc");
        let b = remote_index_cache_key("https://github.com/o/r", Some("dev"), "abc");
        assert_ne!(a, b);
    }

    #[test]
    fn remote_index_cache_path_requires_repo_url() {
        assert!(remote_index_cache_db_path(None, None, "abc").is_none());
    }
}

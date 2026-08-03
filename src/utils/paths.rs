//! Path normalization and URL sanitization.
//!
//! Provides path normalization utilities for consistent path handling
//! across different operating systems.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static ATOMIC_WRITE_SEQ: AtomicU64 = AtomicU64::new(1);

/// Normalizes a path by converting backslashes to forward slashes.
///
/// This ensures consistent path representation regardless of the
/// operating system's path separator conventions.
///
/// # Arguments
/// * `path` - Path string to normalize
///
/// # Returns
/// Normalized path with forward slashes
///
/// # Examples
/// ```
/// use repo_context::utils::normalize_path;
/// assert_eq!(normalize_path("foo\\bar\\baz"), "foo/bar/baz");
/// assert_eq!(normalize_path("foo/bar/baz"), "foo/bar/baz");
/// ```
pub fn normalize_path(path: &str) -> String {
    // Convert backslashes to forward slashes and normalize
    path.replace('\\', "/")
}

/// Write a file through a sibling temporary and rename it into place.
///
/// Export artifacts are consumed by other tools, so a failed render must not
/// leave a truncated file at the advertised path.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }

    let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or("output");
    let temporary = path.with_file_name(format!(
        ".{file_name}.repo-context-{}-{}.tmp",
        std::process::id(),
        ATOMIC_WRITE_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        replace_file(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, target: &Path) -> io::Result<()> {
    fs::rename(temporary, target)
}

#[cfg(windows)]
fn replace_file(temporary: &Path, target: &Path) -> io::Result<()> {
    // Windows does not let rename replace an existing file. Keep a same-dir
    // backup long enough to restore the previous artifact if the second
    // rename fails.
    if !target.exists() {
        return fs::rename(temporary, target);
    }
    let backup = target.with_file_name(format!(
        ".{}.repo-context-backup-{}-{}",
        target.file_name().and_then(|name| name.to_str()).unwrap_or("output"),
        std::process::id(),
        ATOMIC_WRITE_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    fs::rename(target, &backup)?;
    match fs::rename(temporary, target) {
        Ok(()) => {
            let _ = fs::remove_file(backup);
            Ok(())
        }
        Err(error) => {
            let _ = fs::rename(&backup, target);
            Err(error)
        }
    }
}

/// Strips user credentials from a URL for safe display and serialization.
///
/// Replaces `user:password@` in `https://user:pass@host/...` with `***@`.
/// Also strips query parameters that look like tokens.
///
/// # Examples
/// ```
/// use repo_context::utils::redact_url_credentials;
/// assert_eq!(redact_url_credentials("https://user:token@github.com/org/repo"), "https://***@github.com/org/repo");
/// assert_eq!(redact_url_credentials("https://github.com/org/repo"), "https://github.com/org/repo");
/// ```
pub fn redact_url_credentials(url: &str) -> String {
    let (without_fragment, _) = url.split_once('#').unwrap_or((url, ""));
    let (without_query, _) = without_fragment.split_once('?').unwrap_or((without_fragment, ""));
    for scheme in ["https://", "http://"] {
        if let Some(rest) = without_query.strip_prefix(scheme) {
            let authority_end = rest.find('/').unwrap_or(rest.len());
            let authority = &rest[..authority_end];
            let suffix = &rest[authority_end..];
            let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
            let prefix = if host == authority { "" } else { "***@" };
            return format!("{scheme}{prefix}{host}{suffix}");
        }
    }
    // Also handle git-over-SSH: git@host:user/repo is safe to display.
    without_query.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_https_credentials() {
        let out = redact_url_credentials("https://user:token@github.com/org/repo.git");
        assert!(!out.contains("token"));
        assert!(out.starts_with("https://***@"));
    }

    #[test]
    fn passes_clean_urls_unchanged() {
        let clean = "https://github.com/org/repo";
        assert_eq!(redact_url_credentials(clean), clean);
    }

    #[test]
    fn handles_ssh_urls() {
        let ssh = "git@github.com:org/repo.git";
        assert_eq!(redact_url_credentials(ssh), ssh);
    }

    #[test]
    fn redacts_query_and_fragment_tokens() {
        let out = redact_url_credentials("https://github.com/org/repo?token=secret#readme");
        assert_eq!(out, "https://github.com/org/repo");
        assert!(!out.contains("secret"));
    }

    #[test]
    fn atomic_write_replaces_without_partial_artifacts() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("artifact.json");
        write_atomic(&path, b"first").expect("first write");
        write_atomic(&path, b"second").expect("replacement write");
        assert_eq!(fs::read_to_string(&path).expect("read artifact"), "second");
        assert_eq!(fs::read_dir(directory.path()).expect("read directory").count(), 1);
    }
}

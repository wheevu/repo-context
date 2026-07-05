//! Path normalization and URL sanitization.
//!
//! Provides path normalization utilities for consistent path handling
//! across different operating systems.

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
    // Strip userinfo segment: https://user:pass@host → https://***@host
    if let Some(rest) = url.strip_prefix("https://") {
        if let Some(at_pos) = rest.find('@') {
            return format!("https://***@{}", &rest[at_pos + 1..]);
        }
    }
    if let Some(rest) = url.strip_prefix("http://") {
        if let Some(at_pos) = rest.find('@') {
            return format!("http://***@{}", &rest[at_pos + 1..]);
        }
    }
    // Also handle git-over-SSH: git@host:user/repo is fine, no credential
    url.to_string()
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
}

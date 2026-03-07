//! Path normalization
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

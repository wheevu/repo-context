//! File classification helpers for detecting minified, generated, lock, and vendored files.

use once_cell::sync::Lazy;
use regex::Regex;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Common patterns indicating generated files
static GENERATED_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r"(?i)generated").unwrap(),
        Regex::new(r"(?i)auto-generated").unwrap(),
        Regex::new(r"(?i)do not edit").unwrap(),
        Regex::new(r"(?i)machine generated").unwrap(),
    ]
});

const MINIFIED_INDICATORS: &[&str] = &[".min.", ".bundle.", ".packed."];

/// Check if a file appears to be minified based on filename or line length.
///
/// # Arguments
/// * `path` - Path to the file
/// * `max_line_length` - Threshold line length (default: 5000 chars)
///
/// # Returns
/// `true` if the file appears to be minified
pub fn is_likely_minified(path: &Path, max_line_length: usize) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();

    // Check filename indicators first (fast path)
    for indicator in MINIFIED_INDICATORS {
        if name.contains(indicator) {
            return true;
        }
    }

    // Check first line length
    if let Ok(mut file) = File::open(path) {
        let mut buffer = vec![0u8; max_line_length + 1];
        if let Ok(bytes_read) = file.read(&mut buffer) {
            if bytes_read == 0 {
                return false;
            }

            // Find first newline
            if let Some(newline_pos) = buffer[..bytes_read].iter().position(|&b| b == b'\n') {
                return newline_pos > max_line_length;
            }

            // No newline found - if we read the full buffer, line is too long
            return bytes_read > max_line_length;
        }
    }

    false
}

/// Check if a file appears to be generated or auto-generated.
///
/// Uses filename hints, directory location, and content sampling.
///
/// # Arguments
/// * `path` - Path to the file
/// * `content_sample` - Optional content snippet for header-marker checks
///
/// # Returns
/// `true` if the file appears to be generated
pub fn is_likely_generated(path: &Path, content_sample: &str) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();

    // Check filename indicators
    for indicator in MINIFIED_INDICATORS {
        if name.contains(indicator) {
            return true;
        }
    }

    // Check common generated directories
    let path_str = path.to_str().unwrap_or("").to_lowercase();
    let path_normalized = path_str.replace('\\', "/");
    for dir in ["generated/", "gen/", "auto/", "build/"] {
        if path_normalized.contains(dir) {
            return true;
        }
    }

    // Check content for generated markers
    if !content_sample.is_empty() {
        let sample_lower = content_sample.chars().take(2000).collect::<String>().to_lowercase();

        for pattern in GENERATED_PATTERNS.iter() {
            if pattern.is_match(&sample_lower) {
                return true;
            }
        }

        // Check for extremely long first line (common in minified files)
        if let Some(first_line) = content_sample.lines().next() {
            if first_line.len() > 1000 {
                return true;
            }
        }
    }

    false
}

/// Check if a file is a dependency lock file.
///
/// # Arguments
/// * `path` - Path to check
///
/// # Returns
/// `true` if the filename matches a known lock file
pub fn is_lock_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();

    matches!(
        name.as_str(),
        "package-lock.json"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            | "poetry.lock"
            | "pipfile.lock"
            | "cargo.lock"
            | "gemfile.lock"
            | "composer.lock"
            | "go.sum"
    )
}

/// Check if a file likely belongs to vendored/third-party code.
///
/// # Arguments
/// * `path` - Path to check
///
/// # Returns
/// `true` if the path contains a known vendor directory segment
pub fn is_vendored(path: &Path) -> bool {
    let path_str = path.to_str().unwrap_or("").to_lowercase();
    let path_normalized = path_str.replace('\\', "/");

    for vendor_dir in [
        "vendor/",
        "vendors/",
        "third_party/",
        "third-party/",
        "thirdparty/",
        "external/",
        "extern/",
        "node_modules/",
    ] {
        if path_normalized.contains(vendor_dir) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_is_likely_minified_by_name() {
        assert!(is_likely_minified(Path::new("bundle.min.js"), 5000));
        assert!(is_likely_minified(Path::new("app.bundle.js"), 5000));
        assert!(!is_likely_minified(Path::new("app.js"), 5000));
    }

    #[test]
    fn test_is_likely_minified_by_line_length() {
        let mut file = NamedTempFile::new().unwrap();
        // Write a very long line
        let long_line = "a".repeat(6000);
        file.write_all(long_line.as_bytes()).unwrap();
        file.flush().unwrap();

        assert!(is_likely_minified(file.path(), 5000));
    }

    #[test]
    fn test_is_lock_file() {
        assert!(is_lock_file(Path::new("package-lock.json")));
        assert!(is_lock_file(Path::new("yarn.lock")));
        assert!(is_lock_file(Path::new("Cargo.lock")));
        assert!(!is_lock_file(Path::new("package.json")));
    }

    #[test]
    fn test_is_vendored() {
        assert!(is_vendored(Path::new("vendor/foo/bar.js")));
        assert!(is_vendored(Path::new("node_modules/react/index.js")));
        assert!(is_vendored(Path::new("third_party/lib.c")));
        assert!(!is_vendored(Path::new("src/main.rs")));
    }

    #[test]
    fn test_is_likely_generated() {
        assert!(is_likely_generated(Path::new("generated/api.ts"), ""));
        assert!(is_likely_generated(
            Path::new("src/file.ts"),
            "// This file is auto-generated. Do not edit."
        ));
        assert!(!is_likely_generated(Path::new("src/main.rs"), "fn main() {}"));
    }
}

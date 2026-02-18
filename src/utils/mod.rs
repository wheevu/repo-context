//! Utility functions

pub mod classify;
pub mod encoding;
pub mod hashing;
pub mod paths;
pub mod tokens;

pub use classify::{is_likely_generated, is_likely_minified, is_lock_file, is_vendored};
pub use encoding::{is_binary_file, read_file_safe};
pub use hashing::stable_hash;
pub use paths::normalize_path;
pub use tokens::estimate_tokens;

/// Format a number with thousands separators (e.g. 1048576 → "1,048,576").
///
/// Matches Python's `{:,}` format specifier used in the context pack header
/// and info command output.
pub fn format_with_commas(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            result.push(',');
        }
        result.push(b as char);
    }
    result
}

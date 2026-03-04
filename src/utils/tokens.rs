//! Token estimation

/// Estimate tokens using a simple heuristic (bytes / 4).
///
/// Uses byte length for O(1) performance instead of O(n) char counting.
/// This is a fast approximation that may slightly over-count for multi-byte
/// UTF-8 content (e.g. CJK text, emoji), but is accurate enough for token
/// budgeting purposes and significantly faster for large files.
#[inline]
pub fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

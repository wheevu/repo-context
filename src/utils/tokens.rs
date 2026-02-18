//! Token estimation

/// Estimate tokens using a simple heuristic (chars / 4).
///
/// Matches Python's fallback: `len(text) // 4` where `len` counts Unicode
/// code points, not bytes.  Using byte length over-counts for multi-byte UTF-8
/// content (e.g. CJK text, emoji).
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count() / 4
}

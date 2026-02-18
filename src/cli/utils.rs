//! Shared CLI utilities.

/// Parse a comma-separated string into a `Vec<String>`, trimming whitespace and
/// discarding empty segments.  Returns `None` when `value` is `None`.
pub fn parse_csv(value: &Option<String>) -> Option<Vec<String>> {
    value.as_ref().map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| part.to_string())
            .collect::<Vec<_>>()
    })
}

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

/// Parse repeated CLI values where each value may itself be comma-separated.
pub fn parse_csv_multi(values: &[String]) -> Vec<String> {
    values
        .iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect()
}

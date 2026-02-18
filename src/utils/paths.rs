//! Path normalization

pub fn normalize_path(path: &str) -> String {
    // Convert backslashes to forward slashes and normalize
    path.replace('\\', "/")
}

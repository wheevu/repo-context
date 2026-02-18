//! Stable hashing for chunk IDs

use sha2::{Digest, Sha256};

pub fn stable_hash(content: &str, path: &str, start_line: usize, end_line: usize) -> String {
    // Match Python: hashlib.sha256(f"{path}:{start_line}-{end_line}:{content[:1000]}".encode()).hexdigest()[:16]
    // content[:1000] in Python slices by character, so use char-boundary-safe truncation.
    let content_prefix: String = content.chars().take(1000).collect();
    let hash_input = format!("{path}:{start_line}-{end_line}:{content_prefix}");
    let mut hasher = Sha256::new();
    hasher.update(hash_input.as_bytes());
    let result = hasher.finalize();
    format!("{:x}", result)[..16].to_string()
}

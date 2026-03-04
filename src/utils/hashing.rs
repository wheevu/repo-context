//! Stable hashing for chunk IDs

use sha2::{Digest, Sha256};

/// A buffer large enough for 32 bytes of hex (64 chars).
const HEX_BUF_SIZE: usize = 64;

/// Convert bytes to hex in-place without allocating a String.
#[inline]
fn bytes_to_hex<'a>(bytes: &[u8], buf: &'a mut [u8; HEX_BUF_SIZE]) -> &'a str {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    for (i, byte) in bytes.iter().enumerate() {
        let idx = i * 2;
        buf[idx] = HEX_CHARS[(byte >> 4) as usize];
        buf[idx + 1] = HEX_CHARS[(byte & 0x0f) as usize];
    }
    // SAFETY: we only write valid ASCII hex characters
    std::str::from_utf8(&buf[..bytes.len() * 2]).unwrap()
}

pub fn stable_hash(content: &str, path: &str, start_line: usize, end_line: usize) -> String {
    // Match Python: hashlib.sha256(f"{path}:{start_line}-{end_line}:{content[:1000]}".encode()).hexdigest()[:16]
    // content[:1000] in Python slices by character, so use char-boundary-safe truncation.
    let content_prefix: String = content.chars().take(1000).collect();
    let hash_input = format!("{path}:{start_line}-{end_line}:{content_prefix}");
    let mut hasher = Sha256::new();
    hasher.update(hash_input.as_bytes());
    let result = hasher.finalize();

    // Avoid allocating the full hex string by formatting directly into a buffer
    let mut hex_buf = [0u8; HEX_BUF_SIZE];
    let hex_str = bytes_to_hex(&result, &mut hex_buf);
    hex_str[..16].to_string()
}

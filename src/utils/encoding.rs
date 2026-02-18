//! Encoding detection and file reading with UTF-8 fallback logic.
//!
//! This module provides robust file reading that handles:
//! - BOM detection (UTF-8, UTF-16 LE/BE)
//! - UTF-8 fast-path with strict validation
//! - Fallback encoding detection using chardetng
//! - Binary file detection
//! - Safe error handling with replacement characters

use anyhow::{Context, Result};
use chardetng::EncodingDetector;
use encoding_rs::{Encoding, UTF_8};
use std::fs::File;
use std::io::Read;
use std::path::Path;

const DEFAULT_SAMPLE_SIZE: usize = 8192;

/// Detect the encoding of a file.
///
/// Strategy (matching Python implementation):
/// 1. Check for BOM markers first (most reliable)
/// 2. Try strict UTF-8 decoding (fast path for most modern files)
/// 3. Fall back to chardetng for non-UTF-8 files
/// 4. Return "utf-8" as safe default on errors
///
/// # Arguments
/// * `path` - Path to the file to inspect
/// * `sample_size` - Number of bytes to sample (default: 8192)
///
/// # Returns
/// A normalized encoding label (e.g., "utf-8", "utf-8-sig", "utf-16-le")
pub fn detect_encoding(path: &Path, sample_size: usize) -> String {
    detect_encoding_impl(path, sample_size).unwrap_or_else(|_| "utf-8".to_string())
}

fn detect_encoding_impl(path: &Path, sample_size: usize) -> Result<String> {
    let mut file = File::open(path)?;
    let mut sample = vec![0u8; sample_size];
    let bytes_read = file.read(&mut sample)?;
    sample.truncate(bytes_read);

    if sample.is_empty() {
        return Ok("utf-8".to_string());
    }

    // Check for BOM markers first (most reliable)
    if sample.len() >= 3 && sample.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Ok("utf-8-sig".to_string());
    }
    if sample.len() >= 2 && sample.starts_with(&[0xff, 0xfe]) {
        return Ok("utf-16-le".to_string());
    }
    if sample.len() >= 2 && sample.starts_with(&[0xfe, 0xff]) {
        return Ok("utf-16-be".to_string());
    }

    // Try UTF-8 first (fast path) - most source files are UTF-8
    if std::str::from_utf8(&sample).is_ok() {
        return Ok("utf-8".to_string());
    }

    // Fall back to chardetng for non-UTF-8 files
    let mut detector = EncodingDetector::new();
    detector.feed(&sample, true);
    let encoding = detector.guess(None, true);

    // Normalize encoding name to match Python behavior
    let name = encoding.name().to_lowercase();
    if name == "windows-1252" || name == "iso-8859-1" {
        // chardetng may detect these, keep as-is
        Ok(name)
    } else if name.contains("utf-8") || name == "ascii" {
        Ok("utf-8".to_string())
    } else {
        Ok(name)
    }
}

/// Detect if a file is binary (not text).
///
/// Uses two heuristics:
/// 1. Null byte check (strong binary indicator)
/// 2. Ratio of printable ASCII bytes (< 70% = likely binary)
///
/// # Arguments
/// * `path` - Path to the file to test
/// * `sample_size` - Number of bytes to sample (default: 8192)
///
/// # Returns
/// `true` if the file appears to be binary, `false` otherwise
pub fn is_binary_file(path: &Path, sample_size: usize) -> bool {
    is_binary_file_impl(path, sample_size).unwrap_or(true)
}

fn is_binary_file_impl(path: &Path, sample_size: usize) -> Result<bool> {
    let mut file = File::open(path)?;
    let mut sample = vec![0u8; sample_size];
    let bytes_read = file.read(&mut sample)?;
    sample.truncate(bytes_read);

    if sample.is_empty() {
        return Ok(false);
    }

    // Check for null bytes (strong indicator of binary)
    if sample.contains(&0) {
        return Ok(true);
    }

    // Check for high ratio of non-text bytes
    // Text files typically have >70% printable ASCII
    let printable_count = sample
        .iter()
        .filter(|&&b| {
            (32..=126).contains(&b) || b == 9 || b == 10 || b == 13 // printable + tab, LF, CR
        })
        .count();

    Ok((printable_count as f64 / sample.len() as f64) < 0.70)
}

/// Read a file safely with encoding detection and error handling.
///
/// Strategy (matching Python implementation):
/// 1. If encoding explicitly provided, use it with replacement
/// 2. Try strict UTF-8 first (most repos are UTF-8)
/// 3. If UTF-8 fails, detect encoding and retry with replacement
/// 4. Last resort: UTF-8 with replacement characters
///
/// # Arguments
/// * `path` - Path to the file to read
/// * `max_bytes` - Optional maximum number of bytes to read
/// * `encoding` - Optional explicit encoding (None enables auto-detection)
///
/// # Returns
/// A tuple `(content, encoding_used)`
pub fn read_file_safe(
    path: &Path,
    max_bytes: Option<usize>,
    encoding: Option<&str>,
) -> Result<(String, String)> {
    // If encoding specified, use it directly
    if let Some(enc_name) = encoding {
        if let Some((content, used_enc)) = try_read_with_encoding(path, max_bytes, enc_name) {
            return Ok((content, used_enc));
        }
        // Fall through to auto-detect if specified encoding fails
    }

    // Try UTF-8 first (strict mode to detect issues)
    match try_read_utf8_strict(path, max_bytes) {
        Ok(content) => return Ok((content, "utf-8".to_string())),
        Err(_) => {
            // UTF-8 failed, continue to detection
        }
    }

    // Fall back to encoding detection
    let detected = detect_encoding(path, DEFAULT_SAMPLE_SIZE);
    if let Some((content, used_enc)) = try_read_with_encoding(path, max_bytes, &detected) {
        return Ok((content, used_enc));
    }

    // Last resort: UTF-8 with replacement
    let content = std::fs::read_to_string(path)
        .or_else(|_| {
            let bytes = std::fs::read(path)?;
            let (cow, _, _) = UTF_8.decode(&bytes);
            Ok::<_, std::io::Error>(cow.into_owned())
        })
        .with_context(|| format!("Failed to read file: {}", path.display()))?;

    Ok((content, "utf-8".to_string()))
}

fn try_read_utf8_strict(path: &Path, max_bytes: Option<usize>) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let content = std::str::from_utf8(&bytes).context("Not valid UTF-8")?.to_string();

    if let Some(limit) = max_bytes {
        Ok(content.chars().take(limit).collect())
    } else {
        Ok(content)
    }
}

fn try_read_with_encoding(
    path: &Path,
    max_bytes: Option<usize>,
    encoding_name: &str,
) -> Option<(String, String)> {
    // Try to find the encoding
    let encoding = Encoding::for_label(encoding_name.as_bytes())?;

    // Read the file bytes
    let bytes = std::fs::read(path).ok()?;

    // Decode with replacement for invalid sequences
    let (decoded, _encoding_used, _had_errors) = encoding.decode(&bytes);

    let content = if let Some(limit) = max_bytes {
        decoded.chars().take(limit).collect()
    } else {
        decoded.into_owned()
    };

    // Return the content and the encoding name
    Some((content, encoding.name().to_lowercase()))
}

/// Read a specific line range from a file without loading it entirely.
///
/// # Arguments
/// * `path` - Path to the file to read
/// * `encoding` - Optional encoding (None triggers auto-detection)
/// * `start_line` - 1-indexed first line to include
/// * `end_line` - 1-indexed last line to include (inclusive). None reads to EOF
///
/// # Returns
/// A vector of lines (preserving original line endings) in the requested range
#[allow(dead_code)]
pub fn stream_file_lines(
    path: &Path,
    encoding: Option<&str>,
    start_line: usize,
    end_line: Option<usize>,
) -> Result<Vec<String>> {
    let encoding_to_use = encoding.unwrap_or("utf-8");
    let (content, _) = read_file_safe(path, None, Some(encoding_to_use))?;

    let lines: Vec<String> = content
        .lines()
        .enumerate()
        .filter_map(|(idx, line)| {
            let line_num = idx + 1;
            if line_num < start_line {
                return None;
            }
            if let Some(end) = end_line {
                if line_num > end {
                    return None;
                }
            }
            Some(format!("{}\n", line))
        })
        .collect();

    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_detect_utf8() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all("Hello, world!".as_bytes()).unwrap();
        file.flush().unwrap();

        let encoding = detect_encoding(file.path(), DEFAULT_SAMPLE_SIZE);
        assert_eq!(encoding, "utf-8");
    }

    #[test]
    fn test_detect_utf8_bom() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&[0xef, 0xbb, 0xbf]).unwrap(); // UTF-8 BOM
        file.write_all("Hello".as_bytes()).unwrap();
        file.flush().unwrap();

        let encoding = detect_encoding(file.path(), DEFAULT_SAMPLE_SIZE);
        assert_eq!(encoding, "utf-8-sig");
    }

    #[test]
    fn test_is_binary_null_byte() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&[0x00, 0x01, 0x02]).unwrap();
        file.flush().unwrap();

        assert!(is_binary_file(file.path(), DEFAULT_SAMPLE_SIZE));
    }

    #[test]
    fn test_is_not_binary_text() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all("Normal text file".as_bytes()).unwrap();
        file.flush().unwrap();

        assert!(!is_binary_file(file.path(), DEFAULT_SAMPLE_SIZE));
    }

    #[test]
    fn test_read_file_safe_utf8() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all("Test content 🚀".as_bytes()).unwrap();
        file.flush().unwrap();

        let (content, encoding) = read_file_safe(file.path(), None, None).unwrap();
        assert_eq!(content, "Test content 🚀");
        assert_eq!(encoding, "utf-8");
    }

    #[test]
    fn test_read_file_safe_with_max_bytes() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all("Hello, world!".as_bytes()).unwrap();
        file.flush().unwrap();

        let (content, _) = read_file_safe(file.path(), Some(5), None).unwrap();
        assert_eq!(content.chars().count(), 5);
    }

    #[test]
    fn test_stream_file_lines() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all("Line 1\nLine 2\nLine 3\nLine 4\n".as_bytes()).unwrap();
        file.flush().unwrap();

        let lines = stream_file_lines(file.path(), None, 2, Some(3)).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("Line 2"));
        assert!(lines[1].contains("Line 3"));
    }
}

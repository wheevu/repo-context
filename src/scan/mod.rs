//! File scanning with gitignore support

use crate::domain::{FileInfo, ScanStats};
use anyhow::Result;
use std::path::Path;

pub mod scanner;
pub mod tree;

pub use scanner::FileScanner;

#[allow(dead_code)]
pub fn scan_repository<P: AsRef<Path>>(root: P) -> Result<(Vec<FileInfo>, ScanStats)> {
    let mut scanner = FileScanner::new(root.as_ref().to_path_buf());
    let files = scanner.scan()?;
    let stats = scanner.stats().clone();
    Ok((files, stats))
}

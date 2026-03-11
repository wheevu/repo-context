//! File ranking by importance.

use crate::domain::{FileInfo, RankingWeights};
use anyhow::Result;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::Path;

pub mod ranker;

pub use ranker::FileRanker;

/// Ranks files by importance using default weights.
///
/// # Arguments
/// * `root_path` - Path to the repository root
/// * `files` - Files to rank
///
/// # Returns
/// Ranked list of files sorted by priority
pub fn rank_files(root_path: &Path, files: Vec<FileInfo>) -> Result<Vec<FileInfo>> {
    rank_files_with_weights(root_path, files, RankingWeights::default())
}

/// Ranks files by importance with custom weights.
///
/// # Arguments
/// * `root_path` - Path to the repository root
/// * `files` - Files to rank
/// * `weights` - Custom ranking weights
///
/// # Returns
/// Ranked list of files sorted by priority
pub fn rank_files_with_weights(
    root_path: &Path,
    mut files: Vec<FileInfo>,
    weights: RankingWeights,
) -> Result<Vec<FileInfo>> {
    let scanned_files = files.iter().map(|f| f.relative_path.clone()).collect();
    let ranker = FileRanker::with_weights(root_path, scanned_files, weights);
    ranker.rank_files(&mut files);
    Ok(files)
}

/// Same as `rank_files_with_weights` but also returns manifest info extracted during ranking.
/// The manifest info includes `scripts`, `name`, `description` from `package.json` and similar.
pub fn rank_files_with_manifest(
    root_path: &Path,
    mut files: Vec<FileInfo>,
    weights: RankingWeights,
) -> Result<(Vec<FileInfo>, HashMap<String, JsonValue>)> {
    let scanned_files = files.iter().map(|f| f.relative_path.clone()).collect();
    let ranker = FileRanker::with_weights(root_path, scanned_files, weights);
    ranker.rank_files(&mut files);
    Ok((files, ranker.get_manifest_info().clone()))
}

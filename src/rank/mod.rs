//! File ranking by importance

use crate::domain::{FileInfo, RankingWeights};
use anyhow::Result;
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub mod ranker;

pub use ranker::FileRanker;

pub fn rank_files(root_path: &Path, files: Vec<FileInfo>) -> Result<Vec<FileInfo>> {
    rank_files_with_weights(root_path, files, RankingWeights::default())
}

pub fn rank_files_with_weights(
    root_path: &Path,
    mut files: Vec<FileInfo>,
    weights: RankingWeights,
) -> Result<Vec<FileInfo>> {
    let scanned_files: HashSet<String> = files.iter().map(|f| f.relative_path.clone()).collect();
    let ranker = FileRanker::with_weights(root_path, scanned_files, weights);
    ranker.rank_files(&mut files);
    Ok(files)
}

/// Same as `rank_files_with_weights` but also returns manifest info extracted during ranking.
/// The manifest info includes `scripts`, `name`, `description` from `package.json` etc.
pub fn rank_files_with_manifest(
    root_path: &Path,
    mut files: Vec<FileInfo>,
    weights: RankingWeights,
) -> Result<(Vec<FileInfo>, HashMap<String, JsonValue>)> {
    let scanned_files: HashSet<String> = files.iter().map(|f| f.relative_path.clone()).collect();
    let ranker = FileRanker::with_weights(root_path, scanned_files, weights);
    ranker.rank_files(&mut files);
    let manifest = ranker.get_manifest_info().clone();
    Ok((files, manifest))
}

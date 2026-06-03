//! Entry point detection for module mode.
//!
//! Kept for backward compatibility. New code should use `focus::discover_candidates`.

#![allow(dead_code)]

use crate::domain::{FileInfo, ModuleConfig};
use crate::module::graph::ImportGraph;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const ENTRY_DIRS: &[&str] =
    &["pages", "routes", "views", "screens", "cmd", "handlers", "controllers"];
const CANDIDATE_EXTENSIONS: &[&str] = &[".ts", ".tsx", ".js", ".jsx", ".rs", ".go"];
const EXCLUDED_NAME_FRAGMENTS: &[&str] = &["config", "env", "vite-env", "timestamp"];
const EXCLUDED_SRC_ROOT_NAMES: &[&str] = &["app.tsx", "app.ts", "index.tsx", "index.ts"];
const TOPOLOGY_EXCLUDED_PATH_SEGMENTS: &[&str] = &["/ui/", "/components/", "/lib/"];

/// Detects entry point candidates using config, directory, and topology strategies.
#[must_use]
pub fn detect(
    root: &Path,
    files: &[FileInfo],
    graph: &ImportGraph,
    cfg: &ModuleConfig,
) -> Vec<PathBuf> {
    let mut candidates = if !cfg.module_roots.is_empty() {
        from_config(root, files, &cfg.module_roots)
    } else {
        let mut all = from_directory_heuristics(files);
        all.extend(from_topology(graph));
        all
    };
    candidates.sort();
    candidates.dedup();
    candidates
}

fn from_config(root: &Path, files: &[FileInfo], roots: &[PathBuf]) -> Vec<PathBuf> {
    let root_set: HashSet<PathBuf> = roots.iter().map(|p| normalize(root.join(p))).collect();
    files
        .iter()
        .filter(|f| is_candidate_file(f))
        .filter(|f| f.path.parent().map(|p| root_set.contains(&normalize(p))).unwrap_or(false))
        .map(|f| normalize(&f.path))
        .collect()
}

fn from_directory_heuristics(files: &[FileInfo]) -> Vec<PathBuf> {
    files
        .iter()
        .filter(|f| is_candidate_file(f))
        .filter(|f| {
            let path = Path::new(&f.relative_path);
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(|dir| ENTRY_DIRS.iter().any(|entry| entry.eq_ignore_ascii_case(dir)))
                .unwrap_or(false)
        })
        .map(|f| normalize(&f.path))
        .collect()
}

fn from_topology(graph: &ImportGraph) -> Vec<PathBuf> {
    graph
        .files
        .iter()
        .filter(|(_, file)| is_candidate_file(file))
        .filter(|(_, file)| !is_topology_excluded_path(file))
        .filter(|(path, _)| graph.incoming.get(*path).copied().unwrap_or(0) == 0)
        .map(|(path, _)| path.clone())
        .collect()
}

fn is_candidate_file(file: &FileInfo) -> bool {
    if file.relative_path.ends_with(".d.ts") {
        return false;
    }
    if !CANDIDATE_EXTENSIONS.contains(&file.extension.as_str()) {
        return false;
    }
    let name = file.path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
    if EXCLUDED_NAME_FRAGMENTS.iter().any(|fragment| name.contains(fragment)) {
        return false;
    }
    if is_src_root_app_or_index(file, &name) {
        return false;
    }
    if is_entry_pattern(&name) {
        return false;
    }
    true
}

fn is_src_root_app_or_index(file: &FileInfo, name: &str) -> bool {
    EXCLUDED_SRC_ROOT_NAMES.contains(&name)
        && Path::new(&file.relative_path)
            .parent()
            .and_then(|p| p.to_str())
            .map(|parent| parent == "src")
            .unwrap_or(false)
}

fn is_entry_pattern(name: &str) -> bool {
    let stem = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);
    stem.starts_with("entry-") || stem.ends_with("-entry")
}

fn is_topology_excluded_path(file: &FileInfo) -> bool {
    let rel = format!("/{}", file.relative_path.replace('\\', "/").to_ascii_lowercase());
    TOPOLOGY_EXCLUDED_PATH_SEGMENTS.iter().any(|segment| rel.contains(segment))
}

fn normalize(path: impl AsRef<Path>) -> PathBuf {
    path.as_ref().canonicalize().unwrap_or_else(|_| path.as_ref().to_path_buf())
}

//! Module mode orchestration.

pub mod css_scope;
pub mod detect;
pub mod graph;
pub mod picker;

use crate::domain::{Config, FileInfo};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Result of the module-mode selection and dependency walk.
#[derive(Debug, Clone)]
pub struct ModuleRun {
    /// Files to feed into the normal renderer, ordered by import depth.
    pub files: Vec<FileInfo>,
    /// Scoped CSS content keyed by original CSS path.
    pub content_overrides: HashMap<PathBuf, String>,
    /// Metadata header to prepend to the prompt output.
    pub header: String,
    /// Entry point filename without extension, used for module output paths.
    pub entry_basename: String,
}

/// Runs the full module flow: detect, pick, traverse, and scope CSS.
pub fn run(root: &Path, scanned_files: &[FileInfo], config: &Config) -> Result<ModuleRun> {
    let graph = graph::build(scanned_files);
    let candidates = detect::detect(root, scanned_files, &graph, &config.module);
    let entry = picker::pick_entry(root, candidates)?.context("No module entry points detected")?;
    let entry_basename =
        entry.file_stem().and_then(|stem| stem.to_str()).unwrap_or("module").to_string();
    let reachable = graph::traverse(&graph, &entry);
    let depths = graph::depths(&graph, &entry);
    let mut reachable_set: HashSet<PathBuf> = reachable.iter().cloned().collect();

    let css_files = configured_or_detected_css(root, scanned_files, config);
    let classnames = css_scope::extract_classnames(&reachable);
    let mut overrides = HashMap::new();
    let mut css_lines = Vec::new();
    for css in css_files {
        let scoped = css_scope::scope_css(&css, &classnames);
        if !scoped.trim().is_empty() {
            let included = css_scope::count_rules_from_text(&scoped);
            let total = css_scope::count_rules(&css);
            css_lines.push(format!(
                "# CSS: {} (scoped — {} of {} rules included)",
                rel(root, &css),
                included,
                total
            ));
            overrides.insert(css.clone(), scoped);
            reachable_set.insert(css);
        }
    }

    let by_path: HashMap<PathBuf, &FileInfo> = scanned_files
        .iter()
        .map(|f| (f.path.canonicalize().unwrap_or_else(|_| f.path.clone()), f))
        .collect();
    let mut files: Vec<FileInfo> =
        reachable_set.iter().filter_map(|p| by_path.get(p).map(|f| (*f).clone())).collect();
    files.sort_by(|a, b| {
        let da = depths.get(&canon(&a.path)).copied().unwrap_or(usize::MAX - 1);
        let db = depths.get(&canon(&b.path)).copied().unwrap_or(usize::MAX - 1);
        da.cmp(&db).then_with(|| a.relative_path.cmp(&b.relative_path))
    });
    for file in &mut files {
        let depth = depths.get(&canon(&file.path)).copied().unwrap_or(10);
        file.priority = (1.0 - (depth as f64 * 0.1)).max(0.1);
        if depth == 0 {
            file.tags.insert("entrypoint".to_string());
        }
    }

    let direct = graph.edges.get(&canon(&entry)).map(|v| v.len()).unwrap_or(0);
    let transitive = reachable.len().saturating_sub(1 + direct);
    let max_depth = depths.values().copied().max().unwrap_or(0);
    let mut header = format!(
        "# Module: {}\n# Dependencies: {} files ({} direct, {} transitive)\n",
        rel(root, &entry),
        reachable.len(),
        direct,
        transitive
    );
    if css_lines.is_empty() {
        header.push_str("# CSS: none\n");
    } else {
        for line in css_lines {
            header.push_str(&line);
            header.push('\n');
        }
    }
    header.push_str(&format!("# Max depth: {}\n\n", max_depth));

    Ok(ModuleRun { files, content_overrides: overrides, header, entry_basename })
}

fn configured_or_detected_css(root: &Path, files: &[FileInfo], config: &Config) -> Vec<PathBuf> {
    if config.module.css_files.is_empty() {
        css_scope::detect_css_files(root, files)
    } else {
        config.module.css_files.iter().map(|p| canon(&root.join(p))).collect()
    }
}

fn canon(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

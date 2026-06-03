//! Focused export mode orchestration.
//!
//! Supports two scan paths:
//! - **Focused** (new): small repos show files, large repos show modules.
//!   File focus = selected file + callers + tests + entry path.
//!   Module focus = entry + dependency graph traversal.
//! - **Module** (legacy): entry detection + picker + graph traversal.

pub mod css_scope;
pub mod detect;
pub mod focus;
pub mod focus_picker;
pub mod graph;
pub mod picker;

use crate::domain::{Config, FileInfo};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub use focus::{FocusKind, FocusScope, InclusionReason};
pub use focus_picker::FocusAction;

// ── Legacy ModuleRun ──

/// Result of the module-mode selection and dependency walk.
/// Kept for backward compatibility; new code should use `FocusScope`.
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
    /// Focus scope metadata for the report (new focused mode only).
    pub focus_scope: Option<FocusScope>,
}

/// Runs the legacy module flow: detect, pick, traverse, and scope CSS.
/// Kept for backward compatibility; new code should use `run_focused`.
#[allow(dead_code)]
pub fn run(root: &Path, scanned_files: &[FileInfo], config: &Config) -> Result<ModuleRun> {
    let g = graph::build(scanned_files);
    let candidates = detect::detect(root, scanned_files, &g, &config.module);
    let entry = picker::pick_entry(root, candidates)?.context("No module entry points detected")?;
    let entry_basename =
        entry.file_stem().and_then(|stem| stem.to_str()).unwrap_or("module").to_string();
    let reachable = graph::traverse(&g, &entry);
    let depths = graph::depths(&g, &entry);
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

    let by_path: HashMap<PathBuf, &FileInfo> =
        scanned_files.iter().map(|f| (canon(&f.path), f)).collect();
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

    let direct = g.edges.get(&canon(&entry)).map(|v| v.len()).unwrap_or(0);
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

    Ok(ModuleRun { files, content_overrides: overrides, header, entry_basename, focus_scope: None })
}

// ── New focused export flow ──

/// Builds a `ModuleRun` from a `FocusScope` so the existing render pipeline
/// can consume it unchanged.
pub fn module_run_from_scope(
    root: &Path,
    scope: &FocusScope,
    scanned_files: &[FileInfo],
    config: &Config,
) -> ModuleRun {
    let entry_basename =
        scope.selected.file_stem().and_then(|stem| stem.to_str()).unwrap_or("focus").to_string();

    let mut files: Vec<FileInfo> = scope.files.iter().map(|(f, _)| f.clone()).collect();

    // Apply CSS scoping for JS/TSX focused exports.
    let by_path: HashMap<PathBuf, &FileInfo> =
        scanned_files.iter().map(|f| (canon(&f.path), f)).collect();

    let reachable_paths: Vec<PathBuf> = files.iter().map(|f| canon(&f.path)).collect();
    let css_files = configured_or_detected_css(root, scanned_files, config);
    let classnames = css_scope::extract_classnames(&reachable_paths);
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
            // Include the scoped CSS file.
            if let Some(fi) = by_path.get(&css) {
                files.push((*fi).clone());
            }
        }
    }

    // Build header.
    let kind_label = match scope.kind {
        FocusKind::File => "File focus",
        FocusKind::Module => "Module",
    };
    let mut header = format!(
        "# {kind_label}: {}\n# Files: {} (selected + callers + deps)\n",
        rel(root, &scope.selected),
        scope.files.len()
    );
    if css_lines.is_empty() {
        header.push_str("# CSS: none\n");
    } else {
        for line in css_lines {
            header.push_str(&line);
            header.push('\n');
        }
    }

    // Check for fallback usage.
    if scope.files.iter().any(|(_, r)| matches!(r, InclusionReason::CrateFallback)) {
        header.push_str(
            "# ⚠ Rust module graph found few dependencies; included via crate-root fallback.\n",
        );
    }

    header.push('\n');

    ModuleRun {
        files,
        content_overrides: overrides,
        header,
        entry_basename,
        focus_scope: Some(scope.clone()),
    }
}

/// Runs the full interactive focused export flow.
///
/// Returns `Some(ModuleRun)` if the user chooses to export, `None` if cancelled.
pub fn run_focused(
    root: &Path,
    scanned_files: &[FileInfo],
    config: &Config,
) -> Result<Option<ModuleRun>> {
    let g = graph::build(scanned_files);
    let candidates = focus::discover_candidates(root, scanned_files, &g);

    loop {
        let candidate = match focus_picker::pick_focus(&candidates)? {
            Some(c) => c,
            None => return Ok(None),
        };

        let scope = focus::build_scope(root, scanned_files, &g, &candidate.path);

        match focus_picker::preview_and_confirm(&scope, root)? {
            FocusAction::Export => {
                return Ok(Some(module_run_from_scope(root, &scope, scanned_files, config)));
            }
            FocusAction::FullContext => return Ok(None),
            FocusAction::Cancel => return Ok(None),
            FocusAction::ChangeFocus => continue,
        }
    }
}

/// Non-interactive focused export with a pre-selected path.
pub fn run_focused_with_file(
    root: &Path,
    scanned_files: &[FileInfo],
    config: &Config,
    selected_path: &Path,
) -> Result<ModuleRun> {
    let g = graph::build(scanned_files);
    let scope = focus::build_scope(root, scanned_files, &g, selected_path);
    Ok(module_run_from_scope(root, &scope, scanned_files, config))
}

// ── Shared helpers ──

fn configured_or_detected_css(root: &Path, files: &[FileInfo], config: &Config) -> Vec<PathBuf> {
    if config.module.css_files.is_empty() {
        css_scope::detect_css_files(root, files)
    } else {
        config.module.css_files.iter().map(|p| canon(&root.join(p))).collect()
    }
}

/// Canonicalize a path, falling back to the path itself.
pub fn canon(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Display a path relative to a root.
pub fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// Public alias for `rel` used by the picker and tests.
pub fn display_rel(root: &Path, path: &Path) -> String {
    rel(root, path)
}

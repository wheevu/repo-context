//! Focused export mode orchestration.
//!
//! File focus = selected file + callers + tests + entry path.
//! Module focus = entry + dependency graph traversal.

pub mod css_scope;
pub mod focus;
pub mod focus_picker;
pub mod graph;

use crate::domain::{Config, FileInfo};
use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use focus::{FocusKind, FocusScope, InclusionReason};
pub use focus_picker::FocusAction;

/// Result of the focus selection and dependency walk.
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
    /// Focus scope metadata for the report.
    pub focus_scope: Option<FocusScope>,
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

/// Returned by `run_focused` to distinguish the user's intent.
#[derive(Debug)]
pub enum FocusResult {
    /// User chose to export with the selected scope.
    Export(ModuleRun),
    /// User chose full-context export instead.
    FullContext,
    /// User cancelled the export entirely.
    Cancelled,
}

/// Runs the full interactive focused export flow.
pub fn run_focused(
    root: &Path,
    scanned_files: &[FileInfo],
    config: &Config,
) -> Result<FocusResult> {
    let g = graph::build(scanned_files);
    let candidates = focus::discover_candidates(root, scanned_files, &g);
    if candidates.is_empty() {
        return Ok(FocusResult::Cancelled);
    }

    loop {
        let candidate = match focus_picker::pick_focus(&candidates)? {
            Some(c) => c,
            None => return Ok(FocusResult::Cancelled),
        };

        let scope = focus::build_scope(root, scanned_files, &g, &candidate.path);

        match focus_picker::preview_and_confirm(&scope, root)? {
            FocusAction::Export => {
                let module = module_run_from_scope(root, &scope, scanned_files, config);
                return Ok(FocusResult::Export(module));
            }
            FocusAction::FullContext => return Ok(FocusResult::FullContext),
            FocusAction::Cancel => return Ok(FocusResult::Cancelled),
            FocusAction::ChangeFocus => continue,
        }
    }
}

/// Non-interactive focused export with a pre-selected path.
///
/// Returns an error if the focus path does not correspond to any scanned
/// file or a directory containing source files.
pub fn run_focused_with_file(
    root: &Path,
    scanned_files: &[FileInfo],
    config: &Config,
    selected_path: &Path,
) -> Result<ModuleRun> {
    let g = graph::build(scanned_files);
    let scope = focus::build_scope(root, scanned_files, &g, selected_path);
    if scope.files.is_empty() {
        anyhow::bail!("Focus path '{}' matched no scanned files", selected_path.display());
    }
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

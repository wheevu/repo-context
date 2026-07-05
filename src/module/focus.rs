//! Focused export mode — file-level and module-level scoping.
//!
//! Replaces the old "module mode" entry-picker with a context-sensitive
//! UX: small repos show files, large repos show modules/features.
//!
//! Types in this module are consumed primarily by the binary crate via
//! `module::run_focused`, so some appear unused to lib-level analysis.

#![allow(dead_code)]

use crate::domain::FileInfo;
use crate::module::graph::{self, ImportGraph};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// ── Repo-size thresholds ──

/// Maximum number of scannable source files before presenting modules
/// instead of individual files.
const SMALL_REPO_FILE_LIMIT: usize = 45;

// ── Focus candidate types ──

/// Kinds of focus the user can select for a focused export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusKind {
    /// A single source file; scope = file + callers + tests + entry path.
    File,
    /// A module/directory/crate entry; scope = entry + dependency graph.
    Module,
}

/// How each file was included in the focused export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InclusionReason {
    /// The file the user selected as the focus target.
    Selected,
    /// Imported by the selected file (outbound dependency).
    OutboundDependency,
    /// File that imports the selected file (caller).
    Caller,
    /// Test file related to the selected file or its callers.
    RelatedTest,
    /// Nearest entry path (e.g. src/main.rs) for context.
    EntryPath,
    /// Included via crate-root fallback when graph traversal found nothing.
    CrateFallback,
    /// Runtime module in the dependency graph of a crate root.
    RuntimeModule,
    /// Scoped CSS file included for JS/TSX projects.
    CssScope,
}

/// Metadata about why each file is in the focused scope.
#[derive(Debug, Clone)]
pub struct FocusScope {
    /// The thing the user selected (file or module entry).
    pub selected: PathBuf,
    /// Kind of focus (File or Module).
    pub kind: FocusKind,
    /// Whether the repo was presented as files or modules.
    #[allow(dead_code)]
    pub presentation: Presentation,
    /// Files to include, with why.
    pub files: Vec<(FileInfo, InclusionReason)>,
    /// Number of source files in the repo (used for the heuristic).
    pub repo_source_file_count: usize,
}

/// Whether the picker showed files or modules/features.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presentation {
    /// Show individual source files (small repos).
    Files,
    /// Show module/feature groups (large repos).
    Modules,
}

/// Encodes a focus candidate for the picker.
#[derive(Debug, Clone)]
pub struct FocusCandidate {
    /// Absolute path to the file or module entry.
    pub path: PathBuf,
    /// Human-readable label shown in the picker.
    pub display: String,
    /// Extra metadata shown in the picker (e.g. "imports 11 files").
    pub detail: String,
    /// Whether this is a file or module focus.
    pub kind: FocusKind,
}

// ── Main entry point ──

/// Builds the focus scope from the user's selection.
///
/// If `selected` is a directory, it collects all source files under it.
/// If it is a Rust crate root (src/main.rs, src/lib.rs, src/bin/*.rs),
/// expands as a module (dependency graph). Otherwise, expands as a file focus
/// (selected + callers + tests + entry path).
pub fn build_scope(
    root: &Path,
    scanned_files: &[FileInfo],
    graph: &ImportGraph,
    selected: &Path,
) -> FocusScope {
    let source_count = count_source_files(scanned_files);
    let presentation = if source_count <= SMALL_REPO_FILE_LIMIT {
        Presentation::Files
    } else {
        Presentation::Modules
    };

    // Directory candidates (e.g. JS/TS pages/, routes/) — collect all
    // source files under the directory.
    if selected.is_dir() {
        return build_directory_scope(root, scanned_files, selected, presentation, source_count);
    }

    if graph::is_rust_crate_root(selected, root) {
        build_module_scope(root, scanned_files, graph, selected, presentation, source_count)
    } else {
        build_file_scope(root, scanned_files, graph, selected, presentation, source_count)
    }
}

// ── Candidate discovery ──

/// Builds focus candidates for the picker.
pub fn discover_candidates(
    root: &Path,
    files: &[FileInfo],
    graph: &ImportGraph,
) -> Vec<FocusCandidate> {
    let source_count = count_source_files(files);

    if source_count <= SMALL_REPO_FILE_LIMIT {
        discover_file_candidates(files, graph, root)
    } else {
        discover_module_candidates(root, files, graph)
    }
}

// ── File-level scope ──

fn build_file_scope(
    _root: &Path,
    scanned_files: &[FileInfo],
    graph: &ImportGraph,
    selected: &Path,
    presentation: Presentation,
    source_count: usize,
) -> FocusScope {
    let selected_abs = canon(selected);
    let by_path: HashMap<PathBuf, &FileInfo> =
        scanned_files.iter().map(|f| (canon(&f.path), f)).collect();

    let mut included: HashMap<PathBuf, InclusionReason> = HashMap::new();
    included.insert(selected_abs.clone(), InclusionReason::Selected);

    // 1. Outbound dependencies (what the selected file imports).
    for dep in graph.edges.get(&selected_abs).cloned().unwrap_or_default() {
        if by_path.contains_key(&dep) {
            included.entry(dep).or_insert(InclusionReason::OutboundDependency);
        }
    }

    // 2. Nearest entry path — walk up callers to find the crate root.
    //    Must run before step 3 (callers) so entry paths are labeled correctly.
    let entry = find_entry_path(graph, &selected_abs, &included);
    if let Some(ref entry_path) = entry {
        if !included.contains_key(entry_path) && by_path.contains_key(entry_path) {
            included.insert(entry_path.clone(), InclusionReason::EntryPath);
        }
    }

    // 3. Related tests: find test files that import anything in our scope.
    for file in scanned_files {
        let is_test = is_likely_test_file(file);
        if !is_test {
            continue;
        }
        let abs = canon(&file.path);
        if included.contains_key(&abs) {
            continue;
        }
        // Check if this test file imports anything in our scope.
        if let Some(test_deps) = graph.edges.get(&abs) {
            for dep in test_deps {
                if included.contains_key(dep) {
                    included.insert(abs.clone(), InclusionReason::RelatedTest);
                    break;
                }
            }
        }
    }

    // 4. Remaining direct callers — files that import the selected file
    //    but haven't been included by steps 1–3.
    for caller in graph::direct_callers(graph, &selected_abs) {
        if by_path.contains_key(&caller) && caller != selected_abs {
            included.entry(caller).or_insert(InclusionReason::Caller);
        }
    }

    // If entry path was already included as a caller in step 4, upgrade it.
    // This handles the case where a crate root directly imports the selected file.
    if let Some(ref entry_path) = entry {
        included.entry(entry_path.clone()).and_modify(|r| {
            if matches!(r, InclusionReason::Caller) {
                *r = InclusionReason::EntryPath;
            }
        });
    }

    // Build the ordered file list.
    let files: Vec<(FileInfo, InclusionReason)> = included
        .iter()
        .filter_map(|(p, reason)| by_path.get(p).map(|f| ((*f).clone(), reason.clone())))
        .collect();

    FocusScope {
        selected: selected_abs,
        kind: FocusKind::File,
        presentation,
        files,
        repo_source_file_count: source_count,
    }
}

// ── Module-level scope ──

fn build_module_scope(
    root: &Path,
    scanned_files: &[FileInfo],
    graph: &ImportGraph,
    entry: &Path,
    presentation: Presentation,
    source_count: usize,
) -> FocusScope {
    let entry_abs = canon(entry);
    let reachable = graph::traverse(graph, &entry_abs);

    let by_path: HashMap<PathBuf, &FileInfo> =
        scanned_files.iter().map(|f| (canon(&f.path), f)).collect();

    let mut included: HashMap<PathBuf, InclusionReason> = HashMap::new();
    included.insert(entry_abs.clone(), InclusionReason::Selected);

    // Check if graph traversal is empty (no dependencies found).
    let used_fallback =
        reachable.len() <= 1 && graph::is_rust_crate_root(entry, root) && !scanned_files.is_empty();

    if used_fallback {
        // Fallback: include all Rust source files from the crate's src tree.
        for file in scanned_files {
            let abs = canon(&file.path);
            if included.contains_key(&abs) {
                continue;
            }
            if file.relative_path.starts_with("src/") && is_rust_file(file) {
                // Skip obvious test-only files.
                if is_likely_test_file(file) {
                    continue;
                }
                included.insert(abs, InclusionReason::CrateFallback);
            }
        }
    } else {
        // Normal module graph mode.
        for dep in reachable {
            included.entry(dep).or_insert(InclusionReason::RuntimeModule);
        }
    }

    let mut files: Vec<(FileInfo, InclusionReason)> = included
        .iter()
        .filter_map(|(p, reason)| by_path.get(p).map(|f| ((*f).clone(), reason.clone())))
        .collect();

    // Sort by depth then path.
    let depths = graph::depths(graph, &entry_abs);
    files.sort_by(|a, b| {
        let da = depths.get(&canon(&a.0.path)).copied().unwrap_or(usize::MAX);
        let db = depths.get(&canon(&b.0.path)).copied().unwrap_or(usize::MAX);
        da.cmp(&db).then_with(|| a.0.relative_path.cmp(&b.0.relative_path))
    });

    // Tag entrypoint and adjust priorities.
    for (file, _) in &mut files {
        let depth = depths.get(&canon(&file.path)).copied().unwrap_or(10);
        file.priority = (1.0 - (depth as f64 * 0.1)).max(0.1);
        if depth == 0 {
            file.tags.insert("entrypoint".to_string());
        }
    }

    FocusScope {
        selected: entry_abs,
        kind: FocusKind::Module,
        presentation,
        files,
        repo_source_file_count: source_count,
    }
}

// ── Directory-level scope ──

/// Builds a scope for a directory candidate (e.g. JS/TS `pages/`, `routes/`).
/// Collects all source files under the directory.
fn build_directory_scope(
    _root: &Path,
    scanned_files: &[FileInfo],
    selected_dir: &Path,
    presentation: Presentation,
    source_count: usize,
) -> FocusScope {
    let selected_abs = canon(selected_dir);
    let mut files: Vec<(FileInfo, InclusionReason)> = scanned_files
        .iter()
        .filter(|f| {
            let abs = canon(&f.path);
            abs.starts_with(&selected_abs) && is_source_file(f)
        })
        .cloned()
        .map(|f| (f, InclusionReason::RuntimeModule))
        .collect();

    // Tag likely entrypoints (index.*, main.*) with higher priority.
    for (file, _) in &mut files {
        file.priority = 0.9;
        let name = file.path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
        if name.starts_with("index.") || name.starts_with("main.") || name.starts_with("app.") {
            file.tags.insert("entrypoint".to_string());
        }
    }

    FocusScope {
        selected: selected_abs,
        kind: FocusKind::Module,
        presentation,
        files,
        repo_source_file_count: source_count,
    }
}

// ── Candidate discovery helpers ──

fn discover_file_candidates(
    files: &[FileInfo],
    graph: &ImportGraph,
    root: &Path,
) -> Vec<FocusCandidate> {
    let source_files: Vec<&FileInfo> = files.iter().filter(|f| is_source_file(f)).collect();

    source_files
        .iter()
        .map(|f| {
            let abs = canon(&f.path);
            let import_count = graph.edges.get(&abs).map(|v| v.len()).unwrap_or(0);
            let caller_count = graph::direct_callers(graph, &abs).len();
            let detail = if calls_is_crate_root(f, graph) {
                format!("entrypoint, imports {} files", import_count)
            } else if caller_count > 0 {
                format!("imports {}, used by {}", import_count, caller_count)
            } else {
                format!("imports {} files", import_count)
            };
            FocusCandidate {
                path: f.path.clone(),
                display: rel(root, &f.path),
                detail,
                kind: FocusKind::File,
            }
        })
        .collect()
}

fn discover_module_candidates(
    root: &Path,
    files: &[FileInfo],
    graph: &ImportGraph,
) -> Vec<FocusCandidate> {
    let mut candidates = Vec::new();

    // 1. Rust crate roots.
    for root_path in graph::rust_crate_roots(root, files) {
        let reachable = graph::traverse(graph, &root_path);
        let count = reachable.len().saturating_sub(1);
        candidates.push(FocusCandidate {
            path: root_path.clone(),
            display: format!("Rust crate: {}", rel(root, &root_path)),
            detail: format!("{} reachable files", count),
            kind: FocusKind::Module,
        });
    }

    // 2. JS/TS route/page directories.
    let entry_dirs: &[&str] =
        &["pages", "routes", "views", "screens", "cmd", "handlers", "controllers"];
    for dir_name in entry_dirs {
        let dir_files: Vec<&FileInfo> = files
            .iter()
            .filter(|f| {
                let path = Path::new(&f.relative_path);
                path.parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .map(|n| n.eq_ignore_ascii_case(dir_name))
                    .unwrap_or(false)
            })
            .collect();
        if dir_files.len() >= 2 {
            let total = dir_files.len();
            candidates.push(FocusCandidate {
                path: root.join(format!("src/{dir_name}")),
                display: format!("{dir_name}/ ({total} files)"),
                detail: format!("{total} route/page files"),
                kind: FocusKind::Module,
            });
        }
    }

    // 3. Topology-based entries (files with no incoming edges).
    for (path, file) in &graph.files {
        let incoming = graph.incoming.get(path).copied().unwrap_or(0);
        if incoming == 0 && is_source_file(file) && !is_entry_pattern_file(file) {
            // Only emit if not already covered by crate roots.
            if !candidates.iter().any(|c| canon(&c.path) == *path) {
                let reachable = graph::traverse(graph, path);
                let count = reachable.len().saturating_sub(1);
                if count > 0 {
                    candidates.push(FocusCandidate {
                        path: path.clone(),
                        display: rel(root, path),
                        detail: format!("{} reachable files", count),
                        kind: FocusKind::Module,
                    });
                }
            }
        }
    }

    // If still no candidates or too few, fall back to file candidates.
    if candidates.is_empty() {
        return discover_file_candidates(files, graph, root);
    }

    candidates.sort_by(|a, b| a.display.cmp(&b.display));
    candidates.dedup_by(|a, b| a.path == b.path);
    candidates
}

// ── Helpers ──

fn count_source_files(files: &[FileInfo]) -> usize {
    files.iter().filter(|f| is_source_file(f)).count()
}

fn is_source_file(file: &FileInfo) -> bool {
    let ext = file.extension.to_ascii_lowercase();
    matches!(
        ext.as_str(),
        ".rs"
            | ".py"
            | ".js"
            | ".jsx"
            | ".ts"
            | ".tsx"
            | ".go"
            | ".java"
            | ".kt"
            | ".kts"
            | ".c"
            | ".cpp"
            | ".cc"
            | ".cxx"
            | ".h"
            | ".hpp"
            | ".cs"
            | ".rb"
            | ".swift"
            | ".scala"
            | ".vue"
            | ".svelte"
            | "rs"
            | "py"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "go"
    )
}

fn is_rust_file(file: &FileInfo) -> bool {
    let ext = file.extension.to_ascii_lowercase();
    ext == ".rs" || ext == "rs"
}

fn is_likely_test_file(file: &FileInfo) -> bool {
    let name = file.path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
    name.contains("test")
        || name.contains("spec")
        || file.relative_path.contains("/test/")
        || file.relative_path.contains("/tests/")
        || file.relative_path.contains("/spec/")
}

fn is_entry_pattern_file(file: &FileInfo) -> bool {
    let name = file.path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
    let stem = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(&name);
    stem.starts_with("entry-") || stem.ends_with("-entry")
}

/// Walk up the call chain from `target` to find a file with no callers
/// (an entry point), stopping if we loop or exceed a reasonable depth.
/// Skips test files since they are dead-end leaf nodes, not real entries.
fn find_entry_path(
    graph: &ImportGraph,
    target: &Path,
    visited: &HashMap<PathBuf, InclusionReason>,
) -> Option<PathBuf> {
    // First check direct callers, skipping obvious test files.
    let callers: Vec<PathBuf> = graph::direct_callers(graph, target)
        .into_iter()
        .filter(|c| !path_looks_like_test(c))
        .collect();
    if callers.is_empty() {
        // target IS the entry (no non-test callers).
        return Some(target.to_path_buf());
    }

    // Walk up (BFS limited) to find the highest caller.
    let mut current_set: HashSet<PathBuf> = callers.iter().cloned().collect();
    let mut seen: HashSet<PathBuf> = visited.keys().cloned().collect();
    seen.insert(target.to_path_buf());
    let mut best = callers.first().cloned();

    // Try up to 5 levels.
    for _ in 0..5 {
        let mut next_set = HashSet::new();
        for caller in &current_set {
            let higher: Vec<PathBuf> = graph::direct_callers(graph, caller)
                .into_iter()
                .filter(|c| !path_looks_like_test(c))
                .collect();
            if higher.is_empty() {
                // This caller has no non-test callers → it's an entry.
                return Some(caller.clone());
            }
            best = Some(caller.clone());
            for h in higher {
                if !seen.contains(&h) {
                    seen.insert(h.clone());
                    next_set.insert(h);
                }
            }
        }
        if next_set.is_empty() {
            break;
        }
        current_set = next_set;
    }

    best
}

/// Quick heuristic: does the path look like a test file?
fn path_looks_like_test(path: &Path) -> bool {
    let s = path.to_string_lossy().to_ascii_lowercase();
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
    name.contains("test") || name.contains("spec") || s.contains("/test/") || s.contains("/tests/")
}

/// Returns whether a file looks like a crate root (main.rs or lib.rs under src/).
fn calls_is_crate_root(file: &FileInfo, graph: &ImportGraph) -> bool {
    let abs = canon(&file.path);
    let callers = graph::direct_callers(graph, &abs);
    // A crate root typically has no callers (nobody imports main.rs).
    callers.is_empty()
}

fn canon(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

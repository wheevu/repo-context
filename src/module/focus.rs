//! Focused export mode — file-level and module-level scoping.
//!
//! Replaces the old "module mode" entry-picker with a context-sensitive
//! UX: small repos show files, large repos show modules/features.
//!
//! Types in this module are consumed primarily by the binary crate via
//! `module::run_focused`, so some appear unused to lib-level analysis.

#![allow(dead_code)]

use crate::domain::FileInfo;
use crate::godot;
use crate::module::graph::{self, ImportGraph};
use crate::utils::read_file_safe;
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

    if graph::is_rust_crate_root(selected, root)
        || godot_main_scene(scanned_files).is_some_and(|path| canon(&path) == canon(selected))
    {
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
        let mut candidates = discover_file_candidates(files, graph, root);
        prioritize_godot_main_scene(root, files, &mut candidates);
        candidates
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
    let mut files: Vec<(FileInfo, InclusionReason)> = included
        .iter()
        .filter_map(|(p, reason)| by_path.get(p).map(|f| ((*f).clone(), reason.clone())))
        .collect();
    sort_scope_files(&mut files);

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
        // Fallback: include all Rust source files from the selected crate's
        // actual `src` tree, not every `src/` directory in the repository.
        let crate_src = rust_crate_source_root(entry);
        for file in scanned_files {
            let abs = canon(&file.path);
            if included.contains_key(&abs) {
                continue;
            }
            if crate_src.as_ref().is_some_and(|src| abs.starts_with(src)) && is_rust_file(file) {
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
    files.sort_by(|a, b| a.0.relative_path.cmp(&b.0.relative_path));

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

    let mut candidates: Vec<FocusCandidate> = source_files
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
        .collect();
    candidates.sort_by_key(candidate_path_key);
    candidates
}

fn discover_module_candidates(
    root: &Path,
    files: &[FileInfo],
    graph: &ImportGraph,
) -> Vec<FocusCandidate> {
    let mut candidates = Vec::new();

    // Godot's configured main scene is the runtime entry point. Its existing
    // `res://` graph reaches attached scripts, resources, and their dependencies.
    let main_scene = godot_main_scene(files);
    if let Some(path) = &main_scene {
        let reachable = graph::traverse(graph, path);
        candidates.push(FocusCandidate {
            path: path.clone(),
            display: format!("Godot main scene: {}", rel(root, path)),
            detail: format!("{} reachable files", reachable.len().saturating_sub(1)),
            kind: FocusKind::Module,
        });
    }

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

    // 2. JS/TS route/page directories. Group by the actual parent path so
    // nested applications and repeated directory names remain distinct.
    let entry_dirs: &[&str] =
        &["pages", "routes", "views", "screens", "cmd", "handlers", "controllers"];
    let mut route_dirs: HashMap<PathBuf, usize> = HashMap::new();
    for file in files {
        let path = Path::new(&file.relative_path);
        let Some(parent) = path.parent() else { continue };
        let Some(name) = parent.file_name().and_then(|name| name.to_str()) else { continue };
        if entry_dirs.iter().any(|entry| name.eq_ignore_ascii_case(entry)) {
            *route_dirs.entry(canon(&root.join(parent))).or_insert(0) += 1;
        }
    }
    let mut route_dirs: Vec<(PathBuf, usize)> = route_dirs.into_iter().collect();
    route_dirs.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (path, total) in route_dirs {
        if total < 2 {
            continue;
        }
        candidates.push(FocusCandidate {
            display: format!("{}/ ({} files)", rel(root, &path), total),
            detail: format!("{total} route/page files"),
            path,
            kind: FocusKind::Module,
        });
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

    candidates.sort_by(|a, b| {
        let a_main = main_scene.as_ref().is_some_and(|path| canon(path) == canon(&a.path));
        let b_main = main_scene.as_ref().is_some_and(|path| canon(path) == canon(&b.path));
        b_main
            .cmp(&a_main)
            .then_with(|| candidate_path_key(a).cmp(&candidate_path_key(b)))
            .then_with(|| a.display.cmp(&b.display))
            .then_with(|| a.detail.cmp(&b.detail))
    });
    candidates.dedup_by(|a, b| canon(&a.path) == canon(&b.path));
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
            | ".gd"
            | ".tscn"
            | ".tres"
            | ".godot"
            | ".gdshader"
            | ".gdshaderinc"
            | "rs"
            | "py"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "go"
            | "svelte"
            | "gd"
            | "tscn"
            | "tres"
            | "godot"
            | "gdshader"
            | "gdshaderinc"
    )
}

fn godot_main_scene(files: &[FileInfo]) -> Option<PathBuf> {
    let project_file = files.iter().find(|file| file.relative_path == "project.godot")?;
    let (content, _) = read_file_safe(&project_file.path, None, None).ok()?;
    let configured = godot::parse_project(&content).main_scene?;
    let relative = configured.strip_prefix("res://")?.replace('\\', "/");
    files.iter().find(|file| file.relative_path == relative).map(|file| file.path.clone())
}

fn prioritize_godot_main_scene(
    root: &Path,
    files: &[FileInfo],
    candidates: &mut Vec<FocusCandidate>,
) {
    let Some(main_scene) = godot_main_scene(files) else { return };
    let Some(index) =
        candidates.iter().position(|candidate| canon(&candidate.path) == canon(&main_scene))
    else {
        return;
    };
    let mut candidate = candidates.remove(index);
    candidate.display = format!("Godot main scene: {}", rel(root, &candidate.path));
    candidate.detail = format!("{}, configured entrypoint", candidate.detail);
    candidate.kind = FocusKind::Module;
    candidates.insert(0, candidate);
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
    let mut callers: Vec<PathBuf> = graph::direct_callers(graph, target)
        .into_iter()
        .filter(|c| !path_looks_like_test(c))
        .collect();
    callers.sort();
    callers.dedup();
    if callers.is_empty() {
        // target IS the entry (no non-test callers).
        return Some(target.to_path_buf());
    }

    // Walk up (BFS limited) to find the highest caller.
    let mut current_set = callers;
    let mut seen: HashSet<PathBuf> = visited.keys().cloned().collect();
    seen.insert(target.to_path_buf());
    let mut best = current_set.first().cloned();

    // Try up to 5 levels.
    for _ in 0..5 {
        let mut next_set = HashSet::new();
        for caller in &current_set {
            let mut higher: Vec<PathBuf> = graph::direct_callers(graph, caller)
                .into_iter()
                .filter(|c| !path_looks_like_test(c))
                .collect();
            higher.sort();
            higher.dedup();
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
        current_set = next_set.into_iter().collect();
        current_set.sort();
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

fn sort_scope_files(files: &mut [(FileInfo, InclusionReason)]) {
    files.sort_by(|(a_file, a_reason), (b_file, b_reason)| {
        inclusion_reason_order(a_reason)
            .cmp(&inclusion_reason_order(b_reason))
            .then_with(|| a_file.relative_path.cmp(&b_file.relative_path))
            .then_with(|| a_file.path.cmp(&b_file.path))
    });
}

fn inclusion_reason_order(reason: &InclusionReason) -> u8 {
    match reason {
        InclusionReason::Selected => 0,
        InclusionReason::EntryPath => 1,
        InclusionReason::OutboundDependency => 2,
        InclusionReason::Caller => 3,
        InclusionReason::RelatedTest => 4,
        InclusionReason::CrateFallback => 5,
        InclusionReason::RuntimeModule => 6,
        InclusionReason::CssScope => 7,
    }
}

fn candidate_path_key(candidate: &FocusCandidate) -> PathBuf {
    canon(&candidate.path)
}

fn rust_crate_source_root(entry: &Path) -> Option<PathBuf> {
    entry
        .ancestors()
        .find(|path| path.file_name().and_then(|name| name.to_str()) == Some("src"))
        .map(canon)
}

fn canon(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn rel(root: &Path, path: &Path) -> String {
    let root = canon(root);
    let path = canon(path);
    path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn godot_main_scene_is_the_first_focus_candidate() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path();
        fs::create_dir_all(root.join("scripts")).expect("mkdir scripts");
        fs::write(
            root.join("project.godot"),
            "config_version=5\n[application]\nrun/main_scene=\"res://main.tscn\"\n",
        )
        .expect("write project");
        fs::write(
            root.join("main.tscn"),
            "[ext_resource path=\"res://scripts/player.gd\" type=\"Script\" id=\"1\"]\n",
        )
        .expect("write scene");
        fs::write(root.join("scripts/player.gd"), "extends Node\n").expect("write script");
        let files = vec![
            test_file(root, "project.godot"),
            test_file(root, "main.tscn"),
            test_file(root, "scripts/player.gd"),
        ];
        let graph = graph::build(&files);

        let candidates = discover_candidates(root, &files, &graph);

        assert_eq!(canon(&candidates[0].path), canon(&root.join("main.tscn")));
        assert_eq!(candidates[0].kind, FocusKind::Module);
    }

    #[test]
    fn godot_only_repository_without_main_scene_has_file_candidates() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path();
        fs::write(root.join("player.gd"), "extends Node\n").expect("write script");
        let files = vec![test_file(root, "player.gd")];
        let graph = graph::build(&files);

        let candidates = discover_candidates(root, &files, &graph);

        assert_eq!(candidates.len(), 1);
        assert_eq!(canon(&candidates[0].path), canon(&root.join("player.gd")));
    }

    #[test]
    fn nested_rust_fallback_stays_within_selected_crate() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path();
        fs::create_dir_all(root.join("crates/alpha/src")).expect("mkdir alpha");
        fs::create_dir_all(root.join("crates/beta/src")).expect("mkdir beta");

        let alpha_main = root.join("crates/alpha/src/main.rs");
        let alpha_feature = root.join("crates/alpha/src/feature.rs");
        let beta_main = root.join("crates/beta/src/main.rs");
        fs::write(&alpha_main, "fn main() {}\n").expect("write alpha main");
        fs::write(&alpha_feature, "pub fn feature() {}\n").expect("write alpha feature");
        fs::write(&beta_main, "fn main() {}\n").expect("write beta main");

        let files = vec![
            test_file(root, "crates/beta/src/main.rs"),
            test_file(root, "crates/alpha/src/feature.rs"),
            test_file(root, "crates/alpha/src/main.rs"),
        ];
        let graph = graph::build(&files);
        let scope = build_scope(root, &files, &graph, &alpha_main);

        let included: Vec<PathBuf> =
            scope.files.iter().map(|(file, _)| canon(&file.path)).collect();
        assert!(included.contains(&canon(&alpha_main)));
        assert!(included.contains(&canon(&alpha_feature)));
        assert!(!included.contains(&canon(&beta_main)));
    }

    #[test]
    fn route_candidates_use_real_nested_directories_and_stable_order() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path();
        for directory in ["apps/web/pages", "packages/site/pages"] {
            fs::create_dir_all(root.join(directory)).expect("mkdir route directory");
        }
        let relative_paths = [
            "packages/site/pages/Home.ts",
            "apps/web/pages/About.ts",
            "packages/site/pages/About.ts",
            "apps/web/pages/Home.ts",
        ];
        for relative in relative_paths {
            fs::write(root.join(relative), "export const page = true;\n").expect("write page");
        }
        let files: Vec<FileInfo> =
            relative_paths.iter().map(|path| test_file(root, path)).collect();
        let reversed: Vec<FileInfo> = files.iter().cloned().rev().collect();
        let graph = graph::build(&files);
        let reversed_graph = graph::build(&reversed);

        let candidates = discover_module_candidates(root, &files, &graph);
        let reversed_candidates = discover_module_candidates(root, &reversed, &reversed_graph);
        let paths: Vec<PathBuf> =
            candidates.iter().map(|candidate| canon(&candidate.path)).collect();
        let reversed_paths: Vec<PathBuf> =
            reversed_candidates.iter().map(|candidate| canon(&candidate.path)).collect();

        assert_eq!(paths, reversed_paths);
        assert_eq!(
            paths,
            vec![canon(&root.join("apps/web/pages")), canon(&root.join("packages/site/pages"))]
        );
        assert_eq!(candidates[0].display, "apps/web/pages/ (2 files)");
        assert_eq!(candidates[1].display, "packages/site/pages/ (2 files)");
    }

    #[test]
    fn file_scope_order_is_stable_across_input_order() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path();
        fs::create_dir_all(root.join("src")).expect("mkdir src");
        fs::create_dir_all(root.join("tests")).expect("mkdir tests");
        fs::write(root.join("src/main.ts"), "import './selected';\n").expect("write main");
        fs::write(root.join("src/selected.ts"), "import './dependency';\n")
            .expect("write selected");
        fs::write(root.join("src/dependency.ts"), "export const value = 1;\n")
            .expect("write dependency");
        fs::write(root.join("tests/selected.test.ts"), "import '../src/selected';\n")
            .expect("write test");

        let files = vec![
            test_file(root, "tests/selected.test.ts"),
            test_file(root, "src/dependency.ts"),
            test_file(root, "src/main.ts"),
            test_file(root, "src/selected.ts"),
        ];
        let reversed: Vec<FileInfo> = files.iter().cloned().rev().collect();
        let graph = graph::build(&files);
        let reversed_graph = graph::build(&reversed);
        let selected = root.join("src/selected.ts");

        let scope = build_scope(root, &files, &graph, &selected);
        let reversed_scope = build_scope(root, &reversed, &reversed_graph, &selected);
        let paths: Vec<PathBuf> = scope.files.iter().map(|(file, _)| file.path.clone()).collect();
        let reversed_paths: Vec<PathBuf> =
            reversed_scope.files.iter().map(|(file, _)| file.path.clone()).collect();

        assert_eq!(paths, reversed_paths);
        assert_eq!(scope.files[0].0.path, selected);
    }

    fn test_file(root: &Path, relative_path: &str) -> FileInfo {
        let path = root.join(relative_path);
        FileInfo {
            path: path.clone(),
            relative_path: relative_path.to_string(),
            size_bytes: 0,
            extension: path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| format!(".{extension}"))
                .unwrap_or_default(),
            language: String::new(),
            id: String::new(),
            priority: 0.0,
            token_estimate: 0,
            tags: Default::default(),
            is_readme: false,
            is_config: false,
            is_doc: false,
        }
    }
}

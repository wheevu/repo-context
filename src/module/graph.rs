//! Static import graph construction for module mode.

use crate::domain::FileInfo;
use crate::utils::read_file_safe;
use regex::Regex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

/// Alias matching the module-mode specification.
pub type ScannedFile = FileInfo;

/// Directed graph where edges point from importer to imported file.
#[derive(Debug, Clone, Default)]
pub struct ImportGraph {
    /// All known files by absolute path.
    pub files: HashMap<PathBuf, FileInfo>,
    /// Import edges by absolute source path.
    pub edges: HashMap<PathBuf, Vec<PathBuf>>,
    /// Reverse import counts by absolute target path.
    pub incoming: HashMap<PathBuf, usize>,
    /// Reverse edges: file → files that import it (callers).
    pub reverse: HashMap<PathBuf, Vec<PathBuf>>,
}

/// Builds a static import graph over scanned files.
#[must_use]
pub fn build(files: &[ScannedFile]) -> ImportGraph {
    let by_path: HashMap<PathBuf, FileInfo> =
        files.iter().map(|f| (normalize_abs(&f.path), f.clone())).collect();
    let rel_to_abs: HashMap<String, PathBuf> = files
        .iter()
        .map(|f| (f.relative_path.replace('\\', "/"), normalize_abs(&f.path)))
        .collect();

    let mut graph = ImportGraph { files: by_path.clone(), ..ImportGraph::default() };

    for file in files {
        let source = normalize_abs(&file.path);
        let Ok((content, _)) = read_file_safe(&file.path, None, None) else { continue };
        let deps = dedup(imports_for(file, &content, &by_path, &rel_to_abs));
        for dep in deps {
            *graph.incoming.entry(dep.clone()).or_insert(0) += 1;
            graph.edges.entry(source.clone()).or_default().push(dep.clone());
            graph.reverse.entry(dep).or_default().push(source.clone());
        }
        graph.edges.entry(source).or_default();
    }

    graph
}

/// Breadth-first traversal returning entry plus all transitive dependencies.
/// Also returns all reverse callers reachable from the target (who imports it).
#[must_use]
pub fn traverse(graph: &ImportGraph, entry: &Path) -> Vec<PathBuf> {
    let start = normalize_abs(entry);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut queued = HashSet::from([start.clone()]);
    let mut queue = VecDeque::from([start]);

    while let Some(path) = queue.pop_front() {
        if !seen.insert(path.clone()) {
            continue;
        }
        out.push(path.clone());
        if let Some(next) = graph.edges.get(&path) {
            let mut sorted = next.clone();
            sorted.sort();
            for dep in sorted {
                if graph.files.contains_key(&dep)
                    && !seen.contains(&dep)
                    && queued.insert(dep.clone())
                {
                    queue.push_back(dep);
                }
            }
        }
    }
    out
}

/// Breadth-first reverse traversal: returns all files that directly or
/// transitively import the given file (its callers).
#[allow(dead_code)]
#[must_use]
pub fn reverse_reachable(graph: &ImportGraph, target: &Path) -> Vec<PathBuf> {
    let start = normalize_abs(target);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    if let Some(callers) = graph.reverse.get(&start) {
        for caller in callers {
            queue.push_back(caller.clone());
        }
    }

    while let Some(path) = queue.pop_front() {
        if !seen.insert(path.clone()) {
            continue;
        }
        out.push(path.clone());
        if let Some(callers) = graph.reverse.get(&path) {
            for caller in callers {
                if !seen.contains(caller) {
                    queue.push_back(caller.clone());
                }
            }
        }
    }
    out
}

/// Returns direct callers of a file (one hop reverse).
#[must_use]
pub fn direct_callers(graph: &ImportGraph, target: &Path) -> Vec<PathBuf> {
    let start = normalize_abs(target);
    graph.reverse.get(&start).cloned().unwrap_or_default()
}

/// Detects Rust crate-root candidates from scanned files.
///
/// Returns absolute paths to `src/main.rs`, `src/lib.rs`, and `src/bin/*.rs`.
#[must_use]
pub fn rust_crate_roots(root: &Path, files: &[ScannedFile]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let by_rel: HashMap<&str, &PathBuf> =
        files.iter().map(|f| (f.relative_path.as_str(), &f.path)).collect();

    for candidate in &["src/main.rs", "src/lib.rs"] {
        if let Some(path) = by_rel.get(*candidate) {
            roots.push(normalize_abs(path));
        }
    }

    // Also detect src/bin/*.rs for multi-binary crates.
    let bin_rs = files.iter().filter(|f| {
        f.relative_path.starts_with("src/bin/")
            && f.extension.to_ascii_lowercase().as_str() == ".rs"
            || f.extension.eq_ignore_ascii_case("rs")
    });
    for file in bin_rs {
        roots.push(normalize_abs(&file.path));
    }

    // If no explicit crate root found, check Cargo.toml for [[bin]] entries.
    if roots.is_empty() {
        let cargo_toml = root.join("Cargo.toml");
        if let Ok((content, _)) = read_file_safe(&cargo_toml, None, None) {
            if let Ok(value) = toml::from_str::<toml::Value>(&content) {
                if let Some(bins) = value.get("bin").and_then(|b| b.as_array()) {
                    for bin in bins {
                        if let Some(path) = bin.get("path").and_then(|p| p.as_str()) {
                            let abs = normalize_abs(&root.join(path));
                            if files.iter().any(|f| normalize_abs(&f.path) == abs) {
                                roots.push(abs);
                            }
                        }
                    }
                }
            }
        }
    }

    roots.sort();
    roots.dedup();
    roots
}

/// Returns whether a path looks like a Rust crate root (main.rs or lib.rs at src/).
#[must_use]
pub fn is_rust_crate_root(path: &Path, root: &Path) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/");
    rel == "src/main.rs" || rel == "src/lib.rs" || rel.starts_with("src/bin/")
}

/// Returns shortest import depth for each reachable file.
#[must_use]
pub fn depths(graph: &ImportGraph, entry: &Path) -> HashMap<PathBuf, usize> {
    let start = normalize_abs(entry);
    let mut depths = HashMap::from([(start.clone(), 0)]);
    let mut queue = VecDeque::from([start]);
    while let Some(path) = queue.pop_front() {
        let depth = depths.get(&path).copied().unwrap_or(0);
        if let Some(next) = graph.edges.get(&path) {
            for dep in next {
                if graph.files.contains_key(dep) && !depths.contains_key(dep) {
                    depths.insert(dep.clone(), depth + 1);
                    queue.push_back(dep.clone());
                }
            }
        }
    }
    depths
}

fn imports_for(
    file: &FileInfo,
    content: &str,
    by_path: &HashMap<PathBuf, FileInfo>,
    rel_to_abs: &HashMap<String, PathBuf>,
) -> Vec<PathBuf> {
    let ext = file.extension.to_ascii_lowercase();
    match ext.as_str() {
        ".ts" | ".tsx" | ".js" | ".jsx" | "ts" | "tsx" | "js" | "jsx" => {
            js_imports(&file.path, content, by_path)
        }
        ".rs" | "rs" => rust_imports(&file.path, content, by_path),
        ".go" | "go" => go_imports(content, rel_to_abs),
        _ => Vec::new(),
    }
}

fn js_imports(path: &Path, content: &str, by_path: &HashMap<PathBuf, FileInfo>) -> Vec<PathBuf> {
    let re = Regex::new(r#"(?m)import\s+(?:[^'\"]+?\s+from\s+)?['\"]([^'\"]+)['\"]"#)
        .expect("valid regex");
    re.captures_iter(content)
        .filter_map(|cap| cap.get(1).map(|m| m.as_str()))
        .filter(|spec| spec.starts_with('.'))
        .filter_map(|spec| resolve_relative(path, spec, &[".ts", ".tsx", ".js", ".jsx"], by_path))
        .collect()
}

fn rust_imports(path: &Path, content: &str, by_path: &HashMap<PathBuf, FileInfo>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let src_root = path.ancestors().find(|p| p.file_name().and_then(|n| n.to_str()) == Some("src"));

    // Rust's file-module path conventions:
    //   mod foo;    → foo.rs  or  foo/mod.rs
    //   mod foo { } → children at foo/bar.rs or foo/bar/mod.rs
    // Visibility and cfg guards are respected.
    let mod_re = Regex::new(
        r#"(?m)^\s*(?:pub(?:\s*\(\s*crate\s*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;"#,
    )
    .expect("valid mod regex");
    let path_attr_re = Regex::new(r#"#\[path\s*=\s*"([^"]+)"\]"#).expect("valid path attr regex");

    // Extract #[path] attributes to map custom module file paths.
    let path_attrs: HashMap<&str, &str> = path_attr_re
        .captures_iter(content)
        .filter_map(|cap| {
            // Find the mod declaration that follows this attribute.
            let pos = cap.get(0)?.end();
            let rest = &content[pos..];
            let mod_follow = Regex::new(
                r#"\s*(?:pub(?:\s*\(\s*crate\s*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*[;{]"#,
            )
            .expect("valid mod follow regex");
            mod_follow.captures(rest).map(|m| {
                let name = m.get(1).unwrap().as_str();
                (name, cap.get(1).unwrap().as_str())
            })
        })
        .collect();

    for cap in mod_re.captures_iter(content) {
        if let Some(name) = cap.get(1) {
            let name_str = name.as_str();

            // Skip modules guarded by #[cfg(test)].
            let match_start = cap.get(0).map(|m| m.start()).unwrap_or(0);
            let prefix = &content[..match_start];
            if is_cfg_test_guard(prefix) {
                continue;
            }

            let dir = path.parent().unwrap_or_else(|| Path::new(""));

            if let Some(custom_path) = path_attrs.get(name_str) {
                let candidate = dir.join(custom_path);
                let candidate = normalize_abs(&candidate);
                if by_path.contains_key(&candidate) {
                    // Also check for nested children of the custom module.
                    collect_nested_children(&candidate, by_path, &mut out);
                    out.push(candidate);
                }
                continue;
            }

            // Standard Rust module resolution: foo.rs or foo/mod.rs.
            for candidate in
                &[dir.join(format!("{}.rs", name_str)), dir.join(name_str).join("mod.rs")]
            {
                let candidate = normalize_abs(candidate);
                if by_path.contains_key(&candidate) {
                    // If the module is a directory (foo/mod.rs), also resolve children
                    // like foo/bar.rs or foo/bar/mod.rs.
                    if candidate.file_name().and_then(|n| n.to_str()) == Some("mod.rs") {
                        collect_nested_children(&candidate, by_path, &mut out);
                    }
                    out.push(candidate);
                }
            }
        }
    }

    let use_re = Regex::new(r#"(?m)^\s*use\s+crate::([A-Za-z0-9_:]+)"#).expect("valid use regex");
    if let Some(root) = src_root {
        for cap in use_re.captures_iter(content) {
            if let Some(spec) = cap.get(1) {
                let parts: Vec<&str> = spec.as_str().split("::").collect();
                for len in (1..=parts.len()).rev() {
                    let prefix = parts[..len].join("/");
                    for candidate in
                        &[root.join(format!("{prefix}.rs")), root.join(&prefix).join("mod.rs")]
                    {
                        let candidate = normalize_abs(candidate);
                        if by_path.contains_key(&candidate) {
                            out.push(candidate);
                            break;
                        }
                    }
                }
            }
        }
    }

    // Also resolve use self:: and use super::
    let self_super_re = Regex::new(r#"(?m)^\s*use\s+(self|super)::([A-Za-z0-9_:]+)"#)
        .expect("valid self super regex");
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    for cap in self_super_re.captures_iter(content) {
        if let Some(prefix_kind) = cap.get(1) {
            if let Some(spec) = cap.get(2) {
                let base = match prefix_kind.as_str() {
                    "self" => parent.to_path_buf(),
                    "super" => parent.parent().unwrap_or_else(|| Path::new("")).to_path_buf(),
                    _ => continue,
                };
                let parts: Vec<&str> = spec.as_str().split("::").collect();
                let module_name = parts[0];
                for candidate in
                    &[base.join(format!("{module_name}.rs")), base.join(module_name).join("mod.rs")]
                {
                    let candidate = normalize_abs(candidate);
                    if by_path.contains_key(&candidate) {
                        out.push(candidate);
                        break;
                    }
                }
            }
        }
    }

    dedup(out)
}

/// Checks whether the text immediately preceding a position contains
/// a `#[cfg(test)]` attribute, meaning the declaration is test-only.
fn is_cfg_test_guard(preceding_text: &str) -> bool {
    let re = Regex::new(r#"#\[cfg\s*\(\s*test\s*\)\s*\]\s*$"#).expect("valid cfg test regex");
    re.is_match(preceding_text)
}

/// Collects direct child modules of a directory-based module.
/// For `src/foo/mod.rs`, this finds `src/foo/bar.rs` and `src/foo/bar/mod.rs`.
///
/// Resolution rule (RFC): `mod bar;` inside `foo/mod.rs` resolves to
/// `foo/bar.rs` or `foo/bar/mod.rs`.
fn collect_nested_children(
    module_mod_rs: &Path,
    by_path: &HashMap<PathBuf, FileInfo>,
    out: &mut Vec<PathBuf>,
) {
    let Some(module_dir) = module_mod_rs.parent() else { return };
    // Read the module file to find nested `mod` declarations.
    let Ok((content, _)) = read_file_safe(module_mod_rs, None, None) else { return };
    let mod_re = Regex::new(
        r#"(?m)^\s*(?:pub(?:\s*\(\s*crate\s*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*[;{]"#,
    )
    .expect("valid nested mod regex");
    for cap in mod_re.captures_iter(&content) {
        if let Some(name) = cap.get(1) {
            // Check for #[cfg(test)] guard on nested module.
            let match_start = cap.get(0).map(|m| m.start()).unwrap_or(0);
            let prefix = &content[..match_start];
            if is_cfg_test_guard(prefix) {
                continue;
            }
            let name_str = name.as_str();
            for candidate in &[
                module_dir.join(format!("{name_str}.rs")),
                module_dir.join(name_str).join("mod.rs"),
            ] {
                let candidate = normalize_abs(candidate);
                if by_path.contains_key(&candidate) {
                    out.push(candidate.clone());
                }
            }
        }
    }
}

fn go_imports(content: &str, rel_to_abs: &HashMap<String, PathBuf>) -> Vec<PathBuf> {
    let re = Regex::new(r#"(?m)^\s*(?:import\s+)?(?:[._A-Za-z0-9-]+\s+)?\"([^\"]+)\""#)
        .expect("valid regex");
    let mut out = Vec::new();
    for spec in re.captures_iter(content).filter_map(|c| c.get(1).map(|m| m.as_str())) {
        // Match by directory-prefix: a Go file belongs to the package if its
        // relative path starts with the import path converted to '/' form.
        let spec_path = spec.replace('-', "/");
        for (rel, abs) in rel_to_abs {
            if rel.ends_with(".go") && rel.starts_with(&spec_path) {
                out.push(abs.clone());
            }
        }
    }
    dedup(out)
}

fn resolve_relative(
    importer: &Path,
    spec: &str,
    extensions: &[&str],
    by_path: &HashMap<PathBuf, FileInfo>,
) -> Option<PathBuf> {
    let base = importer.parent()?.join(spec);
    let candidates = if base.extension().is_some() {
        vec![
            base.clone(),
            base.join("index.ts"),
            base.join("index.tsx"),
            base.join("index.js"),
            base.join("index.jsx"),
        ]
    } else {
        let mut c = extensions
            .iter()
            .map(|ext| PathBuf::from(format!("{}{}", base.display(), ext)))
            .collect::<Vec<_>>();
        c.extend([
            base.join("index.ts"),
            base.join("index.tsx"),
            base.join("index.js"),
            base.join("index.jsx"),
        ]);
        c
    };
    candidates.into_iter().map(|p| normalize_abs(&p)).find(|p| by_path.contains_key(p))
}

fn dedup(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths.into_iter().filter(|p| seen.insert(p.clone())).collect()
}

fn normalize_abs(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traverse_follows_transitive_dependencies() {
        let entry = PathBuf::from("/repo/src/pages/Home.tsx");
        let dep = PathBuf::from("/repo/src/components/Card.tsx");
        let transitive = PathBuf::from("/repo/src/lib/format.ts");
        let graph = ImportGraph {
            files: HashMap::from([
                (entry.clone(), test_file(&entry)),
                (dep.clone(), test_file(&dep)),
                (transitive.clone(), test_file(&transitive)),
            ]),
            edges: HashMap::from([
                (entry.clone(), vec![dep.clone()]),
                (dep.clone(), vec![transitive.clone()]),
                (transitive.clone(), Vec::new()),
            ]),
            ..ImportGraph::default()
        };

        assert_eq!(traverse(&graph, &entry), vec![entry, dep, transitive]);
    }

    #[test]
    fn reverse_reachable_finds_all_callers() {
        let main = PathBuf::from("/repo/src/main.rs");
        let app = PathBuf::from("/repo/src/app.rs");
        let combat = PathBuf::from("/repo/src/combat.rs");
        let graph = ImportGraph {
            files: HashMap::from([
                (main.clone(), test_file(&main)),
                (app.clone(), test_file(&app)),
                (combat.clone(), test_file(&combat)),
            ]),
            edges: HashMap::from([
                (main.clone(), vec![app.clone()]),
                (app.clone(), vec![combat.clone()]),
                (combat.clone(), Vec::new()),
            ]),
            reverse: HashMap::from([
                (app.clone(), vec![main.clone()]),
                (combat.clone(), vec![app.clone()]),
            ]),
            ..ImportGraph::default()
        };

        let callers = reverse_reachable(&graph, &combat);
        assert_eq!(callers.len(), 2);
        assert!(callers.contains(&app));
        assert!(callers.contains(&main));
    }

    #[test]
    fn direct_callers_returns_one_hop() {
        let combat = PathBuf::from("/repo/src/combat.rs");
        let app = PathBuf::from("/repo/src/app.rs");
        let graph = ImportGraph {
            files: HashMap::from([
                (combat.clone(), test_file(&combat)),
                (app.clone(), test_file(&app)),
            ]),
            edges: HashMap::from([
                (app.clone(), vec![combat.clone()]),
                (combat.clone(), Vec::new()),
            ]),
            reverse: HashMap::from([(combat.clone(), vec![app.clone()])]),
            ..ImportGraph::default()
        };

        let callers = direct_callers(&graph, &combat);
        assert_eq!(callers, vec![app.clone()]);
    }

    #[test]
    fn rust_mod_declarations_resolve_to_files() {
        use std::fs;
        use tempfile::TempDir;
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        fs::create_dir_all(root.join("src")).expect("mkdir src");

        let main_rs = root.join("src/main.rs");
        let app_rs = root.join("src/app.rs");
        let combat_rs = root.join("src/combat.rs");
        fs::write(&main_rs, "mod app;\nmod combat;\nfn main() {}\n").expect("write main");
        fs::write(&app_rs, "use crate::combat;\npub fn run() {}\n").expect("write app");
        fs::write(&combat_rs, "pub fn resolve() -> i32 { 1 }\n").expect("write combat");

        let files: Vec<FileInfo> =
            [&main_rs, &app_rs, &combat_rs].iter().map(|p| test_file_abs(p)).collect();

        let graph = build(&files);

        let main_abs = normalize_abs(&main_rs);
        let app_abs = normalize_abs(&app_rs);
        let combat_abs = normalize_abs(&combat_rs);

        let deps = graph.edges.get(&main_abs).expect("main should have edges");
        assert!(deps.contains(&app_abs), "main should import app via mod app;");
        assert!(deps.contains(&combat_abs), "main should import combat via mod combat;");

        // Traverse from main should include all three.
        let reachable = traverse(&graph, &main_abs);
        assert_eq!(reachable.len(), 3);
        assert!(reachable.contains(&main_abs));
        assert!(reachable.contains(&app_abs));
        assert!(reachable.contains(&combat_abs));
    }

    #[test]
    fn rust_cfg_test_modules_are_skipped() {
        use std::fs;
        use tempfile::TempDir;
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        fs::create_dir_all(root.join("src")).expect("mkdir src");

        let main_rs = root.join("src/main.rs");
        let app_rs = root.join("src/app.rs");
        let tests_rs = root.join("src/tests.rs");
        fs::write(&main_rs, "mod app;\n#[cfg(test)]\nmod tests;\nfn main() {}\n")
            .expect("write main");
        fs::write(&app_rs, "pub fn run() {}\n").expect("write app");
        fs::write(&tests_rs, "#[test]\nfn test() {}\n").expect("write tests");

        let files: Vec<FileInfo> =
            [&main_rs, &app_rs, &tests_rs].iter().map(|p| test_file_abs(p)).collect();

        let graph = build(&files);

        let main_abs = normalize_abs(&main_rs);
        let app_abs = normalize_abs(&app_rs);
        let tests_abs = normalize_abs(&tests_rs);

        let deps = graph.edges.get(&main_abs).expect("main should have edges");
        assert!(deps.contains(&app_abs), "main should import app");
        assert!(!deps.contains(&tests_abs), "main should NOT import tests (cfg(test) guard)");
    }

    fn test_file(path: &Path) -> FileInfo {
        test_file_abs(path)
    }

    fn test_file_abs(path: &Path) -> FileInfo {
        FileInfo {
            path: path.to_path_buf(),
            relative_path: path.to_string_lossy().replace('\\', "/"),
            size_bytes: 0,
            extension: path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| format!(".{e}"))
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

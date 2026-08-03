//! Static import graph construction for module mode.

use crate::domain::FileInfo;
use crate::utils::read_file_safe;
use regex::Regex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

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
pub fn build(files: &[FileInfo]) -> ImportGraph {
    let mut ordered_files = files.to_vec();
    ordered_files
        .sort_by_cached_key(|file| (normalize_abs(&file.path), file.relative_path.clone()));

    let by_path: HashMap<PathBuf, FileInfo> =
        ordered_files.iter().map(|f| (normalize_abs(&f.path), f.clone())).collect();
    let rel_to_abs: HashMap<String, PathBuf> = ordered_files
        .iter()
        .map(|f| (f.relative_path.replace('\\', "/"), normalize_abs(&f.path)))
        .collect();

    let mut graph = ImportGraph { files: by_path.clone(), ..ImportGraph::default() };

    for file in &ordered_files {
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
        let mut callers = callers.clone();
        callers.sort();
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
            let mut callers = callers.clone();
            callers.sort();
            for caller in callers {
                if !seen.contains(&caller) {
                    queue.push_back(caller);
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
    let mut callers = graph.reverse.get(&start).cloned().unwrap_or_default();
    callers.sort();
    callers.dedup();
    callers
}

/// Detects Rust crate-root candidates from scanned files.
///
/// Returns absolute paths to `src/main.rs`, `src/lib.rs`, flat `src/bin/*.rs`
/// binaries, and directory binaries at `src/bin/*/main.rs` for the repository
/// root and nested workspace crates.
#[must_use]
pub fn rust_crate_roots(root: &Path, files: &[FileInfo]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = files
        .iter()
        .filter(|file| is_rust_source(file))
        .filter(|file| is_rust_crate_root(&file.path, root))
        .map(|file| normalize_abs(&file.path))
        .collect();

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

/// Returns whether a path looks like a Rust crate root.
#[must_use]
pub fn is_rust_crate_root(path: &Path, root: &Path) -> bool {
    let root = normalize_abs(root);
    let normalized = normalize_abs(path);
    let rel_path = normalized.strip_prefix(&root).unwrap_or(&normalized);
    let rel = rel_path.to_string_lossy().replace('\\', "/");
    crate_dir_for_rust_entry(&rel).is_some()
}

fn is_rust_source(file: &FileInfo) -> bool {
    matches!(file.extension.to_ascii_lowercase().as_str(), ".rs" | "rs")
}

fn crate_dir_for_rust_entry(rel: &str) -> Option<&str> {
    let (crate_dir, source_path) = if let Some(source_path) = rel.strip_prefix("src/") {
        ("", source_path)
    } else {
        rel.split_once("/src/")?
    };

    if matches!(source_path, "lib.rs" | "main.rs") {
        return Some(crate_dir);
    }

    let binary_path = source_path.strip_prefix("bin/")?;
    let is_flat_binary = binary_path.ends_with(".rs") && !binary_path.contains('/');
    let is_directory_binary = binary_path
        .strip_suffix("/main.rs")
        .is_some_and(|directory| !directory.is_empty() && !directory.contains('/'));
    (is_flat_binary || is_directory_binary).then_some(crate_dir)
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
        ".svelte" | "svelte" => svelte_imports(&file.path, content, by_path),
        ".py" | "py" => python_imports(file, content, rel_to_abs),
        ".rs" | "rs" => rust_imports(&file.path, content, by_path),
        ".go" | "go" => go_imports(file, content, by_path),
        ".gd" | ".tscn" | ".tres" | ".godot" | ".gdshader" | ".gdshaderinc" => {
            godot_imports(content, rel_to_abs)
        }
        _ => Vec::new(),
    }
}

fn godot_imports(content: &str, rel_to_abs: &HashMap<String, PathBuf>) -> Vec<PathBuf> {
    let re = Regex::new(r#"res://[^\"'\s)\],}]+"#).expect("valid Godot resource regex");
    let mut out = Vec::new();
    for resource_path in re.find_iter(content).map(|matched| matched.as_str()) {
        let relative = resource_path.trim_start_matches("res://").replace('\\', "/");
        if let Some(path) = rel_to_abs.get(&relative) {
            out.push(path.clone());
        }
    }
    dedup(out)
}

fn js_imports(path: &Path, content: &str, by_path: &HashMap<PathBuf, FileInfo>) -> Vec<PathBuf> {
    let static_import_re =
        Regex::new(r#"(?m)\bimport\s+(?:[^'\"\n]+?\s+from\s+)?['\"]([^'\"]+)['\"]"#)
            .expect("valid JavaScript static import regex");
    let export_re =
        Regex::new(r#"(?m)\bexport\s+(?:\*[^;\n]*?|\{[^}\n]*\})\s+from\s+['\"]([^'\"]+)['\"]"#)
            .expect("valid JavaScript re-export regex");
    let require_re = Regex::new(r#"(?m)\brequire\s*\(\s*['\"]([^'\"]+)['\"]\s*\)"#)
        .expect("valid JavaScript require regex");
    let dynamic_import_re = Regex::new(r#"(?m)\bimport\s*\(\s*['\"]([^'\"]+)['\"]\s*\)"#)
        .expect("valid JavaScript dynamic import regex");

    let mut out = Vec::new();
    for re in [&static_import_re, &export_re, &require_re, &dynamic_import_re] {
        for spec in re.captures_iter(content).filter_map(|cap| cap.get(1).map(|m| m.as_str())) {
            if let Some(path) = spec
                .starts_with('.')
                .then(|| {
                    resolve_relative(
                        path,
                        spec,
                        &[".ts", ".tsx", ".js", ".jsx", ".svelte"],
                        by_path,
                    )
                })
                .flatten()
            {
                out.push(path);
            }
        }
    }
    dedup(out)
}

fn svelte_imports(
    path: &Path,
    content: &str,
    by_path: &HashMap<PathBuf, FileInfo>,
) -> Vec<PathBuf> {
    let script_re =
        Regex::new(r#"(?s)<script(?:\s[^>]*)?>(.*?)</script>"#).expect("valid Svelte script regex");
    dedup(
        script_re
            .captures_iter(content)
            .filter_map(|captures| captures.get(1).map(|script| script.as_str()))
            .flat_map(|script| js_imports(path, script, by_path))
            .collect(),
    )
}

fn python_imports(
    file: &FileInfo,
    content: &str,
    rel_to_abs: &HashMap<String, PathBuf>,
) -> Vec<PathBuf> {
    let from_re =
        Regex::new(r#"(?m)^\s*from\s+([.]*[A-Za-z_][A-Za-z0-9_.]*|[.]+)\s+import\s+([^\n#]+)"#)
            .expect("valid Python from-import regex");
    let import_re = Regex::new(r#"(?m)^\s*import\s+([^\n#]+)"#).expect("valid Python import regex");
    let mut out = Vec::new();

    for captures in from_re.captures_iter(content) {
        let Some(module) = captures.get(1).map(|value| value.as_str()) else { continue };
        if let Some(path) = resolve_python_module(file, module, rel_to_abs) {
            out.push(path);
        }

        // `from . import models` names a module in the current package. For a
        // normal `from package import Name`, only add Name when it is itself a
        // scanned module; unresolved symbols and third-party imports disappear.
        let imported = captures.get(2).map(|value| value.as_str()).unwrap_or("");
        for name in imported.split(',').filter_map(python_imported_name) {
            let child = if module.ends_with('.') {
                format!("{module}{name}")
            } else {
                format!("{module}.{name}")
            };
            if let Some(path) = resolve_python_module(file, &child, rel_to_abs) {
                out.push(path);
            }
        }
    }

    for captures in import_re.captures_iter(content) {
        let Some(imports) = captures.get(1).map(|value| value.as_str()) else { continue };
        for module in imports.split(',').filter_map(python_imported_name) {
            if let Some(path) = resolve_python_module(file, module, rel_to_abs) {
                out.push(path);
            }
        }
    }

    dedup(out)
}

fn python_imported_name(value: &str) -> Option<&str> {
    let name = value.split_whitespace().next()?.trim_matches(['(', ')']);
    (!name.is_empty()).then_some(name)
}

fn resolve_python_module(
    importer: &FileInfo,
    spec: &str,
    rel_to_abs: &HashMap<String, PathBuf>,
) -> Option<PathBuf> {
    let importer_path = Path::new(&importer.relative_path);
    let package_dir = importer_path.parent().unwrap_or_else(|| Path::new(""));
    let dot_count = spec.bytes().take_while(|byte| *byte == b'.').count();
    let module = spec[dot_count..].replace('.', "/");

    if dot_count > 0 {
        let mut base = package_dir.to_path_buf();
        for _ in 1..dot_count {
            base.pop();
        }
        return python_module_at(&base.join(module), rel_to_abs);
    }

    // Try the repository root first, then each enclosing source root. This
    // covers both `pkg.module` and common `src/pkg` / `api/app` layouts without
    // treating unresolved third-party imports as local dependencies.
    if let Some(path) = python_module_at(Path::new(&module), rel_to_abs) {
        return Some(path);
    }
    let ancestors: Vec<&Path> = package_dir.ancestors().collect();
    for prefix in ancestors.iter().rev().skip(1) {
        if let Some(path) = python_module_at(&prefix.join(&module), rel_to_abs) {
            return Some(path);
        }
    }
    None
}

fn python_module_at(base: &Path, rel_to_abs: &HashMap<String, PathBuf>) -> Option<PathBuf> {
    let base = base.to_string_lossy().replace('\\', "/");
    let module_file = format!("{base}.py");
    let package_file = format!("{base}/__init__.py");
    rel_to_abs.get(&module_file).or_else(|| rel_to_abs.get(&package_file)).cloned()
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

            // Standard Rust module resolution. An ordinary `foo.rs` owns the
            // `foo/` directory, while crate roots and `mod.rs` own siblings.
            let module_dir = rust_child_module_dir(path, src_root);
            for candidate in &[
                module_dir.join(format!("{}.rs", name_str)),
                module_dir.join(name_str).join("mod.rs"),
            ] {
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

    // Resolve crate, self, and super imports, including grouped forms such as
    // `use crate::{a, b::Thing};` and `pub use self::{a, b};`.
    let use_re = Regex::new(
        r#"(?m)^[ \t]*(?:pub(?:[ \t]*\([^)]*\))?[ \t]+)?use[ \t]+(crate|self|super)::([^;]*);"#,
    )
    .expect("valid Rust use regex");
    let module_dir = rust_child_module_dir(path, src_root);
    for cap in use_re.captures_iter(content) {
        let Some(prefix_kind) = cap.get(1).map(|value| value.as_str()) else { continue };
        let Some(body) = cap.get(2).map(|value| value.as_str()) else { continue };
        let mut base = match prefix_kind {
            "crate" => match rust_crate_module_dir(path, src_root, by_path) {
                Some(root) => root,
                None => continue,
            },
            "self" => module_dir.clone(),
            "super" => module_dir.parent().unwrap_or_else(|| Path::new("")).to_path_buf(),
            _ => continue,
        };
        let mut spec = body.trim();
        while let Some(rest) = spec.strip_prefix("super::") {
            base = base.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
            spec = rest;
        }
        for parts in expand_rust_use_paths(&[], spec) {
            out.extend(resolve_rust_use_path(&base, &parts, by_path));
        }
    }

    dedup(out)
}

fn expand_rust_use_paths(prefix: &[String], spec: &str) -> Vec<Vec<String>> {
    let spec = strip_rust_use_alias(spec.trim());
    if spec.is_empty() || spec == "self" {
        return vec![prefix.to_vec()];
    }

    if spec.starts_with('{') {
        let Some(close) = matching_brace(spec, 0) else { return Vec::new() };
        if !spec[close + 1..].trim().is_empty() {
            return Vec::new();
        }
        return split_top_level(&spec[1..close])
            .into_iter()
            .flat_map(|part| expand_rust_use_paths(prefix, part))
            .collect();
    }

    if let Some(open) = find_top_level_open_brace(spec) {
        let Some(close) = matching_brace(spec, open) else { return Vec::new() };
        if !spec[close + 1..].trim().is_empty() {
            return Vec::new();
        }
        let mut next = prefix.to_vec();
        next.extend(rust_use_components(spec[..open].trim_end_matches(':')));
        return split_top_level(&spec[open + 1..close])
            .into_iter()
            .flat_map(|part| expand_rust_use_paths(&next, part))
            .collect();
    }

    let mut path = prefix.to_vec();
    path.extend(rust_use_components(spec));
    vec![path]
}

fn resolve_rust_use_path(
    base: &Path,
    parts: &[String],
    by_path: &HashMap<PathBuf, FileInfo>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for len in (1..=parts.len()).rev() {
        let prefix = parts[..len].join("/");
        for candidate in &[base.join(format!("{prefix}.rs")), base.join(&prefix).join("mod.rs")] {
            let candidate = normalize_abs(candidate);
            if by_path.contains_key(&candidate) {
                out.push(candidate);
                break;
            }
        }
    }
    out
}

fn rust_use_components(value: &str) -> Vec<String> {
    value
        .split("::")
        .map(str::trim)
        .filter(|part| !part.is_empty() && *part != "self" && *part != "*")
        .map(ToOwned::to_owned)
        .collect()
}

fn strip_rust_use_alias(value: &str) -> &str {
    let mut brace_depth = 0usize;
    for (index, character) in value.char_indices() {
        match character {
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            _ if brace_depth == 0 && value[index..].starts_with(" as ") => {
                return value[..index].trim_end()
            }
            _ => {}
        }
    }
    value
}

fn split_top_level(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut brace_depth = 0usize;
    for (index, character) in value.char_indices() {
        match character {
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            ',' if brace_depth == 0 => {
                parts.push(value[start..index].trim());
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(value[start..].trim());
    parts.into_iter().filter(|part| !part.is_empty()).collect()
}

fn find_top_level_open_brace(value: &str) -> Option<usize> {
    value.char_indices().find_map(|(index, character)| (character == '{').then_some(index))
}

fn matching_brace(value: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, character) in value.char_indices().skip_while(|(index, _)| *index < open) {
        match character {
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn rust_child_module_dir(path: &Path, src_root: Option<&Path>) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
    let is_crate_root = src_root.is_some_and(|root| is_rust_crate_root_file(path, root));
    if file_name == "mod.rs" || is_crate_root {
        parent.to_path_buf()
    } else {
        path.with_extension("")
    }
}

fn is_rust_crate_root_file(path: &Path, src_root: &Path) -> bool {
    if path == src_root.join("lib.rs") || path == src_root.join("main.rs") {
        return true;
    }

    let Ok(binary_path) = path.strip_prefix(src_root.join("bin")) else { return false };
    let parts: Vec<_> = binary_path.components().collect();
    (parts.len() == 1 && path.extension().and_then(|extension| extension.to_str()) == Some("rs"))
        || (parts.len() == 2 && path.file_name().and_then(|name| name.to_str()) == Some("main.rs"))
}

fn rust_crate_module_dir(
    path: &Path,
    src_root: Option<&Path>,
    by_path: &HashMap<PathBuf, FileInfo>,
) -> Option<PathBuf> {
    let src_root = src_root?;
    let bin_root = src_root.join("bin");
    let Ok(binary_path) = path.strip_prefix(&bin_root) else {
        return Some(src_root.to_path_buf());
    };
    let binary_name = binary_path.components().next()?.as_os_str();

    if binary_path.components().count() == 1 {
        return Some(bin_root);
    }

    let directory_root = bin_root.join(binary_name);
    if by_path.contains_key(&normalize_abs(&directory_root.join("main.rs"))) {
        return Some(directory_root);
    }

    let flat_root = bin_root.join(binary_name).with_extension("rs");
    if by_path.contains_key(&normalize_abs(&flat_root)) {
        return Some(bin_root);
    }

    Some(src_root.to_path_buf())
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

fn go_imports(
    file: &FileInfo,
    content: &str,
    by_path: &HashMap<PathBuf, FileInfo>,
) -> Vec<PathBuf> {
    let re = Regex::new(r#"(?m)^\s*(?:import\s+)?(?:[._A-Za-z0-9-]+\s+)?\"([^\"]+)\""#)
        .expect("valid Go import regex");
    let Some((module_root, module_path)) = go_module(file.path.as_path()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for spec in re.captures_iter(content).filter_map(|c| c.get(1).map(|m| m.as_str())) {
        let Some(package_path) = go_local_package_path(spec, &module_path) else { continue };
        for candidate in by_path.values().filter(|candidate| is_go_file(candidate)) {
            let abs = normalize_abs(&candidate.path);
            let Ok(relative) = abs.strip_prefix(&module_root) else { continue };
            let relative_package = relative
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .to_string_lossy()
                .replace('\\', "/");
            if relative_package == package_path {
                out.push(abs);
            }
        }
    }
    dedup(out)
}

fn go_module(importer: &Path) -> Option<(PathBuf, String)> {
    let mut directory = importer.parent()?;
    loop {
        let manifest = directory.join("go.mod");
        if let Ok((content, _)) = read_file_safe(&manifest, None, None) {
            let module = content.lines().find_map(|line| {
                let line = line.trim_start();
                let rest = line.strip_prefix("module")?;
                if !rest.chars().next().is_some_and(char::is_whitespace) {
                    return None;
                }
                rest.split_whitespace().next().map(str::to_owned)
            });
            if let Some(module) = module {
                return Some((normalize_abs(directory), module));
            }
        }
        let parent = directory.parent()?;
        if parent == directory {
            return None;
        }
        directory = parent;
    }
}

fn go_local_package_path(spec: &str, module: &str) -> Option<String> {
    if spec == module {
        return Some(String::new());
    }
    spec.strip_prefix(module)?.strip_prefix('/').map(str::to_owned)
}

fn is_go_file(file: &FileInfo) -> bool {
    matches!(file.extension.to_ascii_lowercase().as_str(), ".go" | "go")
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
    let mut paths: Vec<PathBuf> = paths.into_iter().filter(|p| seen.insert(p.clone())).collect();
    paths.sort();
    paths
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
    fn godot_imports_resolve_resource_paths() {
        let script = PathBuf::from("/repo/scripts/player.gd");
        let scene = PathBuf::from("/repo/scenes/hud.tscn");
        let map = HashMap::from([("scenes/hud.tscn".to_string(), scene.clone())]);

        let imports = godot_imports("const HUD = preload(\"res://scenes/hud.tscn\")\n", &map);

        assert_eq!(imports, vec![scene]);
        let _ = script;
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

    #[test]
    fn rust_crate_roots_include_workspace_crates() {
        use std::fs;
        use tempfile::TempDir;
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"tokio\"]\n")
            .expect("write workspace manifest");
        fs::create_dir_all(root.join("tokio/src/bin")).expect("mkdir crate src");
        fs::write(root.join("tokio/Cargo.toml"), "[package]\nname = \"tokio\"\n")
            .expect("write crate manifest");

        let lib_rs = root.join("tokio/src/lib.rs");
        let bin_rs = root.join("tokio/src/bin/console.rs");
        fs::write(&lib_rs, "pub mod runtime;\n").expect("write lib");
        fs::write(&bin_rs, "fn main() {}\n").expect("write bin");

        let files = vec![
            test_file_rel(&lib_rs, "tokio/src/lib.rs"),
            test_file_rel(&bin_rs, "tokio/src/bin/console.rs"),
        ];

        let roots = rust_crate_roots(root, &files);

        assert!(roots.contains(&normalize_abs(&lib_rs)), "workspace crate lib.rs is a crate root");
        assert!(roots.contains(&normalize_abs(&bin_rs)), "workspace crate bin is a crate root");
    }

    #[test]
    fn rust_directory_binary_owns_sibling_modules() {
        use std::fs;
        use tempfile::TempDir;
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        fs::create_dir_all(root.join("src/bin/tool")).expect("mkdir directory binary");

        let main_rs = root.join("src/bin/tool/main.rs");
        let command_rs = root.join("src/bin/tool/command.rs");
        let shared_rs = root.join("src/bin/tool/shared.rs");
        fs::write(&main_rs, "mod command;\nmod shared;\nfn main() { command::run(); }\n")
            .expect("write main");
        fs::write(&command_rs, "use crate::shared;\npub fn run() { shared::work(); }\n")
            .expect("write command");
        fs::write(&shared_rs, "pub fn work() {}\n").expect("write shared");

        let files =
            vec![test_file_abs(&main_rs), test_file_abs(&command_rs), test_file_abs(&shared_rs)];
        let roots = rust_crate_roots(root, &files);
        let graph = build(&files);

        assert_eq!(roots, vec![normalize_abs(&main_rs)]);
        assert_eq!(
            traverse(&graph, &main_rs),
            vec![normalize_abs(&main_rs), normalize_abs(&command_rs), normalize_abs(&shared_rs),]
        );
        assert!(graph
            .edges
            .get(&normalize_abs(&command_rs))
            .expect("command dependencies")
            .contains(&normalize_abs(&shared_rs)));
    }

    #[test]
    fn python_package_imports_resolve_local_modules_only() {
        use std::fs;
        use tempfile::TempDir;
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        fs::create_dir_all(root.join("api/app")).expect("mkdir package");

        let main_py = root.join("api/app/main.py");
        let services_py = root.join("api/app/services.py");
        let models_py = root.join("api/app/models.py");
        fs::write(
            &main_py,
            "from app.services import run\nfrom . import models\nimport requests\n",
        )
        .expect("write main");
        fs::write(&services_py, "def run(): pass\n").expect("write services");
        fs::write(&models_py, "class Model: pass\n").expect("write models");

        let files = vec![
            test_file_rel(&main_py, "api/app/main.py"),
            test_file_rel(&services_py, "api/app/services.py"),
            test_file_rel(&models_py, "api/app/models.py"),
        ];
        let graph = build(&files);
        let dependencies = graph.edges.get(&normalize_abs(&main_py)).expect("main dependencies");

        assert_eq!(dependencies.len(), 2, "unresolved third-party imports must be ignored");
        assert!(dependencies.contains(&normalize_abs(&services_py)));
        assert!(dependencies.contains(&normalize_abs(&models_py)));
    }

    #[test]
    fn svelte_imports_resolve_typescript_and_svelte_targets() {
        use std::fs;
        use tempfile::TempDir;
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        fs::create_dir_all(root.join("src/lib")).expect("mkdir lib");

        let page = root.join("src/Page.svelte");
        let card = root.join("src/lib/Card.svelte");
        let format = root.join("src/lib/format.ts");
        let fake = root.join("src/lib/fake.ts");
        let required = root.join("src/lib/required.ts");
        let lazy = root.join("src/lib/lazy.ts");
        fs::write(
            &page,
            "<script lang=\"ts\">\nimport Card from './lib/Card.svelte';\nimport { format } from './lib/format';\nexport { required } from './lib/required';\nconst loaded = require('./lib/required');\nconst lazy = import('./lib/lazy');\n</script>\n<p>import fake from './lib/fake'</p>\n",
        )
        .expect("write page");
        fs::write(&card, "<div>card</div>\n").expect("write card");
        fs::write(&format, "export const format = String;\n").expect("write format");
        fs::write(&fake, "export default false;\n").expect("write fake");
        fs::write(&required, "export const required = true;\n").expect("write required");
        fs::write(&lazy, "export const lazy = true;\n").expect("write lazy");

        let files = vec![
            test_file_abs(&page),
            test_file_abs(&card),
            test_file_abs(&format),
            test_file_abs(&fake),
            test_file_abs(&required),
            test_file_abs(&lazy),
        ];
        let graph = build(&files);
        let dependencies = graph.edges.get(&normalize_abs(&page)).expect("page dependencies");

        assert!(dependencies.contains(&normalize_abs(&card)));
        assert!(dependencies.contains(&normalize_abs(&format)));
        assert!(dependencies.contains(&normalize_abs(&required)));
        assert!(dependencies.contains(&normalize_abs(&lazy)));
        assert!(!dependencies.contains(&normalize_abs(&fake)));
    }

    #[test]
    fn rust_ordinary_module_owns_a_same_named_child_directory() {
        use std::fs;
        use tempfile::TempDir;
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        fs::create_dir_all(root.join("src/foo")).expect("mkdir module");

        let lib_rs = root.join("src/lib.rs");
        let foo_rs = root.join("src/foo.rs");
        let child_rs = root.join("src/foo/child.rs");
        let helper_rs = root.join("src/foo/helper.rs");
        let nested_main_rs = root.join("src/foo/main.rs");
        let deep_rs = root.join("src/foo/main/deep.rs");
        fs::create_dir_all(root.join("src/foo/main")).expect("mkdir nested main module");
        fs::write(&lib_rs, "mod foo;\n").expect("write lib");
        fs::write(&foo_rs, "mod child;\nmod helper;\nmod main;\nuse self::helper;\n")
            .expect("write foo");
        fs::write(&child_rs, "use super::helper;\npub fn value() -> u8 { 1 }\n")
            .expect("write child");
        fs::write(&helper_rs, "pub fn help() {}\n").expect("write helper");
        fs::write(&nested_main_rs, "mod deep;\n").expect("write nested main");
        fs::write(&deep_rs, "pub fn deep() {}\n").expect("write deep");

        let files = vec![
            test_file_abs(&lib_rs),
            test_file_abs(&foo_rs),
            test_file_abs(&child_rs),
            test_file_abs(&helper_rs),
            test_file_abs(&nested_main_rs),
            test_file_abs(&deep_rs),
        ];
        let graph = build(&files);
        let reachable = traverse(&graph, &lib_rs);

        assert_eq!(reachable.len(), 6);
        assert!(reachable.contains(&normalize_abs(&child_rs)));
        assert!(reachable.contains(&normalize_abs(&helper_rs)));
        assert!(reachable.contains(&normalize_abs(&deep_rs)));
        assert!(graph
            .edges
            .get(&normalize_abs(&child_rs))
            .expect("child dependencies")
            .contains(&normalize_abs(&helper_rs)));
    }

    #[test]
    fn rust_legacy_and_path_attribute_modules_still_resolve() {
        use std::fs;
        use tempfile::TempDir;
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        fs::create_dir_all(root.join("src/legacy")).expect("mkdir legacy");
        fs::create_dir_all(root.join("src/custom")).expect("mkdir custom");

        let lib_rs = root.join("src/lib.rs");
        let legacy_rs = root.join("src/legacy/mod.rs");
        let nested_rs = root.join("src/legacy/nested.rs");
        let custom_rs = root.join("src/custom/runtime.rs");
        fs::write(&lib_rs, "mod legacy;\n#[path = \"custom/runtime.rs\"]\nmod runtime;\n")
            .expect("write lib");
        fs::write(&legacy_rs, "mod nested;\n").expect("write legacy");
        fs::write(&nested_rs, "pub fn nested() {}\n").expect("write nested");
        fs::write(&custom_rs, "pub fn runtime() {}\n").expect("write custom");

        let files = vec![
            test_file_abs(&lib_rs),
            test_file_abs(&legacy_rs),
            test_file_abs(&nested_rs),
            test_file_abs(&custom_rs),
        ];
        let graph = build(&files);
        let reachable = traverse(&graph, &lib_rs);

        assert!(reachable.contains(&normalize_abs(&legacy_rs)));
        assert!(reachable.contains(&normalize_abs(&nested_rs)));
        assert!(reachable.contains(&normalize_abs(&custom_rs)));
    }

    #[test]
    fn javascript_literal_import_forms_are_sorted_and_resolved() {
        use std::fs;
        use tempfile::TempDir;
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        fs::create_dir_all(root.join("src")).expect("mkdir src");

        let entry = root.join("src/main.ts");
        let exported = root.join("src/exported.ts");
        let required = root.join("src/required.ts");
        let dynamic = root.join("src/dynamic.ts");
        fs::write(
            &entry,
            "export { value } from './exported';\nconst required = require('./required');\nconst lazy = import('./dynamic');\n",
        )
        .expect("write entry");
        fs::write(&exported, "export const value = 1;\n").expect("write exported");
        fs::write(&required, "export const value = 2;\n").expect("write required");
        fs::write(&dynamic, "export const value = 3;\n").expect("write dynamic");

        // Deliberately pass files in reverse order; build order must not depend
        // on scanner or caller iteration order.
        let files = vec![
            test_file_abs(&required),
            test_file_abs(&entry),
            test_file_abs(&dynamic),
            test_file_abs(&exported),
        ];
        let graph = build(&files);
        let dependencies = graph.edges.get(&normalize_abs(&entry)).expect("entry dependencies");

        assert_eq!(
            dependencies,
            &vec![normalize_abs(&dynamic), normalize_abs(&exported), normalize_abs(&required)]
        );
    }

    #[test]
    fn rust_grouped_use_paths_resolve_nested_modules() {
        use std::fs;
        use tempfile::TempDir;
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        fs::create_dir_all(root.join("src/baz")).expect("mkdir baz");

        let lib_rs = root.join("src/lib.rs");
        let foo_rs = root.join("src/foo.rs");
        let bar_rs = root.join("src/bar.rs");
        let baz_mod_rs = root.join("src/baz/mod.rs");
        let deep_rs = root.join("src/baz/deep.rs");
        let nested_rs = root.join("src/baz/nested.rs");
        fs::write(&lib_rs, "mod foo;\npub use crate::{bar::Thing, foo, baz::{deep, nested}};\n")
            .expect("write lib");
        fs::write(&foo_rs, "pub fn foo() {}\n").expect("write foo");
        fs::write(&bar_rs, "pub struct Thing;\n").expect("write bar");
        fs::write(&baz_mod_rs, "pub fn root() {}\n").expect("write baz");
        fs::write(&deep_rs, "pub fn deep() {}\n").expect("write deep");
        fs::write(&nested_rs, "pub fn nested() {}\n").expect("write nested");

        let files = vec![
            test_file_abs(&nested_rs),
            test_file_abs(&lib_rs),
            test_file_abs(&deep_rs),
            test_file_abs(&bar_rs),
            test_file_abs(&baz_mod_rs),
            test_file_abs(&foo_rs),
        ];
        let graph = build(&files);
        let dependencies = graph.edges.get(&normalize_abs(&lib_rs)).expect("lib dependencies");

        for expected in [&foo_rs, &bar_rs, &baz_mod_rs, &deep_rs, &nested_rs] {
            assert!(dependencies.contains(&normalize_abs(expected)), "missing {expected:?}");
        }
    }

    #[test]
    fn go_mod_imports_resolve_only_local_package_files() {
        use std::fs;
        use tempfile::TempDir;
        let tmp = TempDir::new().expect("tmp");
        let root = tmp.path();
        fs::create_dir_all(root.join("cmd/app")).expect("mkdir app");
        fs::create_dir_all(root.join("internal/auth/nested")).expect("mkdir auth");
        fs::write(root.join("go.mod"), "module example.com/acme\n").expect("write go.mod");

        let main_go = root.join("cmd/app/main.go");
        let auth_go = root.join("internal/auth/auth.go");
        let nested_go = root.join("internal/auth/nested/extra.go");
        let other_go = root.join("internal/other/other.go");
        fs::write(
            &main_go,
            "package main\nimport (\n\t\"example.com/acme/internal/auth\"\n\t\"fmt\"\n)\n",
        )
        .expect("write main");
        fs::write(&auth_go, "package auth\n").expect("write auth");
        fs::write(&nested_go, "package nested\n").expect("write nested");
        fs::create_dir_all(root.join("internal/other")).expect("mkdir other");
        fs::write(&other_go, "package other\n").expect("write other");

        let files = vec![
            test_file_rel(&nested_go, "internal/auth/nested/extra.go"),
            test_file_rel(&main_go, "cmd/app/main.go"),
            test_file_rel(&other_go, "internal/other/other.go"),
            test_file_rel(&auth_go, "internal/auth/auth.go"),
        ];
        let graph = build(&files);
        let dependencies = graph.edges.get(&normalize_abs(&main_go)).expect("main dependencies");

        assert_eq!(dependencies, &vec![normalize_abs(&auth_go)]);
    }

    fn test_file(path: &Path) -> FileInfo {
        test_file_abs(path)
    }

    fn test_file_abs(path: &Path) -> FileInfo {
        test_file_rel(path, &path.to_string_lossy().replace('\\', "/"))
    }

    fn test_file_rel(path: &Path, relative_path: &str) -> FileInfo {
        FileInfo {
            path: path.to_path_buf(),
            relative_path: relative_path.to_string(),
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

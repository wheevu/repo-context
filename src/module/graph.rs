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
            graph.edges.entry(source.clone()).or_default().push(dep);
        }
        graph.edges.entry(source).or_default();
    }

    graph
}

/// Breadth-first traversal returning entry plus all transitive dependencies.
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
    match file.extension.as_str() {
        ".ts" | ".tsx" | ".js" | ".jsx" => js_imports(&file.path, content, by_path),
        ".rs" => rust_imports(&file.path, content, by_path),
        ".go" => go_imports(content, rel_to_abs),
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
    let mod_re = Regex::new(r#"(?m)^\s*(?:pub\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;"#)
        .expect("valid regex");
    for cap in mod_re.captures_iter(content) {
        if let Some(name) = cap.get(1) {
            let dir = path.parent().unwrap_or_else(|| Path::new(""));
            for candidate in
                [dir.join(format!("{}.rs", name.as_str())), dir.join(name.as_str()).join("mod.rs")]
            {
                let candidate = normalize_abs(&candidate);
                if by_path.contains_key(&candidate) {
                    out.push(candidate);
                }
            }
        }
    }
    let use_re = Regex::new(r#"(?m)^\s*use\s+crate::([A-Za-z0-9_:]+)"#).expect("valid regex");
    if let Some(root) = src_root {
        for cap in use_re.captures_iter(content) {
            if let Some(spec) = cap.get(1) {
                let parts: Vec<&str> = spec.as_str().split("::").collect();
                for len in (1..=parts.len()).rev() {
                    let prefix = parts[..len].join("/");
                    for candidate in
                        [root.join(format!("{prefix}.rs")), root.join(&prefix).join("mod.rs")]
                    {
                        let candidate = normalize_abs(&candidate);
                        if by_path.contains_key(&candidate) {
                            out.push(candidate);
                            break;
                        }
                    }
                }
            }
        }
    }
    dedup(out)
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

    fn test_file(path: &Path) -> FileInfo {
        FileInfo {
            path: path.to_path_buf(),
            relative_path: path.to_string_lossy().to_string(),
            size_bytes: 0,
            extension: path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_string(),
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

//! CSS class extraction and global stylesheet scoping for module mode.

use crate::utils::read_file_safe;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Extracts class names and static class prefixes from TSX/JSX files.
#[must_use]
pub fn extract_classnames(files: &[PathBuf]) -> HashSet<String> {
    let mut names = HashSet::new();
    for path in files.iter().filter(|p| matches!(ext(p).as_deref(), Some("tsx" | "jsx"))) {
        let Ok((content, _)) = read_file_safe(path, None, None) else { continue };
        extract_from_content(&content, &mut names);
    }
    names
}

/// Returns CSS text filtered to selectors that mention extracted classes.
#[must_use]
pub fn scope_css(css_path: &Path, classnames: &HashSet<String>) -> String {
    let Ok(content) = fs::read_to_string(css_path) else { return String::new() };
    let blocks = parse_blocks(&content);
    let mut scoped = String::new();
    for (selector, body) in blocks {
        if selector_matches(&selector, classnames) {
            scoped.push_str(selector.trim());
            scoped.push_str(" {");
            scoped.push_str(&body);
            scoped.push_str("}\n\n");
        }
    }
    scoped
}

/// Counts top-level CSS rule blocks.
#[must_use]
pub fn count_rules(css_path: &Path) -> usize {
    fs::read_to_string(css_path).map(|s| parse_blocks(&s).len()).unwrap_or(0)
}

/// Counts CSS rule blocks in already loaded CSS text.
#[must_use]
pub fn count_rules_from_text(css: &str) -> usize {
    parse_blocks(css).len()
}

/// Auto-detects globally imported CSS files.
#[must_use]
pub fn detect_css_files(root: &Path, files: &[crate::domain::FileInfo]) -> Vec<PathBuf> {
    let entry_names = [
        "app.tsx",
        "app.ts",
        "entry-client.tsx",
        "entry-client.ts",
        "index.tsx",
        "index.ts",
        "main.tsx",
        "main.ts",
    ];
    let scanned_css: HashMap<PathBuf, PathBuf> = files
        .iter()
        .filter(|f| f.extension == ".css")
        .map(|f| (canon(&f.path), canon(&f.path)))
        .collect();
    let mut out = Vec::new();
    let css_import = Regex::new(r#"import\s+['\"]([^'\"]+\.css)['\"]"#).expect("valid regex");
    for file in files.iter().filter(|f| {
        entry_names.contains(&f.path.file_name().and_then(|n| n.to_str()).unwrap_or(""))
    }) {
        let Ok((content, _)) = read_file_safe(&file.path, None, None) else { continue };
        for cap in css_import.captures_iter(&content) {
            if let Some(spec) = cap.get(1).map(|m| m.as_str()) {
                if spec.starts_with('.') {
                    if let Some(parent) = file.path.parent() {
                        let path = canon(&parent.join(spec));
                        if let Some(scanned) = scanned_css.get(&path) {
                            out.push(scanned.clone());
                        }
                    }
                }
            }
        }
    }

    for candidate in [
        root.join("src/styles.css"),
        root.join("src/index.css"),
        root.join("src/main.css"),
        root.join("styles.css"),
        root.join("index.css"),
    ] {
        let path = canon(&candidate);
        if candidate.exists() {
            if let Some(scanned) = scanned_css.get(&path) {
                out.push(scanned.clone());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Regex to strip `${...}` placeholders from template literals.
static TEMPLATE_CLEANUP: once_cell::sync::Lazy<Regex> =
    once_cell::sync::Lazy::new(|| Regex::new(r#"\$\{[^}]+\}"#).expect("valid regex"));

/// Regex to extract static class prefix from template literal (e.g. `btn-${...}`).
static TEMPLATE_PREFIX: once_cell::sync::Lazy<Regex> =
    once_cell::sync::Lazy::new(|| Regex::new(r#"([A-Za-z0-9_-]+-)\$\{"#).expect("valid regex"));

fn extract_from_content(content: &str, names: &mut HashSet<String>) {
    let literal = Regex::new(r#"(?:class|className)\s*=\s*\"([^\"]+)\""#).expect("valid regex");
    let brace_string = Regex::new(r#"(?:class|className)\s*=\s*\{\s*['\"]([^'\"]+)['\"]\s*\}"#)
        .expect("valid regex");
    let template =
        Regex::new(r#"(?:class|className)\s*=\s*\{\s*`([^`]+)`\s*\}"#).expect("valid regex");
    for cap in literal.captures_iter(content).chain(brace_string.captures_iter(content)) {
        if let Some(value) = cap.get(1) {
            add_tokens(value.as_str(), names);
        }
    }
    for cap in template.captures_iter(content) {
        if let Some(value) = cap.get(1) {
            let cleaned = TEMPLATE_CLEANUP.replace_all(value.as_str(), " ");
            add_tokens(&cleaned, names);
            for prefix in TEMPLATE_PREFIX.captures_iter(value.as_str()) {
                if let Some(prefix) = prefix.get(1) {
                    names.insert(prefix.as_str().to_string());
                }
            }
        }
    }
}

fn add_tokens(value: &str, names: &mut HashSet<String>) {
    for token in value
        .split_whitespace()
        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_'))
        .filter(|s| !s.is_empty())
    {
        names.insert(token.to_string());
    }
}

fn parse_blocks(css: &str) -> Vec<(String, String)> {
    let mut blocks = Vec::new();
    let mut start = 0usize;
    while let Some(open_rel) = css[start..].find('{') {
        let open = start + open_rel;
        let selector = css[start..open].trim().to_string();
        let mut depth = 1i32;
        let mut i = open + 1;
        for (offset, ch) in css[i..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        i += offset;
                        break;
                    }
                }
                _ => {}
            }
        }
        if depth == 0 && !selector.is_empty() {
            blocks.push((selector, css[open + 1..i].to_string()));
            start = i + 1;
        } else {
            break;
        }
    }
    blocks
}

fn selector_matches(selector: &str, classnames: &HashSet<String>) -> bool {
    classnames.iter().any(|name| selector.contains(name))
}

fn ext(path: &Path) -> Option<String> {
    path.extension().and_then(|e| e.to_str()).map(|s| s.to_ascii_lowercase())
}

fn canon(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn detect_css_files_includes_common_global_paths_without_imports() {
        let temp = TempDir::new().expect("temp dir");
        fs::create_dir_all(temp.path().join("src")).expect("mkdir src");
        fs::write(temp.path().join("src/styles.css"), ".btn {}").expect("write src styles");
        fs::write(temp.path().join("styles.css"), ".root {}").expect("write root styles");
        fs::write(temp.path().join("src/index.css"), ".index {}").expect("write index css");

        let files = vec![
            test_file(&temp.path().join("src/styles.css")),
            test_file(&temp.path().join("styles.css")),
            test_file(&temp.path().join("src/index.css")),
        ];

        let detected = detect_css_files(temp.path(), &files);
        assert!(
            detected.contains(&temp.path().join("src/styles.css").canonicalize().expect("canon"))
        );
        assert!(detected.contains(&temp.path().join("styles.css").canonicalize().expect("canon")));
        assert!(
            detected.contains(&temp.path().join("src/index.css").canonicalize().expect("canon"))
        );
    }

    #[test]
    fn detect_css_files_resolves_entry_imports_and_common_paths() {
        let temp = TempDir::new().expect("temp dir");
        fs::create_dir_all(temp.path().join("src")).expect("mkdir src");
        fs::write(temp.path().join("src/app.tsx"), "import './global.css';").expect("write app");
        fs::write(temp.path().join("src/global.css"), ".app {}").expect("write global css");
        fs::write(temp.path().join("src/styles.css"), ".fallback {}").expect("write styles css");
        let files = vec![
            test_file(&temp.path().join("src/app.tsx")),
            test_file(&temp.path().join("src/global.css")),
            test_file(&temp.path().join("src/styles.css")),
        ];

        let detected = detect_css_files(temp.path(), &files);

        assert!(
            detected.contains(&temp.path().join("src/global.css").canonicalize().expect("canon"))
        );
        assert!(
            detected.contains(&temp.path().join("src/styles.css").canonicalize().expect("canon"))
        );
    }

    #[test]
    fn extract_classnames_supports_solid_class_attribute() {
        let mut names = HashSet::new();

        extract_from_content(
            r#"<section class="home-page u-stack-md"><div class={'home-tree-wrap'}><span class={`winter-tree-svg ${active}`}></span></div></section>"#,
            &mut names,
        );

        assert!(names.contains("home-page"));
        assert!(names.contains("u-stack-md"));
        assert!(names.contains("home-tree-wrap"));
        assert!(names.contains("winter-tree-svg"));
    }

    fn test_file(path: &Path) -> crate::domain::FileInfo {
        crate::domain::FileInfo {
            path: path.to_path_buf(),
            relative_path: path.to_string_lossy().to_string(),
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

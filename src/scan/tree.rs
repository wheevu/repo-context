//! Directory tree generation.

use crate::utils::normalize_path;
use anyhow::Result;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const SKIPPED_DIRS: &[&str] = &[
    "node_modules",
    "__pycache__",
    ".git",
    "venv",
    ".venv",
    "dist",
    "build",
    "out",
    "target",
    ".tox",
    ".eggs",
];

pub fn generate_tree(
    root_path: &Path,
    max_depth: usize,
    include_files: bool,
    files_to_highlight: &HashSet<String>,
) -> Result<String> {
    let mut lines =
        vec![format!("{}/", root_path.file_name().and_then(|n| n.to_str()).unwrap_or("."))];
    walk_tree(
        root_path,
        root_path,
        "",
        1,
        max_depth,
        include_files,
        files_to_highlight,
        &mut lines,
    )?;
    Ok(lines.join("\n"))
}

fn walk_tree(
    root_path: &Path,
    current_path: &Path,
    prefix: &str,
    depth: usize,
    max_depth: usize,
    include_files: bool,
    files_to_highlight: &HashSet<String>,
    lines: &mut Vec<String>,
) -> Result<()> {
    if depth > max_depth {
        return Ok(());
    }

    let mut entries: Vec<(bool, String, PathBuf)> = fs::read_dir(current_path)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let file_type = entry.file_type().ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path();

            if should_skip_render_entry(&name, file_type.is_dir()) {
                return None;
            }

            Some((file_type.is_dir(), name, path))
        })
        .collect();

    entries.sort_by(|a, b| {
        let dir_cmp = b.0.cmp(&a.0);
        if dir_cmp == std::cmp::Ordering::Equal {
            a.1.cmp(&b.1)
        } else {
            dir_cmp
        }
    });

    let total_entries = entries.len();
    for (idx, (is_dir, name, path)) in entries.into_iter().enumerate() {
        let is_last = idx == total_entries - 1;
        let connector = if is_last { "└── " } else { "├── " };

        let rel_path = path
            .strip_prefix(root_path)
            .ok()
            .and_then(|p| p.to_str())
            .map(normalize_path)
            .unwrap_or_else(|| name.clone());

        let marker = if files_to_highlight.contains(&rel_path) { " ⭐" } else { "" };

        if is_dir {
            lines.push(format!("{}{}{}/{}", prefix, connector, name, marker));
            let extension = if is_last { "    " } else { "│   " };
            walk_tree(
                root_path,
                &path,
                &format!("{}{}", prefix, extension),
                depth + 1,
                max_depth,
                include_files,
                files_to_highlight,
                lines,
            )?;
        } else if include_files {
            lines.push(format!("{}{}{}{}", prefix, connector, name, marker));
        }
    }

    Ok(())
}

fn should_skip_render_entry(name: &str, is_dir: bool) -> bool {
    if name.starts_with('.') && name != ".github" && name != ".env.example" {
        return true;
    }

    if is_dir && SKIPPED_DIRS.contains(&name) {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_generate_tree_includes_dirs_and_files() {
        let tmp = TempDir::new().expect("tmp dir");
        let root = tmp.path();
        fs::create_dir(root.join("src")).expect("mkdir src");
        fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write main");
        fs::write(root.join("README.md"), "# Demo\n").expect("write readme");

        let tree = generate_tree(root, 4, true, &HashSet::new()).expect("tree");
        assert!(tree.contains("src/"));
        assert!(tree.contains("main.rs"));
        assert!(tree.contains("README.md"));
    }

    #[test]
    fn test_generate_tree_skips_known_noise_dirs() {
        let tmp = TempDir::new().expect("tmp dir");
        let root = tmp.path();
        fs::create_dir(root.join("target")).expect("mkdir target");
        fs::write(root.join("target/app"), "bin").expect("write app");
        fs::create_dir(root.join("src")).expect("mkdir src");
        fs::write(root.join("src/lib.rs"), "pub fn x() {}\n").expect("write lib");

        let tree = generate_tree(root, 4, true, &HashSet::new()).expect("tree");
        assert!(!tree.contains("target/"));
        assert!(tree.contains("src/"));
    }
}

//! Cargo workspace discovery helpers.

use crate::utils::{normalize_path, read_file_safe};
use globset::{Glob, GlobSetBuilder};
use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct CrateNode {
    pub name: String,
    pub root: String,
    pub path_deps: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct WorkspaceGraph {
    pub members: Vec<CrateNode>,
    pub member_roots: BTreeSet<String>,
}

pub fn discover_workspace_graph(root: &Path) -> Option<WorkspaceGraph> {
    let root_manifest = root.join("Cargo.toml");
    if !root_manifest.exists() {
        return None;
    }
    let (content, _) = read_file_safe(&root_manifest, None, None).ok()?;
    let value = toml::from_str::<toml::Value>(&content).ok()?;
    let workspace = value.get("workspace")?.as_table()?;
    let members = workspace.get("members")?.as_array()?;

    let mut builder = GlobSetBuilder::new();
    let mut has_patterns = false;
    for member in members.iter().filter_map(toml::Value::as_str) {
        if let Ok(glob) = Glob::new(member) {
            builder.add(glob);
            has_patterns = true;
        }
    }
    if !has_patterns {
        return None;
    }
    let matcher = builder.build().ok()?;

    let mut member_dirs = BTreeSet::new();
    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() || entry.file_name() != "Cargo.toml" {
            continue;
        }
        let path = entry.path();
        if path == root_manifest {
            continue;
        }
        let Some(parent) = path.parent() else {
            continue;
        };
        let Ok(rel) = parent.strip_prefix(root) else {
            continue;
        };
        let rel_norm = normalize_path(rel.to_string_lossy().as_ref());
        if matcher.is_match(&rel_norm) {
            member_dirs.insert(rel_norm);
        }
    }
    if member_dirs.is_empty() {
        return None;
    }

    let mut members_out = Vec::new();
    for member_root in &member_dirs {
        let manifest_path = root.join(member_root).join("Cargo.toml");
        let Some(member) = parse_member_manifest(root, member_root, &manifest_path) else {
            continue;
        };
        members_out.push(member);
    }
    if members_out.is_empty() {
        return None;
    }

    let root_by_name: HashMap<String, String> =
        members_out.iter().map(|m| (m.name.clone(), m.root.clone())).collect();
    for member in &mut members_out {
        member.path_deps = member
            .path_deps
            .iter()
            .filter_map(|dep_root| {
                root_by_name
                    .iter()
                    .find(|(_, root)| *root == dep_root)
                    .map(|(name, _)| name.clone())
            })
            .collect();
    }

    Some(WorkspaceGraph { members: members_out, member_roots: member_dirs })
}

fn parse_member_manifest(
    root: &Path,
    member_root: &str,
    manifest_path: &Path,
) -> Option<CrateNode> {
    let (content, _) = read_file_safe(manifest_path, None, None).ok()?;
    let value = toml::from_str::<toml::Value>(&content).ok()?;
    let package = value.get("package")?.as_table()?;
    let name = package.get("name")?.as_str()?.to_string();

    let mut path_deps = Vec::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = value.get(section).and_then(toml::Value::as_table) {
            for dep in table.values() {
                if let Some(dep_path) =
                    dep.as_table().and_then(|t| t.get("path")).and_then(toml::Value::as_str)
                {
                    let absolute = root.join(member_root).join(dep_path);
                    let absolute_clean = clean_path(&absolute);
                    if let Ok(rel) = absolute_clean.strip_prefix(root) {
                        path_deps.push(normalize_path(rel.to_string_lossy().as_ref()));
                    }
                }
            }
        }
    }

    Some(CrateNode { name, root: member_root.to_string(), path_deps })
}

fn clean_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
            Component::RootDir => out.push(Path::new("/")),
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::discover_workspace_graph;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn discovers_workspace_members_and_path_deps() {
        let tmp = TempDir::new().expect("tmp");
        fs::write(tmp.path().join("Cargo.toml"), "[workspace]\nmembers=[\"crates/*\"]\n")
            .expect("workspace cargo");

        fs::create_dir_all(tmp.path().join("crates/a")).expect("mkdir a");
        fs::create_dir_all(tmp.path().join("crates/b")).expect("mkdir b");
        fs::write(
            tmp.path().join("crates/a/Cargo.toml"),
            "[package]\nname=\"a\"\nversion=\"0.1.0\"\n[dependencies]\nb={path=\"../b\"}\n",
        )
        .expect("cargo a");
        fs::write(
            tmp.path().join("crates/b/Cargo.toml"),
            "[package]\nname=\"b\"\nversion=\"0.1.0\"\n",
        )
        .expect("cargo b");

        let graph = discover_workspace_graph(tmp.path()).expect("workspace graph");
        assert!(graph.member_roots.contains("crates/a"));
        assert!(graph.member_roots.contains("crates/b"));
        let a = graph.members.iter().find(|m| m.name == "a").expect("member a");
        assert!(a.path_deps.contains(&"b".to_string()));
    }
}

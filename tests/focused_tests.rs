//! Integration tests for focused export mode.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

/// Helper: run a focused export with a given focus file.
#[allow(dead_code)]
fn run_focused(repo_root: &Path, output_dir: &Path, focus_file: &str, expect_success: bool) {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        repo_root.to_str().expect("repo root"),
        "--mode",
        "rag",
        "--scan-mode",
        "focused",
        "--focus-file",
        focus_file,
        "--output-dir",
        output_dir.to_str().expect("output dir"),
        "--no-timestamp",
        "--chunk-tokens",
        "200",
    ]);
    cmd.env("HOME", output_dir);

    let result = cmd.assert();
    if expect_success {
        result.success();
    } else {
        result.failure();
    }
}

#[test]
fn focused_export_produces_scoped_output() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(root.join("README.md"), "# Project\n").expect("write readme");
    fs::write(root.join("src/main.rs"), "mod app;\nfn main() {}\n").expect("write main");
    fs::write(root.join("src/app.rs"), "pub fn run() {}\n").expect("write app");

    let out = TempDir::new().expect("temp out");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        root.to_str().expect("root"),
        "--mode",
        "rag",
        "--scan-mode",
        "focused",
        "--focus-file",
        "src/main.rs",
        "--output-dir",
        out.path().to_str().expect("out"),
        "--no-timestamp",
    ]);
    cmd.env("HOME", out.path());
    cmd.assert().success();

    let repo_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let actual = out.path().join(repo_name).join("focus_main");
    let jsonl = fs::read_to_string(actual.join(format!("{repo_name}_focus_main_chunks.jsonl")))
        .expect("read jsonl");

    // The focused output should contain the selected file.
    assert!(jsonl.contains("src/main.rs"), "should contain selected file");
    // It should also contain the dependency (app.rs via mod app).
    assert!(jsonl.contains("src/app.rs"), "should contain dependency");
    // README should NOT be included since it's not in the focus scope.
    assert!(!jsonl.contains("README.md"), "README should be excluded from focused scope");
}

#[test]
fn focused_budget_report_lists_reasons_only_for_emitted_files() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(root.join("src/main.rs"), "mod one;\nmod two;\nfn main() {}\n").expect("main");
    fs::write(root.join("src/one.rs"), "pub fn one() {}\n".repeat(100)).expect("one");
    fs::write(root.join("src/two.rs"), "pub fn two() {}\n".repeat(100)).expect("two");

    let out = TempDir::new().expect("temp out");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        root.to_str().expect("root"),
        "--mode",
        "rag",
        "--scan-mode",
        "focused",
        "--focus-file",
        "src/main.rs",
        "--max-tokens",
        "500",
        "--output-dir",
        out.path().to_str().expect("out"),
        "--no-timestamp",
    ]);
    cmd.env("HOME", out.path());
    cmd.assert().success();

    let repo_name = root.file_name().and_then(|name| name.to_str()).unwrap_or("repo");
    let report: Value = serde_json::from_str(
        &fs::read_to_string(
            out.path()
                .join(repo_name)
                .join("focus_main")
                .join(format!("{repo_name}_focus_main_report.json")),
        )
        .expect("report"),
    )
    .expect("valid report");
    let mut files: Vec<&str> = report["files"]
        .as_array()
        .expect("files")
        .iter()
        .filter_map(|file| file["path"].as_str())
        .collect();
    let mut reasons: Vec<&str> = report["focus"]["included_reasons"]
        .as_object()
        .expect("reasons")
        .keys()
        .map(String::as_str)
        .collect();
    files.sort_unstable();
    reasons.sort_unstable();

    assert!(!files.is_empty());
    assert!(files.len() < 3, "budget should drop at least one focus candidate");
    assert_eq!(reasons, files);
}

#[test]
fn focused_export_with_invalid_focus_file_errors() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write main");

    let out = TempDir::new().expect("temp out");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        root.to_str().expect("root"),
        "--mode",
        "rag",
        "--scan-mode",
        "focused",
        "--focus-file",
        "nonexistent.rs",
        "--output-dir",
        out.path().to_str().expect("out"),
        "--no-timestamp",
    ]);
    cmd.env("HOME", out.path());
    cmd.assert().failure().stderr(predicate::str::contains("matched no scanned files"));
}

#[test]
fn focused_export_handles_directory_candidate() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    let pages_dir = root.join("src/pages");
    fs::create_dir_all(&pages_dir).expect("mkdir pages");
    fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write main");
    fs::write(pages_dir.join("index.tsx"), "export default function Home() {}\n")
        .expect("write index");
    fs::write(pages_dir.join("about.tsx"), "export default function About() {}\n")
        .expect("write about");

    let out = TempDir::new().expect("temp out");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        root.to_str().expect("root"),
        "--mode",
        "rag",
        "--scan-mode",
        "focused",
        "--focus-file",
        "src/pages",
        "--output-dir",
        out.path().to_str().expect("out"),
        "--no-timestamp",
    ]);
    cmd.env("HOME", out.path());
    cmd.assert().success();

    let repo_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let actual = out.path().join(repo_name).join("focus_pages");
    let jsonl = fs::read_to_string(actual.join(format!("{repo_name}_focus_pages_chunks.jsonl")))
        .expect("read jsonl");

    // Both TSX files in the pages directory should be included.
    assert!(jsonl.contains("src/pages/index.tsx"), "should contain index.tsx");
    assert!(jsonl.contains("src/pages/about.tsx"), "should contain about.tsx");
    // src/main.rs should not be in scope.
    assert!(!jsonl.contains("src/main.rs"), "main.rs should not be in directory scope");
}

#[test]
fn focused_python_export_includes_local_dependencies_callers_and_tests() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    fs::create_dir_all(root.join("api/app")).expect("mkdir app");
    fs::create_dir_all(root.join("api/tests")).expect("mkdir tests");
    fs::write(root.join("api/app/main.py"), "from app.services import run\nfrom . import models\n")
        .expect("write main");
    fs::write(root.join("api/app/services.py"), "def run(): pass\n").expect("write service");
    fs::write(root.join("api/app/models.py"), "class Model: pass\n").expect("write model");
    fs::write(root.join("api/app/cli.py"), "from app.main import main\n").expect("write caller");
    fs::write(root.join("api/tests/test_main.py"), "from app.main import main\n")
        .expect("write test");
    fs::write(root.join("api/app/unrelated.py"), "VALUE = 1\n").expect("write unrelated");

    let out = TempDir::new().expect("temp out");
    run_focused(root, out.path(), "api/app/main.py", true);

    let repo_name = root.file_name().and_then(|name| name.to_str()).unwrap_or("repo");
    let jsonl = fs::read_to_string(
        out.path()
            .join(repo_name)
            .join("focus_main")
            .join(format!("{repo_name}_focus_main_chunks.jsonl")),
    )
    .expect("read jsonl");

    for expected in [
        "api/app/main.py",
        "api/app/services.py",
        "api/app/models.py",
        "api/app/cli.py",
        "api/tests/test_main.py",
    ] {
        assert!(jsonl.contains(expected), "focused output should contain {expected}");
    }
    assert!(!jsonl.contains("api/app/unrelated.py"));
}

#[test]
fn focused_svelte_export_includes_local_components_typescript_and_tests() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    fs::create_dir_all(root.join("src/lib")).expect("mkdir lib");
    fs::write(
        root.join("src/Page.svelte"),
        "<script>import Card from './lib/Card.svelte';</script>\n<Card />\n",
    )
    .expect("write page");
    fs::write(
        root.join("src/lib/Card.svelte"),
        "<script lang=\"ts\">import { label } from './label';</script>\n<p>{label}</p>\n",
    )
    .expect("write card");
    fs::write(root.join("src/lib/label.ts"), "export const label = 'Card';\n")
        .expect("write label");
    fs::write(root.join("src/lib/Card.spec.ts"), "import Card from './Card.svelte';\n")
        .expect("write test");

    let out = TempDir::new().expect("temp out");
    run_focused(root, out.path(), "src/lib/Card.svelte", true);

    let repo_name = root.file_name().and_then(|name| name.to_str()).unwrap_or("repo");
    let jsonl = fs::read_to_string(
        out.path()
            .join(repo_name)
            .join("focus_Card")
            .join(format!("{repo_name}_focus_Card_chunks.jsonl")),
    )
    .expect("read jsonl");

    for expected in
        ["src/lib/Card.svelte", "src/lib/label.ts", "src/Page.svelte", "src/lib/Card.spec.ts"]
    {
        assert!(jsonl.contains(expected), "focused output should contain {expected}");
    }
}

#[test]
fn focused_godot_main_scene_follows_transitive_resource_edges() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    fs::create_dir_all(root.join("scripts")).expect("mkdir scripts");
    fs::create_dir_all(root.join("data")).expect("mkdir data");
    fs::write(
        root.join("project.godot"),
        "config_version=5\n[application]\nrun/main_scene=\"res://main.tscn\"\n",
    )
    .expect("write project");
    fs::write(
        root.join("main.tscn"),
        "[gd_scene load_steps=2 format=3]\n[ext_resource path=\"res://scripts/player.gd\" type=\"Script\" id=\"1\"]\n[node name=\"Main\" type=\"Node\"]\nscript = ExtResource(\"1\")\n",
    )
    .expect("write scene");
    fs::write(
        root.join("scripts/player.gd"),
        "extends Node\nconst STATS = preload(\"res://data/stats.tres\")\n",
    )
    .expect("write script");
    fs::write(root.join("data/stats.tres"), "[gd_resource type=\"Resource\" format=3]\n")
        .expect("write resource");
    fs::write(root.join("scripts/unrelated.gd"), "extends Node\n").expect("write unrelated");

    let out = TempDir::new().expect("temp out");
    run_focused(root, out.path(), "main.tscn", true);

    let repo_name = root.file_name().and_then(|name| name.to_str()).unwrap_or("repo");
    let jsonl = fs::read_to_string(
        out.path()
            .join(repo_name)
            .join("focus_main")
            .join(format!("{repo_name}_focus_main_chunks.jsonl")),
    )
    .expect("read jsonl");

    for expected in ["main.tscn", "scripts/player.gd", "data/stats.tres"] {
        assert!(jsonl.contains(expected), "focused output should contain {expected}");
    }
    assert!(!jsonl.contains("scripts/unrelated.gd"));
}

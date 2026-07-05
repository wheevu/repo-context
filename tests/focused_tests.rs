//! Integration tests for focused export mode.

use assert_cmd::Command;
use predicates::prelude::*;
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

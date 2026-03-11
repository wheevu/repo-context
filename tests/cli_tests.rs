//! Integration tests for stable CLI commands.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_cli_help_lists_stable_commands() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("export"))
        .stdout(predicate::str::contains("info"))
        .stdout(predicate::str::contains("Convert repositories"));
}

#[test]
fn test_export_requires_path_or_repo() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.arg("export");
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Either --path or --repo must be specified"));
}

#[test]
fn test_export_rejects_both_path_and_repo() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args(["export", "--path", ".", "--repo", "https://github.com/test/test"]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Cannot specify both --path and --repo"));
}

#[test]
fn test_export_writes_core_artifacts() {
    let repo = TempDir::new().expect("temp repo");
    fs::create_dir_all(repo.path().join("src")).expect("mkdir src");
    fs::write(repo.path().join("src/main.rs"), "fn main() { println!(\"hi\"); }\n")
        .expect("write source");
    fs::write(repo.path().join("README.md"), "# Demo\n").expect("write readme");

    let out = TempDir::new().expect("temp out");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        repo.path().to_str().expect("repo path"),
        "--output-dir",
        out.path().to_str().expect("out path"),
        "--no-timestamp",
    ]);
    cmd.assert().success();

    let repo_name = repo.path().file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let actual = out.path().join(repo_name);

    assert!(actual.join(format!("{}_context_pack.md", repo_name)).exists());
    assert!(actual.join(format!("{}_chunks.jsonl", repo_name)).exists());
    assert!(actual.join(format!("{}_report.json", repo_name)).exists());
}

#[test]
fn test_info_succeeds_on_small_repo() {
    let repo = TempDir::new().expect("temp repo");
    fs::create_dir_all(repo.path().join("src")).expect("mkdir src");
    fs::write(repo.path().join("src/lib.rs"), "pub fn ping() {}\n").expect("write lib");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args(["info", repo.path().to_str().expect("repo path")]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Repository:"))
        .stdout(predicate::str::contains("Statistics:"));
}

//! Integration tests for CLI

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn test_cli_version() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-to-prompt"));
    cmd.arg("--version");
    cmd.assert().success().stdout(predicate::str::contains("repo-to-prompt"));
}

#[test]
fn test_cli_help() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-to-prompt"));
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Convert repositories"))
        .stdout(predicate::str::contains("export"))
        .stdout(predicate::str::contains("info"));
}

#[test]
fn test_export_requires_path_or_repo() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-to-prompt"));
    cmd.arg("export");
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Either --path or --repo must be specified"));
}

#[test]
fn test_export_rejects_both_path_and_repo() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-to-prompt"));
    cmd.args(["export", "--path", ".", "--repo", "https://github.com/test/test"]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Cannot specify both --path and --repo"));
}

#[test]
fn test_export_rejects_invalid_redaction_mode() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-to-prompt"));
    cmd.args(["export", "--path", ".", "--redaction-mode", "invalid"]);
    cmd.assert().failure().stderr(predicate::str::contains("Invalid redaction mode"));
}

#[test]
fn test_info_reports_tree_sitter_capabilities() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-to-prompt"));
    cmd.args(["info", "."]);
    cmd.assert().success().stdout(predicate::str::contains("Statistics:"));
}

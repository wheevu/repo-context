//! Integration tests for CLI

use assert_cmd::Command;
use predicates::prelude::*;
use rusqlite::Connection;
use std::fs;
use std::process::Command as StdCommand;
use tempfile::TempDir;

#[test]
fn test_cli_version() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.arg("--version");
    cmd.assert().success().stdout(predicate::str::contains("repo-context"));
}

#[test]
fn test_cli_help() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Convert repositories"))
        .stdout(predicate::str::contains("export"))
        .stdout(predicate::str::contains("info"))
        .stdout(predicate::str::contains("index"))
        .stdout(predicate::str::contains("query"))
        .stdout(predicate::str::contains("codeintel"))
        .stdout(predicate::str::contains("diff"));
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
fn test_export_rejects_invalid_redaction_mode() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args(["export", "--path", ".", "--redaction-mode", "invalid"]);
    cmd.assert().failure().stderr(predicate::str::contains("Invalid redaction mode"));
}

#[test]
fn test_info_reports_tree_sitter_capabilities() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args(["info", "."]);
    cmd.assert().success().stdout(predicate::str::contains("Statistics:"));
}

#[test]
fn test_export_accepts_contribution_mode() {
    let out = TempDir::new().expect("temp out dir");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        ".",
        "--mode",
        "contribution",
        "--max-tokens",
        "10",
        "--allow-over-budget",
        "--output-dir",
        out.path().to_str().expect("utf8 path"),
        "--no-timestamp",
    ]);
    cmd.assert().success();
}

#[test]
fn test_diff_compares_two_exports() {
    let before = TempDir::new().expect("temp before");
    let after = TempDir::new().expect("temp after");

    fs::write(
        before.path().join("report.json"),
        r#"{"schema_version":"1.0.0","stats":{},"config":{},"output_files":[],"files":[{"id":"a1","path":"src/a.rs","priority":0.75,"tokens":10}]}"#,
    )
    .expect("write before report");
    fs::write(
        after.path().join("report.json"),
        r#"{"schema_version":"1.0.0","stats":{},"config":{},"output_files":[],"files":[{"id":"a2","path":"src/a.rs","priority":0.8,"tokens":12},{"id":"b1","path":"src/b.rs","priority":0.6,"tokens":5}]}"#,
    )
    .expect("write after report");

    fs::write(
        before.path().join("chunks.jsonl"),
        r#"{"content":"x","end_line":1,"id":"c1","lang":"rust","path":"src/a.rs","priority":0.7,"start_line":1,"tags":["def:a"]}"#,
    )
    .expect("write before chunks");
    fs::write(
        after.path().join("chunks.jsonl"),
        r#"{"content":"x","end_line":1,"id":"c1","lang":"rust","path":"src/a.rs","priority":0.7,"start_line":1,"tags":["def:a","async:await"]}
{"content":"y","end_line":2,"id":"c2","lang":"rust","path":"src/b.rs","priority":0.6,"start_line":1,"tags":["def:b"]}"#,
    )
    .expect("write after chunks");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "diff",
        before.path().to_str().expect("before path"),
        after.path().to_str().expect("after path"),
    ]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("Files: +1 added, -0 removed, 1 modified"))
        .stdout(predicate::str::contains("Changed chunk tags: 1"));
}

#[test]
fn test_diff_json_output() {
    let before = TempDir::new().expect("temp before");
    let after = TempDir::new().expect("temp after");
    fs::write(
        before.path().join("report.json"),
        r#"{"schema_version":"1.0.0","stats":{},"config":{},"output_files":[],"files":[]}"#,
    )
    .expect("write before report");
    fs::write(
        after.path().join("report.json"),
        r#"{"schema_version":"1.0.0","stats":{},"config":{},"output_files":[],"files":[{"id":"x","path":"src/x.rs","priority":0.9,"tokens":3}]}"#,
    )
    .expect("write after report");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "diff",
        before.path().to_str().expect("before path"),
        after.path().to_str().expect("after path"),
        "--format",
        "json",
    ]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("\"files_added\": 1"))
        .stdout(predicate::str::contains("\"files_removed\": 0"));
}

#[test]
fn test_index_creates_sqlite_database_with_symbols() {
    let repo = TempDir::new().expect("temp repo dir");
    fs::create_dir_all(repo.path().join("src")).expect("mkdir src");
    fs::create_dir_all(repo.path().join("tests")).expect("mkdir tests");
    fs::write(repo.path().join("src/auth.py"), "def refresh_token(user):\n    return user\n")
        .expect("write source file");
    fs::write(
        repo.path().join("tests/test_auth.py"),
        "from src.auth import refresh_token\n\ndef test_refresh_token():\n    assert refresh_token('x')\n",
    )
    .expect("write test file");

    let db_path = repo.path().join("index.sqlite");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "index",
        "--path",
        repo.path().to_str().expect("utf8 repo path"),
        "--db",
        db_path.to_str().expect("utf8 db path"),
        "--chunk-tokens",
        "64",
        "--chunk-overlap",
        "8",
    ]);
    cmd.assert().success().stdout(predicate::str::contains("Index created at"));

    let mut cmd_again = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd_again.args([
        "index",
        "--path",
        repo.path().to_str().expect("utf8 repo path"),
        "--db",
        db_path.to_str().expect("utf8 db path"),
    ]);
    cmd_again.assert().success().stdout(predicate::str::contains("files reused: 2"));

    let conn = Connection::open(&db_path).expect("open sqlite");
    let file_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0)).expect("count files");
    assert!(file_count >= 2);

    let chunk_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0)).expect("count chunks");
    assert!(chunk_count >= 2);

    let symbol_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbols WHERE symbol = 'refresh_token' AND kind = 'def'",
            [],
            |row| row.get(0),
        )
        .expect("count symbols");
    assert!(symbol_count >= 1);

    let mut query_cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    query_cmd.args([
        "query",
        "--db",
        db_path.to_str().expect("utf8 db path"),
        "--task",
        "refresh token",
        "--limit",
        "5",
    ]);
    query_cmd
        .assert()
        .success()
        .stdout(predicate::str::contains("Top matches for task"))
        .stdout(predicate::str::contains("src/auth.py"));

    let out_path = repo.path().join("codeintel.json");
    let mut codeintel_cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    codeintel_cmd.args([
        "codeintel",
        "--db",
        db_path.to_str().expect("utf8 db path"),
        "--out",
        out_path.to_str().expect("utf8 out path"),
    ]);
    codeintel_cmd
        .assert()
        .success()
        .stdout(predicate::str::contains("Code-intel export written to"));

    let exported = fs::read_to_string(&out_path).expect("read codeintel output");
    let doc: serde_json::Value = serde_json::from_str(&exported).expect("parse codeintel json");
    assert_eq!(doc.get("format").and_then(|v| v.as_str()), Some("scip-lite"));
    assert_eq!(doc.get("schema_version").and_then(|v| v.as_str()), Some("0.4.0"));
    assert!(doc
        .get("symbols")
        .and_then(|v| v.as_array())
        .map(|arr| !arr.is_empty())
        .unwrap_or(false));
    assert!(doc
        .get("occurrences")
        .and_then(|v| v.as_array())
        .map(|arr| !arr.is_empty())
        .unwrap_or(false));
    assert!(doc
        .get("relationships")
        .and_then(|v| v.as_array())
        .map(|arr| !arr.is_empty())
        .unwrap_or(false));
    assert!(doc.get("symbol_links").and_then(|v| v.as_array()).is_some());
    assert!(doc.get("stats").and_then(|v| v.as_object()).is_some());
}

#[test]
fn test_index_lsp_creates_symbol_edges_when_available() {
    if !rust_analyzer_available() {
        eprintln!("skipping LSP integration test: rust-analyzer not available");
        return;
    }

    let repo = TempDir::new().expect("temp repo dir");
    fs::create_dir_all(repo.path().join("src")).expect("mkdir src");
    fs::create_dir_all(repo.path().join("tests")).expect("mkdir tests");
    fs::write(
        repo.path().join("Cargo.toml"),
        "[package]\nname = \"lsp-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n",
    )
    .expect("write cargo manifest");
    fs::write(repo.path().join("src/lib.rs"), "pub mod auth;\npub mod handler;\n")
        .expect("write lib.rs");
    fs::write(
        repo.path().join("src/auth.rs"),
        "pub fn refresh_token(user: &str) -> String {\n    user.to_string()\n}\n",
    )
    .expect("write auth.rs");
    fs::write(
        repo.path().join("src/handler.rs"),
        "use crate::auth::refresh_token;\n\npub fn handle(user: &str) -> String {\n    refresh_token(user)\n}\n",
    )
    .expect("write handler.rs");
    fs::write(
        repo.path().join("tests/auth_test.rs"),
        "use lsp_fixture::handler::handle;\n\n#[test]\nfn test_handle() {\n    assert_eq!(handle(\"x\"), \"x\");\n}\n",
    )
    .expect("write integration test");

    let db_path = repo.path().join("index.sqlite");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "index",
        "--path",
        repo.path().to_str().expect("utf8 repo path"),
        "--db",
        db_path.to_str().expect("utf8 db path"),
        "--lsp",
        "--chunk-tokens",
        "64",
        "--chunk-overlap",
        "8",
    ]);
    cmd.assert().success().stdout(predicate::str::contains("lsp edges indexed:"));

    let conn = Connection::open(&db_path).expect("open sqlite");
    let symbol_edges_table: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='symbol_edges'",
            [],
            |row| row.get(0),
        )
        .expect("symbol_edges table exists");
    assert_eq!(symbol_edges_table, 1);

    let invalid_edge_kinds: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbol_edges WHERE kind NOT IN ('ref', 'call', 'test', 'import')",
            [],
            |row| row.get(0),
        )
        .expect("edge kinds check");
    assert_eq!(invalid_edge_kinds, 0);

    let indexed_mtime_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE extension = '.rs' AND mtime IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .expect("mtime count");
    assert!(indexed_mtime_count >= 3);
}

fn rust_analyzer_available() -> bool {
    StdCommand::new("rust-analyzer")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

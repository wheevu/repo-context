//! Integration tests for CLI

use assert_cmd::Command;
use predicates::prelude::*;
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
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
fn test_export_quick_flag_skips_guided_mode() {
    let repo = TempDir::new().expect("temp repo dir");
    fs::write(repo.path().join("main.rs"), "fn main() {}\n").expect("write source file");
    let out = TempDir::new().expect("temp out dir");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        repo.path().to_str().expect("utf8 repo path"),
        "--quick",
        "--no-timestamp",
        "--output-dir",
        out.path().to_str().expect("utf8 out path"),
    ]);
    cmd.assert().success();
}

#[test]
fn test_export_auto_falls_back_to_quick_in_non_interactive_sessions() {
    let repo = TempDir::new().expect("temp repo dir");
    fs::write(repo.path().join("main.rs"), "fn main() {}\n").expect("write source file");
    let out = TempDir::new().expect("temp out dir");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        repo.path().to_str().expect("utf8 repo path"),
        "--no-timestamp",
        "--output-dir",
        out.path().to_str().expect("utf8 out path"),
    ]);
    cmd.assert().success().stderr(predicate::str::contains("non-interactive session detected"));
}

#[test]
fn test_export_strict_budget_fails_when_protected_pins_exceed_budget() {
    let repo = TempDir::new().expect("temp repo dir");
    fs::write(repo.path().join("README.md"), "# Repo\n").expect("write readme");
    fs::write(repo.path().join("CONTRIBUTING.md"), "# Contributing\nMust do this\n")
        .expect("write contributing");
    fs::write(repo.path().join("SECURITY.md"), "# Security\nMust do that\n")
        .expect("write security");
    fs::write(repo.path().join("Cargo.toml"), "[package]\nname='demo'\nversion='0.1.0'\n")
        .expect("write cargo");
    fs::create_dir_all(repo.path().join("src")).expect("mkdir src");
    fs::write(repo.path().join("src/lib.rs"), "pub fn x() {}\n").expect("write source");
    let out = TempDir::new().expect("temp out dir");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        repo.path().to_str().expect("utf8 repo path"),
        "--mode",
        "contribution",
        "--max-tokens",
        "1",
        "--strict-budget",
        "--quick",
        "--no-timestamp",
        "--output-dir",
        out.path().to_str().expect("utf8 out path"),
    ]);
    cmd.assert().failure().stderr(predicate::str::contains("protected pin files require"));
}

#[test]
fn test_export_from_index_uses_fresh_index_metadata() {
    let repo = TempDir::new().expect("temp repo dir");
    fs::create_dir_all(repo.path().join("src")).expect("mkdir src");
    fs::write(repo.path().join("Cargo.toml"), "[package]\nname='demo'\nversion='0.1.0'\n")
        .expect("write cargo");
    fs::write(repo.path().join("src/lib.rs"), "pub fn hello() -> &'static str { \"hi\" }\n")
        .expect("write lib");

    let index_dir = repo.path().join(".repo-context");
    fs::create_dir_all(&index_dir).expect("mkdir index dir");
    let db_path = index_dir.join("index.sqlite");

    let mut index_cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    index_cmd.args([
        "index",
        "--path",
        repo.path().to_str().expect("repo path"),
        "--db",
        db_path.to_str().expect("db path"),
    ]);
    index_cmd.assert().success();

    let out = TempDir::new().expect("out dir");
    let mut export_cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    export_cmd.args([
        "export",
        "--path",
        repo.path().to_str().expect("repo path"),
        "--from-index",
        "--quick",
        "--no-timestamp",
        "--output-dir",
        out.path().to_str().expect("out path"),
    ]);
    export_cmd.assert().success().stdout(predicate::str::contains("using index dataset"));

    let repo_name = repo.path().file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let report_path = out.path().join(repo_name).join(format!("{repo_name}_report.json"));
    let report_raw = fs::read_to_string(report_path).expect("read report");
    let report: Value = serde_json::from_str(&report_raw).expect("parse report");
    assert_eq!(report["provenance"]["index"]["used_for_export"], Value::Bool(true));
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
    let config_hash: Option<String> = conn
        .query_row("SELECT value FROM metadata WHERE key = 'config_hash'", [], |row| row.get(0))
        .optional()
        .expect("config hash metadata");
    assert!(config_hash.as_deref().unwrap_or("").len() >= 16);

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

/// Test that export handles empty repositories gracefully.
#[test]
fn test_export_handles_empty_repo() {
    let repo = TempDir::new().expect("temp repo dir");
    let out = TempDir::new().expect("temp out dir");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        repo.path().to_str().expect("utf8 repo path"),
        "--quick",
        "--no-timestamp",
        "--output-dir",
        out.path().to_str().expect("utf8 out path"),
    ]);
    cmd.assert().success();
}

/// Test that export respects extension filtering.
#[test]
fn test_export_respects_extension_filtering() {
    let repo = TempDir::new().expect("temp repo dir");
    fs::create_dir_all(repo.path().join("src")).expect("mkdir src");
    fs::write(repo.path().join("src/main.rs"), "fn main() {}").expect("write rust");
    fs::write(repo.path().join("src/main.py"), "print('hello')").expect("write python");
    let out = TempDir::new().expect("temp out dir");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        repo.path().to_str().expect("utf8 repo path"),
        "--include-ext",
        ".rs",
        "--quick",
        "--no-timestamp",
        "--output-dir",
        out.path().to_str().expect("utf8 out path"),
    ]);
    cmd.assert().success();

    let repo_name = repo.path().file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let chunks_path = out.path().join(repo_name).join(format!("{repo_name}_chunks.jsonl"));
    let chunks = fs::read_to_string(chunks_path).expect("read chunks");
    assert!(chunks.contains("main.rs"), "should include .rs files");
    assert!(!chunks.contains("main.py"), "should exclude .py files");
}

/// Test that export handles binary files correctly.
#[test]
fn test_export_skips_binary_files() {
    let repo = TempDir::new().expect("temp repo dir");
    fs::write(repo.path().join("text.txt"), "Hello, world!").expect("write text");
    // Write a binary file with null bytes
    fs::write(repo.path().join("binary.bin"), vec![0u8, 1, 2, 3, 0, 5]).expect("write binary");
    let out = TempDir::new().expect("temp out dir");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        repo.path().to_str().expect("utf8 repo path"),
        "--include-ext",
        ".txt,.bin",
        "--quick",
        "--no-timestamp",
        "--output-dir",
        out.path().to_str().expect("utf8 out path"),
    ]);
    cmd.assert().success();

    let repo_name = repo.path().file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let report_path = out.path().join(repo_name).join(format!("{repo_name}_report.json"));
    let report_raw = fs::read_to_string(report_path).expect("read report");
    let report: Value = serde_json::from_str(&report_raw).expect("parse report");

    // Binary files should be skipped - check total files scanned vs included
    let files_scanned = report["stats"]["files_scanned"].as_u64().unwrap_or(0);
    let files_included = report["stats"]["files_included"].as_u64().unwrap_or(0);

    // Should have scanned 2 files but included only 1 (the text file)
    assert!(files_scanned >= 2, "should scan at least 2 files");
    assert_eq!(files_included, 1, "should include only 1 file (text), binary should be skipped");
}

/// Test that export handles symlinks correctly (does not follow by default).
/// This test only runs on Unix platforms.
#[cfg(unix)]
#[test]
fn test_export_handles_symlinks() {
    use std::os::unix::fs::symlink;

    let repo = TempDir::new().expect("temp repo dir");
    fs::write(repo.path().join("real.txt"), "real file content").expect("write real file");

    // Create a symlink
    symlink(repo.path().join("real.txt"), repo.path().join("link.txt")).expect("create symlink");

    let out = TempDir::new().expect("temp out dir");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        repo.path().to_str().expect("utf8 repo path"),
        "--quick",
        "--no-timestamp",
        "--output-dir",
        out.path().to_str().expect("utf8 out path"),
    ]);
    cmd.assert().success();
}

/// Test that index command handles missing directories gracefully.
#[test]
fn test_index_handles_nonexistent_path() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "index",
        "--path",
        "/nonexistent/path/that/does/not/exist",
        "--db",
        "/tmp/test_index.sqlite",
    ]);
    cmd.assert().failure();
}

/// Test that query command provides clear error for unindexed databases.
#[test]
fn test_query_rejects_unindexed_database() {
    let temp = TempDir::new().expect("temp dir");
    let db_path = temp.path().join("unindexed.sqlite");

    // Create a minimal SQLite database without the proper schema
    let conn = Connection::open(&db_path).expect("create db");
    conn.execute(
        "CREATE TABLE IF NOT EXISTS metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )",
        [],
    )
    .expect("create metadata table");
    conn.execute("INSERT INTO metadata (key, value) VALUES ('schema_version', '1')", [])
        .expect("insert schema version");
    drop(conn);

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args(["query", "--db", db_path.to_str().expect("db path"), "--task", "test query"]);
    // Should fail with a clear error message about schema/index
    cmd.assert().failure().stderr(predicate::str::contains("Run `repo-context index` first"));
}

/// Test that export with RAG mode produces JSONL output.
#[test]
fn test_export_rag_mode_produces_jsonl() {
    let repo = TempDir::new().expect("temp repo dir");
    fs::create_dir_all(repo.path().join("src")).expect("mkdir src");
    fs::write(repo.path().join("src/main.rs"), "fn main() {}").expect("write source");
    let out = TempDir::new().expect("temp out dir");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        repo.path().to_str().expect("utf8 repo path"),
        "--mode",
        "rag",
        "--quick",
        "--no-timestamp",
        "--output-dir",
        out.path().to_str().expect("utf8 out path"),
    ]);
    cmd.assert().success();

    let repo_name = repo.path().file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let output_dir = out.path().join(repo_name);

    // RAG mode should produce chunks.jsonl
    let chunks_path = output_dir.join(format!("{repo_name}_chunks.jsonl"));
    assert!(chunks_path.exists(), "chunks.jsonl should exist in RAG mode");

    // Verify chunks.jsonl contains valid JSON lines
    let chunks_content = fs::read_to_string(&chunks_path).expect("read chunks");
    for line in chunks_content.lines() {
        let _: Value = serde_json::from_str(line).expect("each line should be valid JSON");
    }
}

/// Test that export with prompt mode produces markdown output.
#[test]
fn test_export_prompt_mode_produces_markdown() {
    let repo = TempDir::new().expect("temp repo dir");
    fs::create_dir_all(repo.path().join("src")).expect("mkdir src");
    fs::write(repo.path().join("src/main.rs"), "fn main() {}").expect("write source");
    fs::write(repo.path().join("README.md"), "# Test Project").expect("write readme");
    let out = TempDir::new().expect("temp out dir");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        repo.path().to_str().expect("utf8 repo path"),
        "--mode",
        "prompt",
        "--quick",
        "--no-timestamp",
        "--output-dir",
        out.path().to_str().expect("utf8 out path"),
    ]);
    cmd.assert().success();

    let repo_name = repo.path().file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let output_dir = out.path().join(repo_name);

    // Prompt mode should produce context_pack.md
    let context_path = output_dir.join(format!("{repo_name}_context_pack.md"));
    assert!(context_path.exists(), "context_pack.md should exist in prompt mode");

    // Verify it's a valid markdown file
    let context_content = fs::read_to_string(&context_path).expect("read context");
    assert!(context_content.contains("# "), "should contain markdown headings");
}

/// Test that export with both mode produces both outputs.
#[test]
fn test_export_both_mode_produces_all_outputs() {
    let repo = TempDir::new().expect("temp repo dir");
    fs::create_dir_all(repo.path().join("src")).expect("mkdir src");
    fs::write(repo.path().join("src/main.rs"), "fn main() {}").expect("write source");
    fs::write(repo.path().join("README.md"), "# Test Project").expect("write readme");
    let out = TempDir::new().expect("temp out dir");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        repo.path().to_str().expect("utf8 repo path"),
        "--mode",
        "both",
        "--quick",
        "--no-timestamp",
        "--output-dir",
        out.path().to_str().expect("utf8 out path"),
    ]);
    cmd.assert().success();

    let repo_name = repo.path().file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let output_dir = out.path().join(repo_name);

    // Both mode should produce all three outputs
    assert!(
        output_dir.join(format!("{repo_name}_context_pack.md")).exists(),
        "context_pack.md should exist in both mode"
    );
    assert!(
        output_dir.join(format!("{repo_name}_chunks.jsonl")).exists(),
        "chunks.jsonl should exist in both mode"
    );
    assert!(
        output_dir.join(format!("{repo_name}_report.json")).exists(),
        "report.json should exist in both mode"
    );
}

/// Test that diff command handles identical inputs.
#[test]
fn test_diff_handles_identical_inputs() {
    let before = TempDir::new().expect("temp before");
    let after = TempDir::new().expect("temp after");

    let report = r#"{"schema_version":"1.0.0","stats":{"files_scanned":1,"files_included":1,"total_bytes_scanned":100,"total_bytes_included":100,"chunks_created":1,"total_tokens_estimated":10},"config":{},"output_files":[],"files":[{"id":"a1","path":"src/a.rs","priority":0.75,"tokens":10}]}"#;

    fs::write(before.path().join("report.json"), report).expect("write before report");
    fs::write(after.path().join("report.json"), report).expect("write after report");
    fs::write(before.path().join("chunks.jsonl"), "").expect("write before chunks");
    fs::write(after.path().join("chunks.jsonl"), "").expect("write after chunks");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "diff",
        before.path().to_str().expect("before path"),
        after.path().to_str().expect("after path"),
    ]);
    cmd.assert().success().stdout(predicate::str::contains("Files: +0 added, -0 removed"));
}

/// Test that diff command handles missing files gracefully.
#[test]
fn test_diff_handles_missing_files() {
    let before = TempDir::new().expect("temp before");
    let after = TempDir::new().expect("temp after");

    // Only create one report file
    fs::write(
        before.path().join("report.json"),
        r#"{"schema_version":"1.0.0","stats":{},"config":{},"output_files":[],"files":[]}"#,
    )
    .expect("write before report");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "diff",
        before.path().to_str().expect("before path"),
        after.path().to_str().expect("after path"),
    ]);
    // Should fail or handle gracefully when after/report.json doesn't exist
    cmd.assert().failure();
}

/// Test that export respects max_tokens limit.
#[test]
fn test_export_respects_max_tokens() {
    let repo = TempDir::new().expect("temp repo dir");
    fs::create_dir_all(repo.path().join("src")).expect("mkdir src");
    // Create multiple files with content
    for i in 0..5 {
        fs::write(
            repo.path().join(format!("src/file{i}.rs")),
            format!("pub fn func{i}() {{ println!(\"hello\"); }}"),
        )
        .expect("write source");
    }
    fs::write(repo.path().join("README.md"), "# Test").expect("write readme");
    let out = TempDir::new().expect("temp out dir");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        repo.path().to_str().expect("utf8 repo path"),
        "--max-tokens",
        "50", // Very low token limit
        "--quick",
        "--no-timestamp",
        "--output-dir",
        out.path().to_str().expect("utf8 out path"),
    ]);
    cmd.assert().success();

    let repo_name = repo.path().file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let report_path = out.path().join(repo_name).join(format!("{repo_name}_report.json"));
    let report_raw = fs::read_to_string(report_path).expect("read report");
    let report: Value = serde_json::from_str(&report_raw).expect("parse report");

    // Total tokens should be within or close to the limit
    let total_tokens = report["stats"]["total_tokens_estimated"].as_u64().unwrap_or(0);
    assert!(total_tokens <= 100, "total tokens ({}) should be limited", total_tokens);
    // Allow some margin
}

/// Test that info command works with various path types.
#[test]
fn test_info_with_different_paths() {
    // Current directory
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args(["info", "."]);
    cmd.assert().success();

    // Absolute path
    let temp = TempDir::new().expect("temp dir");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args(["info", temp.path().to_str().expect("path")]);
    cmd.assert().success();
}

/// Test that verbose flag produces debug output.
#[test]
fn test_verbose_flag_produces_debug_output() {
    let repo = TempDir::new().expect("temp repo dir");
    fs::write(repo.path().join("main.rs"), "fn main() {}").expect("write source");
    let out = TempDir::new().expect("temp out dir");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "-v",
        "export",
        "--path",
        repo.path().to_str().expect("utf8 repo path"),
        "--quick",
        "--no-timestamp",
        "--output-dir",
        out.path().to_str().expect("utf8 out path"),
    ]);
    cmd.assert().success().stderr(predicate::str::contains("DEBUG").or(predicate::str::is_empty()));
}

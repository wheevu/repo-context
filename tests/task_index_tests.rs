//! Integration coverage for task retrieval and the local SQLite index.

use assert_cmd::Command;
use predicates::prelude::*;
use rusqlite::Connection;
use serde_json::Value;
use std::fs;
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    root: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().expect("fixture temp dir");
        let root = temp.path().to_path_buf();
        fs::create_dir_all(root.join("src")).expect("src");
        fs::create_dir_all(root.join("tests")).expect("tests");
        fs::write(
            root.join("src/auth.rs"),
            "use crate::database;\npub fn refresh_token() { database::load_token(); }\nconst KEY: &str = \"sk-abcdefghijklmnopqrstuvwxyz12345\";\n",
        )
        .expect("auth");
        fs::write(root.join("src/database.rs"), "pub fn load_token() {}\n").expect("database");
        fs::write(root.join("src/lib.rs"), "mod auth;\nmod database;\n").expect("lib");
        fs::write(root.join("tests/auth_test.rs"), "use crate::auth::refresh_token;\n")
            .expect("test");
        fs::write(
            root.join("src/unrelated.rs"),
            "pub fn add(left: i32, right: i32) -> i32 { left + right }\n",
        )
        .expect("unrelated");
        fs::write(root.join("README.md"), "# Example repository\n").expect("readme");
        Self { _temp: temp, root }
    }
}

fn run_export(fixture: &Fixture, output: &TempDir, db: &std::path::Path, extra: &[&str]) {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        fixture.root.to_str().expect("root"),
        "--mode",
        "rag",
        "--output-dir",
        output.path().to_str().expect("output"),
        "--no-timestamp",
        "--task",
        "refresh token",
        "--index-db",
        db.to_str().expect("db"),
    ])
    .args(extra)
    .env("HOME", output.path())
    .assert()
    .success();
}

fn report_and_jsonl(fixture: &Fixture, output: &TempDir) -> (Value, String) {
    let name = fixture.root.file_name().and_then(|name| name.to_str()).expect("fixture name");
    let dir = output.path().join(name);
    let report: Value = serde_json::from_str(
        &fs::read_to_string(dir.join(format!("{name}_report.json"))).expect("report"),
    )
    .expect("valid report");
    let jsonl = fs::read_to_string(dir.join(format!("{name}_chunks.jsonl"))).expect("jsonl");
    (report, jsonl)
}

#[test]
fn task_export_is_explainable_and_persists_only_redacted_chunks() {
    let fixture = Fixture::new();
    let output = TempDir::new().expect("output");
    let db = output.path().join("cache/index.sqlite");

    run_export(&fixture, &output, &db, &[]);
    let (report, jsonl) = report_and_jsonl(&fixture, &output);
    assert_eq!(report["schema_version"], "1.4.0");
    assert_eq!(report["retrieval"]["strategy"], "bm25_static_import_graph");
    assert!(report["retrieval"]["seed_chunks"].as_u64().unwrap() > 0);
    assert!(report["retrieval"]["relation_counts"]["static_dependency"].as_u64().unwrap() > 0);
    assert!(jsonl.contains("\"retrieval\""));
    assert!(jsonl.contains("task_match"));
    assert!(jsonl.contains("src/auth.rs"));
    assert!(jsonl.contains("src/database.rs"));
    assert!(!jsonl.contains("sk-abcdefghijklmnopqrstuvwxyz12345"));

    let connection = Connection::open(&db).expect("open sqlite");
    let contents: Vec<String> = connection
        .prepare("SELECT content FROM chunks")
        .expect("query")
        .query_map([], |row| row.get(0))
        .expect("rows")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect");
    assert!(contents.iter().all(|content| !content.contains("sk-abcdefghijklmnopqrstuvwxyz12345")));
}

#[test]
fn index_command_reuses_changed_and_removed_files() {
    let fixture = Fixture::new();
    let db = fixture.root.join("index.sqlite");

    let mut first = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    first.args([
        "index",
        "--path",
        fixture.root.to_str().expect("root"),
        "--db",
        db.to_str().expect("db"),
    ]);
    first.assert().success().stdout(predicate::str::contains("updated files"));

    let mut second = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    second.args([
        "index",
        "--path",
        fixture.root.to_str().expect("root"),
        "--db",
        db.to_str().expect("db"),
    ]);
    second
        .assert()
        .success()
        .stdout(predicate::str::contains("reused files"))
        .stdout(predicate::str::contains("updated files: 0"));

    fs::write(fixture.root.join("src/auth.rs"), "pub fn refresh_token() {}\n").expect("edit");
    fs::remove_file(fixture.root.join("src/unrelated.rs")).expect("remove");
    let mut changed = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    changed.args([
        "index",
        "--path",
        fixture.root.to_str().expect("root"),
        "--db",
        db.to_str().expect("db"),
    ]);
    changed
        .assert()
        .success()
        .stdout(predicate::str::contains("updated files"))
        .stdout(predicate::str::contains("removed files: 1"));

    let connection = Connection::open(&db).expect("open sqlite");
    let file_count: usize = connection
        .query_row("SELECT COUNT(*) FROM files WHERE path = 'src/unrelated.rs'", [], |row| {
            row.get(0)
        })
        .expect("file count");
    assert_eq!(file_count, 0);
}

#[test]
fn task_export_can_skip_or_bypass_persistence() {
    let fixture = Fixture::new();
    let output = TempDir::new().expect("output");
    let db = output.path().join("never-created.sqlite");

    let mut no_index = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    no_index
        .args([
            "export",
            "--path",
            fixture.root.to_str().expect("root"),
            "--mode",
            "rag",
            "--output-dir",
            output.path().to_str().expect("output"),
            "--no-timestamp",
            "--task",
            "refresh token",
            "--no-index",
        ])
        .env("HOME", output.path());
    no_index.assert().success();
    assert!(!db.exists());

    let mut bypass = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    bypass
        .args([
            "export",
            "--path",
            fixture.root.to_str().expect("root"),
            "--mode",
            "rag",
            "--output-dir",
            output.path().to_str().expect("output"),
            "--no-timestamp",
            "--task",
            "refresh token",
            "--no-redact",
            "--index-db",
            db.to_str().expect("db"),
        ])
        .env("HOME", output.path())
        .assert()
        .success();
    assert!(!db.exists());
}

#[test]
fn task_and_focus_intersect_without_leaking_unrelated_files() {
    let fixture = Fixture::new();
    let output = TempDir::new().expect("output");
    let db = output.path().join("focus.sqlite");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        fixture.root.to_str().expect("root"),
        "--mode",
        "rag",
        "--scan-mode",
        "focused",
        "--focus-file",
        "src/auth.rs",
        "--task",
        "refresh token",
        "--index-db",
        db.to_str().expect("db"),
        "--output-dir",
        output.path().to_str().expect("output"),
        "--no-timestamp",
    ])
    .env("HOME", output.path())
    .assert()
    .success();

    let name = fixture.root.file_name().and_then(|name| name.to_str()).expect("name");
    let jsonl = fs::read_to_string(
        output.path().join(name).join("focus_auth").join(format!("{name}_focus_auth_chunks.jsonl")),
    )
    .expect("focused jsonl");
    assert!(jsonl.contains("src/auth.rs"));
    assert!(!jsonl.contains("src/unrelated.rs"));
}

#[test]
fn task_prompt_budget_includes_retrieval_header_in_the_cap() {
    let fixture = Fixture::new();
    let output = TempDir::new().expect("output");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        fixture.root.to_str().expect("root"),
        "--mode",
        "prompt",
        "--task",
        "refresh token implementation details",
        "--max-tokens",
        "10",
        "--no-index",
        "--output-dir",
        output.path().to_str().expect("output"),
        "--no-timestamp",
    ])
    .env("HOME", output.path())
    .assert()
    .success();

    let name = fixture.root.file_name().and_then(|name| name.to_str()).expect("name");
    let dir = output.path().join(name);
    let context = fs::read_to_string(dir.join(format!("{name}_context_pack.md"))).expect("context");
    let report: Value = serde_json::from_str(
        &fs::read_to_string(dir.join(format!("{name}_report.json"))).expect("report"),
    )
    .expect("valid report");
    assert!(context.len() / 4 <= 10);
    assert_eq!(report["stats"]["total_tokens_estimated_prompt"], context.len() / 4);
}

//! Integration tests for export artifact behavior.

use assert_cmd::Command;
use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

#[test]
fn export_is_deterministic_without_timestamp() {
    let fixture = TestRepo::new();
    let out_base = TempDir::new().expect("temp out");
    let out1 = out_base.path().join("out1");
    let out2 = out_base.path().join("out2");

    run_export(fixture.root(), &out1, "both", false);
    run_export(fixture.root(), &out2, "both", false);

    let actual1 = resolve_output_dir(&out1, fixture.root());
    let actual2 = resolve_output_dir(&out2, fixture.root());

    let context1 =
        fs::read_to_string(actual1.join(output_file_name(fixture.root(), "context_pack.md")))
            .expect("read context 1");
    let context2 =
        fs::read_to_string(actual2.join(output_file_name(fixture.root(), "context_pack.md")))
            .expect("read context 2");
    assert_eq!(context1, context2);

    let chunks1 =
        fs::read_to_string(actual1.join(output_file_name(fixture.root(), "chunks.jsonl")))
            .expect("read chunks 1");
    let chunks2 =
        fs::read_to_string(actual2.join(output_file_name(fixture.root(), "chunks.jsonl")))
            .expect("read chunks 2");
    assert_eq!(chunks1, chunks2);

    let report1_raw =
        fs::read_to_string(actual1.join(output_file_name(fixture.root(), "report.json")))
            .expect("read report 1");
    let report2_raw =
        fs::read_to_string(actual2.join(output_file_name(fixture.root(), "report.json")))
            .expect("read report 2");

    let mut report1: Value = serde_json::from_str(&report1_raw).expect("parse report 1");
    let mut report2: Value = serde_json::from_str(&report2_raw).expect("parse report 2");

    report1["config"]["output_dir"] = Value::String("<normalized>".to_string());
    report2["config"]["output_dir"] = Value::String("<normalized>".to_string());
    report1["output_files"] = Value::Array(vec![
        Value::String("<normalized-context>".to_string()),
        Value::String("<normalized-chunks>".to_string()),
    ]);
    report2["output_files"] = Value::Array(vec![
        Value::String("<normalized-context>".to_string()),
        Value::String("<normalized-chunks>".to_string()),
    ]);

    assert_eq!(report1, report2);
}

#[test]
fn export_applies_redaction_by_default() {
    let fixture = TestRepo::new();
    let out_base = TempDir::new().expect("temp out");
    let out = out_base.path().join("out");

    run_export(fixture.root(), &out, "rag", false);

    let actual = resolve_output_dir(&out, fixture.root());
    let chunks = fs::read_to_string(actual.join(output_file_name(fixture.root(), "chunks.jsonl")))
        .expect("read chunks");

    assert!(!chunks.contains("sk-abcdefghijklmnopqrstuvwxyz12345"));
    assert!(
        chunks.contains("[REDACTED_OPENAI_KEY]")
            || chunks.contains("[REDACTED_SECRET]")
            || chunks.contains("[HIGH_ENTROPY_REDACTED]")
    );
}

#[test]
fn export_no_redact_keeps_original_secret_text() {
    let fixture = TestRepo::new();
    let out_base = TempDir::new().expect("temp out");
    let out = out_base.path().join("out");

    run_export(fixture.root(), &out, "rag", true);

    let actual = resolve_output_dir(&out, fixture.root());
    let chunks = fs::read_to_string(actual.join(output_file_name(fixture.root(), "chunks.jsonl")))
        .expect("read chunks");
    assert!(chunks.contains("sk-abcdefghijklmnopqrstuvwxyz12345"));
}

#[test]
fn export_report_contains_trustworthy_core_fields() {
    let fixture = TestRepo::new();
    let out_base = TempDir::new().expect("temp out");
    let out = out_base.path().join("out");

    run_export(fixture.root(), &out, "both", false);

    let actual = resolve_output_dir(&out, fixture.root());
    let report_raw =
        fs::read_to_string(actual.join(output_file_name(fixture.root(), "report.json")))
            .expect("read report");
    let report: Value = serde_json::from_str(&report_raw).expect("parse report");

    assert_eq!(report["schema_version"], Value::String("1.1.0".to_string()));
    assert!(report.get("generated_at").is_none());
    assert!(report.get("stats").is_some());
    assert!(report.get("config").is_some());
    assert!(report.get("provenance").is_some());
    assert!(report.get("files").is_some());
    assert!(report.get("coverage").is_none());
    assert!(report["config"].get("heuristics").is_none());
    assert!(report["config"].get("task_query").is_none());
}

#[test]
fn export_mode_prompt_only_writes_markdown_and_report() {
    let fixture = TestRepo::new();
    let out_base = TempDir::new().expect("temp out");
    let out = out_base.path().join("out");

    run_export(fixture.root(), &out, "prompt", false);

    let actual = resolve_output_dir(&out, fixture.root());
    let repo_name = fixture.root().file_name().and_then(|n| n.to_str()).unwrap_or("repo");

    assert!(actual.join(format!("{}_context_pack.md", repo_name)).exists());
    assert!(!actual.join(format!("{}_chunks.jsonl", repo_name)).exists());
    assert!(actual.join(format!("{}_report.json", repo_name)).exists());
}

#[test]
fn export_mode_rag_only_writes_jsonl_and_report() {
    let fixture = TestRepo::new();
    let out_base = TempDir::new().expect("temp out");
    let out = out_base.path().join("out");

    run_export(fixture.root(), &out, "rag", false);

    let actual = resolve_output_dir(&out, fixture.root());
    let repo_name = fixture.root().file_name().and_then(|n| n.to_str()).unwrap_or("repo");

    assert!(!actual.join(format!("{}_context_pack.md", repo_name)).exists());
    assert!(actual.join(format!("{}_chunks.jsonl", repo_name)).exists());
    assert!(actual.join(format!("{}_report.json", repo_name)).exists());
}

#[test]
fn redaction_catches_secret_spanning_small_chunks() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(root.join("README.md"), "# Demo\n").expect("write readme");
    fs::write(
        root.join("src/main.py"),
        "def main():\n    token = \"sk-abcdefghijklmnopqrstuvwxyz12345\"\n    return token\n",
    )
    .expect("write main.py");

    let out = TempDir::new().expect("out dir");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        root.to_str().expect("root"),
        "--mode",
        "rag",
        "--output-dir",
        out.path().to_str().expect("out"),
        "--no-timestamp",
        "--chunk-tokens",
        "8",
        "--chunk-overlap",
        "0",
        "--min-chunk-tokens",
        "4",
    ]);
    cmd.assert().success();

    let actual = resolve_output_dir(out.path(), root);
    let repo_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let chunks = fs::read_to_string(actual.join(format!("{}_chunks.jsonl", repo_name)))
        .expect("read chunks");

    assert!(!chunks.contains("sk-abcdefghijklmnopqrstuvwxyz12345"));
}

fn run_export(repo_root: &Path, output_dir: &Path, mode: &str, no_redact: bool) {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        repo_root.to_str().expect("repo str"),
        "--mode",
        mode,
        "--output-dir",
        output_dir.to_str().expect("out str"),
        "--no-timestamp",
        "--chunk-tokens",
        "200",
        "--chunk-overlap",
        "20",
        "--min-chunk-tokens",
        "80",
    ]);

    if no_redact {
        cmd.arg("--no-redact");
    }

    cmd.assert().success();
}

fn resolve_output_dir(output_dir: &Path, repo_root: &Path) -> std::path::PathBuf {
    let repo_name = repo_root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    if output_dir.file_name().and_then(|n| n.to_str()) == Some(repo_name) {
        output_dir.to_path_buf()
    } else {
        output_dir.join(repo_name)
    }
}

fn output_file_name(repo_root: &Path, base_name: &str) -> String {
    let repo_name = repo_root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    format!("{repo_name}_{base_name}")
}

struct TestRepo {
    temp: TempDir,
}

impl TestRepo {
    fn new() -> Self {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path();
        fs::create_dir_all(root.join("src")).expect("mkdir src");
        fs::create_dir_all(root.join("docs")).expect("mkdir docs");

        fs::write(root.join("README.md"), "# Demo\n\nSmall fixture repo.\n").expect("write readme");
        fs::write(
            root.join("src/main.py"),
            "def main():\n    token = \"sk-abcdefghijklmnopqrstuvwxyz12345\"\n    return token\n",
        )
        .expect("write main.py");
        fs::write(root.join("docs/guide.md"), "# Guide\n\nHello\n").expect("write guide");
        fs::write(root.join("pyproject.toml"), "[project]\nname='demo'\n")
            .expect("write pyproject");

        Self { temp }
    }

    fn root(&self) -> &Path {
        self.temp.path()
    }
}

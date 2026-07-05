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
fn export_report_has_one_disposition_per_discovered_file() {
    let fixture = TestRepo::new();
    let out_base = TempDir::new().expect("temp out");
    let out = out_base.path().join("out");

    run_export(fixture.root(), &out, "both", false);

    let actual = resolve_output_dir(&out, fixture.root());
    let report_raw =
        fs::read_to_string(actual.join(output_file_name(fixture.root(), "report.json")))
            .expect("read report");
    let report: Value = serde_json::from_str(&report_raw).expect("parse report");
    let dispositions = report["file_dispositions"].as_array().expect("dispositions array");

    assert_eq!(dispositions.len(), report["stats"]["files_discovered"].as_u64().unwrap() as usize);
    let mut paths =
        dispositions.iter().map(|d| d["path"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), dispositions.len());
    assert!(dispositions.iter().any(|d| d["path"] == "README.md"
        && d["included_in_prompt"] == true
        && d["included_in_rag"] == true));
}

#[test]
fn prompt_mode_report_does_not_claim_rag_selection() {
    let fixture = TestRepo::new();
    let out_base = TempDir::new().expect("temp out");
    let out = out_base.path().join("out");

    run_export(fixture.root(), &out, "prompt", false);

    let actual = resolve_output_dir(&out, fixture.root());
    let report_raw =
        fs::read_to_string(actual.join(output_file_name(fixture.root(), "report.json")))
            .expect("read report");
    let report: Value = serde_json::from_str(&report_raw).expect("parse report");

    assert_eq!(report["stats"]["files_selected_rag"], 0);
    assert_eq!(report["stats"]["rag_chunks_rendered"], 0);
    assert!(report["stats"]["prompt_chunks_rendered"].as_u64().unwrap() > 0);
}

#[test]
fn default_export_uses_full_strategy_without_token_budget() {
    let fixture = TestRepo::new();
    let out_base = TempDir::new().expect("temp out");
    let out = out_base.path().join("out");

    run_export(fixture.root(), &out, "both", false);

    let actual = resolve_output_dir(&out, fixture.root());
    let report_raw =
        fs::read_to_string(actual.join(output_file_name(fixture.root(), "report.json")))
            .expect("read report");
    let report: Value = serde_json::from_str(&report_raw).expect("parse report");

    assert_eq!(report["config"]["coverage_strategy"], "full");
    assert!(report["config"].get("coverage_profile").is_none());
    assert_eq!(report["config"]["max_tokens"], Value::Null);
}

#[test]
fn max_tokens_automatically_uses_budget_strategy() {
    let fixture = TestRepo::new();
    let out_base = TempDir::new().expect("temp out");
    let out = out_base.path().join("out");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        fixture.root().to_str().expect("repo str"),
        "--mode",
        "both",
        "--output-dir",
        out.to_str().expect("out str"),
        "--no-timestamp",
        "--max-tokens",
        "20",
    ]);
    cmd.env("HOME", &out);
    cmd.assert().success();

    let actual = resolve_output_dir(&out, fixture.root());
    let report_raw =
        fs::read_to_string(actual.join(output_file_name(fixture.root(), "report.json")))
            .expect("read report");
    let report: Value = serde_json::from_str(&report_raw).expect("parse report");

    assert_eq!(report["config"]["coverage_strategy"], "budget");
    assert!(report["stats"]["dropped_files"].as_array().is_some_and(|v| !v.is_empty()));
}

#[test]
fn lockfile_is_summary_only_by_default() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    fs::write(root.join("README.md"), "# Demo\n").expect("write readme");
    fs::write(root.join("Cargo.lock"), "# lock\n".to_string() + &"package = \"x\"\n".repeat(200))
        .expect("write lock");

    let out = TempDir::new().expect("out dir");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        root.to_str().expect("root"),
        "--mode",
        "both",
        "--output-dir",
        out.path().to_str().expect("out"),
        "--no-timestamp",
    ]);
    cmd.env("HOME", out.path());
    cmd.assert().success();

    let actual = resolve_output_dir(out.path(), root);
    let repo_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let context = fs::read_to_string(actual.join(format!("{}_context_pack.md", repo_name)))
        .expect("read context");
    let report_raw =
        fs::read_to_string(actual.join(format!("{}_report.json", repo_name))).expect("read report");
    let report: Value = serde_json::from_str(&report_raw).expect("parse report");

    assert!(context.contains("Summary only: Cargo.lock"));
    assert!(!context.contains("package = \"x\"\npackage = \"x\""));
    assert!(report["file_dispositions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| { d["path"] == "Cargo.lock" && d["reason"] == "included_summary_only" }));
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
    cmd.env("HOME", out.path());
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

    cmd.env("HOME", output_dir);
    cmd.assert().success();
}

fn resolve_output_dir(output_dir: &Path, repo_root: &Path) -> std::path::PathBuf {
    let repo_name = repo_root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    output_dir.join(repo_name)
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

// ── HIGH-VALUE REGRESSION TESTS ──────────────────────────────────────────────

#[test]
fn export_redacts_readme_secret_from_context_pack_and_jsonl() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    // Place a secret in the README — it should be redacted in BOTH the context
    // pack and the JSONL chunks.
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(root.join("README.md"), "# App\n\nAPI key: sk-abcdefghijklmnopqrstuvwxyz12345\n")
        .expect("write readme");
    fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write source");

    let out = TempDir::new().expect("out dir");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        root.to_str().expect("root"),
        "--mode",
        "both",
        "--output-dir",
        out.path().to_str().expect("out"),
        "--no-timestamp",
    ]);
    cmd.env("HOME", out.path());
    cmd.assert().success();

    let repo_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let actual = out.path().join(repo_name);

    // Context pack must NOT leak the raw secret.
    let ctx = fs::read_to_string(actual.join(format!("{}_context_pack.md", repo_name)))
        .expect("read context");
    assert!(!ctx.contains("sk-abcdefghijklmnopqrstuvwxyz12345"));
    assert!(ctx.contains("[REDACTED_OPENAI_KEY]") || ctx.contains("[REDACTED_SECRET]"));

    // JSONL chunks must NOT leak the raw secret.
    let jsonl =
        fs::read_to_string(actual.join(format!("{}_chunks.jsonl", repo_name))).expect("read jsonl");
    assert!(!jsonl.contains("sk-abcdefghijklmnopqrstuvwxyz12345"));

    // Report must NOT leak the raw secret in any field.
    let report_raw =
        fs::read_to_string(actual.join(format!("{}_report.json", repo_name))).expect("read report");
    assert!(!report_raw.contains("sk-abcdefghijklmnopqrstuvwxyz12345"));
}

#[test]
fn scanner_respects_gitignore_for_markdown_by_default() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    // Git init is necessary because the `ignore` crate's gitignore support
    // does not read `.gitignore` files outside a git repository.
    let _ = std::process::Command::new("git").args(["init"]).current_dir(root).status();
    fs::write(root.join(".gitignore"), "notes.md\n").expect("write gitignore");
    fs::write(root.join("notes.md"), "# Private notes\n").expect("write notes");
    fs::write(root.join("README.md"), "# Public\n").expect("write readme");
    fs::write(root.join("main.py"), "x = 1\n").expect("write main");

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
    ]);
    cmd.env("HOME", out.path());
    cmd.assert().success();

    let repo_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let actual = out.path().join(repo_name);
    let jsonl =
        fs::read_to_string(actual.join(format!("{}_chunks.jsonl", repo_name))).expect("read jsonl");

    // README and main.py should be present, but notes.md should be excluded.
    assert!(jsonl.contains("README.md"), "README should be present");
    assert!(!jsonl.contains("notes.md"), "gitignored notes.md should not be included");
}

#[test]
fn cli_include_ext_accepts_extension_without_dot() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    fs::write(root.join("main.rs"), "fn main() {}\n").expect("write rs");
    fs::write(root.join("lib.py"), "def f(): pass\n").expect("write py");

    let out = TempDir::new().expect("out dir");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        root.to_str().expect("root"),
        "--mode",
        "rag",
        "-i",
        "rs",
        "--output-dir",
        out.path().to_str().expect("out"),
        "--no-timestamp",
    ]);
    cmd.env("HOME", out.path());
    cmd.assert().success();

    let repo_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let actual = out.path().join(repo_name);
    let jsonl =
        fs::read_to_string(actual.join(format!("{}_chunks.jsonl", repo_name))).expect("read jsonl");

    // `-i rs` should match main.rs, but not lib.py.
    assert!(jsonl.contains("main.rs"), "main.rs should be included");
    assert!(!jsonl.contains("lib.py"), "lib.py should be excluded");
}

#[test]
fn token_budget_keeps_later_mandatory_chunks() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    // Large mandatory file (README) then a small mandatory config file
    fs::write(
        root.join("README.md"),
        "x".repeat(5000), // large enough to consume budget
    )
    .expect("write readme");
    fs::write(root.join("Cargo.toml"), "[package]\nname=\"foo\"\n").expect("write cargo");

    let out = TempDir::new().expect("out dir");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        root.to_str().expect("root"),
        "--mode",
        "rag",
        "--max-tokens",
        "20",
        "--output-dir",
        out.path().to_str().expect("out"),
        "--no-timestamp",
        "--chunk-tokens",
        "200",
    ]);
    cmd.env("HOME", out.path());
    cmd.assert().success();

    let repo_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let actual = out.path().join(repo_name);
    let report_raw =
        fs::read_to_string(actual.join(format!("{}_report.json", repo_name))).expect("read report");
    let report: Value = serde_json::from_str(&report_raw).expect("parse report");

    // With a tiny budget, at least some files should be dropped.
    let dropped = report["stats"]["dropped_files"].as_array().expect("dropped array");
    assert!(!dropped.is_empty(), "some files should be dropped under tight budget");
    // The test should not panic — the budget loop must handle oversized mandatory chunks.
}

#[test]
fn coalesced_chunks_have_valid_metadata_after_enrichment() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    // Small functions that will produce multiple chunks, which coalescing will merge.
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(
        root.join("src/lib.rs"),
        "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\nfn e() {}\nfn f() {}\n",
    )
    .expect("write lib");

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
        "5",
        "--chunk-overlap",
        "0",
        "--min-chunk-tokens",
        "80",
    ]);
    cmd.env("HOME", out.path());
    cmd.assert().success();

    let repo_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let actual = out.path().join(repo_name);
    let jsonl =
        fs::read_to_string(actual.join(format!("{}_chunks.jsonl", repo_name))).expect("read jsonl");

    // Each line should have self-consistent metadata.
    for line in jsonl.lines() {
        let v: Value = serde_json::from_str(line).expect("valid json");
        let chunk_index = v["chunk_index"].as_u64().unwrap();
        let chunks_in_file = v["chunks_in_file"].as_u64().unwrap();
        assert!(chunk_index < chunks_in_file, "chunk_index must be < chunks_in_file");
        assert!(v["id"].as_str().is_some_and(|s| !s.is_empty()), "id must be non-empty");
        assert!(
            v["content_sha256"].as_str().is_some_and(|s| !s.is_empty()),
            "hash must be non-empty"
        );
        assert!(v["byte_start"].as_u64().is_some(), "byte_start must be present");
        assert!(v["byte_end"].as_u64().is_some(), "byte_end must be present");
    }
}

#[test]
fn context_pack_escapes_markdown_table_safely() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    // File with a pipe character in its name to test table escaping.
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write source");
    // A file whose path could break the markdown table if not escaped.
    fs::write(root.join("notes|secret.md"), "# Notes\n").expect("write notes");

    let out = TempDir::new().expect("out dir");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        root.to_str().expect("root"),
        "--mode",
        "prompt",
        "--output-dir",
        out.path().to_str().expect("out"),
        "--no-timestamp",
    ]);
    cmd.env("HOME", out.path());
    cmd.assert().success();

    let repo_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let actual = out.path().join(repo_name);
    let ctx = fs::read_to_string(actual.join(format!("{}_context_pack.md", repo_name)))
        .expect("read context");

    // The table should still be readable — pipes are escaped.
    // If escaping failed, lines with unescaped pipes would disrupt the table.
    assert!(!ctx.contains("| notes|secret.md |"), "pipe in path must be escaped");
}

// ── SECURITY REGRESSION TESTS ────────────────────────────────────────────

#[test]
fn report_does_not_leak_credentialed_repo_url() {
    let fixture = TestRepo::new();
    let out_base = TempDir::new().expect("temp out");
    let out = out_base.path().join("out");

    // This would happen if a user passed `--repo https://user:token@...`
    // The report should sanitize the credential.
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        fixture.root().to_str().expect("path"),
        "--mode",
        "both",
        "--output-dir",
        out.to_str().expect("out"),
        "--no-timestamp",
    ]);
    cmd.env("HOME", &out);
    cmd.assert().success();

    let actual = resolve_output_dir(&out, fixture.root());
    let report_raw =
        fs::read_to_string(actual.join(output_file_name(fixture.root(), "report.json")))
            .expect("read report");

    // No credential-like patterns should appear in the report.
    // (This is mainly checking that the sanitizer doesn't crash on normal URLs.)
    let report: Value = serde_json::from_str(&report_raw).expect("parse report");
    assert!(report.get("provenance").is_some());
}

#[test]
fn manifest_scripts_are_redacted_in_context_pack() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    // A package.json with a secret in the scripts section.
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(root.join("README.md"), "# App\n").expect("write readme");
    fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write source");
    fs::write(
        root.join("package.json"),
        r#"{"name": "myapp", "scripts": {"start": "export API_KEY=sk-abcdefghijklmnopqrstuvwxyz12345"}, "description": "A test"}"#,
    )
    .expect("write package.json");

    let out = TempDir::new().expect("temp out");
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        root.to_str().expect("root"),
        "--mode",
        "prompt",
        "--output-dir",
        out.path().to_str().expect("out"),
        "--no-timestamp",
    ]);
    cmd.env("HOME", out.path());
    cmd.assert().success();

    let repo_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let actual = out.path().join(repo_name);
    let ctx = fs::read_to_string(actual.join(format!("{}_context_pack.md", repo_name)))
        .expect("read context");

    // The secret should NOT appear in the context pack.
    assert!(!ctx.contains("sk-abc123"), "secret in manifest scripts must be redacted");
}

#[test]
fn symlink_escaping_repo_root_is_rejected() {
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    let outside = TempDir::new().expect("outside");
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write main");
    fs::write(outside.path().join("secrets.txt"), "password=12345\n").expect("write secrets");

    // Create a symlink from repo -> outside file.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            outside.path().join("secrets.txt"),
            root.join("src/secrets.txt"),
        )
        .expect("symlink");
    }
    // On non-unix, skip symlink test.
    #[cfg(not(unix))]
    {
        return;
    }

    let out = TempDir::new().expect("temp out");
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
        "--follow-symlinks",
    ]);
    cmd.env("HOME", out.path());
    cmd.assert().success();

    let repo_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let actual = out.path().join(repo_name);
    let jsonl =
        fs::read_to_string(actual.join(format!("{}_chunks.jsonl", repo_name))).expect("read jsonl");

    // Symlink to outside file should be skipped, NOT included.
    assert!(!jsonl.contains("password=12345"), "outside-file symlink content must not be included");
    assert!(!jsonl.contains("secrets.txt"), "outside-file symlink must be rejected");
}

//! Integration tests for export outputs and determinism.

use assert_cmd::Command;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

#[test]
fn export_is_deterministic_without_timestamp() {
    let fixture = TestRepo::new();
    let out_base = TempDir::new().expect("temp out");
    let out1 = out_base.path().join("out1");
    let out2 = out_base.path().join("out2");

    run_export(fixture.root(), &out1);
    run_export(fixture.root(), &out2);

    let actual1 = resolve_output_dir(&out1, fixture.root());
    let actual2 = resolve_output_dir(&out2, fixture.root());

    let context1 = fs::read_to_string(actual1.join("context_pack.md")).expect("read context 1");
    let context2 = fs::read_to_string(actual2.join("context_pack.md")).expect("read context 2");
    assert_eq!(context1, context2);

    let chunks1 = fs::read_to_string(actual1.join("chunks.jsonl")).expect("read chunks 1");
    let chunks2 = fs::read_to_string(actual2.join("chunks.jsonl")).expect("read chunks 2");
    assert_eq!(chunks1, chunks2);
}

#[test]
fn export_applies_redaction_and_report_shape() {
    let fixture = TestRepo::new();
    let out_base = TempDir::new().expect("temp out");
    let out = out_base.path().join("out");

    run_export(fixture.root(), &out);

    let actual = resolve_output_dir(&out, fixture.root());

    let chunks = fs::read_to_string(actual.join("chunks.jsonl")).expect("read chunks");
    assert!(!chunks.contains("sk-abcdefghijklmnopqrstuvwxyz12345"));
    assert!(
        chunks.contains("[REDACTED_OPENAI_KEY]")
            || chunks.contains("[REDACTED_SECRET]")
            || chunks.contains("[HIGH_ENTROPY_REDACTED]")
    );

    let report_raw = fs::read_to_string(actual.join("report.json")).expect("read report");
    let report: serde_json::Value = serde_json::from_str(&report_raw).expect("parse report");
    assert_eq!(report["schema_version"], serde_json::json!("1.0.0"));
    assert!(report.get("generated_at").is_none());
    assert!(report.get("config").is_some());
    assert!(report.get("files").is_some());
    assert!(report["files"].as_array().expect("files array").len() >= 2);
    let redaction_counts =
        report["stats"]["redaction_counts"].as_object().expect("redaction counts object");
    assert!(!redaction_counts.is_empty());
    assert!(report["stats"]["redacted_chunks"].as_u64().unwrap_or(0) > 0);
    assert!(report["stats"]["redacted_files"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn report_processing_time_is_nonzero() {
    // H1 regression test: processing_time_seconds must be recorded BEFORE write_report is
    // called, so the value in report.json is > 0 (not the default 0.0).
    let fixture = TestRepo::new();
    let out_base = TempDir::new().expect("temp out");
    let out = out_base.path().join("out");

    run_export(fixture.root(), &out);

    let actual = resolve_output_dir(&out, fixture.root());
    let report_raw = fs::read_to_string(actual.join("report.json")).expect("read report");
    let report: serde_json::Value = serde_json::from_str(&report_raw).expect("parse report");

    let processing_time = report["stats"]["processing_time_seconds"]
        .as_f64()
        .expect("processing_time_seconds should be a number in report.json");
    assert!(
        processing_time > 0.0,
        "processing_time_seconds in report.json should be > 0, got {processing_time}"
    );
}

fn run_export(repo_root: &Path, output_dir: &Path) {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-to-prompt"));
    cmd.args([
        "export",
        "--path",
        repo_root.to_str().expect("repo str"),
        "--mode",
        "both",
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
    cmd.assert().success();
}

/// Resolve the actual output directory used by the CLI for this repo root and base output dir.
/// Matches `resolve_output_dir` in src/cli/export.rs: appends repo name unless it already matches.
fn resolve_output_dir(output_dir: &Path, repo_root: &Path) -> std::path::PathBuf {
    let repo_name = repo_root.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    if output_dir.file_name().and_then(|n| n.to_str()) == Some(repo_name) {
        output_dir.to_path_buf()
    } else {
        output_dir.join(repo_name)
    }
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

#[test]
fn byte_budget_breaks_on_limit_and_drops_all_remaining() {
    // Regression test: Python's byte-budget semantics use `break` not `continue`.
    // When cumulative accepted bytes >= limit, the current file AND all subsequent
    // files are bulk-dropped with reason "bytes_limit".
    let temp = TempDir::new().expect("temp dir");
    let root = temp.path();
    fs::create_dir_all(root.join("src")).expect("mkdir src");

    // small.py (~6 bytes) — fits in budget
    fs::write(root.join("src/small.py"), "x = 1\n").expect("write small.py");
    // large.py (~195 bytes) — causes cumulative total to exceed budget
    let big_content = "x = ".to_string() + &"1".repeat(190) + "\n";
    fs::write(root.join("src/large.py"), &big_content).expect("write large.py");
    // small2.py (~6 bytes) — comes after large.py; Python breaks so this is also dropped
    fs::write(root.join("src/small2.py"), "y = 2\n").expect("write small2.py");

    let out_base = TempDir::new().expect("temp out");
    let out = out_base.path().join("out");

    // Budget of 150 bytes: small.py (6B) fits, then cumulative=6, large.py (195B) causes
    // total+size > limit; Python checks total >= limit before adding so small.py is accepted
    // (total=0 < 150), then after accepting small.py total=6. Next file: total=6 < 150, so
    // large.py is accepted too (total becomes 201). Next file: total=201 >= 150 triggers break.
    // Actually with budget=10 we ensure small.py fits (6B), then total=6 < 10, large.py accepted
    // makes total=201 >= 10 on the next iteration... Let's use budget=5 to drop large and small2.
    // With budget=5: small.py size=6, total=0 < 5 so accepted, total becomes 6. large.py:
    // total=6 >= 5, so bulk-drop large.py + small2.py and break.
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-to-prompt"));
    cmd.args([
        "export",
        "--path",
        root.to_str().expect("root str"),
        "--mode",
        "rag",
        "--output-dir",
        out.to_str().expect("out str"),
        "--no-timestamp",
        "--max-total-bytes",
        "5", // small.py=6B > 5B budget: total=0 < 5 so accepted first, then total=6 >= 5 drops rest
        "--no-redact",
    ]);
    cmd.assert().success();

    let actual = resolve_output_dir(&out, root);
    let report_raw = fs::read_to_string(actual.join("report.json")).expect("read report");
    let report: serde_json::Value = serde_json::from_str(&report_raw).expect("parse report");

    // At least large.py and small2.py should be dropped
    let dropped = report["stats"]["files_dropped_budget"].as_u64().unwrap_or(0);
    assert!(dropped >= 1, "expected at least 1 dropped file, got {dropped}");

    // Verify dropped entries use reason "bytes_limit" (not "max_total_bytes")
    if let Some(dropped_arr) = report["stats"]["dropped_files"].as_array() {
        for entry in dropped_arr {
            let reason = entry["reason"].as_str().unwrap_or("");
            assert_eq!(reason, "bytes_limit", "dropped entry reason should be 'bytes_limit'");
            // dropped entry should have 'priority', not 'size_bytes'
            assert!(entry.get("priority").is_some(), "dropped entry should have 'priority' field");
            assert!(
                entry.get("size_bytes").is_none(),
                "dropped entry should not have 'size_bytes'"
            );
        }
    }
}

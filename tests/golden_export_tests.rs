//! Golden snapshot tests for export outputs.

use assert_cmd::Command;
use insta::{assert_json_snapshot, assert_snapshot};
use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

#[test]
fn golden_export_outputs_are_stable() {
    let fixture = GoldenRepo::new();
    let out = TempDir::new().expect("temp out");
    let output_dir = out.path().join("export");

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("repo-context"));
    cmd.args([
        "export",
        "--path",
        fixture.root().to_str().expect("fixture path"),
        "--mode",
        "both",
        "--output-dir",
        output_dir.to_str().expect("output path"),
        "--no-timestamp",
        "--chunk-tokens",
        "220",
        "--chunk-overlap",
        "30",
        "--min-chunk-tokens",
        "80",
        "--max-tokens",
        "2000",
    ]);
    cmd.assert().success();

    // The CLI namespaces output by repo name (matches Python get_repo_output_dir).
    let repo_name = fixture.root().file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    let actual_output_dir = if output_dir.file_name().and_then(|n| n.to_str()) == Some(repo_name) {
        output_dir.clone()
    } else {
        output_dir.join(repo_name)
    };

    let context =
        fs::read_to_string(actual_output_dir.join("context_pack.md")).expect("context pack");
    let chunks = fs::read_to_string(actual_output_dir.join("chunks.jsonl")).expect("chunks");
    let report_raw = fs::read_to_string(actual_output_dir.join("report.json")).expect("report");
    let report_json: Value = serde_json::from_str(&report_raw).expect("report json");

    let normalized_context = normalize_context(&context, fixture.root());
    let normalized_chunks = normalize_chunks(&chunks, fixture.root());
    let normalized_report = normalize_report(report_json, fixture.root());

    assert_snapshot!("golden_context_pack", normalized_context);
    assert_snapshot!("golden_chunks_jsonl", normalized_chunks);
    assert_json_snapshot!("golden_report_json", normalized_report);
}

fn normalize_context(input: &str, fixture_root: &Path) -> String {
    let mut normalized =
        input.replace(fixture_root.to_str().expect("fixture root str"), "/<FIXTURE_ROOT>");

    if let Some(name) = fixture_root.file_name().and_then(|n| n.to_str()) {
        normalized = normalized.replace(
            &format!("# Repository Context Pack: {}", name),
            "# Repository Context Pack: <FIXTURE_REPO>",
        );
        normalized = normalized.replace(&format!("\n{}/\n", name), "\n<FIXTURE_REPO>/\n");
    }

    normalized
}

fn normalize_chunks(input: &str, fixture_root: &Path) -> String {
    input.replace(fixture_root.to_str().expect("fixture root str"), "/<FIXTURE_ROOT>")
}

fn normalize_report(mut report: Value, _fixture_root: &Path) -> Value {
    if let Some(config) = report.get_mut("config").and_then(Value::as_object_mut) {
        if let Some(path) = config.get_mut("path") {
            *path = Value::String("/<FIXTURE_ROOT>".to_string());
        }
        if let Some(output_dir) = config.get_mut("output_dir") {
            *output_dir = Value::String("/<OUTPUT_DIR>".to_string());
        }
        if let Some(include) = config.get_mut("include_extensions").and_then(Value::as_array_mut) {
            include.sort_by_key(|v| v.as_str().map(|s| s.to_string()).unwrap_or_default());
        }
        if let Some(exclude) = config.get_mut("exclude_globs").and_then(Value::as_array_mut) {
            exclude.sort_by_key(|v| v.as_str().map(|s| s.to_string()).unwrap_or_default());
        }
    }

    if let Some(outputs) = report.get_mut("output_files").and_then(Value::as_array_mut) {
        for output in outputs.iter_mut() {
            if let Some(path_str) = output.as_str() {
                let file_name = Path::new(path_str)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(path_str)
                    .to_string();
                *output = Value::String(format!("/<OUTPUT_DIR>/{}", file_name));
            }
        }
    }

    // Normalize non-deterministic processing_time_seconds so the snapshot is stable.
    if let Some(stats) = report.get_mut("stats").and_then(Value::as_object_mut) {
        stats.insert(
            "processing_time_seconds".to_string(),
            Value::Number(serde_json::Number::from_f64(0.0).expect("0.0 is valid f64")),
        );
    }

    report
}

struct GoldenRepo {
    temp: TempDir,
}

impl GoldenRepo {
    fn new() -> Self {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path();

        fs::create_dir_all(root.join("src")).expect("mkdir src");
        fs::create_dir_all(root.join("docs")).expect("mkdir docs");

        fs::write(
            root.join("README.md"),
            "# Golden Fixture\n\nThis is a stable fixture repository for snapshot tests.\n",
        )
        .expect("write readme");

        fs::write(
            root.join("src/main.py"),
            "def greet(name: str) -> str:\n    token = \"sk-abcdefghijklmnopqrstuvwxyz12345\"\n    return f\"Hello {name}\"\n\n\ndef main() -> None:\n    print(greet(\"world\"))\n",
        )
        .expect("write main.py");

        fs::write(
            root.join("src/helpers.py"),
            "class Helper:\n    def run(self) -> None:\n        pass\n",
        )
        .expect("write helpers.py");

        fs::write(root.join("docs/guide.md"), "# Guide\n\nUse `python -m app`.\n")
            .expect("write guide");

        fs::write(
            root.join("pyproject.toml"),
            "[project]\nname='golden-fixture'\n\n[project.scripts]\nfixture='src.main:main'\n",
        )
        .expect("write pyproject");

        Self { temp }
    }

    fn root(&self) -> &Path {
        self.temp.path()
    }
}

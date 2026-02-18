//! Report JSON generation.

use crate::domain::{FileInfo, ScanStats, REPORT_SCHEMA_VERSION};
use anyhow::Result;
use chrono::Utc;
use serde_json::{json, Map, Value};
use std::path::Path;

pub fn write_report(
    report_path: &Path,
    _root_path: &Path,
    stats: &ScanStats,
    files: &[FileInfo],
    output_files: &[String],
    config: &Value,
    include_timestamp: bool,
) -> Result<()> {
    let mut sorted_output_files = output_files.to_vec();
    sorted_output_files.sort();

    let mut sorted_files: Vec<&FileInfo> = files.iter().collect();
    sorted_files.sort_by(|a, b| {
        b.priority
            .partial_cmp(&a.priority)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.relative_path.cmp(&b.relative_path))
    });

    let file_manifest = sorted_files
        .iter()
        .map(|f| {
            json!({
                "id": f.id,
                "path": f.relative_path,
                "priority": round_priority(f.priority),
                "tokens": f.token_estimate,
            })
        })
        .collect::<Vec<_>>();

    let mut report = Map::new();
    report.insert("schema_version".to_string(), Value::String(REPORT_SCHEMA_VERSION.to_string()));
    if include_timestamp {
        report.insert(
            "generated_at".to_string(),
            Value::String(Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()),
        );
    }
    report.insert("stats".to_string(), stats.to_report_value());
    report.insert("config".to_string(), config.clone());
    report.insert("output_files".to_string(), serde_json::to_value(sorted_output_files)?);
    if !file_manifest.is_empty() {
        report.insert("files".to_string(), serde_json::to_value(file_manifest)?);
    }

    if let Some(parent) = report_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(report_path, serde_json::to_string_pretty(&Value::Object(report))?)?;
    Ok(())
}

fn round_priority(priority: f64) -> f64 {
    (priority * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::write_report;
    use crate::domain::{FileInfo, ScanStats};
    use serde_json::json;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn to_report_value_has_nested_files_skipped() {
        let mut stats = ScanStats::default();
        stats.files_scanned = 10;
        stats.files_included = 7;
        stats.files_skipped_binary = 1;
        stats.files_skipped_extension = 2;
        stats.files_skipped_gitignore = 3;
        stats.files_skipped_glob = 4;
        stats.files_skipped_size = 5;

        let v = stats.to_report_value();

        // Top-level counts
        assert_eq!(v["files_scanned"], json!(10));
        assert_eq!(v["files_included"], json!(7));

        // files_skipped must be a nested object, not a flat integer
        let skipped = &v["files_skipped"];
        assert!(skipped.is_object(), "files_skipped should be an object");
        assert_eq!(skipped["binary"], json!(1));
        assert_eq!(skipped["extension"], json!(2));
        assert_eq!(skipped["gitignore"], json!(3));
        assert_eq!(skipped["glob"], json!(4));
        assert_eq!(skipped["size"], json!(5));
    }

    #[test]
    fn report_omits_timestamp_when_disabled() {
        let tmp = TempDir::new().expect("tmp");
        let report_path = tmp.path().join("report.json");
        let file = FileInfo {
            path: PathBuf::from("/tmp/a.rs"),
            relative_path: "src/a.rs".to_string(),
            size_bytes: 100,
            extension: ".rs".to_string(),
            language: "rust".to_string(),
            id: "abc".to_string(),
            priority: 0.81234,
            token_estimate: 25,
            tags: BTreeSet::new(),
            is_readme: false,
            is_config: false,
            is_doc: false,
        };

        write_report(
            &report_path,
            tmp.path(),
            &ScanStats::default(),
            &[file],
            &["out/chunks.jsonl".to_string()],
            &json!({"mode":"rag"}),
            false,
        )
        .expect("write report");

        let content = fs::read_to_string(report_path).expect("read report");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("json");
        assert!(parsed.get("generated_at").is_none());
        assert_eq!(parsed["files"][0]["priority"], json!(0.812));
    }
}

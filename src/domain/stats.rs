#![allow(missing_docs)]

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Current report schema version.
pub const REPORT_SCHEMA_VERSION: &str = "1.1.0";

/// Statistics from scanning and processing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanStats {
    pub files_scanned: usize,
    pub files_included: usize,
    #[serde(default)]
    pub files_skipped_size: usize,
    #[serde(default)]
    pub files_skipped_binary: usize,
    #[serde(default)]
    pub files_skipped_extension: usize,
    #[serde(default)]
    pub files_skipped_gitignore: usize,
    #[serde(default)]
    pub files_skipped_glob: usize,
    #[serde(default)]
    pub files_skipped: usize,
    pub files_dropped_budget: usize,
    pub total_bytes_scanned: u64,
    pub total_bytes_included: u64,
    pub chunks_created: usize,
    pub total_tokens_estimated: usize,
    #[serde(default)]
    pub languages_detected: HashMap<String, usize>,
    #[serde(default)]
    pub top_ignored_patterns: HashMap<String, usize>,
    #[serde(default)]
    pub processing_time_seconds: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_ranked_files: Vec<HashMap<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped_files: Vec<HashMap<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub redaction_counts: BTreeMap<String, usize>,
    #[serde(default)]
    pub redacted_chunks: usize,
    #[serde(default)]
    pub redacted_files: usize,
}

impl ScanStats {
    /// Produce a stable JSON value for report emission.
    pub fn to_report_value(&self) -> serde_json::Value {
        let mut langs: Vec<(&String, &usize)> = self.languages_detected.iter().collect();
        langs.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let languages_detected: serde_json::Map<String, serde_json::Value> =
            langs.into_iter().map(|(k, v)| (k.clone(), serde_json::json!(v))).collect();

        let mut patterns: Vec<(&String, &usize)> = self.top_ignored_patterns.iter().collect();
        patterns.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let top_ignored_patterns: serde_json::Map<String, serde_json::Value> =
            patterns.into_iter().take(10).map(|(k, v)| (k.clone(), serde_json::json!(v))).collect();

        let mut value = serde_json::json!({
            "files_scanned": self.files_scanned,
            "files_included": self.files_included,
            "files_skipped": {
                "binary": self.files_skipped_binary,
                "extension": self.files_skipped_extension,
                "gitignore": self.files_skipped_gitignore,
                "glob": self.files_skipped_glob,
                "size": self.files_skipped_size,
            },
            "files_dropped_budget": self.files_dropped_budget,
            "total_bytes_scanned": self.total_bytes_scanned,
            "total_bytes_included": self.total_bytes_included,
            "chunks_created": self.chunks_created,
            "total_tokens_estimated": self.total_tokens_estimated,
            "languages_detected": languages_detected,
            "top_ignored_patterns": top_ignored_patterns,
            "redaction_counts": self.redaction_counts,
            "processing_time_seconds": self.processing_time_seconds,
        });

        if self.redacted_files > 0 {
            value["redacted_files"] = serde_json::json!(self.redacted_files);
        }
        if self.redacted_chunks > 0 {
            value["redacted_chunks"] = serde_json::json!(self.redacted_chunks);
        }
        if !self.top_ranked_files.is_empty() {
            value["top_ranked_files"] = serde_json::json!(self.top_ranked_files);
        }
        if !self.dropped_files.is_empty() {
            value["dropped_files"] = serde_json::json!(self.dropped_files);
        }

        value
    }
}

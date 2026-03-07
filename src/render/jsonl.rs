//! JSONL rendering for RAG
//!
//! Provides functionality to render chunks as JSON Lines format for RAG pipelines.

use crate::domain::Chunk;
use serde_json::Value;
use std::collections::BTreeMap;

/// Renders chunks as JSON Lines format.
///
/// Each chunk is serialized as a JSON object with fields:
/// - `content`: The chunk content
/// - `end_line`: Ending line number
/// - `id`: Unique chunk ID
/// - `lang`: Programming language
/// - `path`: File path
/// - `priority`: Priority score (rounded to 3 decimals)
/// - `start_line`: Starting line number
/// - `tags`: Array of tags
///
/// # Arguments
/// * `chunks` - Slices of chunks to render
///
/// # Returns
/// JSON Lines formatted string (one JSON object per line)
pub fn render_jsonl(chunks: &[Chunk]) -> String {
    let mut lines = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        let mut tags: Vec<&str> = chunk.tags.iter().map(String::as_str).collect();
        tags.sort();

        // Use BTreeMap so keys are serialized in alphabetical order,
        // matching Python's json.dumps(..., sort_keys=True).
        let mut entry: BTreeMap<&str, Value> = BTreeMap::new();
        entry.insert("content", Value::String(chunk.content.clone()));
        entry.insert("end_line", Value::Number(chunk.end_line.into()));
        entry.insert("id", Value::String(chunk.id.clone()));
        entry.insert("lang", Value::String(chunk.language.clone()));
        entry.insert("path", Value::String(chunk.path.clone()));
        entry.insert(
            "priority",
            serde_json::to_value((chunk.priority * 1000.0).round() / 1000.0).unwrap(),
        );
        entry.insert("start_line", Value::Number(chunk.start_line.into()));
        entry.insert(
            "tags",
            Value::Array(tags.iter().map(|t| Value::String((*t).to_string())).collect()),
        );

        if let Ok(line) = serde_json::to_string(&entry) {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

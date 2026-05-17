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
        if let Some(byte_end) = chunk.byte_end {
            entry.insert("byte_end", Value::Number(byte_end.into()));
        }
        if let Some(byte_start) = chunk.byte_start {
            entry.insert("byte_start", Value::Number(byte_start.into()));
        }
        entry.insert("chunk_hash", Value::String(chunk.content_sha256.clone()));
        entry.insert("chunk_index", Value::Number(chunk.chunk_index.into()));
        entry.insert("chunks_in_file", Value::Number(chunk.chunks_in_file.into()));
        entry.insert("content_sha256", Value::String(chunk.content_sha256.clone()));
        entry.insert("end_line", Value::Number(chunk.end_line.into()));
        entry.insert("file_id", Value::String(chunk.file_id.clone()));
        entry.insert("file_sha256", Value::String(chunk.file_sha256.clone()));
        entry.insert("generated", Value::Bool(chunk.tags.contains("generated")));
        entry.insert("id", Value::String(chunk.id.clone()));
        entry.insert("lang", Value::String(chunk.language.clone()));
        entry.insert("lockfile", Value::Bool(chunk.tags.contains("lock-file")));
        entry.insert("minified", Value::Bool(chunk.tags.contains("minified")));
        entry.insert("path", Value::String(chunk.path.clone()));
        entry.insert(
            "priority",
            serde_json::to_value((chunk.priority * 1000.0).round() / 1000.0).unwrap(),
        );
        entry.insert("role", Value::String(tags.join(",")));
        entry.insert(
            "disposition",
            Value::String(
                if chunk.tags.contains("lock-file") {
                    "included_summary_only"
                } else if chunk.chunks_in_file > 1 {
                    "included_chunked"
                } else {
                    "included_full"
                }
                .to_string(),
            ),
        );
        entry.insert("start_line", Value::Number(chunk.start_line.into()));
        entry.insert(
            "tags",
            Value::Array(tags.iter().map(|t| Value::String((*t).to_string())).collect()),
        );
        entry.insert("symbols", entry.get("tags").cloned().unwrap_or(Value::Array(Vec::new())));
        entry.insert("token_estimate", Value::Number(chunk.token_estimate.into()));

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

#[cfg(test)]
mod tests {
    use super::render_jsonl;
    use crate::domain::Chunk;
    use std::collections::BTreeSet;

    #[test]
    fn jsonl_includes_rich_metadata_fields() {
        let chunk = Chunk {
            id: "c1".to_string(),
            path: "src/lib.rs".to_string(),
            language: "rust".to_string(),
            start_line: 1,
            end_line: 2,
            content: "fn main() {}\n".to_string(),
            priority: 0.9,
            tags: BTreeSet::from(["entrypoint".to_string()]),
            token_estimate: 4,
            file_id: "file1".to_string(),
            chunk_index: 0,
            chunks_in_file: 1,
            byte_start: Some(0),
            byte_end: Some(13),
            content_sha256: "abc".to_string(),
            file_sha256: "def".to_string(),
        };

        let jsonl = render_jsonl(&[chunk]);
        let value: serde_json::Value = serde_json::from_str(jsonl.trim()).unwrap();

        assert_eq!(value["token_estimate"], 4);
        assert_eq!(value["file_id"], "file1");
        assert_eq!(value["chunk_index"], 0);
        assert_eq!(value["chunks_in_file"], 1);
        assert_eq!(value["byte_start"], 0);
        assert_eq!(value["content_sha256"], "abc");
    }
}

//! JSONL rendering for RAG

use crate::domain::Chunk;
use serde_json::Value;
use std::collections::BTreeMap;

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

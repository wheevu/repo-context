#![allow(missing_docs)]

//! Structure-aware JSON chunking with prompt-safe schema summaries.

use crate::domain::{Chunk, FileInfo};
use crate::utils::{estimate_tokens, stable_hash};
use serde_json::{Map, Value};

pub fn chunk_json(file: &FileInfo, content: &str, max_tokens: usize) -> Option<Vec<Chunk>> {
    let value: Value = serde_json::from_str(content).ok()?;
    let mut chunks = Vec::new();
    let schema = describe_value(&value, 0);
    chunks.push(make_chunk(
        file,
        format!("JSON structural summary for {}\n{}\n", file.relative_path, schema),
        1,
        &["json-schema", "structural-summary", "synthetic-summary"],
    ));

    match value {
        Value::Object(map) => {
            for (index, (key, value)) in map.into_iter().enumerate() {
                add_member_chunks(&mut chunks, file, &key, value, max_tokens, index + 1);
            }
        }
        Value::Array(values) => add_array_chunks(&mut chunks, file, "$", values, max_tokens, 1),
        _ => {}
    }
    Some(chunks)
}

fn add_member_chunks(
    chunks: &mut Vec<Chunk>,
    file: &FileInfo,
    key: &str,
    value: Value,
    max_tokens: usize,
    ordinal: usize,
) {
    match value {
        Value::Array(values) => add_array_chunks(chunks, file, key, values, max_tokens, ordinal),
        Value::Object(values)
            if estimate_tokens(&value_string(&Value::Object(values.clone()))) > max_tokens =>
        {
            add_object_chunks(chunks, file, key, values, max_tokens, ordinal);
        }
        value => {
            let content = format!("JSON member: {key}\n{}\n", value_string(&value));
            chunks.push(make_detail_chunk(file, content, key, ordinal, 0));
        }
    }
}

fn add_array_chunks(
    chunks: &mut Vec<Chunk>,
    file: &FileInfo,
    key: &str,
    values: Vec<Value>,
    max_tokens: usize,
    ordinal: usize,
) {
    let batches = batch_values(values, max_tokens);
    let total = batches.len();
    for (batch_index, batch) in batches.into_iter().enumerate() {
        let content = format!(
            "JSON array member: {key} (batch {}/{total})\n{}\n",
            batch_index + 1,
            value_string(&Value::Array(batch))
        );
        chunks.push(make_detail_chunk(file, content, key, ordinal, batch_index));
    }
}

fn add_object_chunks(
    chunks: &mut Vec<Chunk>,
    file: &FileInfo,
    key: &str,
    values: Map<String, Value>,
    max_tokens: usize,
    ordinal: usize,
) {
    let mut batches: Vec<Map<String, Value>> = Vec::new();
    let mut current = Map::new();
    let mut current_tokens = 0usize;
    for (member, value) in values {
        let member_tokens = estimate_tokens(&member) + estimate_tokens(&value_string(&value)) + 2;
        if !current.is_empty() && current_tokens.saturating_add(member_tokens) > max_tokens {
            batches.push(current);
            current = Map::new();
            current_tokens = 0;
        }
        current_tokens = current_tokens.saturating_add(member_tokens);
        current.insert(member, value);
    }
    if !current.is_empty() {
        batches.push(current);
    }
    let total = batches.len();
    for (batch_index, batch) in batches.into_iter().enumerate() {
        let content = format!(
            "JSON object member: {key} (batch {}/{total})\n{}\n",
            batch_index + 1,
            value_string(&Value::Object(batch))
        );
        chunks.push(make_detail_chunk(file, content, key, ordinal, batch_index));
    }
}

fn batch_values(values: Vec<Value>, max_tokens: usize) -> Vec<Vec<Value>> {
    let mut batches = Vec::new();
    let mut current = Vec::new();
    let mut current_tokens = 0usize;
    for value in values {
        let value_tokens = estimate_tokens(&value_string(&value)) + 1;
        if !current.is_empty() && current_tokens.saturating_add(value_tokens) > max_tokens {
            batches.push(current);
            current = Vec::new();
            current_tokens = 0;
        }
        current_tokens = current_tokens.saturating_add(value_tokens);
        current.push(value);
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

fn describe_value(value: &Value, depth: usize) -> String {
    if depth >= 2 {
        return value_type(value).to_string();
    }
    match value {
        Value::Object(map) => {
            let mut members = map
                .iter()
                .take(16)
                .map(|(key, value)| format!("{key}: {}", describe_value(value, depth + 1)))
                .collect::<Vec<_>>();
            if map.len() > members.len() {
                members.push(format!("… {} more keys", map.len() - members.len()));
            }
            format!("object({} members) {{{}}}", map.len(), members.join(", "))
        }
        Value::Array(values) => {
            let item = values
                .first()
                .map(|value| describe_value(value, depth + 1))
                .unwrap_or_else(|| "unknown".to_string());
            format!("array({} items) of {item}", values.len())
        }
        _ => value_type(value).to_string(),
    }
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn value_string(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

fn make_detail_chunk(
    file: &FileInfo,
    content: String,
    key: &str,
    ordinal: usize,
    batch: usize,
) -> Chunk {
    let line = 1000 + ordinal * 100 + batch;
    let mut chunk = make_chunk(
        file,
        content,
        line,
        &["json-data", "rag-only", "synthetic-summary", &format!("json-key:{key}")],
    );
    chunk.priority = (chunk.priority - 0.35).max(0.1);
    chunk
}

fn make_chunk(file: &FileInfo, content: String, line: usize, extra_tags: &[&str]) -> Chunk {
    let mut tags = file.tags.clone();
    tags.extend(extra_tags.iter().map(|tag| (*tag).to_string()));
    Chunk {
        id: stable_hash(&content, &file.relative_path, line, line),
        path: file.relative_path.clone(),
        language: file.language.clone(),
        start_line: line,
        end_line: line,
        token_estimate: estimate_tokens(&content),
        content,
        priority: file.priority,
        tags,
        file_id: String::new(),
        chunk_index: 0,
        chunks_in_file: 0,
        byte_start: None,
        byte_end: None,
        content_sha256: String::new(),
        file_sha256: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    #[test]
    fn one_line_json_gets_schema_and_logical_member_chunks() {
        let file = FileInfo {
            path: PathBuf::from("data/map.json"),
            relative_path: "data/map.json".to_string(),
            size_bytes: 100,
            extension: ".json".to_string(),
            language: "json".to_string(),
            id: "json".to_string(),
            priority: 0.5,
            token_estimate: 0,
            tags: BTreeSet::new(),
            is_readme: false,
            is_config: false,
            is_doc: false,
        };
        let chunks = chunk_json(
            &file,
            r#"{"roads":[{"id":1},{"id":2}],"signals":[{"node":1}],"meta":{"source":"osm"}}"#,
            20,
        )
        .expect("valid json");

        assert!(chunks[0].content.contains("roads: array(2 items)"));
        assert!(chunks.iter().any(|chunk| chunk.tags.contains("json-key:roads")));
        assert!(chunks.iter().skip(1).all(|chunk| chunk.tags.contains("rag-only")));
    }
}

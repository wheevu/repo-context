#![allow(missing_docs)]

//! Semantic chunking and structural summaries for Godot text formats.

use crate::chunk::code_chunker::chunk_code;
use crate::domain::{Chunk, FileInfo};
use crate::godot::{parse_gdscript, parse_project, parse_scene, parse_shader, SceneSummary};
use crate::utils::{estimate_tokens, stable_hash};

pub fn chunk_gdscript(
    file: &FileInfo,
    content: &str,
    max_tokens: usize,
    overlap_tokens: usize,
) -> Vec<Chunk> {
    let parsed = parse_gdscript(content);
    let mut chunks = chunk_code(file, content, max_tokens, overlap_tokens);
    for chunk in &mut chunks {
        let local = parse_gdscript(&chunk.content);
        if let Some(value) = &parsed.extends {
            chunk.tags.insert(format!("extends:{value}"));
        }
        if let Some(value) = &parsed.class_name {
            chunk.tags.insert(format!("class:{value}"));
        }
        chunk.tags.extend(local.signals.iter().map(|value| format!("signal:{value}")));
        chunk.tags.extend(local.methods.iter().map(|value| format!("def:{value}")));
        chunk.tags.extend(local.references.iter().map(|value| format!("ref:{value}")));
        chunk.tags.extend(local.input_actions.iter().map(|value| format!("input:{value}")));
        chunk.tags.extend(local.node_paths.iter().map(|value| format!("nodepath:{value}")));
    }
    chunks
}

pub fn chunk_scene(file: &FileInfo, content: &str, max_tokens: usize) -> Vec<Chunk> {
    let parsed = parse_scene(content);
    let mut chunks = vec![summary_chunk(
        file,
        scene_summary_text(file, &parsed, max_tokens),
        1,
        "scene-summary",
    )];
    if estimate_tokens(content) > max_tokens {
        let node_lines = parsed.nodes.iter().map(scene_node_line).collect::<Vec<_>>();
        for (index, content) in batch_summary_lines(
            &format!("Godot node details for {}", file.relative_path),
            node_lines,
            max_tokens,
        )
        .into_iter()
        .enumerate()
        {
            let mut chunk = summary_chunk(file, content, 1000 + index, "scene-nodes");
            mark_rag_only(&mut chunk);
            chunks.push(chunk);
        }

        let mut metadata_lines = Vec::new();
        metadata_lines.extend(parsed.external_resources.iter().map(|(id, path)| {
            format!(
                "resource {id}: {} ({})\n",
                path,
                parsed.external_resource_types.get(id).map(String::as_str).unwrap_or("unknown")
            )
        }));
        metadata_lines
            .extend(parsed.subresources.iter().map(|value| format!("subresource {value}\n")));
        metadata_lines
            .extend(parsed.connections.iter().map(|value| format!("connection {value}\n")));
        for (index, content) in batch_summary_lines(
            &format!("Godot resource and connection details for {}", file.relative_path),
            metadata_lines,
            max_tokens,
        )
        .into_iter()
        .enumerate()
        {
            let mut chunk = summary_chunk(file, content, 2000 + index, "scene-metadata");
            mark_rag_only(&mut chunk);
            chunks.push(chunk);
        }
    }
    if let Some(chunk) = chunks.first_mut() {
        for target in parsed.external_resources.values() {
            chunk.tags.insert(format!("ref:{target}"));
        }
    }
    chunks
}

pub fn chunk_project(file: &FileInfo, content: &str, max_tokens: usize) -> Vec<Chunk> {
    let parsed = parse_project(content);
    let mut text = format!("Godot project configuration: {}\n", file.relative_path);
    text.push_str(&format!(
        "config_version: {}\n",
        parsed.config_version.as_deref().unwrap_or("unknown")
    ));
    text.push_str(&format!("features: {}\n", parsed.features.join(", ")));
    text.push_str(&format!(
        "main_scene: {}\n",
        parsed.main_scene.as_deref().unwrap_or("not configured")
    ));
    text.push_str("autoloads:\n");
    for (name, path) in &parsed.autoloads {
        text.push_str(&format!("- {name}: {path}\n"));
    }
    text.push_str(&format!("input_actions: {}\n", parsed.input_actions.join(", ")));
    text.push_str(&format!("enabled_plugins: {}\n", parsed.enabled_plugins.join(", ")));
    append_settings(&mut text, "display", &parsed.display);
    append_settings(&mut text, "rendering", &parsed.rendering);
    append_settings(&mut text, "physics", &parsed.physics);
    append_settings(&mut text, "localization", &parsed.localization);
    append_settings(&mut text, "layers", &parsed.layers);
    let content_batches = if estimate_tokens(&text) <= max_tokens {
        vec![text]
    } else {
        batch_summary_lines(
            &format!("Godot project configuration: {}", file.relative_path),
            text.lines().skip(1).map(|line| format!("{line}\n")).collect(),
            max_tokens,
        )
    };
    content_batches
        .into_iter()
        .enumerate()
        .map(|(index, content)| {
            let mut chunk = summary_chunk(file, content, index + 1, "project-summary");
            if index > 0 {
                chunk.tags.remove("config");
                chunk.tags.remove("entrypoint");
            }
            if let Some(main_scene) = &parsed.main_scene {
                chunk.tags.insert(format!("ref:{main_scene}"));
            }
            for path in parsed.autoloads.values() {
                chunk.tags.insert(format!("ref:{path}"));
            }
            chunk
        })
        .collect()
}

fn mark_rag_only(chunk: &mut Chunk) {
    chunk.tags.insert("rag-only".to_string());
    chunk.tags.remove("config");
    chunk.tags.remove("entrypoint");
    chunk.tags.remove("readme");
    chunk.priority = (chunk.priority - 0.35).max(0.1);
}

pub fn chunk_shader(
    file: &FileInfo,
    content: &str,
    max_tokens: usize,
    overlap_tokens: usize,
) -> Vec<Chunk> {
    let parsed = parse_shader(content);
    let mut chunks = chunk_code(file, content, max_tokens, overlap_tokens);
    for chunk in &mut chunks {
        if let Some(value) = &parsed.shader_type {
            chunk.tags.insert(format!("shader-type:{value}"));
        }
        chunk.tags.extend(parsed.uniforms.iter().map(|value| format!("uniform:{value}")));
        chunk.tags.extend(parsed.functions.iter().map(|value| format!("def:{value}")));
        chunk.tags.extend(parsed.includes.iter().map(|value| format!("ref:{value}")));
    }
    chunks
}

fn scene_summary_text(file: &FileInfo, parsed: &SceneSummary, max_tokens: usize) -> String {
    let mut text = format!("Godot {} structural summary: {}\n", parsed.format, file.relative_path);
    text.push_str(&format!("root: {}\n", parsed.root.as_deref().unwrap_or("not found")));
    text.push_str(&format!("nodes: {}\n", parsed.nodes.len()));
    let mut shown_nodes = 0usize;
    for node in &parsed.nodes {
        if !push_with_budget(&mut text, &scene_node_line(node), max_tokens, 120) {
            break;
        }
        shown_nodes += 1;
    }
    if parsed.nodes.len() > shown_nodes {
        text.push_str(&format!(
            "- … {} more nodes available in RAG node batches\n",
            parsed.nodes.len() - shown_nodes
        ));
    }
    text.push_str("external_resources:\n");
    let mut shown_resources = 0usize;
    for (id, path) in &parsed.external_resources {
        let line = format!(
            "- {id}: {} ({})\n",
            path,
            parsed.external_resource_types.get(id).map(String::as_str).unwrap_or("unknown")
        );
        if !push_with_budget(&mut text, &line, max_tokens, 60) {
            break;
        }
        shown_resources += 1;
    }
    if parsed.external_resources.len() > shown_resources {
        text.push_str(&format!(
            "- … {} more resources available in RAG metadata batches\n",
            parsed.external_resources.len() - shown_resources
        ));
    }
    text.push_str(&format!("subresources: {}\n", parsed.subresources.len()));
    text.push_str("signal_connections:\n");
    let mut shown_connections = 0usize;
    for connection in &parsed.connections {
        if !push_with_budget(&mut text, &format!("- {connection}\n"), max_tokens, 20) {
            break;
        }
        shown_connections += 1;
    }
    if parsed.connections.len() > shown_connections {
        text.push_str(&format!(
            "- … {} more connections available in RAG metadata batches\n",
            parsed.connections.len() - shown_connections
        ));
    }
    text
}

fn scene_node_line(node: &crate::godot::SceneNode) -> String {
    format!(
        "- {} type={} parent={} script={} instance={} groups={}{}\n",
        node.path,
        node.node_type.as_deref().unwrap_or("instanced"),
        node.parent.as_deref().unwrap_or("<root>"),
        node.script.as_deref().unwrap_or("-"),
        node.instance.as_deref().unwrap_or("-"),
        node.groups.join(","),
        if node.important_properties.is_empty() {
            String::new()
        } else {
            format!(" properties={:?}", node.important_properties)
        }
    )
}

fn push_with_budget(text: &mut String, line: &str, max_tokens: usize, reserve: usize) -> bool {
    if estimate_tokens(text).saturating_add(estimate_tokens(line)).saturating_add(reserve)
        > max_tokens
    {
        return false;
    }
    text.push_str(line);
    true
}

fn batch_summary_lines(header: &str, lines: Vec<String>, max_tokens: usize) -> Vec<String> {
    if lines.is_empty() {
        return Vec::new();
    }
    let mut batches = Vec::new();
    let mut current = format!("{header}\n");
    for line in lines {
        if !push_with_budget(&mut current, &line, max_tokens, 0) {
            if current.lines().count() > 1 {
                batches.push(current);
                current = format!("{header}\n{line}");
            } else {
                current.push_str(&line);
            }
        }
    }
    if current.lines().count() > 1 {
        batches.push(current);
    }
    batches
}

fn append_settings(
    text: &mut String,
    name: &str,
    values: &std::collections::BTreeMap<String, String>,
) {
    if values.is_empty() {
        return;
    }
    text.push_str(&format!("{name}:\n"));
    for (key, value) in values {
        text.push_str(&format!("- {key}={value}\n"));
    }
}

fn summary_chunk(file: &FileInfo, content: String, line: usize, tag: &str) -> Chunk {
    let mut tags = file.tags.clone();
    tags.insert(tag.to_string());
    tags.insert("synthetic-summary".to_string());
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

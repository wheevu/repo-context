#![allow(missing_docs)]

use crate::domain::{FileDisposition, FileInfo};
use crate::utils::{normalize_path, read_file_safe};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

static QUOTED_RES_PATH: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?:preload|load)\(\s*\"(res://[^\"]+)\"\s*\)"#).unwrap());
static INPUT_ACTION: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"Input\.(?:is_action_(?:pressed|released|just_pressed|just_released)|get_action_strength|get_axis|get_vector)\s*\(([^)]*)\)"#).unwrap()
});
static STRING_LITERAL: Lazy<Regex> = Lazy::new(|| Regex::new(r#"\"([^\"]+)\""#).unwrap());
static NODE_PATH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"NodePath\(\s*[\"']([^\"']+)[\"']|(?:\$|%)([A-Za-z_][A-Za-z0-9_./:]*)"#).unwrap()
});
static NODE_HEADER: Lazy<Regex> = Lazy::new(|| Regex::new(r#"^\[node\s+(.+)\]"#).unwrap());
static CONNECTION: Lazy<Regex> = Lazy::new(|| Regex::new(r#"^\[connection\s+(.+)\]"#).unwrap());
static ATTRIBUTE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"([A-Za-z_]+)=(?:\"([^\"]*)\"|([^\s]+))"#).unwrap());
static SHADER_INCLUDE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"#include\s+\"([^\"]+)\""#).unwrap());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct GodotRelationship {
    pub source: String,
    pub kind: String,
    pub target: String,
    #[serde(default = "default_relationship_resolved")]
    pub resolved: bool,
}

fn default_relationship_resolved() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GodotProjectConfig {
    pub config_version: Option<String>,
    pub features: Vec<String>,
    pub main_scene: Option<String>,
    pub autoloads: BTreeMap<String, String>,
    pub input_actions: Vec<String>,
    pub enabled_plugins: Vec<String>,
    pub rendering: BTreeMap<String, String>,
    pub display: BTreeMap<String, String>,
    pub physics: BTreeMap<String, String>,
    pub localization: BTreeMap<String, String>,
    pub layers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GdscriptSummary {
    pub extends: Option<String>,
    pub class_name: Option<String>,
    pub annotations: Vec<String>,
    pub signals: Vec<String>,
    pub enums: Vec<String>,
    pub constants: Vec<String>,
    pub members: Vec<String>,
    pub exported: Vec<String>,
    pub onready: Vec<String>,
    pub methods: Vec<String>,
    pub static_methods: Vec<String>,
    pub inner_classes: Vec<String>,
    pub references: Vec<String>,
    pub node_paths: Vec<String>,
    pub input_actions: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneNode {
    pub path: String,
    pub name: String,
    pub node_type: Option<String>,
    pub parent: Option<String>,
    pub instance: Option<String>,
    pub groups: Vec<String>,
    pub script: Option<String>,
    pub important_properties: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneSummary {
    pub format: String,
    pub root: Option<String>,
    pub nodes: Vec<SceneNode>,
    pub external_resources: BTreeMap<String, String>,
    pub external_resource_types: BTreeMap<String, String>,
    pub subresources: Vec<String>,
    pub connections: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShaderSummary {
    pub shader_type: Option<String>,
    pub render_modes: Vec<String>,
    pub uniforms: Vec<String>,
    pub functions: Vec<String>,
    pub includes: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GodotSummary {
    pub detected: bool,
    pub signals: Vec<String>,
    pub project: GodotProjectConfig,
    pub scene_count: usize,
    pub resource_count: usize,
    pub gdscript_count: usize,
    pub shader_count: usize,
    pub test_files: Vec<String>,
    pub test_commands: Vec<String>,
    pub test_frameworks: Vec<String>,
    pub central_systems: BTreeMap<String, String>,
    pub input_actions_used: Vec<String>,
    pub relationships: Vec<GodotRelationship>,
    pub asset_counts: BTreeMap<String, usize>,
    pub assets: Vec<String>,
}

pub fn parse_project(content: &str) -> GodotProjectConfig {
    let mut parsed = GodotProjectConfig::default();
    let mut section = String::new();
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].to_string();
            continue;
        }
        let Some((key, raw_value)) = line.split_once('=') else { continue };
        let key = key.trim().to_string();
        let raw_value = raw_value.trim();
        let value = unquote(raw_value);
        match (section.as_str(), key.as_str()) {
            ("", "config_version") => parsed.config_version = Some(value),
            ("application", "run/main_scene") => parsed.main_scene = Some(value),
            ("application", "config/features") => parsed.features = quoted_values(raw_value),
            ("autoload", _) => {
                parsed.autoloads.insert(key, value.trim_start_matches('*').to_string());
            }
            ("input", _) => parsed.input_actions.push(key),
            ("editor_plugins", "enabled") => parsed.enabled_plugins = quoted_values(raw_value),
            ("rendering", _) => {
                parsed.rendering.insert(key, value);
            }
            (section, _) if section.starts_with("display") => {
                parsed.display.insert(key, value);
            }
            (section, _) if section.starts_with("physics") => {
                parsed.physics.insert(key, value);
            }
            (section, _) if section.starts_with("internationalization") => {
                parsed.localization.insert(key, value);
            }
            (section, _) if section.contains("layer_names") => {
                parsed.layers.insert(key, value);
            }
            _ => {}
        }
    }
    parsed.input_actions.sort();
    parsed.input_actions.dedup();
    parsed
}

pub fn parse_gdscript(content: &str) -> GdscriptSummary {
    let mut parsed = GdscriptSummary::default();
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(value) = line.strip_prefix("extends ") {
            parsed.extends.get_or_insert_with(|| value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("class_name ") {
            parsed.class_name.get_or_insert_with(|| symbol_name(value));
        } else if let Some(value) = line.strip_prefix("@export") {
            push_unique(&mut parsed.annotations, line.split_whitespace().next().unwrap_or(line));
            if let Some((_, declaration)) = value.split_once("var ") {
                let name = symbol_name(declaration);
                push_unique(&mut parsed.exported, &name);
                push_unique(&mut parsed.members, &name);
            }
        } else if let Some(value) = line.strip_prefix("@onready") {
            push_unique(&mut parsed.annotations, "@onready");
            if let Some((_, declaration)) = value.split_once("var ") {
                let name = symbol_name(declaration);
                push_unique(&mut parsed.onready, &name);
                push_unique(&mut parsed.members, &name);
            }
        } else if line.starts_with('@') {
            push_unique(&mut parsed.annotations, line.split_whitespace().next().unwrap_or(line));
        } else if let Some(value) = line.strip_prefix("signal ") {
            push_unique(&mut parsed.signals, &symbol_name(value));
        } else if let Some(value) = line.strip_prefix("enum ") {
            push_unique(&mut parsed.enums, &symbol_name(value));
        } else if let Some(value) = line.strip_prefix("const ") {
            push_unique(&mut parsed.constants, &symbol_name(value));
        } else if let Some(value) = line.strip_prefix("static func ") {
            let name = symbol_name(value);
            push_unique(&mut parsed.static_methods, &name);
            push_unique(&mut parsed.methods, &name);
        } else if let Some(value) = line.strip_prefix("func ") {
            push_unique(&mut parsed.methods, &symbol_name(value));
        } else if let Some(value) = line.strip_prefix("class ") {
            push_unique(&mut parsed.inner_classes, &symbol_name(value));
        } else if raw_line == raw_line.trim_start() {
            if let Some(value) = line.strip_prefix("var ") {
                push_unique(&mut parsed.members, &symbol_name(value));
            }
        }
    }
    for captures in QUOTED_RES_PATH.captures_iter(content) {
        push_unique(&mut parsed.references, captures.get(1).unwrap().as_str());
    }
    for captures in INPUT_ACTION.captures_iter(content) {
        for literal in STRING_LITERAL.captures_iter(captures.get(1).unwrap().as_str()) {
            push_unique(&mut parsed.input_actions, literal.get(1).unwrap().as_str());
        }
    }
    for captures in NODE_PATH.captures_iter(content) {
        if let Some(value) = captures.get(1).or_else(|| captures.get(2)) {
            push_unique(&mut parsed.node_paths, value.as_str());
        }
    }
    parsed
}

pub fn parse_scene(content: &str) -> SceneSummary {
    let mut parsed = SceneSummary::default();
    let mut resources: HashMap<String, String> = HashMap::new();
    let mut current_node: Option<usize> = None;
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.starts_with("[gd_scene") {
            parsed.format = "scene".to_string();
        } else if line.starts_with("[gd_resource") {
            parsed.format = "resource".to_string();
        } else if line.starts_with("[ext_resource ") {
            let values =
                attributes(line.trim_start_matches("[ext_resource ").trim_end_matches(']'));
            let (Some(path), Some(resource_type), Some(id)) =
                (values.get("path"), values.get("type"), values.get("id"))
            else {
                continue;
            };
            let path = path.clone();
            let resource_type = resource_type.clone();
            let id = id.clone();
            resources.insert(id.clone(), path.clone());
            parsed.external_resources.insert(id.clone(), path);
            parsed.external_resource_types.insert(id, resource_type);
            current_node = None;
        } else if line.starts_with("[sub_resource") {
            parsed.subresources.push(line.to_string());
            current_node = None;
        } else if let Some(captures) = NODE_HEADER.captures(line) {
            let attributes = attributes(captures.get(1).unwrap().as_str());
            let name = attributes.get("name").cloned().unwrap_or_else(|| "<unnamed>".to_string());
            let parent = attributes.get("parent").cloned();
            let path = match parent.as_deref() {
                None | Some("") => name.clone(),
                Some(".") => name.clone(),
                Some(parent) => format!("{parent}/{name}"),
            };
            let instance = attributes
                .get("instance")
                .and_then(|value| ext_resource_id(value))
                .and_then(|id| resources.get(&id).cloned());
            let groups =
                attributes.get("groups").map(|value| quoted_values(value)).unwrap_or_default();
            if parsed.root.is_none() {
                parsed.root = Some(format!(
                    "{} ({})",
                    name,
                    attributes.get("type").map(String::as_str).unwrap_or("instanced")
                ));
            }
            parsed.nodes.push(SceneNode {
                path,
                name,
                node_type: attributes.get("type").cloned(),
                parent,
                instance,
                groups,
                ..SceneNode::default()
            });
            current_node = Some(parsed.nodes.len() - 1);
        } else if let Some(captures) = CONNECTION.captures(line) {
            let values = attributes(captures.get(1).unwrap().as_str());
            parsed.connections.push(format!(
                "{}: {} -> {}.{}",
                values.get("signal").map(String::as_str).unwrap_or("?"),
                values.get("from").map(String::as_str).unwrap_or("?"),
                values.get("to").map(String::as_str).unwrap_or("?"),
                values.get("method").map(String::as_str).unwrap_or("?")
            ));
            current_node = None;
        } else if let (Some(index), Some((key, value))) = (current_node, line.split_once('=')) {
            let key = key.trim();
            let value = value.trim();
            if key == "script" {
                parsed.nodes[index].script =
                    ext_resource_id(value).and_then(|id| resources.get(&id).cloned());
            } else if is_important_scene_property(key) {
                parsed.nodes[index]
                    .important_properties
                    .insert(key.to_string(), truncate(value, 160));
            }
        }
    }
    parsed
}

pub fn parse_shader(content: &str) -> ShaderSummary {
    let mut parsed = ShaderSummary::default();
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if let Some(value) = line.strip_prefix("shader_type ") {
            parsed.shader_type = Some(value.trim_end_matches(';').to_string());
        } else if let Some(value) = line.strip_prefix("render_mode ") {
            parsed
                .render_modes
                .extend(value.trim_end_matches(';').split(',').map(|mode| mode.trim().to_string()));
        } else if line.starts_with("uniform ") {
            parsed.uniforms.push(truncate(line, 200));
        } else if let Some(paren) = line.find('(') {
            let before = line[..paren].trim();
            if !before.is_empty() && line.ends_with('{') {
                if let Some(name) = before.split_whitespace().last() {
                    push_unique(&mut parsed.functions, name);
                }
            }
        }
    }
    for captures in SHADER_INCLUDE.captures_iter(content) {
        push_unique(&mut parsed.includes, captures.get(1).unwrap().as_str());
    }
    parsed
}

pub fn analyze(
    root: &Path,
    files: &[FileInfo],
    dispositions: &[FileDisposition],
    signals: Vec<String>,
) -> GodotSummary {
    let mut summary = GodotSummary { detected: true, signals, ..GodotSummary::default() };
    let mut relationships = BTreeSet::new();
    let mut named_inheritance = Vec::new();
    let known_paths = known_godot_paths(files, dispositions);
    if let Ok((content, _)) = read_file_safe(&root.join("project.godot"), None, None) {
        summary.project = parse_project(&content);
    }
    for file in files {
        match file.language.as_str() {
            "godot_project" if file.relative_path == "project.godot" => {}
            "gdscript" => {
                summary.gdscript_count += 1;
                if file.tags.contains("test") {
                    summary.test_files.push(file.relative_path.clone());
                }
                if let Ok((content, _)) = read_file_safe(&file.path, None, None) {
                    let script = parse_gdscript(&content);
                    if let Some(class_name) = script.class_name {
                        summary.central_systems.insert(class_name, file.relative_path.clone());
                    }
                    for action in script.input_actions {
                        push_unique(&mut summary.input_actions_used, &action);
                    }
                    if let Some(parent) = script.extends {
                        if parent.starts_with("res://") {
                            relationships.insert(GodotRelationship {
                                source: file.relative_path.clone(),
                                kind: "script_extends_script".to_string(),
                                resolved: is_known_godot_target(&parent, &known_paths),
                                target: parent,
                            });
                        } else {
                            named_inheritance.push((file.relative_path.clone(), parent));
                        }
                    }
                    for target in script.references {
                        relationships.insert(GodotRelationship {
                            source: file.relative_path.clone(),
                            kind: if file.tags.contains("test") {
                                "test_references".to_string()
                            } else {
                                "script_loads".to_string()
                            },
                            resolved: is_known_godot_target(&target, &known_paths),
                            target,
                        });
                    }
                    for autoload in summary.project.autoloads.keys() {
                        let pattern = Regex::new(&format!(r"\b{}\b", regex::escape(autoload)))
                            .expect("escaped autoload regex");
                        if pattern.is_match(&content) {
                            relationships.insert(GodotRelationship {
                                source: file.relative_path.clone(),
                                kind: "script_uses_autoload".to_string(),
                                resolved: is_known_godot_target(autoload, &known_paths),
                                target: autoload.clone(),
                            });
                        }
                    }
                }
            }
            "godot_scene" | "godot_resource" => {
                if file.language == "godot_scene" {
                    summary.scene_count += 1;
                } else {
                    summary.resource_count += 1;
                }
                if let Ok((content, _)) = read_file_safe(&file.path, None, None) {
                    let scene = parse_scene(&content);
                    for node in scene.nodes {
                        if let Some(target) = node.script {
                            relationships.insert(GodotRelationship {
                                source: file.relative_path.clone(),
                                kind: "scene_attaches_script".to_string(),
                                resolved: is_known_godot_target(&target, &known_paths),
                                target,
                            });
                        }
                        if let Some(target) = node.instance {
                            relationships.insert(GodotRelationship {
                                source: file.relative_path.clone(),
                                kind: "scene_instantiates_scene".to_string(),
                                resolved: is_known_godot_target(&target, &known_paths),
                                target,
                            });
                        }
                    }
                    for target in scene.external_resources.values() {
                        relationships.insert(GodotRelationship {
                            source: file.relative_path.clone(),
                            kind: "scene_references_resource".to_string(),
                            resolved: is_known_godot_target(target, &known_paths),
                            target: target.clone(),
                        });
                    }
                }
            }
            "godot_shader" | "godot_shader_include" => {
                summary.shader_count += 1;
                if let Ok((content, _)) = read_file_safe(&file.path, None, None) {
                    for target in parse_shader(&content).includes {
                        relationships.insert(GodotRelationship {
                            source: file.relative_path.clone(),
                            kind: "shader_includes".to_string(),
                            resolved: is_known_godot_target(&target, &known_paths),
                            target,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    if let Some(main_scene) = &summary.project.main_scene {
        relationships.insert(GodotRelationship {
            source: "project.godot".to_string(),
            kind: "project_main_scene".to_string(),
            resolved: is_known_godot_target(main_scene, &known_paths),
            target: main_scene.clone(),
        });
    }
    for (source, parent_class) in named_inheritance {
        if let Some(target) = summary.central_systems.get(&parent_class) {
            relationships.insert(GodotRelationship {
                source,
                kind: "script_extends_script".to_string(),
                resolved: is_known_godot_target(target, &known_paths),
                target: target.clone(),
            });
        }
    }
    for (name, target) in &summary.project.autoloads {
        relationships.insert(GodotRelationship {
            source: "project.godot".to_string(),
            kind: format!("project_autoload:{name}"),
            resolved: is_known_godot_target(target, &known_paths),
            target: target.clone(),
        });
    }

    for disposition in dispositions.iter().filter(|item| item.reason.as_str() == "inventory_only") {
        *summary.asset_counts.entry(disposition.language.clone()).or_insert(0) += 1;
        summary.assets.push(disposition.path.clone());
    }
    summary.assets.sort();
    summary.test_files.sort();
    summary.test_files.dedup();
    summary.test_commands = discover_test_commands(root, files);
    summary.input_actions_used.sort();
    if root.join("addons/gut").is_dir() {
        summary.test_frameworks.push("GUT".to_string());
    }
    if root.join("addons/gdUnit4").is_dir() || root.join("addons/gdunit4").is_dir() {
        summary.test_frameworks.push("GdUnit4".to_string());
    }
    summary.relationships = relationships.into_iter().collect();
    summary
}

fn known_godot_paths(files: &[FileInfo], dispositions: &[FileDisposition]) -> BTreeSet<String> {
    let mut paths =
        files.iter().map(|file| normalize_godot_path(&file.relative_path)).collect::<BTreeSet<_>>();
    paths.extend(
        dispositions
            .iter()
            .filter(|disposition| disposition.reason.as_str() == "inventory_only")
            .map(|disposition| normalize_godot_path(&disposition.path)),
    );
    paths
}

fn normalize_godot_path(path: &str) -> String {
    normalize_path(path).trim_start_matches('/').to_string()
}

fn is_known_godot_target(target: &str, known_paths: &BTreeSet<String>) -> bool {
    target
        .strip_prefix("res://")
        .map(|path| {
            let path = normalize_godot_path(path);
            !path.is_empty() && known_paths.contains(&path)
        })
        .unwrap_or(true)
}

fn discover_test_commands(root: &Path, files: &[FileInfo]) -> Vec<String> {
    let mut commands = BTreeSet::new();
    for file in files.iter().filter(|file| {
        file.is_readme
            || file.relative_path.starts_with(".github/")
            || matches!(file.extension.as_str(), ".sh" | ".bash" | ".zsh" | ".yml" | ".yaml")
    }) {
        let Ok((content, _)) = read_file_safe(&root.join(&file.relative_path), None, None) else {
            continue;
        };
        for line in content.lines() {
            let trimmed =
                line.trim().trim_start_matches(['-', '`', '>', ' ']).trim_end_matches('`');
            if trimmed.contains("godot")
                && trimmed.contains("--headless")
                && trimmed.contains("--script")
            {
                commands.insert(trimmed.to_string());
            }
        }
    }
    commands.into_iter().collect()
}

fn quoted_values(value: &str) -> Vec<String> {
    STRING_LITERAL
        .captures_iter(value)
        .map(|capture| capture.get(1).unwrap().as_str().to_string())
        .collect()
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

fn symbol_name(value: &str) -> String {
    value
        .split(|character: char| {
            character == '(' || character == ':' || character == '=' || character.is_whitespace()
        })
        .find(|part| !part.is_empty())
        .unwrap_or(value)
        .to_string()
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !value.is_empty() && !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn attributes(value: &str) -> BTreeMap<String, String> {
    ATTRIBUTE
        .captures_iter(value)
        .filter_map(|capture| {
            let key = capture.get(1)?.as_str().to_string();
            let value = capture.get(2).or_else(|| capture.get(3))?.as_str().to_string();
            Some((key, value))
        })
        .collect()
}

fn ext_resource_id(value: &str) -> Option<String> {
    let value = value.trim();
    let inner = value.strip_prefix("ExtResource(")?.trim_end_matches(')').trim();
    Some(inner.trim_matches('"').to_string())
}

fn is_important_scene_property(key: &str) -> bool {
    matches!(
        key,
        "visible"
            | "process_mode"
            | "position"
            | "rotation"
            | "rotation_degrees"
            | "scale"
            | "material"
            | "texture"
            | "mesh"
            | "shape"
            | "stream"
            | "text"
            | "theme"
            | "metadata"
    ) || key.starts_with("script_")
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        format!("{}…", value.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gdscript_extracts_symbols_and_dependencies() {
        let parsed = parse_gdscript(
            r#"@tool
class_name Player
extends CharacterBody3D
signal damaged(amount)
enum State { IDLE, RUN }
const HUD = preload("res://ui/hud.tscn")
@export var speed := 4.0
@onready var camera = $Camera3D
static func make():
    pass
func _physics_process(delta):
    Input.is_action_pressed("move_left")
"#,
        );
        assert_eq!(parsed.class_name.as_deref(), Some("Player"));
        assert_eq!(parsed.extends.as_deref(), Some("CharacterBody3D"));
        assert!(parsed.signals.contains(&"damaged".to_string()));
        assert!(parsed.exported.contains(&"speed".to_string()));
        assert!(parsed.onready.contains(&"camera".to_string()));
        assert!(parsed.static_methods.contains(&"make".to_string()));
        assert!(parsed.references.contains(&"res://ui/hud.tscn".to_string()));
        assert!(parsed.input_actions.contains(&"move_left".to_string()));
    }

    #[test]
    fn scene_extracts_hierarchy_scripts_instances_and_connections() {
        let parsed = parse_scene(
            r#"[gd_scene load_steps=3 format=3]
[ext_resource path="res://player.gd" type="Script" id="1"]
[ext_resource path="res://hud.tscn" type="PackedScene" id="2"]
[node name="Main" type="Node"]
script = ExtResource("1")
[node name="HUD" parent="." instance=ExtResource("2") groups=["ui"]]
[connection signal="pressed" from="HUD/Button" to="." method="_start"]
"#,
        );
        assert_eq!(parsed.root.as_deref(), Some("Main (Node)"));
        assert_eq!(parsed.nodes[0].script.as_deref(), Some("res://player.gd"));
        assert_eq!(parsed.nodes[1].instance.as_deref(), Some("res://hud.tscn"));
        assert!(parsed.connections[0].contains("pressed"));
    }

    #[test]
    fn project_extracts_main_scene_autoloads_inputs_and_settings() {
        let parsed = parse_project(
            r#"config_version=5
[application]
run/main_scene="res://main.tscn"
config/features=PackedStringArray("4.3", "GL Compatibility")
[autoload]
Save="*res://save.gd"
[input]
jump={"deadzone": 0.5}
[rendering]
renderer/rendering_method="gl_compatibility"
"#,
        );
        assert_eq!(parsed.main_scene.as_deref(), Some("res://main.tscn"));
        assert_eq!(parsed.autoloads.get("Save").map(String::as_str), Some("res://save.gd"));
        assert_eq!(parsed.input_actions, vec!["jump"]);
        assert!(parsed.features.contains(&"4.3".to_string()));
    }

    #[test]
    fn shader_extracts_render_contract() {
        let parsed = parse_shader(
            "shader_type spatial;\nrender_mode unshaded, cull_disabled;\nuniform float strength;\n#include \"res://shaders/noise.gdshaderinc\"\nvoid fragment() {\n}\n",
        );
        assert_eq!(parsed.shader_type.as_deref(), Some("spatial"));
        assert!(parsed.render_modes.contains(&"unshaded".to_string()));
        assert!(parsed.uniforms[0].contains("strength"));
        assert!(parsed.includes.contains(&"res://shaders/noise.gdshaderinc".to_string()));
    }
}

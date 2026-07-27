//! Godot repository detection and lightweight text-format analysis.

use crate::domain::{godot_text_extensions, Config, ProjectProfile};
use serde::{Deserialize, Serialize};
use std::path::Path;

mod analysis;

pub use analysis::{
    analyze, parse_gdscript, parse_project, parse_scene, parse_shader, GdscriptSummary,
    GodotProjectConfig, GodotRelationship, GodotSummary, SceneNode, SceneSummary, ShaderSummary,
};

/// How the Godot profile should treat a file before normal text scanning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePolicy {
    /// Apply the normal text-file scanner.
    Text,
    /// Keep metadata in the inventory but never read or chunk content.
    InventoryOnly,
}

/// Classify Godot-generated metadata and binary assets as inventory-only.
#[must_use]
pub fn file_policy(path: &Path) -> FilePolicy {
    let extension =
        path.extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "uid"
            | "import"
            | "res"
            | "scn"
            | "png"
            | "jpg"
            | "jpeg"
            | "webp"
            | "svg"
            | "glb"
            | "gltf"
            | "blend"
            | "fbx"
            | "obj"
            | "wav"
            | "ogg"
            | "mp3"
            | "flac"
            | "mp4"
            | "webm"
    ) {
        FilePolicy::InventoryOnly
    } else {
        FilePolicy::Text
    }
}

/// Detection evidence for a Godot repository.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GodotDetection {
    /// Whether Godot-specific behavior is active.
    pub active: bool,
    /// Stable, sorted signals that caused detection.
    pub signals: Vec<String>,
}

/// Detect a Godot project using cheap repository-level signals.
#[must_use]
pub fn detect(root: &Path, requested: ProjectProfile) -> GodotDetection {
    if requested == ProjectProfile::Generic {
        return GodotDetection::default();
    }

    let mut signals = Vec::new();
    if root.join("project.godot").is_file() {
        signals.push("project.godot".to_string());
    }
    if root.join(".godot").is_dir() {
        signals.push(".godot/".to_string());
    }

    if requested == ProjectProfile::Godot || signals.is_empty() {
        let mut extensions = std::collections::BTreeSet::new();
        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                !matches!(
                    entry.file_name().to_str(),
                    Some(".git" | ".godot" | "node_modules" | "target" | "out")
                )
            })
            .flatten()
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let extension = entry
                .path()
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(extension.as_str(), "gd" | "tscn" | "tres" | "gdshader") {
                extensions.insert(format!(".{extension}"));
                break;
            }
        }
        signals.extend(extensions);
    }

    signals.sort();
    signals.dedup();
    GodotDetection { active: requested == ProjectProfile::Godot || !signals.is_empty(), signals }
}

/// Resolve auto-detection and merge Godot text formats into the existing config.
pub fn resolve_profile(config: &mut Config, root: &Path) -> GodotDetection {
    let detection = detect(root, config.profile);
    config.profile = if detection.active { ProjectProfile::Godot } else { ProjectProfile::Generic };
    if detection.active {
        config.include_extensions.extend(godot_text_extensions());
        config.exclude_globs.insert(".godot/**".to_string());
    }
    detection
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn project_file_is_the_strongest_detection_signal() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(temp.path().join("project.godot"), "config_version=5\n").expect("project");

        let detected = detect(temp.path(), ProjectProfile::Auto);

        assert!(detected.active);
        assert!(detected.signals.contains(&"project.godot".to_string()));
    }

    #[test]
    fn generic_profile_disables_auto_detection() {
        let temp = TempDir::new().expect("temp dir");
        fs::write(temp.path().join("player.gd"), "extends Node\n").expect("script");

        assert!(!detect(temp.path(), ProjectProfile::Generic).active);
    }
}

use repo_context::app::export::{execute, ExportExecutionOptions};
use repo_context::chunk::chunk_content;
use repo_context::domain::{Config, FileInfo, OutputMode, ProjectProfile};
use repo_context::module::focus_picker::ScanMode;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn write_fixture(root: &Path) {
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::create_dir_all(root.join("shaders")).unwrap();
    fs::create_dir_all(root.join("data")).unwrap();
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::create_dir_all(root.join(".godot/editor")).unwrap();
    fs::write(
        root.join("project.godot"),
        "config_version=5\n[application]\nrun/main_scene=\"res://main.tscn\"\nconfig/features=PackedStringArray(\"4.3\")\n[autoload]\nSave=\"*res://scripts/save.gd\"\n[input]\njump={\"deadzone\":0.5}\n[rendering]\nrenderer/rendering_method=\"gl_compatibility\"\n",
    )
    .unwrap();
    fs::write(
        root.join("main.tscn"),
        "[gd_scene load_steps=2 format=3]\n[ext_resource path=\"res://scripts/player.gd\" type=\"Script\" id=\"1\"]\n[node name=\"Main\" type=\"Node\"]\nscript = ExtResource(\"1\")\n",
    )
    .unwrap();
    fs::write(
        root.join("scripts/player.gd"),
        "class_name Player\nextends Node\nsignal jumped\n@export var speed := 4.0\nfunc jump():\n\tInput.is_action_pressed(\"jump\")\n\tload(\"res://assets/player.png\")\n",
    )
    .unwrap();
    fs::write(root.join("scripts/save.gd"), "extends Node\nfunc save():\n\tpass\n").unwrap();
    fs::write(
        root.join("tests/test_player.gd"),
        "extends SceneTree\nconst PlayerScript = preload(\"res://scripts/player.gd\")\nfunc _init():\n\tquit(0)\n",
    )
    .unwrap();
    fs::write(
        root.join("shaders/player.gdshader"),
        "shader_type canvas_item;\nuniform float amount;\nvoid fragment() {\n COLOR = texture(TEXTURE, UV);\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("data/world.json"),
        r#"{"roads":[{"id":"r1","nodes":["a","b"]}],"nodes":{"a":[0,0],"b":[1,1]},"signals":[{"node":"b"}]}"#,
    )
    .unwrap();
    fs::write(root.join("assets/player.png"), [0_u8, 1, 0, 2]).unwrap();
    fs::write(root.join("assets/player.png.import"), "[remap]\n").unwrap();
    fs::write(root.join("scripts/player.gd.uid"), "uid://player\n").unwrap();
    fs::write(root.join(".godot/editor/cache.cfg"), "generated=true\n").unwrap();
    fs::write(
        root.join("README.md"),
        "# Fixture\n\n`godot --headless --path . --script tests/test_player.gd`\n",
    )
    .unwrap();
}

fn export_fixture(root: &Path, output: &Path) -> Value {
    let config = Config {
        path: Some(root.to_path_buf()),
        output_dir: output.to_path_buf(),
        mode: OutputMode::Both,
        ..Config::default()
    };
    let outcome = execute(
        config,
        ExportExecutionOptions {
            include_timestamp: false,
            explicit_config_path: None,
            scan_mode: Some(ScanMode::Full),
            focus_path: None,
        },
    )
    .unwrap();
    let report = outcome.output_files.iter().find(|path| path.ends_with("_report.json")).unwrap();
    serde_json::from_str(&fs::read_to_string(report).unwrap()).unwrap()
}

#[test]
fn godot_export_is_structural_complete_and_deterministic() {
    let fixture = TempDir::new().unwrap();
    let output = TempDir::new().unwrap();
    write_fixture(fixture.path());

    let first = export_fixture(fixture.path(), output.path());
    let first_text = serde_json::to_string_pretty(&first).unwrap();
    let second = export_fixture(fixture.path(), output.path());
    let second_text = serde_json::to_string_pretty(&second).unwrap();

    assert_eq!(first_text, second_text);
    assert_eq!(first["config"]["profile"], "godot");
    assert_eq!(first["godot"]["project"]["main_scene"], "res://main.tscn");
    assert_eq!(first["godot"]["test_files"].as_array().unwrap().len(), 1);
    assert_eq!(first["godot"]["test_commands"].as_array().unwrap().len(), 1);
    assert!(first["stats"]["languages_detected"].get("gdscript").is_some());
    assert!(first["stats"]["languages_detected"].get("godot_scene").is_some());
    assert_eq!(first["stats"]["files_skipped"]["minified"], 0);

    let dispositions = first["file_dispositions"].as_array().unwrap();
    assert!(dispositions
        .iter()
        .any(|item| { item["path"] == "assets/player.png" && item["reason"] == "inventory_only" }));
    assert!(dispositions
        .iter()
        .any(|item| { item["path"] == "data/world.json" && item["reason"] != "skipped_minified" }));

    let chunks_path = output
        .path()
        .join(fixture.path().file_name().unwrap())
        .join(format!("{}_chunks.jsonl", fixture.path().file_name().unwrap().to_string_lossy()));
    let chunks = fs::read_to_string(chunks_path).unwrap();
    assert!(!chunks.lines().any(|line| {
        let value: Value = serde_json::from_str(line).unwrap();
        value["path"] == "assets/player.png"
    }));
    assert!(chunks.lines().any(|line| {
        let value: Value = serde_json::from_str(line).unwrap();
        value["path"] == "data/world.json"
            && value["tags"].as_array().unwrap().iter().any(|tag| tag == "json-key:roads")
    }));
}

#[test]
fn malformed_godot_and_json_files_fall_back_without_panicking() {
    let gd = FileInfo {
        path: PathBuf::from("broken.gd"),
        relative_path: "broken.gd".to_string(),
        size_bytes: 10,
        extension: ".gd".to_string(),
        language: "gdscript".to_string(),
        id: "gd".to_string(),
        priority: 0.5,
        token_estimate: 0,
        tags: BTreeSet::new(),
        is_readme: false,
        is_config: false,
        is_doc: false,
    };
    assert!(!chunk_content(&gd, "func broken(:\n  ???\n", 100, 0).unwrap().is_empty());

    let json = FileInfo {
        language: "json".to_string(),
        extension: ".json".to_string(),
        relative_path: "broken.json".to_string(),
        ..gd
    };
    assert!(!chunk_content(&json, "{not json", 100, 0).unwrap().is_empty());
}

#[test]
fn large_scene_uses_bounded_summary_and_rag_detail_batches() {
    let scene = FileInfo {
        path: PathBuf::from("large.tscn"),
        relative_path: "large.tscn".to_string(),
        size_bytes: 10_000,
        extension: ".tscn".to_string(),
        language: "godot_scene".to_string(),
        id: "scene".to_string(),
        priority: 0.6,
        token_estimate: 0,
        tags: BTreeSet::new(),
        is_readme: false,
        is_config: false,
        is_doc: false,
    };
    let mut content = "[gd_scene format=3]\n[node name=\"Root\" type=\"Node\"]\n".to_string();
    for index in 0..300 {
        content.push_str(&format!(
            "[node name=\"Child{index}\" type=\"Node3D\" parent=\".\"]\nposition = Vector3({index}, 0, 0)\n"
        ));
    }

    let chunks = chunk_content(&scene, &content, 200, 0).unwrap();

    assert!(chunks[0].tags.contains("scene-summary"));
    assert!(!chunks[0].tags.contains("rag-only"));
    assert!(chunks.iter().skip(1).any(|chunk| chunk.tags.contains("rag-only")));
    assert!(chunks.iter().all(|chunk| chunk.token_estimate <= 210));
}

#[test]
fn explicit_generic_profile_preserves_non_godot_scanning() {
    let fixture = TempDir::new().unwrap();
    fs::write(fixture.path().join("main.rs"), "fn main() {}\n").unwrap();
    fs::write(fixture.path().join("orphan.gd"), "extends Node\n").unwrap();
    let output = TempDir::new().unwrap();
    let mut config = Config {
        path: Some(fixture.path().to_path_buf()),
        output_dir: output.path().to_path_buf(),
        profile: ProjectProfile::Generic,
        ..Config::default()
    };
    config.mode = OutputMode::Rag;
    let outcome = execute(
        config,
        ExportExecutionOptions {
            include_timestamp: false,
            explicit_config_path: None,
            scan_mode: Some(ScanMode::Full),
            focus_path: None,
        },
    )
    .unwrap();

    assert_eq!(outcome.stats.languages_detected.get("rust"), Some(&1));
    assert!(!outcome.stats.languages_detected.contains_key("gdscript"));
}

#[test]
#[ignore = "requires the developer's external cuoc-cuoi integration fixture"]
fn cuoc_cuoi_acceptance_fixture() {
    let root = Path::new("/Users/nguyenhuyvu/projects/cuoc-cuoi");
    assert!(root.join("project.godot").exists(), "cuoc-cuoi fixture missing");
    let output = TempDir::new().unwrap();
    let report = export_fixture(root, output.path());

    assert_eq!(report["godot"]["test_files"].as_array().unwrap().len(), 5);
    assert_eq!(report["godot"]["gdscript_count"], 15);
    assert_eq!(report["godot"]["shader_count"], 2);
    assert_eq!(report["stats"]["files_skipped"]["minified"], 0);
    assert!(report["files"].as_array().unwrap().iter().any(|file| file["path"] == "main.tscn"));
    assert!(report["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file["path"] == "data/can_tho_core.json"));
}

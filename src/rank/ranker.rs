//! File ranker implementation with manifest-aware entrypoint detection.

use crate::domain::{FileInfo, RankingWeights};
use crate::utils::{
    is_likely_generated, is_lock_file, is_vendored, normalize_path, read_file_safe,
};
use serde_json::Value as JsonValue;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const IMPORTANT_DOC_FILES: &[&str] = &[
    "README.md",
    "README.rst",
    "README.txt",
    "README",
    "CONTRIBUTING.md",
    "CHANGELOG.md",
    "HISTORY.md",
    "docs/index.md",
    "docs/README.md",
    "documentation/index.md",
];

const IMPORTANT_CONFIG_FILES: &[&str] = &[
    "pyproject.toml",
    "package.json",
    "tsconfig.json",
    "go.mod",
    "Cargo.toml",
    "pom.xml",
    "build.gradle",
    "Makefile",
    "Dockerfile",
    "docker-compose.yml",
    "docker-compose.yaml",
    ".env.example",
    "tox.ini",
    "setup.cfg",
];

pub struct FileRanker {
    root_path: PathBuf,
    scanned_files: HashSet<String>,
    entrypoint_candidates: HashSet<String>,
    entrypoints: HashSet<String>,
    detected_languages: HashSet<String>,
    manifest_info: HashMap<String, JsonValue>,
    weights: RankingWeights,
}

impl FileRanker {
    pub fn new(root_path: &Path, scanned_files: HashSet<String>) -> Self {
        Self::with_weights(root_path, scanned_files, RankingWeights::default())
    }
    pub fn with_weights(
        root_path: &Path,
        scanned_files: HashSet<String>,
        weights: RankingWeights,
    ) -> Self {
        let mut ranker = Self {
            root_path: root_path.to_path_buf(),
            scanned_files,
            entrypoint_candidates: HashSet::new(),
            entrypoints: HashSet::new(),
            detected_languages: HashSet::new(),
            manifest_info: HashMap::new(),
            weights,
        };
        ranker.load_manifests();
        ranker.validate_entrypoints();
        ranker
    }

    pub fn rank_file(&self, file: &mut FileInfo) {
        let rel_normalized = normalize_path(&file.relative_path);
        let rel_lower = rel_normalized.to_lowercase();
        let name = file.path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();

        file.is_readme = name.starts_with("readme");
        file.is_config = is_config_file(&name, &rel_normalized);
        file.is_doc = is_doc_file(&name, &rel_normalized);

        let content_sample =
            read_file_safe(&file.path, Some(2000), None).map(|(s, _)| s).unwrap_or_default();

        let mut priority: f64 = self.weights.default;
        if file.is_readme {
            priority = self.weights.readme;
        } else if is_important_doc(&rel_normalized, &name) {
            priority = self.weights.main_doc;
        } else if is_vendored(&file.path) {
            priority = self.weights.vendored;
        } else if is_lock_file(&file.path) {
            priority = self.weights.lock_file;
        } else if is_likely_generated(&file.path, &content_sample) {
            priority = self.weights.generated;
        } else if file.is_config {
            priority = self.weights.config;
        } else if self.entrypoints.contains(&rel_normalized) || is_common_entrypoint(&name) {
            priority = self.weights.entrypoint;
        } else if is_test_file(&name, &rel_lower) {
            priority = self.weights.test;
        } else if is_example_file(&rel_lower) {
            priority = self.weights.example;
        } else if is_core_source(&rel_lower) {
            priority = self.weights.core_source;
        } else if is_api_definition(&name) {
            priority = self.weights.api_definition;
        }

        file.priority = priority;

        if file.is_readme {
            file.tags.insert("readme".to_string());
        }
        if file.is_config {
            file.tags.insert("config".to_string());
        }
        // NOTE: Python does NOT add a "docs" tag in rank_file — is_doc only affects
        // priority score. We intentionally omit the "docs" tag to match Python behavior.
        if self.entrypoints.contains(&rel_normalized) {
            file.tags.insert("entrypoint".to_string());
        }
        if is_lock_file(&file.path) {
            file.tags.insert("lock-file".to_string());
        }
    }

    pub fn rank_files(&self, files: &mut [FileInfo]) {
        for file in files.iter_mut() {
            self.rank_file(file);
        }

        files.sort_by(|a, b| {
            b.priority
                .partial_cmp(&a.priority)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.relative_path.cmp(&b.relative_path))
        });
    }

    #[allow(dead_code)]
    pub fn get_entrypoints(&self) -> &HashSet<String> {
        &self.entrypoints
    }

    #[allow(dead_code)]
    pub fn get_detected_languages(&self) -> &HashSet<String> {
        &self.detected_languages
    }

    #[allow(dead_code)]
    pub fn get_manifest_info(&self) -> &HashMap<String, JsonValue> {
        &self.manifest_info
    }

    fn load_manifests(&mut self) {
        self.parse_pyproject();
        self.parse_package_json();
        self.parse_go_mod();
        self.parse_cargo_toml();

        if self.root_path.join("setup.py").exists() {
            self.detected_languages.insert("python".to_string());
        }
    }

    fn parse_pyproject(&mut self) {
        let path = self.root_path.join("pyproject.toml");
        if !path.exists() {
            return;
        }

        let Ok((content, _)) = read_file_safe(&path, None, None) else {
            return;
        };

        self.detected_languages.insert("python".to_string());

        if let Ok(value) = toml::from_str::<toml::Value>(&content) {
            if let Some(project) = value.get("project") {
                if let Some(project_table) = project.as_table() {
                    if let Some(scripts) = project_table.get("scripts") {
                        if let Some(script_table) = scripts.as_table() {
                            for script in script_table.values() {
                                if let Some(target) = script.as_str() {
                                    // Only add if module path contains a dot (matches Python guard
                                    // ranker.py:176: "." in module_path).
                                    let module_path = target.split(':').next().unwrap_or("");
                                    if module_path.contains('.') {
                                        let module = module_path.replace('.', "/");
                                        self.entrypoint_candidates
                                            .insert(normalize_path(&format!("{module}.py")));
                                        self.entrypoint_candidates.insert(normalize_path(
                                            &format!("{module}/__init__.py"),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn parse_package_json(&mut self) {
        let path = self.root_path.join("package.json");
        if !path.exists() {
            return;
        }

        let Ok((content, _)) = read_file_safe(&path, None, None) else {
            return;
        };

        let Ok(value) = serde_json::from_str::<JsonValue>(&content) else {
            return;
        };

        self.detected_languages.insert("javascript".to_string());

        for key in ["name", "description", "scripts"] {
            if let Some(v) = value.get(key) {
                self.manifest_info.insert(key.to_string(), v.clone());
            }
        }

        for key in ["main", "module", "types"] {
            if let Some(v) = value.get(key).and_then(JsonValue::as_str) {
                self.entrypoint_candidates.insert(normalize_path(v));
            }
        }

        if let Some(bin) = value.get("bin") {
            if let Some(single) = bin.as_str() {
                self.entrypoint_candidates.insert(normalize_path(single));
            } else if let Some(bin_obj) = bin.as_object() {
                for entry in bin_obj.values() {
                    if let Some(path) = entry.as_str() {
                        self.entrypoint_candidates.insert(normalize_path(path));
                    }
                }
            }
        }
    }

    fn parse_go_mod(&mut self) {
        let path = self.root_path.join("go.mod");
        if !path.exists() {
            return;
        }

        let Ok((content, _)) = read_file_safe(&path, None, None) else {
            return;
        };

        self.detected_languages.insert("go".to_string());

        if let Some(line) = content.lines().find(|l| l.trim_start().starts_with("module ")) {
            let module_name = line.trim_start_matches("module ").trim().to_string();
            self.manifest_info.insert("go_module".to_string(), JsonValue::String(module_name));
        }

        let cmd_dir = self.root_path.join("cmd");
        if let Ok(entries) = std::fs::read_dir(cmd_dir) {
            for entry in entries.flatten() {
                let main_go = entry.path().join("main.go");
                if main_go.exists() {
                    if let Ok(rel) = main_go.strip_prefix(&self.root_path) {
                        if let Some(rel_str) = rel.to_str() {
                            self.entrypoints.insert(normalize_path(rel_str));
                        }
                    }
                }
            }
        }
    }

    fn parse_cargo_toml(&mut self) {
        let path = self.root_path.join("Cargo.toml");
        if !path.exists() {
            return;
        }

        self.detected_languages.insert("rust".to_string());
        let Ok((content, _)) = read_file_safe(&path, None, None) else {
            return;
        };

        if let Ok(value) = toml::from_str::<toml::Value>(&content) {
            if let Some(package) = value.get("package") {
                if let Some(table) = package.as_table() {
                    if let Some(name) = table.get("name").and_then(toml::Value::as_str) {
                        self.manifest_info
                            .insert("name".to_string(), JsonValue::String(name.to_string()));
                    }
                }
            }
        }
        // Python only inserts detected_languages for Cargo.toml; it does NOT add
        // src/main.rs or src/lib.rs as entrypoint candidates (ranker.py).
    }

    fn validate_entrypoints(&mut self) {
        for candidate in &self.entrypoint_candidates {
            if self.scanned_files.contains(candidate) || self.root_path.join(candidate).exists() {
                self.entrypoints.insert(candidate.clone());
            }
        }
    }
}

fn is_common_entrypoint(name: &str) -> bool {
    matches!(
        name,
        "main.py" | "main.go" | "main.rs" | "index.js" | "index.ts" | "app.py" | "cli.py"
    )
}

fn is_core_source(rel: &str) -> bool {
    rel.starts_with("src/")
        || rel.starts_with("lib/")
        || rel.starts_with("pkg/")
        || rel.starts_with("app/")
        || rel.starts_with("core/")
        || rel.starts_with("internal/")
        || rel.starts_with("cmd/")
}

fn is_test_file(name: &str, rel: &str) -> bool {
    rel.starts_with("tests/")
        || rel.starts_with("test/")
        || rel.starts_with("__tests__/")
        || rel.starts_with("spec/")
        || name.starts_with("test_")
        || rel.contains("test_")  // matches anywhere in path (Python: re.compile(r"test_").search)
        || name.contains("_test")
        || name.contains(".test.")
        || name.contains(".spec.")
}

fn is_example_file(rel: &str) -> bool {
    rel.starts_with("examples/")
        || rel.starts_with("example/")
        || rel.starts_with("samples/")
        || rel.starts_with("sample/")
        || rel.starts_with("demo/")
}

fn is_doc_file(name: &str, rel: &str) -> bool {
    let rel_lower = rel.to_lowercase();
    // Check extension: .md, .rst, .txt, .adoc are considered doc files (matches Python is_doc)
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_lowercase()))
        .unwrap_or_default();
    let is_doc_ext = matches!(ext.as_str(), ".md" | ".rst" | ".txt" | ".adoc");

    is_important_doc(rel, name)
        || is_doc_ext
        || rel_lower.starts_with("docs/")
        || rel_lower.starts_with("documentation/")
}

fn is_important_doc(rel: &str, name: &str) -> bool {
    IMPORTANT_DOC_FILES.contains(&rel)
        || IMPORTANT_DOC_FILES.contains(&name)
        || IMPORTANT_DOC_FILES.iter().any(|d| d.to_lowercase() == name.to_lowercase())
}

fn is_config_file(name: &str, rel: &str) -> bool {
    IMPORTANT_CONFIG_FILES.contains(&rel) || IMPORTANT_CONFIG_FILES.contains(&name)
}

fn is_api_definition(name: &str) -> bool {
    ["api", "interface", "types", "models", "schema"].iter().any(|needle| name.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::FileRanker;
    use crate::domain::FileInfo;
    use std::collections::{BTreeSet, HashSet};
    use std::fs;
    use tempfile::TempDir;

    fn make_file(path: &std::path::Path, rel: &str, ext: &str, lang: &str) -> FileInfo {
        FileInfo {
            path: path.to_path_buf(),
            relative_path: rel.to_string(),
            size_bytes: 10,
            extension: ext.to_string(),
            language: lang.to_string(),
            id: "id".to_string(),
            priority: 0.0,
            token_estimate: 0,
            tags: BTreeSet::new(),
            is_readme: false,
            is_config: false,
            is_doc: false,
        }
    }

    #[test]
    fn detects_pyproject_script_entrypoints() {
        let tmp = TempDir::new().expect("tmp");
        fs::write(
            tmp.path().join("pyproject.toml"),
            "[project.scripts]\nr2p='repo_to_prompt.cli:main'\n",
        )
        .expect("write pyproject");
        fs::create_dir_all(tmp.path().join("repo_to_prompt")).expect("mkdir");
        fs::write(tmp.path().join("repo_to_prompt/cli.py"), "print('x')\n").expect("write cli");

        let scanned = HashSet::from(["repo_to_prompt/cli.py".to_string()]);
        let ranker = FileRanker::new(tmp.path(), scanned);

        assert!(ranker.get_entrypoints().contains("repo_to_prompt/cli.py"));
    }

    #[test]
    fn readme_ranks_higher_than_test() {
        let tmp = TempDir::new().expect("tmp");
        let readme_path = tmp.path().join("README.md");
        let test_path = tmp.path().join("tests/test_main.py");
        fs::create_dir_all(tmp.path().join("tests")).expect("mkdir tests");
        fs::write(&readme_path, "# hello").expect("write readme");
        fs::write(&test_path, "def test_x(): pass\n").expect("write test");

        let scanned = HashSet::from(["README.md".to_string(), "tests/test_main.py".to_string()]);
        let ranker = FileRanker::new(tmp.path(), scanned);

        let mut readme = make_file(&readme_path, "README.md", ".md", "markdown");
        let mut test_file = make_file(&test_path, "tests/test_main.py", ".py", "python");
        ranker.rank_file(&mut readme);
        ranker.rank_file(&mut test_file);

        assert!(readme.priority > test_file.priority);
    }
}

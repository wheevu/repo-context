//! File ranker implementation with manifest-aware entrypoint detection.

use crate::domain::{FileInfo, RankingWeights};
use crate::rank::workspace::discover_workspace_graph;
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

const CONTRIBUTION_DOC_PREFIXES: &[&str] =
    &["contributing", "code_of_conduct", "security", "authors", "maintainers"];

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

/// Ranks files by importance based on various signals.
///
/// Analyzes repository structure, manifest files, and file paths to assign
/// priority scores that reflect file importance.
pub struct FileRanker {
    root_path: PathBuf,
    scanned_files: HashSet<String>,
    entrypoint_candidates: HashSet<String>,
    entrypoints: HashSet<String>,
    detected_languages: HashSet<String>,
    manifest_info: HashMap<String, JsonValue>,
    workspace_members: Vec<String>,
    weights: RankingWeights,
}

#[derive(Debug, Clone, Copy)]
struct FileSignals {
    is_readme: bool,
    is_contribution_doc: bool,
    is_main_doc: bool,
    is_vendored: bool,
    is_lock_file: bool,
    is_generated: bool,
    is_config: bool,
    is_entrypoint: bool,
    is_test: bool,
    is_example: bool,
    is_core_source: bool,
    is_api_definition: bool,
    is_ci_workflow: bool,
    is_doc: bool,
}

impl FileRanker {
    /// Create a new FileRanker with default weights.
    ///
    /// This is primarily used in tests. In production, prefer [`Self::with_weights`].
    #[cfg(test)]
    #[must_use]
    pub fn new(root_path: &Path, scanned_files: HashSet<String>) -> Self {
        Self::with_weights(root_path, scanned_files, RankingWeights::default())
    }

    /// Creates a new FileRanker with custom weights.
    ///
    /// # Arguments
    /// * `root_path` - Path to the repository root
    /// * `scanned_files` - Set of file paths that were scanned
    /// * `weights` - Custom ranking weights
    ///
    /// # Returns
    /// A new FileRanker instance
    #[must_use]
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
            workspace_members: Vec::new(),
            weights,
        };
        ranker.load_manifests();
        ranker.validate_entrypoints();
        ranker
    }

    /// Assigns a priority score to a single file.
    ///
    /// # Arguments
    /// * `file` - The file to rank (modified in place)
    pub fn rank_file(&self, file: &mut FileInfo) {
        let rel_normalized = normalize_path(&file.relative_path);
        let rel_lower = rel_normalized.to_lowercase();
        let name = file.path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();

        let content_sample =
            read_file_safe(&file.path, Some(2000), None).map(|(s, _)| s).unwrap_or_default();

        let signals =
            self.collect_signals(file, &name, &rel_normalized, &rel_lower, &content_sample);

        file.is_readme = signals.is_readme;
        file.is_config = signals.is_config;
        file.is_doc = signals.is_doc;
        file.priority = self.score_signals(signals);

        if signals.is_readme {
            file.tags.insert("readme".to_string());
        }
        if signals.is_config {
            file.tags.insert("config".to_string());
        }
        if signals.is_contribution_doc {
            file.tags.insert("contribution".to_string());
        }
        if signals.is_ci_workflow {
            file.tags.insert("workflow".to_string());
        }
        if signals.is_entrypoint {
            file.tags.insert("entrypoint".to_string());
        }
        if signals.is_lock_file {
            file.tags.insert("lock-file".to_string());
        }
    }

    fn collect_signals(
        &self,
        file: &FileInfo,
        name: &str,
        rel_normalized: &str,
        rel_lower: &str,
        content_sample: &str,
    ) -> FileSignals {
        let is_readme = name.starts_with("readme");
        let is_config = is_config_file(name, rel_normalized);
        let is_doc = is_doc_file(name, rel_normalized);
        let is_entrypoint = self.entrypoints.contains(rel_normalized) || is_common_entrypoint(name);

        FileSignals {
            is_readme,
            is_contribution_doc: is_contribution_doc(rel_normalized, name),
            is_main_doc: is_important_doc(rel_normalized, name),
            is_vendored: is_vendored(&file.path),
            is_lock_file: is_lock_file(&file.path),
            is_generated: is_likely_generated(&file.path, content_sample),
            is_config,
            is_entrypoint,
            is_test: is_test_file(name, rel_lower),
            is_example: is_example_file(rel_lower),
            is_core_source: is_core_source(rel_lower),
            is_api_definition: is_api_definition(name),
            is_ci_workflow: is_ci_workflow(rel_lower),
            is_doc,
        }
    }

    fn score_signals(&self, s: FileSignals) -> f64 {
        if s.is_readme {
            self.weights.readme
        } else if s.is_contribution_doc {
            self.weights.contribution_doc
        } else if s.is_main_doc {
            self.weights.main_doc
        } else if s.is_vendored {
            self.weights.vendored
        } else if s.is_lock_file {
            self.weights.lock_file
        } else if s.is_generated {
            self.weights.generated
        } else if s.is_ci_workflow || s.is_config {
            self.weights.config
        } else if s.is_entrypoint {
            self.weights.entrypoint
        } else if s.is_test {
            self.weights.test
        } else if s.is_example {
            self.weights.example
        } else if s.is_core_source {
            self.weights.core_source
        } else if s.is_api_definition {
            self.weights.api_definition
        } else {
            self.weights.default
        }
    }

    /// Ranks all files in the provided slice.
    ///
    /// Modifies files in place with priority scores, then sorts by priority.
    ///
    /// # Arguments
    /// * `files` - Files to rank (modified and sorted in place)
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

    /// Returns entrypoints detected from manifest files.
    ///
    /// This is currently only used in tests but is part of the public API.
    #[cfg(test)]
    #[must_use]
    pub fn get_entrypoints(&self) -> &HashSet<String> {
        &self.entrypoints
    }

    /// Returns the detected languages from manifest parsing.
    ///
    /// This is currently only used in tests but is part of the public API.
    #[cfg(test)]
    #[allow(dead_code)]
    #[must_use]
    pub fn get_detected_languages(&self) -> &HashSet<String> {
        &self.detected_languages
    }

    /// Returns manifest information extracted during ranking.
    ///
    /// # Returns
    /// Map of manifest file names to their parsed content
    #[must_use]
    pub fn get_manifest_info(&self) -> &HashMap<String, JsonValue> {
        &self.manifest_info
    }

    /// Returns workspace members detected from Cargo.toml or similar.
    ///
    /// This is currently only used in tests but is part of the public API.
    #[cfg(test)]
    #[allow(dead_code)]
    #[must_use]
    pub fn get_workspace_members(&self) -> &[String] {
        &self.workspace_members
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

        if let Some(graph) = discover_workspace_graph(&self.root_path) {
            let mut members: Vec<String> = graph.member_roots.into_iter().collect();
            members.sort();
            for member in &members {
                self.entrypoint_candidates.insert(normalize_path(&format!("{member}/src/main.rs")));
                self.entrypoint_candidates.insert(normalize_path(&format!("{member}/src/lib.rs")));
            }
            self.workspace_members = members.clone();
            self.manifest_info.insert(
                "cargo_workspace_members".to_string(),
                JsonValue::Array(members.into_iter().map(JsonValue::String).collect()),
            );
            self.manifest_info.insert(
                "cargo_workspace_crates".to_string(),
                JsonValue::Array(
                    graph
                        .members
                        .into_iter()
                        .map(|member| JsonValue::String(member.name))
                        .collect(),
                ),
            );
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

fn is_contribution_doc(rel: &str, name: &str) -> bool {
    let rel_lower = rel.to_lowercase();
    let name_lower = name.to_lowercase();
    if rel_lower.starts_with(".github/pull_request_template")
        || rel_lower.starts_with(".github/issue_template/")
    {
        return true;
    }
    CONTRIBUTION_DOC_PREFIXES
        .iter()
        .any(|prefix| name_lower.starts_with(prefix) || rel_lower.contains(&format!("/{prefix}")))
}

fn is_ci_workflow(rel: &str) -> bool {
    rel.starts_with(".github/workflows/")
}

fn is_config_file(name: &str, rel: &str) -> bool {
    IMPORTANT_CONFIG_FILES.contains(&rel) || IMPORTANT_CONFIG_FILES.contains(&name)
}

fn is_api_definition(name: &str) -> bool {
    ["api", "interface", "types", "models", "schema"].iter().any(|needle| name.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::{FileRanker, JsonValue};
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
            "[project.scripts]\nr2p='repo_context.cli:main'\n",
        )
        .expect("write pyproject");
        fs::create_dir_all(tmp.path().join("repo_context")).expect("mkdir");
        fs::write(tmp.path().join("repo_context/cli.py"), "print('x')\n").expect("write cli");

        let scanned = HashSet::from(["repo_context/cli.py".to_string()]);
        let ranker = FileRanker::new(tmp.path(), scanned);

        assert!(ranker.get_entrypoints().contains("repo_context/cli.py"));
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

    #[test]
    fn contribution_doc_ranks_higher_than_config() {
        let tmp = TempDir::new().expect("tmp");
        let contributing_path = tmp.path().join("CONTRIBUTING.md");
        let cargo_path = tmp.path().join("Cargo.toml");
        fs::write(&contributing_path, "# Contributing\n").expect("write contributing");
        fs::write(&cargo_path, "[package]\nname='x'\nversion='0.1.0'\n").expect("write cargo");

        let scanned = HashSet::from(["CONTRIBUTING.md".to_string(), "Cargo.toml".to_string()]);
        let ranker = FileRanker::new(tmp.path(), scanned);

        let mut contributing = make_file(&contributing_path, "CONTRIBUTING.md", ".md", "markdown");
        let mut cargo = make_file(&cargo_path, "Cargo.toml", ".toml", "toml");
        ranker.rank_file(&mut contributing);
        ranker.rank_file(&mut cargo);

        assert!(contributing.priority > cargo.priority);
        assert!(contributing.tags.contains("contribution"));
    }

    #[test]
    fn workspace_members_add_member_entrypoints() {
        let tmp = TempDir::new().expect("tmp");
        fs::write(tmp.path().join("Cargo.toml"), "[workspace]\nmembers=[\"crates/*\"]\n")
            .expect("write root cargo");
        fs::create_dir_all(tmp.path().join("crates/a/src")).expect("mkdir a/src");
        fs::write(
            tmp.path().join("crates/a/Cargo.toml"),
            "[package]\nname=\"a\"\nversion=\"0.1.0\"\n",
        )
        .expect("write member cargo");
        fs::write(tmp.path().join("crates/a/src/main.rs"), "fn main() {}\n").expect("write main");

        let scanned = HashSet::from(["crates/a/src/main.rs".to_string()]);
        let ranker = FileRanker::new(tmp.path(), scanned);
        assert!(ranker.get_entrypoints().contains("crates/a/src/main.rs"));
        assert!(ranker
            .get_manifest_info()
            .get("cargo_workspace_members")
            .and_then(JsonValue::as_array)
            .is_some());
    }
}

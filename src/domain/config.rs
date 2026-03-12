#![allow(missing_docs)]

use crate::domain::{OutputMode, RankingWeights, RedactionConfig, RedactionMode};
use serde::{de, Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

/// Main configuration for repo-context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default, alias = "repo")]
    pub repo_url: Option<String>,
    #[serde(default, alias = "ref")]
    pub ref_: Option<String>,

    #[serde(
        default = "default_include_extensions",
        alias = "include_ext",
        deserialize_with = "deserialize_extensions"
    )]
    pub include_extensions: HashSet<String>,

    #[serde(
        default = "default_exclude_globs",
        alias = "exclude_glob",
        deserialize_with = "deserialize_globs"
    )]
    pub exclude_globs: HashSet<String>,

    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
    #[serde(default = "default_max_total_bytes")]
    pub max_total_bytes: u64,
    #[serde(default = "default_true")]
    pub respect_gitignore: bool,
    #[serde(default)]
    pub follow_symlinks: bool,
    #[serde(default = "default_true")]
    pub skip_minified: bool,

    pub max_tokens: Option<usize>,

    #[serde(default = "default_chunk_tokens")]
    pub chunk_tokens: usize,
    #[serde(default = "default_chunk_overlap")]
    pub chunk_overlap: usize,
    #[serde(default = "default_min_chunk_tokens")]
    pub min_chunk_tokens: usize,

    #[serde(default)]
    pub mode: OutputMode,
    #[serde(default = "default_output_dir")]
    pub output_dir: PathBuf,
    #[serde(default = "default_tree_depth")]
    pub tree_depth: usize,

    #[serde(default = "default_true")]
    pub redact_secrets: bool,
    #[serde(default)]
    pub redaction_mode: RedactionMode,

    #[serde(default, alias = "weights")]
    pub ranking_weights: RankingWeights,
    #[serde(default, alias = "redact")]
    pub redaction: RedactionConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            path: None,
            repo_url: None,
            ref_: None,
            include_extensions: default_include_extensions(),
            exclude_globs: default_exclude_globs(),
            max_file_bytes: default_max_file_bytes(),
            max_total_bytes: default_max_total_bytes(),
            respect_gitignore: true,
            follow_symlinks: false,
            skip_minified: true,
            max_tokens: None,
            chunk_tokens: default_chunk_tokens(),
            chunk_overlap: default_chunk_overlap(),
            min_chunk_tokens: default_min_chunk_tokens(),
            mode: OutputMode::Both,
            output_dir: default_output_dir(),
            tree_depth: default_tree_depth(),
            redact_secrets: true,
            redaction_mode: RedactionMode::Standard,
            ranking_weights: RankingWeights::default(),
            redaction: RedactionConfig::default(),
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_max_file_bytes() -> u64 {
    1_048_576
}
fn default_max_total_bytes() -> u64 {
    20_000_000
}
fn default_chunk_tokens() -> usize {
    800
}
fn default_chunk_overlap() -> usize {
    120
}
fn default_min_chunk_tokens() -> usize {
    200
}
fn default_output_dir() -> PathBuf {
    PathBuf::from("./out")
}
fn default_tree_depth() -> usize {
    4
}

/// Default file extensions to include in scanning.
pub fn default_include_extensions() -> HashSet<String> {
    [
        ".py",
        ".pyi",
        ".pyx",
        ".js",
        ".jsx",
        ".ts",
        ".tsx",
        ".mjs",
        ".cjs",
        ".go",
        ".java",
        ".kt",
        ".kts",
        ".rs",
        ".c",
        ".h",
        ".cpp",
        ".hpp",
        ".cc",
        ".cxx",
        ".cs",
        ".rb",
        ".php",
        ".swift",
        ".scala",
        ".sh",
        ".bash",
        ".zsh",
        ".md",
        ".rst",
        ".txt",
        ".adoc",
        ".yaml",
        ".yml",
        ".toml",
        ".json",
        ".ini",
        ".cfg",
        ".html",
        ".css",
        ".scss",
        ".less",
        ".vue",
        ".svelte",
        ".sql",
        ".dockerfile",
        ".graphql",
        ".proto",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Default glob patterns to exclude from scanning.
pub fn default_exclude_globs() -> HashSet<String> {
    [
        "dist/**",
        "build/**",
        "out/**",
        "target/**",
        "bin/**",
        "obj/**",
        "_build/**",
        "node_modules/**",
        ".venv/**",
        "venv/**",
        "vendor/**",
        "__pycache__/**",
        ".tox/**",
        ".nox/**",
        ".eggs/**",
        "*.egg-info/**",
        ".idea/**",
        ".vscode/**",
        ".vs/**",
        "*.swp",
        "*.swo",
        ".git/**",
        ".svn/**",
        ".hg/**",
        ".cache/**",
        ".pytest_cache/**",
        ".mypy_cache/**",
        ".ruff_cache/**",
        "*.pyc",
        "coverage/**",
        ".coverage",
        "htmlcov/**",
        ".DS_Store",
        "Thumbs.db",
        "*.min.js",
        "*.min.css",
        "*.bundle.js",
        "*.map",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn deserialize_extensions<'de, D>(deserializer: D) -> Result<HashSet<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct ExtensionsVisitor;

    impl<'de> de::Visitor<'de> for ExtensionsVisitor {
        type Value = HashSet<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string, array, or set of extensions")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            let mut result = HashSet::new();
            for ext in value.split(',') {
                let trimmed = ext.trim();
                if !trimmed.is_empty() {
                    let normalized = if trimmed.starts_with('.') {
                        trimmed.to_string()
                    } else {
                        format!(".{}", trimmed)
                    };
                    result.insert(normalized);
                }
            }
            Ok(result)
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: de::SeqAccess<'de>,
        {
            let mut result = HashSet::new();
            while let Some(ext) = seq.next_element::<String>()? {
                let trimmed = ext.trim();
                if !trimmed.is_empty() {
                    let normalized = if trimmed.starts_with('.') {
                        trimmed.to_string()
                    } else {
                        format!(".{}", trimmed)
                    };
                    result.insert(normalized);
                }
            }
            Ok(result)
        }
    }

    deserializer.deserialize_any(ExtensionsVisitor)
}

fn deserialize_globs<'de, D>(deserializer: D) -> Result<HashSet<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct GlobsVisitor;

    impl<'de> de::Visitor<'de> for GlobsVisitor {
        type Value = HashSet<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string, array, or set of globs")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(value
                .split(',')
                .map(str::trim)
                .filter(|glob| !glob.is_empty())
                .map(|glob| glob.to_string())
                .collect())
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: de::SeqAccess<'de>,
        {
            let mut result = HashSet::new();
            while let Some(glob) = seq.next_element::<String>()? {
                let trimmed = glob.trim();
                if !trimmed.is_empty() {
                    result.insert(trimmed.to_string());
                }
            }
            Ok(result)
        }
    }

    deserializer.deserialize_any(GlobsVisitor)
}

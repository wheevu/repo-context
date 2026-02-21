//! Core domain types and models
//!
//! Equivalent to Python's config.py - defines FileInfo, Chunk, Config, etc.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

/// Current report schema version (matches Python implementation)
pub const REPORT_SCHEMA_VERSION: &str = "1.0.0";

/// Output mode for the tool
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    Prompt,
    Rag,
    Contribution,
    #[serde(rename = "pr-context")]
    PrContext,
    #[default]
    Both,
}

/// Redaction mode controls aggressiveness and syntax safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RedactionMode {
    Fast,
    #[default]
    Standard,
    Paranoid,
    StructureSafe,
}

/// Information about a scanned file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    /// Absolute path to the file
    pub path: PathBuf,

    /// Path relative to repository root
    pub relative_path: String,

    /// File size in bytes
    pub size_bytes: u64,

    /// File extension (with leading dot)
    pub extension: String,

    /// Detected programming language
    pub language: String,

    /// Unique content-based ID
    pub id: String,

    /// Priority score (0.0 to 1.0, higher = more important)
    #[serde(default)]
    pub priority: f64,

    /// Estimated tokens in file
    #[serde(default)]
    pub token_estimate: usize,

    /// Classification tags
    #[serde(default)]
    pub tags: BTreeSet<String>,

    /// Whether this is a README file
    #[serde(default)]
    pub is_readme: bool,

    /// Whether this is a configuration file
    #[serde(default)]
    pub is_config: bool,

    /// Whether this is documentation
    #[serde(default)]
    pub is_doc: bool,
}

/// A chunk of file content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    /// Unique stable ID for this chunk
    pub id: String,

    /// Relative path to source file
    pub path: String,

    /// Programming language
    pub language: String,

    /// Starting line number (1-indexed)
    pub start_line: usize,

    /// Ending line number (inclusive)
    pub end_line: usize,

    /// Chunk content
    pub content: String,

    /// Priority score from parent file
    #[serde(default)]
    pub priority: f64,

    /// Classification tags
    #[serde(default)]
    pub tags: BTreeSet<String>,

    /// Estimated tokens in chunk
    #[serde(default)]
    pub token_estimate: usize,
}

/// Statistics from scanning and processing
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanStats {
    /// Total files scanned (including filtered)
    pub files_scanned: usize,

    /// Files included in output
    pub files_included: usize,

    /// Files skipped due to size limit
    #[serde(default)]
    pub files_skipped_size: usize,

    /// Files skipped due to binary detection
    #[serde(default)]
    pub files_skipped_binary: usize,

    /// Files skipped due to extension filtering
    #[serde(default)]
    pub files_skipped_extension: usize,

    /// Files skipped due to gitignore
    #[serde(default)]
    pub files_skipped_gitignore: usize,

    /// Files skipped due to exclude globs / minified heuristics
    #[serde(default)]
    pub files_skipped_glob: usize,

    /// Files skipped due to filters (legacy, kept for compatibility)
    #[serde(default)]
    pub files_skipped: usize,

    /// Files dropped due to budget limits
    pub files_dropped_budget: usize,

    /// Total bytes scanned
    pub total_bytes_scanned: u64,

    /// Total bytes included in output
    pub total_bytes_included: u64,

    /// Chunks created
    pub chunks_created: usize,

    /// Estimated total tokens in output
    pub total_tokens_estimated: usize,

    /// Language distribution (language -> count)
    #[serde(default)]
    pub languages_detected: HashMap<String, usize>,

    /// Top ignored patterns from gitignore (pattern -> count)
    #[serde(default)]
    pub top_ignored_patterns: HashMap<String, usize>,

    /// Processing time in seconds
    #[serde(default)]
    pub processing_time_seconds: f64,

    /// Top-ranked files for reporting
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_ranked_files: Vec<HashMap<String, serde_json::Value>>,

    /// Files dropped (with reason)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped_files: Vec<HashMap<String, serde_json::Value>>,

    /// Redaction counts by rule name
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub redaction_counts: BTreeMap<String, usize>,

    /// Number of chunks that were modified by redaction
    #[serde(default)]
    pub redacted_chunks: usize,

    /// Number of files that had at least one redacted chunk
    #[serde(default)]
    pub redacted_files: usize,

    /// Rule -> number of chunks affected by the rule
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub redaction_chunk_counts: BTreeMap<String, usize>,

    /// Rule -> number of files affected by the rule
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub redaction_file_counts: BTreeMap<String, usize>,

    /// Number of chunks added by thread stitching.
    #[serde(default)]
    pub stitched_chunks: usize,

    /// Whether export ran in pinned-only fallback mode.
    #[serde(default)]
    pub pinned_only_mode: bool,

    /// Tokens by which protected pins exceeded max_tokens.
    #[serde(default)]
    pub pinned_overflow_tokens: usize,

    /// Protected pin files selected for contribution/pr-context packs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_files: Vec<HashMap<String, serde_json::Value>>,
}

impl ScanStats {
    /// Produce a JSON value matching Python's report schema.
    ///
    /// Python nests the per-category skip counts under a `"files_skipped"` object
    /// (config.py `ScanStats.to_dict()`). This method replicates that shape so
    /// `report.json` is compatible with Python consumers.
    pub fn to_report_value(&self) -> serde_json::Value {
        // languages_detected: sorted by (-count, name)
        let mut langs: Vec<(&String, &usize)> = self.languages_detected.iter().collect();
        langs.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let languages_detected: serde_json::Map<String, serde_json::Value> =
            langs.into_iter().map(|(k, v)| (k.clone(), serde_json::json!(v))).collect();

        // top_ignored_patterns: sorted by (-count, name), top 10
        let mut patterns: Vec<(&String, &usize)> = self.top_ignored_patterns.iter().collect();
        patterns.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let top_ignored_patterns: serde_json::Map<String, serde_json::Value> =
            patterns.into_iter().take(10).map(|(k, v)| (k.clone(), serde_json::json!(v))).collect();

        let mut value = serde_json::json!({
            "files_scanned":  self.files_scanned,
            "files_included": self.files_included,
            "files_skipped": {
                "binary":    self.files_skipped_binary,
                "extension": self.files_skipped_extension,
                "gitignore": self.files_skipped_gitignore,
                "glob":      self.files_skipped_glob,
                "size":      self.files_skipped_size,
            },
            "files_dropped_budget":    self.files_dropped_budget,
            "total_bytes_scanned":     self.total_bytes_scanned,
            "total_bytes_included":    self.total_bytes_included,
            "chunks_created":          self.chunks_created,
            "total_tokens_estimated":  self.total_tokens_estimated,
            "languages_detected":      languages_detected,
            "top_ignored_patterns":    top_ignored_patterns,
            "redaction_counts":        self.redaction_counts,
            "stitched_chunks":         self.stitched_chunks,
            "pinned_only_mode":        self.pinned_only_mode,
            "pinned_overflow_tokens":  self.pinned_overflow_tokens,
            "processing_time_seconds": self.processing_time_seconds,
        });

        // Only emit redacted_files and redacted_chunks when non-zero,
        // matching Python which omits these keys when redaction is not active.
        if self.redacted_files > 0 {
            value["redacted_files"] = serde_json::json!(self.redacted_files);
        }
        if self.redacted_chunks > 0 {
            value["redacted_chunks"] = serde_json::json!(self.redacted_chunks);
        }

        value
    }
}

/// Redaction configuration — mirrors Python's `RedactionConfig`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactionConfig {
    /// File/path glob patterns to skip redaction entirely (allowlist)
    #[serde(default)]
    pub allowlist_patterns: Vec<String>,

    /// Specific literal strings to never redact (false-positive allowlist)
    #[serde(default)]
    pub allowlist_strings: Vec<String>,

    /// Custom regex rules: each entry has `name`, `pattern`, optional `replacement`
    #[serde(default)]
    pub custom_rules: Vec<CustomRedactionRule>,

    /// Entropy detection sub-config
    #[serde(default)]
    pub entropy: EntropyConfig,

    /// Paranoid mode sub-config
    #[serde(default)]
    pub paranoid: ParanoidConfig,

    /// File patterns considered "safe" — skip paranoid mode for these
    #[serde(default = "default_safe_file_patterns")]
    pub safe_file_patterns: Vec<String>,

    /// Source file patterns — use structure-safe redaction for these
    #[serde(default = "default_source_safe_patterns")]
    pub source_safe_patterns: Vec<String>,

    /// Enable structure-safe redaction for source files (default: true)
    #[serde(default = "default_true_redaction")]
    pub structure_safe_redaction: bool,
}

/// One custom redaction rule from the config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomRedactionRule {
    pub name: Option<String>,
    pub pattern: String,
    #[serde(default = "default_custom_replacement")]
    pub replacement: String,
}

/// Entropy detection settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntropyConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_entropy_threshold")]
    pub threshold: f64,
    #[serde(default = "default_entropy_min_length")]
    pub min_length: usize,
}

/// Paranoid mode settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParanoidConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_paranoid_min_length")]
    pub min_length: usize,
}

impl Default for RedactionConfig {
    fn default() -> Self {
        Self {
            allowlist_patterns: Vec::new(),
            allowlist_strings: Vec::new(),
            custom_rules: Vec::new(),
            entropy: EntropyConfig::default(),
            paranoid: ParanoidConfig::default(),
            safe_file_patterns: default_safe_file_patterns(),
            source_safe_patterns: default_source_safe_patterns(),
            structure_safe_redaction: true,
        }
    }
}

impl Default for EntropyConfig {
    fn default() -> Self {
        Self { enabled: false, threshold: 4.5, min_length: 20 }
    }
}

impl Default for ParanoidConfig {
    fn default() -> Self {
        Self { enabled: false, min_length: 32 }
    }
}

fn default_true_redaction() -> bool {
    true
}
fn default_custom_replacement() -> String {
    "[CUSTOM_REDACTED]".to_string()
}
fn default_entropy_threshold() -> f64 {
    4.5
}
fn default_entropy_min_length() -> usize {
    20
}
fn default_paranoid_min_length() -> usize {
    32
}
fn default_safe_file_patterns() -> Vec<String> {
    vec![
        "*.md".into(),
        "*.rst".into(),
        "*.txt".into(),
        "*.json".into(),
        "*.lock".into(),
        "*.sum".into(),
        "go.sum".into(),
        "package-lock.json".into(),
        "yarn.lock".into(),
        "poetry.lock".into(),
        "Cargo.lock".into(),
    ]
}
fn default_source_safe_patterns() -> Vec<String> {
    vec![
        "*.py".into(),
        "*.pyi".into(),
        "*.js".into(),
        "*.jsx".into(),
        "*.ts".into(),
        "*.tsx".into(),
        "*.go".into(),
        "*.rs".into(),
        "*.java".into(),
        "*.kt".into(),
        "*.c".into(),
        "*.cpp".into(),
        "*.h".into(),
        "*.hpp".into(),
        "*.cs".into(),
        "*.rb".into(),
        "*.php".into(),
        "*.swift".into(),
        "*.scala".into(),
        "*.sh".into(),
        "*.bash".into(),
        "*.zsh".into(),
    ]
}

/// Configurable weights for file ranking — mirrors Python's RankingWeights.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RankingWeights {
    #[serde(default = "w_readme")]
    pub readme: f64,
    #[serde(default = "w_contribution_doc")]
    pub contribution_doc: f64,
    #[serde(default = "w_main_doc")]
    pub main_doc: f64,
    #[serde(default = "w_config")]
    pub config: f64,
    #[serde(default = "w_entrypoint")]
    pub entrypoint: f64,
    #[serde(default = "w_api_definition")]
    pub api_definition: f64,
    #[serde(default = "w_core_source")]
    pub core_source: f64,
    #[serde(default = "w_example")]
    pub example: f64,
    #[serde(default = "w_test")]
    pub test: f64,
    #[serde(default = "w_default")]
    pub default: f64,
    #[serde(default = "w_generated")]
    pub generated: f64,
    #[serde(default = "w_lock_file")]
    pub lock_file: f64,
    #[serde(default = "w_vendored")]
    pub vendored: f64,
}

impl Default for RankingWeights {
    fn default() -> Self {
        Self {
            readme: w_readme(),
            contribution_doc: w_contribution_doc(),
            main_doc: w_main_doc(),
            config: w_config(),
            entrypoint: w_entrypoint(),
            api_definition: w_api_definition(),
            core_source: w_core_source(),
            example: w_example(),
            test: w_test(),
            default: w_default(),
            generated: w_generated(),
            lock_file: w_lock_file(),
            vendored: w_vendored(),
        }
    }
}

fn w_readme() -> f64 {
    1.00
}
fn w_contribution_doc() -> f64 {
    0.98
}
fn w_main_doc() -> f64 {
    0.95
}
fn w_config() -> f64 {
    0.90
}
fn w_entrypoint() -> f64 {
    0.85
}
fn w_api_definition() -> f64 {
    0.80
}
fn w_core_source() -> f64 {
    0.75
}
fn w_example() -> f64 {
    0.60
}
fn w_test() -> f64 {
    0.50
}
fn w_default() -> f64 {
    0.50
}
fn w_generated() -> f64 {
    0.20
}
fn w_lock_file() -> f64 {
    0.15
}
fn w_vendored() -> f64 {
    0.10
}

/// Custom deserializer for extensions: normalizes to dot-prefixed format.
///
/// Matches Python's _normalize_extensions (lines 303-329):
/// - Accepts string (comma-separated), array, or set
/// - Adds leading dot if missing
/// - Strips whitespace
fn deserialize_extensions<'de, D>(deserializer: D) -> Result<HashSet<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    use std::fmt;

    struct ExtensionsVisitor;

    impl<'de> Visitor<'de> for ExtensionsVisitor {
        type Value = HashSet<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a string, array, or set of extensions")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            // Comma-separated string (Python line 316)
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

/// Custom deserializer for globs: accepts string (comma-separated), array, or set.
///
/// Matches Python's _normalize_globs (lines 332-353).
fn deserialize_globs<'de, D>(deserializer: D) -> Result<HashSet<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    use std::fmt;

    struct GlobsVisitor;

    impl<'de> Visitor<'de> for GlobsVisitor {
        type Value = HashSet<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a string, array, or set of glob patterns")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            // Comma-separated string
            let mut result = HashSet::new();
            for glob in value.split(',') {
                let trimmed = glob.trim();
                if !trimmed.is_empty() {
                    result.insert(trimmed.to_string());
                }
            }
            Ok(result)
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

/// Main configuration for repo-context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    // Input source
    #[serde(default)]
    pub path: Option<PathBuf>,

    #[serde(default, alias = "repo")]
    pub repo_url: Option<String>,

    #[serde(default, alias = "ref")]
    pub ref_: Option<String>,

    // Filtering options
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

    // Token budget
    pub max_tokens: Option<usize>,

    /// Optional task description used for retrieval-driven reranking.
    #[serde(default)]
    pub task_query: Option<String>,

    /// Enable second-stage semantic reranking over top-K chunks.
    #[serde(default = "default_true")]
    pub semantic_rerank: bool,

    /// Number of top-ranked chunks to run through semantic reranking.
    #[serde(default = "default_rerank_top_k")]
    pub rerank_top_k: usize,

    /// Semantic model identifier for retrieval.
    #[serde(default)]
    pub semantic_model: Option<String>,

    /// Fraction of token budget reserved for stitched context.
    #[serde(default = "default_stitch_budget_fraction")]
    pub stitch_budget_fraction: f64,

    /// Number of top chunks used as stitching seeds.
    #[serde(default = "default_stitch_top_n")]
    pub stitch_top_n: usize,

    // Chunking options
    #[serde(default = "default_chunk_tokens")]
    pub chunk_tokens: usize,

    #[serde(default = "default_chunk_overlap")]
    pub chunk_overlap: usize,

    #[serde(default = "default_min_chunk_tokens")]
    pub min_chunk_tokens: usize,

    // Output options
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

    /// Glob patterns that should always be included even when token budget is exceeded.
    #[serde(default, alias = "always_include_globs")]
    pub always_include_patterns: Vec<String>,

    /// Specific repository-relative paths that are always included.
    #[serde(default)]
    pub always_include_paths: Vec<String>,

    /// Keywords used by invariant discovery for contribution/pr-context exports.
    #[serde(default = "default_invariant_keywords")]
    pub invariant_keywords: Vec<String>,

    /// Custom ranking weights (all fields optional; defaults match Python)
    #[serde(default, alias = "weights")]
    pub ranking_weights: RankingWeights,

    /// Redaction configuration loaded from [redaction] section
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
            task_query: None,
            semantic_rerank: true,
            rerank_top_k: default_rerank_top_k(),
            semantic_model: None,
            stitch_budget_fraction: default_stitch_budget_fraction(),
            stitch_top_n: default_stitch_top_n(),
            chunk_tokens: default_chunk_tokens(),
            chunk_overlap: default_chunk_overlap(),
            min_chunk_tokens: default_min_chunk_tokens(),
            mode: OutputMode::Both,
            output_dir: default_output_dir(),
            tree_depth: default_tree_depth(),
            redact_secrets: true,
            redaction_mode: RedactionMode::Standard,
            always_include_patterns: Vec::new(),
            always_include_paths: Vec::new(),
            invariant_keywords: default_invariant_keywords(),
            ranking_weights: RankingWeights::default(),
            redaction: RedactionConfig::default(),
        }
    }
}

// Default value functions for serde
fn default_true() -> bool {
    true
}

fn default_max_file_bytes() -> u64 {
    1_048_576 // 1 MB
}

fn default_max_total_bytes() -> u64 {
    20_000_000 // 20 MB
}

fn default_chunk_tokens() -> usize {
    800
}

fn default_rerank_top_k() -> usize {
    200
}

fn default_stitch_budget_fraction() -> f64 {
    0.30
}

fn default_stitch_top_n() -> usize {
    20
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

fn default_invariant_keywords() -> Vec<String> {
    [
        "must",
        "invariant",
        "safety",
        "threading",
        "ownership",
        "locking",
        "abi",
        "contract",
        "compatibility",
        "schema",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

pub fn default_include_extensions() -> HashSet<String> {
    [
        // Python
        ".py",
        ".pyi",
        ".pyx",
        // JavaScript/TypeScript
        ".js",
        ".jsx",
        ".ts",
        ".tsx",
        ".mjs",
        ".cjs",
        // Go
        ".go",
        // Java/Kotlin
        ".java",
        ".kt",
        ".kts",
        // Rust
        ".rs",
        // C/C++
        ".c",
        ".h",
        ".cpp",
        ".hpp",
        ".cc",
        ".cxx",
        // C#
        ".cs",
        // Ruby
        ".rb",
        // PHP
        ".php",
        // Swift
        ".swift",
        // Scala
        ".scala",
        // Shell
        ".sh",
        ".bash",
        ".zsh",
        // Documentation
        ".md",
        ".rst",
        ".txt",
        ".adoc",
        // Config
        ".yaml",
        ".yml",
        ".toml",
        ".json",
        ".ini",
        ".cfg",
        // Web
        ".html",
        ".css",
        ".scss",
        ".less",
        ".vue",
        ".svelte",
        // SQL
        ".sql",
        // Misc
        ".dockerfile",
        ".graphql",
        ".proto",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

pub fn default_exclude_globs() -> HashSet<String> {
    [
        // Build outputs
        "dist/**",
        "build/**",
        "out/**",
        "target/**",
        "bin/**",
        "obj/**",
        "_build/**",
        // Dependencies
        "node_modules/**",
        ".venv/**",
        "venv/**",
        "vendor/**",
        "__pycache__/**",
        ".tox/**",
        ".nox/**",
        ".eggs/**",
        "*.egg-info/**",
        // IDE/Editor
        ".idea/**",
        ".vscode/**",
        ".vs/**",
        "*.swp",
        "*.swo",
        // Version control
        ".git/**",
        ".svn/**",
        ".hg/**",
        // Cache
        ".cache/**",
        ".pytest_cache/**",
        ".mypy_cache/**",
        ".ruff_cache/**",
        "*.pyc",
        // Coverage
        "coverage/**",
        ".coverage",
        "htmlcov/**",
        // Misc
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

/// Get language from file extension or special filename.
pub fn get_language(extension: &str, filename: &str) -> String {
    let ext = extension.to_lowercase();
    let lang = match ext.as_str() {
        ".py" | ".pyi" | ".pyx" => "python",
        ".js" | ".jsx" | ".mjs" | ".cjs" => "javascript",
        ".ts" | ".tsx" => "typescript",
        ".go" => "go",
        ".java" => "java",
        ".kt" | ".kts" => "kotlin",
        ".rs" => "rust",
        ".c" | ".h" => "c",
        ".cpp" | ".hpp" | ".cc" | ".cxx" => "cpp",
        ".cs" => "csharp",
        ".rb" => "ruby",
        ".php" => "php",
        ".swift" => "swift",
        ".scala" => "scala",
        ".sh" | ".bash" => "bash",
        ".zsh" => "zsh",
        ".md" => "markdown",
        ".rst" => "restructuredtext",
        ".adoc" => "asciidoc",
        ".txt" => "text",
        ".yaml" | ".yml" => "yaml",
        ".toml" => "toml",
        ".json" => "json",
        ".ini" | ".cfg" => "ini",
        ".html" => "html",
        ".css" => "css",
        ".scss" => "scss",
        ".less" => "less",
        ".vue" => "vue",
        ".svelte" => "svelte",
        ".sql" => "sql",
        ".dockerfile" => "dockerfile",
        ".graphql" => "graphql",
        ".proto" => "protobuf",
        _ => {
            // Special filenames (no extension or empty extension)
            let name = filename.to_lowercase();
            if name == "dockerfile" {
                return "dockerfile".to_string();
            }
            if name == "makefile" {
                return "makefile".to_string();
            }
            if name == "rakefile" {
                return "ruby".to_string();
            }
            if ext.is_empty() && name.ends_with("rc") {
                return "shell".to_string();
            }
            "text"
        }
    };
    lang.to_string()
}

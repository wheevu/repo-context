//! CLI argument merging with config

use crate::domain::Config;
use std::collections::HashSet;
use std::path::PathBuf;

/// CLI-provided overrides for configuration values.
///
/// All fields are optional, allowing selective overrides of config file settings.
#[derive(Debug, Default, Clone)]
pub struct CliOverrides {
    /// Local path to repository
    pub path: Option<PathBuf>,
    /// Remote repository URL
    pub repo_url: Option<String>,
    /// Git reference (branch, tag, commit)
    pub ref_: Option<String>,
    /// File extensions to include
    pub include_extensions: Option<HashSet<String>>,
    /// Glob patterns to exclude
    pub exclude_globs: Option<HashSet<String>>,
    /// Maximum file size in bytes
    pub max_file_bytes: Option<u64>,
    /// Maximum total size in bytes
    pub max_total_bytes: Option<u64>,
    /// Whether to respect .gitignore
    pub respect_gitignore: Option<bool>,
    /// Whether to follow symlinks
    pub follow_symlinks: Option<bool>,
    /// Whether to skip minified files
    pub skip_minified: Option<bool>,
    /// Maximum tokens in output
    pub max_tokens: Option<usize>,
    /// Task query for reranking
    pub task_query: Option<String>,
    /// Whether to enable semantic reranking
    pub semantic_rerank: Option<bool>,
    /// Number of top chunks for reranking
    pub rerank_top_k: Option<usize>,
    /// Semantic model identifier
    pub semantic_model: Option<String>,
    /// Fraction of budget for stitching
    pub stitch_budget_fraction: Option<f64>,
    /// Number of seed chunks for stitching
    pub stitch_top_n: Option<usize>,
    /// Chunk size in tokens
    pub chunk_tokens: Option<usize>,
    /// Chunk overlap in tokens
    pub chunk_overlap: Option<usize>,
    /// Minimum chunk size in tokens
    pub min_chunk_tokens: Option<usize>,
    /// Output mode (prompt, rag, both)
    pub mode: Option<crate::domain::OutputMode>,
    /// Output directory path
    pub output_dir: Option<PathBuf>,
    /// Directory tree depth
    pub tree_depth: Option<usize>,
    /// Whether to redact secrets
    pub redact_secrets: Option<bool>,
    /// Redaction mode
    pub redaction_mode: Option<crate::domain::RedactionMode>,
    /// Patterns to always include
    pub always_include_patterns: Option<Vec<String>>,
    /// Paths to always include
    pub always_include_paths: Option<Vec<String>>,
    /// Keywords for invariant detection
    pub invariant_keywords: Option<Vec<String>>,
}

/// Merges CLI overrides into a base configuration.
///
/// CLI values take precedence over config file values. The base_config
/// is modified in place and returned.
///
/// # Arguments
/// * `base_config` - The configuration loaded from file or defaults
/// * `cli` - CLI-provided overrides
///
/// # Returns
/// The merged configuration
pub fn merge_cli_with_config(mut base_config: Config, cli: CliOverrides) -> Config {
    if let Some(path) = cli.path {
        base_config.path = Some(path);
        base_config.repo_url = None;
    }
    if let Some(repo_url) = cli.repo_url {
        base_config.repo_url = Some(repo_url);
        base_config.path = None;
    }
    if let Some(ref_) = cli.ref_ {
        base_config.ref_ = Some(ref_);
    }

    if let Some(include_extensions) = cli.include_extensions {
        base_config.include_extensions = include_extensions;
    }
    if let Some(exclude_globs) = cli.exclude_globs {
        base_config.exclude_globs = exclude_globs;
    }

    if let Some(max_file_bytes) = cli.max_file_bytes {
        base_config.max_file_bytes = max_file_bytes;
    }
    if let Some(max_total_bytes) = cli.max_total_bytes {
        base_config.max_total_bytes = max_total_bytes;
    }
    if let Some(respect_gitignore) = cli.respect_gitignore {
        base_config.respect_gitignore = respect_gitignore;
    }
    if let Some(follow_symlinks) = cli.follow_symlinks {
        base_config.follow_symlinks = follow_symlinks;
    }
    if let Some(skip_minified) = cli.skip_minified {
        base_config.skip_minified = skip_minified;
    }

    if let Some(max_tokens) = cli.max_tokens {
        base_config.max_tokens = Some(max_tokens);
    }
    if let Some(task_query) = cli.task_query {
        base_config.task_query = Some(task_query);
    }
    if let Some(semantic_rerank) = cli.semantic_rerank {
        base_config.semantic_rerank = semantic_rerank;
    }
    if let Some(rerank_top_k) = cli.rerank_top_k {
        base_config.rerank_top_k = rerank_top_k;
    }
    if let Some(semantic_model) = cli.semantic_model {
        base_config.semantic_model = Some(semantic_model);
    }
    if let Some(stitch_budget_fraction) = cli.stitch_budget_fraction {
        base_config.stitch_budget_fraction = stitch_budget_fraction;
    }
    if let Some(stitch_top_n) = cli.stitch_top_n {
        base_config.stitch_top_n = stitch_top_n;
    }
    if let Some(chunk_tokens) = cli.chunk_tokens {
        base_config.chunk_tokens = chunk_tokens;
    }
    if let Some(chunk_overlap) = cli.chunk_overlap {
        base_config.chunk_overlap = chunk_overlap;
    }
    if let Some(min_chunk_tokens) = cli.min_chunk_tokens {
        base_config.min_chunk_tokens = min_chunk_tokens;
    }

    if let Some(mode) = cli.mode {
        base_config.mode = mode;
    }
    if let Some(output_dir) = cli.output_dir {
        base_config.output_dir = output_dir;
    }
    if let Some(tree_depth) = cli.tree_depth {
        base_config.tree_depth = tree_depth;
    }
    if let Some(redact_secrets) = cli.redact_secrets {
        base_config.redact_secrets = redact_secrets;
    }
    if let Some(redaction_mode) = cli.redaction_mode {
        base_config.redaction_mode = redaction_mode;
    }
    if let Some(always_include_patterns) = cli.always_include_patterns {
        base_config.always_include_patterns = always_include_patterns;
    }
    if let Some(always_include_paths) = cli.always_include_paths {
        base_config.always_include_paths = always_include_paths;
    }
    if let Some(invariant_keywords) = cli.invariant_keywords {
        base_config.invariant_keywords = invariant_keywords;
    }

    base_config
}

#[cfg(test)]
mod tests {
    use super::{merge_cli_with_config, CliOverrides};
    use crate::domain::{Config, OutputMode, RedactionMode};
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn cli_overrides_replace_base_values() {
        let base = Config {
            path: Some(PathBuf::from("/tmp/repo")),
            mode: OutputMode::Prompt,
            max_file_bytes: 100,
            ..Config::default()
        };

        let cli = CliOverrides {
            repo_url: Some("https://github.com/org/repo".to_string()),
            mode: Some(OutputMode::Both),
            max_file_bytes: Some(2048),
            include_extensions: Some(HashSet::from([".rs".to_string()])),
            redaction_mode: Some(RedactionMode::Paranoid),
            ..CliOverrides::default()
        };

        let merged = merge_cli_with_config(base, cli);
        assert!(merged.path.is_none());
        assert_eq!(merged.repo_url.as_deref(), Some("https://github.com/org/repo"));
        assert_eq!(merged.mode, OutputMode::Both);
        assert_eq!(merged.redaction_mode, RedactionMode::Paranoid);
        assert_eq!(merged.max_file_bytes, 2048);
        assert!(merged.include_extensions.contains(".rs"));
    }
}

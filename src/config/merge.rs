#![allow(missing_docs)]

//! CLI argument merging with config.

use crate::config::loader::load_config;
use crate::domain::{Config, OutputMode, RedactionMode};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// CLI-provided overrides for configuration values.
#[derive(Debug, Default, Clone)]
pub struct CliOverrides {
    pub path: Option<PathBuf>,
    pub repo_url: Option<String>,
    pub ref_: Option<String>,
    pub include_extensions: Option<HashSet<String>>,
    pub exclude_globs: Option<HashSet<String>>,
    pub max_file_bytes: Option<u64>,
    pub max_total_bytes: Option<u64>,
    pub respect_gitignore: Option<bool>,
    pub follow_symlinks: Option<bool>,
    pub skip_minified: Option<bool>,
    pub max_tokens: Option<usize>,
    pub chunk_tokens: Option<usize>,
    pub chunk_overlap: Option<usize>,
    pub min_chunk_tokens: Option<usize>,
    pub mode: Option<OutputMode>,
    pub output_dir: Option<PathBuf>,
    pub tree_depth: Option<usize>,
    pub redact_secrets: Option<bool>,
    pub redaction_mode: Option<RedactionMode>,
}

/// Merges CLI overrides into a base configuration.
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

    base_config
}

/// Load config from a repo root and merge it into the given `config`,
/// but only for fields that are still at their default values.  This
/// preserves any values explicitly set via CLI flags or the caller's
/// own config file.
pub fn merge_repo_config(
    config: &mut Config,
    repo_root: &Path,
    explicit_config_path: Option<&Path>,
) {
    let Ok(repo_config) = load_config(repo_root, explicit_config_path) else {
        return;
    };

    let defaults = Config::default();

    // Only apply repo values when the current value matches the default.
    if config.include_extensions == defaults.include_extensions {
        config.include_extensions = repo_config.include_extensions;
    }
    if config.exclude_globs == defaults.exclude_globs {
        config.exclude_globs = repo_config.exclude_globs;
    }
    if config.max_file_bytes == defaults.max_file_bytes {
        config.max_file_bytes = repo_config.max_file_bytes;
    }
    if config.max_total_bytes == defaults.max_total_bytes {
        config.max_total_bytes = repo_config.max_total_bytes;
    }
    if config.respect_gitignore == defaults.respect_gitignore
        && repo_config.respect_gitignore != defaults.respect_gitignore
    {
        config.respect_gitignore = repo_config.respect_gitignore;
    }
    if config.follow_symlinks == defaults.follow_symlinks
        && repo_config.follow_symlinks != defaults.follow_symlinks
    {
        config.follow_symlinks = repo_config.follow_symlinks;
    }
    if config.skip_minified == defaults.skip_minified
        && repo_config.skip_minified != defaults.skip_minified
    {
        config.skip_minified = repo_config.skip_minified;
    }
    if config.max_tokens.is_none() && repo_config.max_tokens.is_some() {
        config.max_tokens = repo_config.max_tokens;
    }
    if config.chunk_tokens == defaults.chunk_tokens {
        config.chunk_tokens = repo_config.chunk_tokens;
    }
    if config.chunk_overlap == defaults.chunk_overlap {
        config.chunk_overlap = repo_config.chunk_overlap;
    }
    if config.min_chunk_tokens == defaults.min_chunk_tokens {
        config.min_chunk_tokens = repo_config.min_chunk_tokens;
    }
    if config.mode == defaults.mode {
        config.mode = repo_config.mode;
    }
    if config.output_dir == defaults.output_dir {
        config.output_dir = repo_config.output_dir;
    }
    if config.tree_depth == defaults.tree_depth {
        config.tree_depth = repo_config.tree_depth;
    }
    if config.redact_secrets == defaults.redact_secrets
        && repo_config.redact_secrets != defaults.redact_secrets
    {
        config.redact_secrets = repo_config.redact_secrets;
    }
    if config.redaction_mode == defaults.redaction_mode {
        config.redaction_mode = repo_config.redaction_mode;
    }
    if config.ranking_weights.readme == defaults.ranking_weights.readme {
        config.ranking_weights = repo_config.ranking_weights;
    }
    if config.redaction == crate::domain::RedactionConfig::default() {
        config.redaction = repo_config.redaction;
    }
    if config.module.module_roots.is_empty() {
        config.module.module_roots = repo_config.module.module_roots;
    }
    if config.module.css_files.is_empty() {
        config.module.css_files = repo_config.module.css_files;
    }
    if config.full_inventory == defaults.full_inventory {
        config.full_inventory = repo_config.full_inventory;
    }
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

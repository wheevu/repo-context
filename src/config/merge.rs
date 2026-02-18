//! CLI argument merging with config

use crate::domain::Config;
use std::collections::HashSet;
use std::path::PathBuf;

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
    pub mode: Option<crate::domain::OutputMode>,
    pub output_dir: Option<PathBuf>,
    pub tree_depth: Option<usize>,
    pub redact_secrets: Option<bool>,
    pub redaction_mode: Option<crate::domain::RedactionMode>,
}

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

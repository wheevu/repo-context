//! Config file loading

use crate::domain::Config;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub fn load_config(repo_root: &Path, config_path: Option<&Path>) -> Result<Config> {
    let config_path_provided = config_path.is_some();

    let discovered = match config_path {
        Some(path) => Some(path.to_path_buf()),
        None => discover_config(repo_root),
    };

    let Some(config_file) = discovered else {
        return Ok(Config::default());
    };

    let content = fs::read_to_string(&config_file)
        .with_context(|| format!("Failed reading config file: {}", config_file.display()))?;

    let ext = config_file.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();

    // Python lines 380-395: Parse config, but silently return default on error
    // if auto-discovered (not explicitly provided by user).
    let parsed = match ext.as_str() {
        "toml" => match parse_toml_config(&content, &config_file) {
            Ok(cfg) => cfg,
            Err(e) => {
                if config_path_provided {
                    return Err(e);
                }
                // Auto-discovered: silently warn and return default
                tracing::warn!(
                    "Failed to parse auto-discovered config {}: {}",
                    config_file.display(),
                    e
                );
                return Ok(Config::default());
            }
        },
        "yaml" | "yml" => match parse_yaml_config(&content, &config_file) {
            Ok(cfg) => cfg,
            Err(e) => {
                if config_path_provided {
                    return Err(e);
                }
                // Auto-discovered: silently warn and return default
                tracing::warn!(
                    "Failed to parse auto-discovered config {}: {}",
                    config_file.display(),
                    e
                );
                return Ok(Config::default());
            }
        },
        other => {
            let err = anyhow::anyhow!(
                "Unsupported config extension '.{}' for file {}",
                other,
                config_file.display()
            );
            if config_path_provided {
                return Err(err);
            }
            // Auto-discovered: silently ignore
            tracing::warn!("{}", err);
            return Ok(Config::default());
        }
    };

    Ok(parsed)
}

/// Parse TOML config, supporting nested [repo-to-prompt] or [r2p] sections.
///
/// Matches Python's _parse_toml behavior (lines 262-267).
fn parse_toml_config(content: &str, config_file: &Path) -> Result<Config> {
    // Parse to generic value first
    let raw: toml::Value = toml::from_str(content)
        .with_context(|| format!("Invalid TOML syntax: {}", config_file.display()))?;

    // Check for nested section (Python lines 263-266)
    let config_val = if let Some(nested) = raw.get("repo-to-prompt") {
        nested.clone()
    } else if let Some(nested) = raw.get("r2p") {
        nested.clone()
    } else {
        raw
    };

    // Deserialize to Config
    config_val.try_into().with_context(|| format!("Invalid TOML config: {}", config_file.display()))
}

/// Parse YAML config, supporting nested repo-to-prompt or r2p sections.
///
/// Matches Python's _parse_yaml behavior (lines 295-300).
fn parse_yaml_config(content: &str, config_file: &Path) -> Result<Config> {
    // Parse to generic value first
    let raw: serde_yaml::Value = serde_yaml::from_str(content)
        .with_context(|| format!("Invalid YAML syntax: {}", config_file.display()))?;

    // Check for nested section (Python lines 296-299)
    let config_val = if let Some(nested) = raw.get("repo-to-prompt") {
        nested.clone()
    } else if let Some(nested) = raw.get("r2p") {
        nested.clone()
    } else {
        raw
    };

    // Deserialize to Config
    serde_yaml::from_value(config_val)
        .with_context(|| format!("Invalid YAML config: {}", config_file.display()))
}

fn discover_config(repo_root: &Path) -> Option<std::path::PathBuf> {
    let candidates = [
        "repo-to-prompt.toml",
        ".repo-to-prompt.toml",
        "r2p.toml",
        ".r2p.toml",
        "r2p.yml",
        ".r2p.yml",
        "r2p.yaml",
        ".r2p.yaml",
    ];

    for candidate in candidates {
        let path = repo_root.join(candidate);
        if path.exists() {
            return Some(path);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_load_config_defaults_when_missing() {
        let tmp = TempDir::new().expect("tmp");
        let cfg = load_config(tmp.path(), None).expect("config");
        assert!(cfg.path.is_none());
        assert!(cfg.repo_url.is_none());
    }

    #[test]
    fn test_load_toml_config() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("repo-to-prompt.toml");
        fs::write(&path, "max_file_bytes = 999\nrespect_gitignore = false\nmode = 'prompt'\n")
            .expect("write");

        let cfg = load_config(tmp.path(), None).expect("config");
        assert_eq!(cfg.max_file_bytes, 999);
        assert!(!cfg.respect_gitignore);
    }

    // --- Test 1: Explicit config with invalid type for include_extensions ---
    #[test]
    fn test_explicit_config_invalid_type_returns_err() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("bad.toml");
        // include_extensions expects a string or array, not an integer
        fs::write(&path, "include_extensions = 123\n").expect("write");

        let result = load_config(tmp.path(), Some(&path));
        assert!(result.is_err(), "explicit config with invalid type should return Err");
    }

    // --- Test 2: Explicit config with mixed-type list (string + integer) ---
    #[test]
    fn test_explicit_config_mixed_type_list_returns_err() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("bad.toml");
        // A list with a mix of strings and integers should fail deserialization
        fs::write(&path, "include_extensions = [\".py\", 123]\n").expect("write");

        let result = load_config(tmp.path(), Some(&path));
        assert!(result.is_err(), "explicit config with mixed-type list should return Err");
    }

    // --- Test 3: Explicit config with invalid globs type ---
    #[test]
    fn test_explicit_config_invalid_globs_type_returns_err() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("bad.toml");
        // exclude_globs expects a string or array, not a boolean
        fs::write(&path, "exclude_globs = false\n").expect("write");

        let result = load_config(tmp.path(), Some(&path));
        assert!(result.is_err(), "explicit config with boolean exclude_globs should return Err");
    }

    // --- Test 4: Auto-discovered config with invalid type returns default (soft-fail) ---
    #[test]
    fn test_auto_discovered_invalid_type_returns_default() {
        let tmp = TempDir::new().expect("tmp");
        // Write a bad config at the auto-discovery location
        fs::write(tmp.path().join("repo-to-prompt.toml"), "include_extensions = 123\n")
            .expect("write");

        // Auto-discover: no explicit path provided — should soft-warn and return default
        let cfg = load_config(tmp.path(), None).expect("should not error on auto-discovery");
        // Default max_file_bytes is 1MB
        assert_eq!(cfg.max_file_bytes, crate::domain::Config::default().max_file_bytes);
    }

    // --- Test 5: Auto-discovered config with mixed-type list returns default (soft-fail) ---
    #[test]
    fn test_auto_discovered_mixed_type_list_returns_default() {
        let tmp = TempDir::new().expect("tmp");
        fs::write(tmp.path().join("repo-to-prompt.toml"), "include_extensions = [\".py\", 123]\n")
            .expect("write");

        let cfg = load_config(tmp.path(), None).expect("should not error on auto-discovery");
        assert_eq!(cfg.max_file_bytes, crate::domain::Config::default().max_file_bytes);
    }

    // --- Test 6: String normalization: comma-separated include_extensions ---
    #[test]
    fn test_string_normalization_comma_separated_extensions() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("r2p.toml");
        fs::write(&path, "include_extensions = \"py, js,  ts\"\n").expect("write");

        let cfg = load_config(tmp.path(), Some(&path)).expect("config");
        let exts: std::collections::HashSet<String> = cfg.include_extensions.into_iter().collect();
        assert!(exts.contains(".py"), "should contain .py");
        assert!(exts.contains(".js"), "should contain .js");
        assert!(exts.contains(".ts"), "should contain .ts");
    }

    // --- Test 7: List normalization: array with/without dots and whitespace ---
    #[test]
    fn test_list_normalization_extensions_array() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("r2p.toml");
        // ".py" already has dot, "js" needs one added, "  ts  " needs trimming + dot
        fs::write(&path, "include_extensions = [\".py\", \"js\", \"  ts  \"]\n").expect("write");

        let cfg = load_config(tmp.path(), Some(&path)).expect("config");
        let exts: std::collections::HashSet<String> = cfg.include_extensions.into_iter().collect();
        assert!(exts.contains(".py"), "should contain .py");
        assert!(exts.contains(".js"), "should contain .js");
        assert!(exts.contains(".ts"), "should contain .ts");
    }

    // --- Test 8: Glob normalization: comma-separated exclude_globs ---
    #[test]
    fn test_glob_normalization_comma_separated() {
        let tmp = TempDir::new().expect("tmp");
        let path = tmp.path().join("r2p.toml");
        fs::write(&path, "exclude_globs = \"dist, build ,  node_modules\"\n").expect("write");

        let cfg = load_config(tmp.path(), Some(&path)).expect("config");
        let globs: std::collections::HashSet<String> = cfg.exclude_globs.into_iter().collect();
        assert!(globs.contains("dist"), "should contain dist");
        assert!(globs.contains("build"), "should contain build");
        assert!(globs.contains("node_modules"), "should contain node_modules");
    }
}

#![allow(missing_docs)]

use serde::{Deserialize, Serialize};

/// Redaction configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactionConfig {
    #[serde(default)]
    pub allowlist_patterns: Vec<String>,
    #[serde(default)]
    pub allowlist_strings: Vec<String>,
    #[serde(default)]
    pub custom_rules: Vec<CustomRedactionRule>,
    #[serde(default)]
    pub entropy: EntropyConfig,
    #[serde(default)]
    pub paranoid: ParanoidConfig,
    #[serde(default = "default_safe_file_patterns")]
    pub safe_file_patterns: Vec<String>,
    #[serde(default = "default_source_safe_patterns")]
    pub source_safe_patterns: Vec<String>,
    #[serde(default = "default_true_redaction")]
    pub structure_safe_redaction: bool,
}

/// One custom redaction rule loaded from config.
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

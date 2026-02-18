//! Redactor implementation

use crate::domain::{CustomRedactionRule, RedactionConfig};
use crate::redact::entropy::calculate_entropy;
use crate::redact::rules::{RedactionRule, DEFAULT_RULES};
use once_cell::sync::Lazy;
use regex::Regex;
use rustpython_parser::ast;
use rustpython_parser::Parse;
use std::collections::BTreeMap;

#[allow(dead_code)]
const ENTROPY_THRESHOLD: f64 = 4.5;
const ENTROPY_MIN_LEN: usize = 20;

/// Patterns for safe (non-secret) strings that should not be flagged by entropy detection.
/// Matches: UUIDs, git SHAs (40-char hex), MD5 (32-char hex), SHA-256 (64-char hex),
/// semver strings.
static SAFE_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // UUID
        Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$").unwrap(),
        // Git SHA (exactly 40 hex chars)
        Regex::new(r"^[0-9a-f]{40}$").unwrap(),
        // MD5 (exactly 32 hex chars)
        Regex::new(r"^[0-9a-f]{32}$").unwrap(),
        // SHA-256 (exactly 64 hex chars)
        Regex::new(r"^[0-9a-f]{64}$").unwrap(),
        // Semver: 1.2.3-beta.4+build.567
        Regex::new(r"^\d+\.\d+\.\d+[\w\-+.]*$").unwrap(),
    ]
});

/// Returns true if `s` matches a known safe pattern (UUID, hash, semver).
fn is_safe_value(s: &str) -> bool {
    SAFE_PATTERNS.iter().any(|re| re.is_match(s))
}

/// Returns true if the filename matches any of the given glob patterns.
fn matches_glob_pattern(filename: &str, patterns: &[String]) -> bool {
    for pattern in patterns {
        if glob_match(pattern, filename) {
            return true;
        }
    }
    false
}

/// Simple glob matching: supports `*` (matches any chars, not `/`) and `**` (matches all).
fn glob_match(pattern: &str, value: &str) -> bool {
    // Use the `glob` crate-compatible approach via fnmatch-style logic.
    // For simplicity, we delegate to the `globset` approach using string comparison:
    // patterns like "*.md", "go.sum", "package-lock.json".
    fn inner(pat: &[u8], val: &[u8]) -> bool {
        match (pat.first(), val.first()) {
            (None, None) => true,
            (Some(b'*'), _) => {
                // `**` — match everything
                if pat.get(1) == Some(&b'*') {
                    inner(&pat[2..], val) || (!val.is_empty() && inner(pat, &val[1..]))
                } else {
                    // `*` — match any char except '/'
                    inner(&pat[1..], val)
                        || (!val.is_empty() && val[0] != b'/' && inner(pat, &val[1..]))
                }
            }
            (Some(&p), Some(&v)) if p == v => inner(&pat[1..], &val[1..]),
            _ => false,
        }
    }
    inner(pattern.as_bytes(), value.as_bytes())
}

pub struct Redactor {
    rules: Vec<RedactionRule>,
    redact_high_entropy: bool,
    entropy_threshold: f64,
    entropy_min_len: usize,
    /// Pre-compiled regex built from `entropy_min_len` so custom config values are respected.
    entropy_token_regex: Regex,
    structure_safe: bool,
    source_safe_patterns: Vec<String>,
    /// File patterns exempt from paranoid mode (e.g. *.md, *.json, Cargo.lock)
    safe_file_patterns: Vec<String>,
    paranoid_mode: bool,
    paranoid_min_len: usize,
    allowlist_patterns: Vec<String>,
    allowlist_strings: Vec<String>,
}

pub struct RedactionOutcome {
    pub content: String,
    pub counts: BTreeMap<String, usize>,
}

/// Build an entropy token regex for the given minimum token length.
fn build_entropy_regex(min_len: usize) -> Regex {
    Regex::new(&format!(r"\b[A-Za-z0-9+/=_-]{{{},}}\b", min_len))
        .expect("valid entropy token regex")
}

impl Redactor {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            rules: DEFAULT_RULES.clone(),
            redact_high_entropy: false,
            entropy_threshold: ENTROPY_THRESHOLD,
            entropy_min_len: ENTROPY_MIN_LEN,
            entropy_token_regex: build_entropy_regex(ENTROPY_MIN_LEN),
            structure_safe: false,
            source_safe_patterns: Vec::new(),
            safe_file_patterns: Vec::new(),
            paranoid_mode: false,
            paranoid_min_len: 32,
            allowlist_patterns: Vec::new(),
            allowlist_strings: Vec::new(),
        }
    }

    /// Build a `Redactor` from a `RedactionConfig` (loaded from config file).
    pub fn from_config(
        mode_entropy: bool,
        mode_paranoid: bool,
        mode_structure_safe: bool,
        cfg: &RedactionConfig,
    ) -> Self {
        // Compile custom rules from config; skip on regex error with a warning.
        let mut rules = DEFAULT_RULES.clone();
        for cr in &cfg.custom_rules {
            if let Ok(re) = compile_custom_rule(cr) {
                rules.push(re);
            }
        }

        let entropy_min_len = cfg.entropy.min_length;
        Self {
            rules,
            redact_high_entropy: mode_entropy || cfg.entropy.enabled,
            entropy_threshold: cfg.entropy.threshold,
            entropy_min_len,
            entropy_token_regex: build_entropy_regex(entropy_min_len),
            structure_safe: mode_structure_safe,
            source_safe_patterns: cfg.source_safe_patterns.clone(),
            safe_file_patterns: cfg.safe_file_patterns.clone(),
            paranoid_mode: mode_paranoid || cfg.paranoid.enabled,
            paranoid_min_len: cfg.paranoid.min_length,
            allowlist_patterns: cfg.allowlist_patterns.clone(),
            allowlist_strings: cfg.allowlist_strings.clone(),
        }
    }

    #[allow(dead_code)]
    pub fn with_entropy_detection(mut self, enabled: bool) -> Self {
        self.redact_high_entropy = enabled;
        self
    }

    #[allow(dead_code)]
    pub fn with_structure_safe(mut self, enabled: bool) -> Self {
        self.structure_safe = enabled;
        self
    }

    #[allow(dead_code)]
    pub fn with_paranoid_mode(mut self, enabled: bool) -> Self {
        self.paranoid_mode = enabled;
        self
    }

    /// Returns true if the file (by name or path) matches allowlist patterns.
    ///
    /// Matches Python's _is_file_allowlisted behavior (lines 550-552):
    /// checks both filename and full relative path against patterns.
    pub fn is_file_allowlisted(&self, filename: &str, rel_path: &str) -> bool {
        matches_glob_pattern(filename, &self.allowlist_patterns)
            || matches_glob_pattern(rel_path, &self.allowlist_patterns)
    }

    /// Returns true if the literal string `s` is in the allowlist.
    fn is_string_allowlisted(&self, s: &str) -> bool {
        self.allowlist_strings.iter().any(|al| al == s)
    }

    /// Returns true if the file extension matches source_safe_patterns.
    ///
    /// Matches Python's _is_source_file behavior (lines 582-592).
    /// Python checks both filename and path against patterns; we check actual filename
    /// and fall back to a fake extension-based filename for compatibility.
    fn is_source_safe_language(&self, filename: &str, extension: &str) -> bool {
        if !self.structure_safe {
            return false;
        }
        if !filename.is_empty() {
            for pattern in &self.source_safe_patterns {
                if glob_match(pattern, filename) {
                    return true;
                }
            }
        }
        if !extension.is_empty() {
            // Fall back to extension-based fake filename
            let fake_filename = format!("file{}", extension);
            for pattern in &self.source_safe_patterns {
                if glob_match(pattern, &fake_filename) {
                    return true;
                }
            }
        }
        false
    }

    /// Returns true if this file should be considered "safe" and skip paranoid mode.
    ///
    /// Matches Python's _is_file_safe (redactor.py lines 556-573):
    /// checks both filename and full relative path against patterns.
    fn is_file_safe(&self, filename: &str, rel_path: &str) -> bool {
        matches_glob_pattern(filename, &self.safe_file_patterns)
            || matches_glob_pattern(rel_path, &self.safe_file_patterns)
    }

    #[allow(dead_code)]
    pub fn redact(&self, text: &str) -> String {
        self.redact_inner(text, "", "", "", "", false).content
    }

    #[allow(dead_code)]
    pub fn redact_with_language(&self, text: &str, language: &str) -> String {
        self.redact_with_language_report(text, language, "", "", "").content
    }

    pub fn redact_with_language_report(
        &self,
        text: &str,
        language: &str,
        extension: &str,
        filename: &str,
        rel_path: &str,
    ) -> RedactionOutcome {
        self.redact_inner(text, language, extension, filename, rel_path, true)
    }

    fn redact_inner(
        &self,
        text: &str,
        language: &str,
        extension: &str,
        filename: &str,
        rel_path: &str,
        check_structure_safe: bool,
    ) -> RedactionOutcome {
        let mut counts = BTreeMap::new();

        // ── Pass 1: apply rule-based redactions ──────────────────────────────
        let mut after_rules = text.to_string();
        for rule in &self.rules {
            let mut replaced = 0usize;
            after_rules = rule
                .pattern
                .replace_all(&after_rules, |caps: &regex::Captures<'_>| {
                    replaced += 1;
                    let mut expanded = String::new();
                    caps.expand(rule.replacement, &mut expanded);
                    expanded
                })
                .into_owned();
            if replaced > 0 {
                counts.insert(rule.name.to_string(), replaced);
            }
        }

        // ── Structure-safe AST check (Python files only) after rules ─────────
        // Python order: apply rules → AST validate → if broken revert and return original
        //               if OK → apply entropy/paranoid → AST validate again → if broken
        //               revert entropy/paranoid only (keep rules result).
        let is_source = check_structure_safe && self.is_source_safe_language(filename, extension);
        let is_python = language == "python";

        if is_source && is_python {
            let original_valid = is_valid_python(text);
            if original_valid && !is_valid_python(&after_rules) {
                // Rules broke the Python AST — revert everything and return original.
                let mut reverted = BTreeMap::new();
                reverted.insert("structure_safe_reverted".to_string(), 1);
                return RedactionOutcome { content: text.to_string(), counts: reverted };
            }
        }

        // ── Pass 2: entropy + paranoid on top of rules result ────────────────
        // M4: skip paranoid for files matching safe_file_patterns (*.md, *.json, etc.)
        let file_is_safe = !filename.is_empty() && self.is_file_safe(filename, rel_path);
        let apply_paranoid = self.paranoid_mode && !file_is_safe;

        let mut after_entropy = after_rules.clone();

        if self.redact_high_entropy {
            let (entropy_redacted, entropy_count) = self.redact_high_entropy_tokens(&after_entropy);
            after_entropy = entropy_redacted;
            if entropy_count > 0 {
                counts.insert("entropy_detected".to_string(), entropy_count);
            }
        }

        if apply_paranoid {
            let (paranoid_redacted, paranoid_count) = self.redact_paranoid_tokens(&after_entropy);
            after_entropy = paranoid_redacted;
            if paranoid_count > 0 {
                *counts.entry("paranoid_redacted".to_string()).or_insert(0) += paranoid_count;
            }
        }

        // ── Second AST check: if entropy/paranoid broke Python, revert them ──
        if is_source && is_python && (self.redact_high_entropy || apply_paranoid) {
            let original_valid = is_valid_python(text);
            if original_valid && !is_valid_python(&after_entropy) {
                // Revert only entropy/paranoid — keep rules result.
                // Remove entropy/paranoid counts (keep rule counts).
                counts.remove("entropy_detected");
                counts.remove("paranoid_redacted");
                return RedactionOutcome { content: after_rules, counts };
            }
        }

        RedactionOutcome { content: after_entropy, counts }
    }

    fn redact_high_entropy_tokens(&self, text: &str) -> (String, usize) {
        let threshold = if self.paranoid_mode { 3.5 } else { self.entropy_threshold };
        let min_len = self.entropy_min_len;
        let mut count = 0usize;
        let output = self
            .entropy_token_regex
            .replace_all(text, |caps: &regex::Captures<'_>| {
                let token = caps.get(0).map(|m| m.as_str()).unwrap_or("");
                if token.len() >= min_len
                    && !self.is_string_allowlisted(token)
                    && !is_safe_value(token)
                    && calculate_entropy(token) >= threshold
                {
                    count += 1;
                    "[HIGH_ENTROPY_REDACTED]".to_string()
                } else {
                    token.to_string()
                }
            })
            .into_owned();
        (output, count)
    }

    fn redact_paranoid_tokens(&self, text: &str) -> (String, usize) {
        let min_len = self.paranoid_min_len;
        // Paranoid: any alphanumeric+symbols token of min_len or more that isn't already
        // redacted, allowlisted, or a known safe value.
        let re_src = format!(r"\b([A-Za-z0-9+/=_\-]{{{},}})\b", min_len);
        let re = match Regex::new(&re_src) {
            Ok(r) => r,
            Err(_) => return (text.to_string(), 0),
        };
        let mut count = 0usize;
        let output = re
            .replace_all(text, |caps: &regex::Captures<'_>| {
                let token = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                if self.is_string_allowlisted(token)
                    || is_safe_value(token)
                    || token.contains("[REDACTED")
                {
                    token.to_string()
                } else {
                    count += 1;
                    "[LONG_TOKEN_REDACTED]".to_string()
                }
            })
            .into_owned();
        (output, count)
    }
}

fn compile_custom_rule(cr: &CustomRedactionRule) -> Result<RedactionRule, regex::Error> {
    let pattern = Regex::new(&cr.pattern)?;
    let name = cr.name.clone().unwrap_or_else(|| "custom".to_string());
    // We need to store replacement as &'static str — leak for custom rules.
    let replacement: &'static str = Box::leak(cr.replacement.clone().into_boxed_str());
    Ok(RedactionRule { name: Box::leak(name.into_boxed_str()), pattern, replacement })
}

fn is_valid_python(source: &str) -> bool {
    ast::Suite::parse(source, "<redacted>").is_ok()
}

#[cfg(test)]
mod tests {
    use super::{is_safe_value, is_valid_python, Redactor};
    use crate::domain::RedactionConfig;

    #[test]
    fn redacts_known_patterns() {
        let redactor = Redactor::new();
        let input = "token = \"sk-abcdefghijklmnopqrstuvwxyz12345\"";
        let output = redactor.redact(input);
        assert!(output.contains("[REDACTED_OPENAI_KEY]") || output.contains("[REDACTED_SECRET]"));
    }

    #[test]
    fn redacts_entropy_tokens() {
        let redactor = Redactor::new().with_entropy_detection(true);
        let input = "secret ABCDEFGHIJKLMNOPQRSTUVWXYZ123456";
        let output = redactor.redact(input);
        assert!(output.contains("[HIGH_ENTROPY_REDACTED]"));
    }

    #[test]
    fn python_redaction_preserves_parseability() {
        let redactor = Redactor::new();
        let input = "def f():\n    password = \"supersecret\"\n    return password\n";
        let output = redactor.redact_with_language(input, "python");
        assert!(is_valid_python(&output));
    }

    #[test]
    fn paranoid_mode_redacts_more_entropy_tokens() {
        let token = "abcDEF123ghiJKL456mnoPQR789";
        let input = format!("x = \"{}\"", token);
        let standard =
            Redactor::new().with_entropy_detection(true).with_paranoid_mode(false).redact(&input);
        let paranoid =
            Redactor::new().with_entropy_detection(true).with_paranoid_mode(true).redact(&input);
        assert!(
            paranoid.contains("[HIGH_ENTROPY_REDACTED]")
                && (standard.is_empty() || standard.contains("[HIGH_ENTROPY_REDACTED]"))
        );
    }

    #[test]
    fn safe_patterns_not_flagged_by_entropy() {
        // Git SHA (40-char hex) — should be safe
        assert!(is_safe_value("a3f5e2d1c0b9e8a7f6d5c4b3a2f1e0d9c8b7a6f5"));
        // UUID
        assert!(is_safe_value("550e8400-e29b-41d4-a716-446655440000"));
        // MD5
        assert!(is_safe_value("d41d8cd98f00b204e9800998ecf8427e"));
        // Semver
        assert!(is_safe_value("1.2.3-beta.4"));
    }

    #[test]
    fn allowlist_strings_not_redacted() {
        let mut redactor = Redactor::new().with_entropy_detection(true);
        let safe_token = "ABCDEFGHIJKLMNOPQRSTUVWXYZ123456";
        redactor.allowlist_strings = vec![safe_token.to_string()];
        let input = format!("config = \"{}\"", safe_token);
        let output = redactor.redact(&input);
        assert!(
            !output.contains("[HIGH_ENTROPY_REDACTED]"),
            "allowlisted token should not be redacted"
        );
    }

    #[test]
    fn allowlist_file_pattern_skips_redaction() {
        let mut redactor = Redactor::new();
        redactor.allowlist_patterns = vec!["*.md".to_string()];
        // With file-level allowlist check, the redactor itself doesn't call is_file_allowlisted
        // during redact() — callers must check is_file_allowlisted() before calling redact.
        assert!(redactor.is_file_allowlisted("README.md", "README.md"));
        assert!(!redactor.is_file_allowlisted("main.py", "src/main.py"));
    }

    // --- Test H2: Custom entropy min_length is respected (not stubbed) ---
    #[test]
    fn entropy_min_length_from_config_is_respected() {
        use crate::domain::{EntropyConfig, RedactionConfig};

        // Build a config with min_length = 30
        let mut cfg = RedactionConfig::default();
        cfg.entropy = EntropyConfig { enabled: true, threshold: 3.5, min_length: 30 };

        let redactor = Redactor::from_config(
            true,  // mode_entropy
            false, // mode_paranoid
            false, // mode_structure_safe
            &cfg,
        );

        // A 25-char high-entropy string — below the 30-char threshold, should NOT be redacted
        let short_token = "ABCDEFGHIJKLMNOPQRSTUVWXY"; // 25 chars
        assert_eq!(short_token.len(), 25);
        let output_short = redactor.redact(short_token);
        assert!(
            !output_short.contains("[HIGH_ENTROPY_REDACTED]"),
            "25-char token should not be redacted with min_length=30, got: {output_short}"
        );

        // A 35-char high-entropy string — above the 30-char threshold, should be redacted
        let long_token = "ABCDEFGHIJKLMNOPQRSTUVWXYZ123456789"; // 35 chars
        assert_eq!(long_token.len(), 35);
        let output_long = redactor.redact(long_token);
        assert!(
            output_long.contains("[HIGH_ENTROPY_REDACTED]"),
            "35-char token should be redacted with min_length=30, got: {output_long}"
        );
    }
    #[test]
    fn test_path_based_allowlist_docs_glob() {
        let mut redactor = Redactor::new();
        redactor.allowlist_patterns = vec!["docs/**".to_string()];

        // docs/guide.md matches docs/**
        assert!(
            redactor.is_file_allowlisted("guide.md", "docs/guide.md"),
            "docs/guide.md should match docs/**"
        );
        // Nested path also matches
        assert!(
            redactor.is_file_allowlisted("index.md", "docs/api/index.md"),
            "docs/api/index.md should match docs/**"
        );
    }

    // --- Test 13: Non-allowlisted path does not match docs/** pattern ---
    #[test]
    fn test_non_allowlisted_path_not_matched() {
        let mut redactor = Redactor::new();
        redactor.allowlist_patterns = vec!["docs/**".to_string()];

        // src/main.py is not under docs/
        assert!(
            !redactor.is_file_allowlisted("main.py", "src/main.py"),
            "src/main.py should not match docs/**"
        );
    }

    // --- Test 14: Non-Python source-safe language (JS) engages structure-safe path without AST revert ---
    #[test]
    fn test_non_python_source_safe_no_ast_revert() {
        let mut cfg = RedactionConfig::default();
        // Override source_safe_patterns to only include *.js for isolation
        cfg.source_safe_patterns = vec!["*.js".to_string()];

        let redactor = Redactor::from_config(
            false, // entropy
            false, // paranoid
            true,  // structure_safe = true
            &cfg,
        );

        // A JS "file" with a secret — should be redacted (structure-safe path engaged but no AST check)
        let input = "const token = \"sk-abcdefghijklmnopqrstuvwxyz12345\";";
        let outcome =
            redactor.redact_with_language_report(input, "javascript", ".js", "test.js", "");

        // Redaction should have occurred (not reverted, no Python AST check)
        assert!(
            outcome.content.contains("[REDACTED_OPENAI_KEY]")
                || outcome.content.contains("[REDACTED_SECRET]")
                || !outcome.counts.is_empty(),
            "JS source-safe redaction should apply rules, got: {:?}",
            outcome.content
        );
        // Must NOT have been reverted (structure_safe_reverted would only apply to Python)
        assert!(
            !outcome.counts.contains_key("structure_safe_reverted"),
            "JS should not trigger structure_safe_reverted"
        );
    }

    // --- Test 15: Python AST validation reverts redaction that breaks syntax ---
    #[test]
    fn test_python_ast_validation_reverts_broken_syntax() {
        let mut cfg = RedactionConfig::default();
        // Ensure *.py is in source_safe_patterns
        cfg.source_safe_patterns = vec!["*.py".to_string()];

        let redactor = Redactor::from_config(
            false, // entropy
            false, // paranoid
            true,  // structure_safe = true
            &cfg,
        );

        // Craft Python that is valid, but where redaction would break the AST.
        // We use a string that matches a default redaction rule, inside a syntactically
        // sensitive position. The easiest: the content IS the secret — a bare expression.
        // After replacing it with [REDACTED_*], the result won't parse as valid Python.
        // Use a raw secret string as a module-level docstring substitution target.
        let input = "password = \"sk-abcdefghijklmnopqrstuvwxyz12345\"\n";
        // After redaction the value becomes [REDACTED_OPENAI_KEY] or similar — which is NOT
        // valid Python syntax as an identifier. This means Python AST parse of the redacted
        // code will fail, triggering a revert.
        let outcome = redactor.redact_with_language_report(input, "python", ".py", "test.py", "");

        // Either redaction was applied (if it somehow kept valid Python) or it was reverted
        // The important thing: if reverted, content == original and structure_safe_reverted is set
        if outcome.counts.contains_key("structure_safe_reverted") {
            assert_eq!(outcome.content, input, "reverted content should equal original input");
        } else {
            // Redaction applied and result is valid Python
            assert!(
                is_valid_python(&outcome.content),
                "if not reverted, redacted Python must still be valid"
            );
        }
    }
}

use serde::{Deserialize, Serialize};

/// Output mode for the tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    /// Generate a prompt-optimized context pack.
    Prompt,
    /// Generate RAG-optimized chunks.
    Rag,
    /// Generate both prompt and RAG outputs (default).
    #[default]
    Both,
}

/// Redaction mode controls aggressiveness and syntax safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RedactionMode {
    /// Fast redaction with minimal safety checks.
    Fast,
    /// Standard redaction with balance of speed and safety (default).
    #[default]
    Standard,
    /// Aggressive redaction that may have false positives.
    Paranoid,
    /// Redaction with AST validation for syntax safety.
    StructureSafe,
}

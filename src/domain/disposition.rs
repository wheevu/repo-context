use serde::{Deserialize, Serialize};

/// Stable disposition reason for every discovered regular file.
#[allow(missing_docs)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileDispositionReason {
    IncludedFull,
    IncludedChunked,
    IncludedSummaryOnly,
    SkippedExtension,
    SkippedBinary,
    SkippedSize,
    SkippedGitignore,
    SkippedGlob,
    SkippedMinified,
    SkippedGenerated,
    DroppedByteBudget,
    DroppedTokenBudget,
    ExcludedNoiseDir,
    ErrorReadingMetadata,
    ErrorReadingContent,
}

impl FileDispositionReason {
    #[allow(missing_docs)]
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::IncludedFull => "included_full",
            Self::IncludedChunked => "included_chunked",
            Self::IncludedSummaryOnly => "included_summary_only",
            Self::SkippedExtension => "skipped_extension",
            Self::SkippedBinary => "skipped_binary",
            Self::SkippedSize => "skipped_size",
            Self::SkippedGitignore => "skipped_gitignore",
            Self::SkippedGlob => "skipped_glob",
            Self::SkippedMinified => "skipped_minified",
            Self::SkippedGenerated => "skipped_generated",
            Self::DroppedByteBudget => "dropped_byte_budget",
            Self::DroppedTokenBudget => "dropped_token_budget",
            Self::ExcludedNoiseDir => "excluded_noise_dir",
            Self::ErrorReadingMetadata => "error_reading_metadata",
            Self::ErrorReadingContent => "error_reading_content",
        }
    }
}

/// Per-file inventory/disposition entry emitted in reports and used by prompt inventory.
#[allow(missing_docs)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDisposition {
    pub path: String,
    pub reason: FileDispositionReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub extension: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub language: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_estimate: Option<usize>,
    pub included_in_prompt: bool,
    pub included_in_rag: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl FileDisposition {
    #[allow(missing_docs)]
    #[must_use]
    pub fn new(path: String, reason: FileDispositionReason) -> Self {
        Self {
            path,
            reason,
            size_bytes: None,
            extension: String::new(),
            language: String::new(),
            priority: None,
            token_estimate: None,
            included_in_prompt: false,
            included_in_rag: false,
            notes: None,
        }
    }
}

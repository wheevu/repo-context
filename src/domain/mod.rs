//! Core domain types and models.

mod chunk;
mod config;
mod file;
mod language;
mod output;
mod ranking;
mod redaction;
mod stats;

pub use chunk::Chunk;
pub use config::{default_exclude_globs, default_include_extensions, Config};
pub use file::FileInfo;
pub use language::get_language;
pub use output::{OutputMode, RedactionMode};
pub use ranking::RankingWeights;
#[allow(unused_imports)]
pub use redaction::{CustomRedactionRule, EntropyConfig, ParanoidConfig, RedactionConfig};
pub use stats::{ScanStats, REPORT_SCHEMA_VERSION};

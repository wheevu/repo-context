//! Output rendering (Markdown, JSONL, reports)

pub mod context_pack;
pub mod guardrails;
pub mod jsonl;
pub mod pr_context;
pub mod report;

pub use context_pack::render_context_pack;
pub use jsonl::render_jsonl;
pub use report::{write_report, ReportOptions};

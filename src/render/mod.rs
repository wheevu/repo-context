//! Output rendering (Markdown, JSONL, reports)

pub mod context_pack;
pub mod jsonl;
pub mod report;

pub use context_pack::{render_context_pack, ContextPackCtx};
pub use jsonl::{render_jsonl, render_jsonl_with_evidence};
pub use report::{write_report, write_report_with_retrieval, ReportOptions};

//! repo-context: Convert repositories into LLM-friendly context packs
//!
//! This tool scans code repositories and generates optimized context packs
//! for large language model prompting and RAG (Retrieval-Augmented Generation) workflows.

use anyhow::Result;

mod analysis;
mod chunk;
mod cli;
mod config;
mod domain;
mod fetch;
mod graph;
mod lsp;
mod rank;
mod redact;
mod render;
mod rerank;
mod scan;
mod utils;

fn main() -> Result<()> {
    cli::run()
}

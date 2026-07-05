//! repo-context: Convert repositories into LLM-friendly context packs
//!
//! This tool scans code repositories and generates optimized context packs
//! for large language model prompting and RAG (Retrieval-Augmented Generation) workflows.

fn main() -> anyhow::Result<()> {
    repo_context::cli::run()
}

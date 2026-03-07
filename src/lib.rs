//! Repo-context: Convert code repositories into LLM-friendly context packs
//!
//! A Rust CLI tool that converts code repositories into LLM-friendly context packs
//! for prompting and RAG (Retrieval-Augmented Generation) workflows.
//!
//! # Overview
//!
//! This library provides utilities for scanning, analyzing, and converting
//! code repositories into formats optimized for Large Language Models.
//!
//! # Pipeline Architecture
//!
//! The tool follows a modular pipeline:
//!
//! 1. **Fetch** - Get repository from local path, GitHub, or HuggingFace
//! 2. **Scan** - Discover files respecting .gitignore patterns
//! 3. **Rank** - Prioritize important files (READMEs, configs, entrypoints)
//! 4. **Chunk** - Split content into model-friendly sizes
//! 5. **Redact** - Remove secrets safely
//! 6. **Render** - Generate outputs (Markdown, JSONL, JSON report)
//!
//! # Example Usage
//!
//! ```rust
//! use repo_context::scan::scanner::FileScanner;
//!
//! // Create a scanner with default settings
//! let mut scanner = FileScanner::new(std::path::PathBuf::from("."));
//! let files = scanner.scan().expect("scan should succeed");
//! ```

#![warn(missing_docs)]

pub mod analysis;
pub mod chunk;
pub mod cli;
pub mod config;
pub mod domain;
pub mod fetch;
pub mod graph;
pub mod lsp;
pub mod rank;
pub mod redact;
pub mod render;
pub mod rerank;
pub mod scan;
pub mod utils;

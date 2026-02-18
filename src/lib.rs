//! Repo-to-Prompt: Convert code repositories into LLM-friendly context packs
//!
//! This library provides utilities for scanning, analyzing, and converting
//! code repositories into formats optimized for Large Language Models.

pub mod chunk;
pub mod cli;
pub mod config;
pub mod domain;
pub mod fetch;
pub mod rank;
pub mod redact;
pub mod render;
pub mod scan;
pub mod utils;

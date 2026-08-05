//! Configuration loading and merging
//!
//! Handles loading TOML config files and merging CLI and repository settings with defaults.

pub mod loader;
pub mod merge;

pub use loader::load_config;
pub use merge::{merge_cli_with_config, merge_repo_config, CliOverrides};

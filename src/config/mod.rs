//! Configuration loading and merging
//!
//! Handles loading from config files, environment variables, and CLI arguments
//! with proper precedence (CLI > Env > File > Defaults).

pub mod loader;
pub mod merge;

pub use loader::load_config;
pub use merge::{merge_cli_with_config, CliOverrides};

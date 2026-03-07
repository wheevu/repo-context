//! File scanning with gitignore support
//!
//! Provides directory scanning functionality that respects .gitignore patterns
//! and generates ASCII tree representations.

pub mod scanner;
pub mod tree;

/// Re-export of FileScanner for convenience.
pub use scanner::FileScanner;

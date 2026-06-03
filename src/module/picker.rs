//! Interactive pickers for module mode.
//!
//! Kept for backward compatibility. New code should use `focus_picker`.

#![allow(dead_code)]

use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, FuzzySelect, Select};
use std::path::{Path, PathBuf};

/// Scan mode chosen by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    /// Existing full-context behavior.
    Full,
    /// New module-scoped behavior.
    Module,
}

/// Prompts for scan mode.
pub fn pick_scan_mode() -> Result<ScanMode> {
    let items = ["Full context", "Module"];
    let selected = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Scan mode")
        .items(&items)
        .default(0)
        .interact()?;
    Ok(if selected == 1 { ScanMode::Module } else { ScanMode::Full })
}

/// Prompts for an entry point with fuzzy filtering.
pub fn pick_entry(root: &Path, mut candidates: Vec<PathBuf>) -> Result<Option<PathBuf>> {
    candidates.sort_by_key(|p| display_rel(root, p));
    candidates.dedup();
    if candidates.is_empty() {
        return Ok(None);
    }
    let labels: Vec<String> = candidates.iter().map(|p| display_rel(root, p)).collect();
    let selected = FuzzySelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Select entry point (type to filter)")
        .items(&labels)
        .default(0)
        .interact()?;
    Ok(candidates.get(selected).cloned())
}

fn display_rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

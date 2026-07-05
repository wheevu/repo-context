//! Interactive pickers for focused export mode.

use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, FuzzySelect, Select};
use std::path::Path;

use super::focus::{FocusCandidate, FocusKind, FocusScope};

/// Actions available after selecting a focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusAction {
    /// Proceed with the focused export.
    Export,
    /// Pick a different focus target.
    ChangeFocus,
    /// Fall back to full-context export.
    FullContext,
    /// Abort the export.
    Cancel,
}

/// Scan mode chosen by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    /// Existing full-context behavior.
    Full,
    /// New focused-scope behavior (files for small, modules for large repos).
    Focused,
}

/// Prompts for scan mode.
pub fn pick_scan_mode() -> Result<ScanMode> {
    let items = ["Full context", "Focused"];
    let selected = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Scan mode")
        .items(&items)
        .default(0)
        .interact()?;
    Ok(if selected == 1 { ScanMode::Focused } else { ScanMode::Full })
}

/// Picks a focus candidate (file or module).
pub fn pick_focus(candidates: &[FocusCandidate]) -> Result<Option<FocusCandidate>> {
    if candidates.is_empty() {
        return Ok(None);
    }
    let labels: Vec<String> =
        candidates.iter().map(|c| format!("{}  ({})", c.display, c.detail)).collect();
    let selected = FuzzySelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Select focus (type to filter)")
        .items(&labels)
        .default(0)
        .interact()?;
    Ok(candidates.get(selected).cloned())
}

/// Shows a preview of the focused scope and asks what to do.
pub fn preview_and_confirm(scope: &FocusScope, root: &Path) -> Result<FocusAction> {
    // Build a compact preview.
    let selected_label = super::display_rel(root, &scope.selected);
    let kind_label = match scope.kind {
        FocusKind::File => "File focus",
        FocusKind::Module => "Module entry",
    };

    let file_list: Vec<String> = scope
        .files
        .iter()
        .map(|(f, reason)| {
            let rel = super::display_rel(root, &f.path);
            let reason_str = match reason {
                super::focus::InclusionReason::Selected => "selected",
                super::focus::InclusionReason::OutboundDependency => "dependency",
                super::focus::InclusionReason::Caller => "caller",
                super::focus::InclusionReason::RelatedTest => "test",
                super::focus::InclusionReason::EntryPath => "entry-path",
                super::focus::InclusionReason::CrateFallback => "crate-fallback",
                super::focus::InclusionReason::RuntimeModule => "runtime",
                super::focus::InclusionReason::CssScope => "css",
            };
            format!("  {rel:50}  [{reason_str}]")
        })
        .collect();

    let fallback_warning = if scope
        .files
        .iter()
        .any(|(_, r)| matches!(r, super::focus::InclusionReason::CrateFallback))
    {
        "\n⚠ Rust module graph traversal found few or no dependencies; used crate-root fallback.\n"
    } else {
        ""
    };

    println!();
    println!("══ Focus: {} → {} ══", kind_label, selected_label);
    println!(
        "Files to include: {} ({} source files in repo)",
        scope.files.len(),
        scope.repo_source_file_count,
    );
    println!();
    for line in &file_list {
        println!("{line}");
    }
    println!();
    println!("{fallback_warning}");

    let items = ["Export", "Change focus", "Full context instead", "Cancel"];
    let selected = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Action")
        .items(&items)
        .default(0)
        .interact()?;

    Ok(match selected {
        0 => FocusAction::Export,
        1 => FocusAction::ChangeFocus,
        2 => FocusAction::FullContext,
        _ => FocusAction::Cancel,
    })
}

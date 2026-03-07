//! PR-context rendering for context pack outputs.
//!
//! Renders PR context reports as markdown for inclusion in context packs.

use crate::analysis::pr::PrContextReport;

/// Renders a PR context report as markdown.
///
/// Includes sections for:
/// - Touch Points
/// - Entrypoints
/// - Invariants
/// - Feature Flags (if present)
/// - Trait Implementations (if present)
/// - Error Flows (if present)
/// - Validation Checklist
///
/// # Arguments
/// * `report` - PR context report to render
///
/// # Returns
/// Markdown formatted string
pub fn render_pr_context(report: &PrContextReport) -> String {
    let mut out = String::new();
    out.push_str("\n## 🔧 PR Context\n\n");

    out.push_str("### Touch Points\n");
    for point in report.touch_points.iter().take(30) {
        let ids = if point.chunk_ids.is_empty() {
            "".to_string()
        } else {
            format!(" (chunks: {})", point.chunk_ids.join(", "))
        };
        out.push_str(&format!("- `{}` — {}{}\n", point.path, point.reason, ids));
    }

    out.push_str("\n### Entrypoints\n");
    for point in report.entrypoints.iter().take(30) {
        out.push_str(&format!(
            "- **{}** `{}` — `{}` ({})\n",
            point.kind, point.path, point.symbol, point.evidence
        ));
    }

    out.push_str("\n### Invariants\n");
    for inv in report.invariants.iter().take(30) {
        out.push_str(&format!(
            "- **{}** `{}` — {} (chunk `{}`)\n",
            inv.kind, inv.path, inv.symbol, inv.chunk_id
        ));
    }

    if !report.feature_flags.is_empty() {
        out.push_str("\n### Feature Flags\n");
        for flag in report.feature_flags.iter().take(30) {
            out.push_str(&format!(
                "- `{}` — feature `{}` (chunk `{}`)\n",
                flag.path, flag.feature, flag.chunk_id
            ));
        }
    }

    if !report.trait_impls.is_empty() {
        out.push_str("\n### Trait Implementations\n");
        for edge in report.trait_impls.iter().take(30) {
            out.push_str(&format!(
                "- `{}` — `impl {}` for `{}` (chunk `{}`)\n",
                edge.path, edge.trait_name, edge.target_type, edge.chunk_id
            ));
        }
    }

    if !report.error_flows.is_empty() {
        out.push_str("\n### Error Flows\n");
        for flow in report.error_flows.iter().take(30) {
            out.push_str(&format!(
                "- `{}` — {} (chunk `{}`)\n",
                flow.path, flow.evidence, flow.chunk_id
            ));
        }
    }

    if report.graph_available {
        out.push_str("\n> Symbol graph available: yes\n");
    } else {
        out.push_str("\n> Symbol graph not available; run export without --no-graph to build it\n");
    }

    out.push_str("\n### Validation Checklist\n");
    out.push_str("- Update touched code paths and their nearest tests together.\n");
    out.push_str("- Re-run linters/formatters before tests.\n");
    out.push_str("- Run the smallest relevant test scope first, then full suite.\n");
    out.push_str("- If behavior/contract changes, update docs and invariants in same PR.\n");

    out
}

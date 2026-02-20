//! PR-context rendering for context pack outputs.

use crate::analysis::pr::PrContextReport;

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

    if report.graph_available {
        out.push_str("\n> Symbol graph available: yes\n");
    } else {
        out.push_str("\n> Symbol graph not available; run export without --no-graph to build it\n");
    }

    out
}

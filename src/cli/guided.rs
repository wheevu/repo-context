//! Interactive guided export presets.

use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, MultiSelect, Select};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use crate::domain::{FileInfo, OutputMode, ScanStats};

const ARCHITECTURE_TASK: &str =
    "Explain the overall architecture, key modules, and dependency relationships.";

#[derive(Debug, Clone, PartialEq)]
pub struct GuidedPlan {
    pub mode: Option<OutputMode>,
    pub max_tokens: Option<usize>,
    pub task_query: Option<String>,
    pub stitch_budget_fraction: Option<f64>,
    pub stitch_top_n: Option<usize>,
    pub rerank_top_k: Option<usize>,
}

pub fn choose_guided_plan(
    root_path: &Path,
    stats: &ScanStats,
    ranked_files: &[FileInfo],
) -> Result<GuidedPlan> {
    print_preview(root_path, stats, ranked_files);

    let items = [
        "Quick scan (fast, high signal)",
        "Architecture overview (systems + dependencies)",
        "Deep dive specific areas (repo-specific)",
        "Full context (as much as possible)",
    ];

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Choose export focus")
        .default(0)
        .items(&items)
        .interact()?;

    let plan = match selection {
        0 => GuidedPlan {
            mode: None,
            max_tokens: None,
            task_query: None,
            stitch_budget_fraction: None,
            stitch_top_n: None,
            rerank_top_k: None,
        },
        1 => GuidedPlan {
            mode: Some(OutputMode::Both),
            max_tokens: Some(140_000),
            task_query: Some(ARCHITECTURE_TASK.to_string()),
            stitch_budget_fraction: Some(0.55),
            stitch_top_n: Some(56),
            rerank_top_k: Some(420),
        },
        2 => deep_dive_plan(ranked_files)?,
        3 => GuidedPlan {
            mode: Some(OutputMode::Both),
            max_tokens: Some(240_000),
            task_query: Some(
                "Provide broad coverage of the repository with emphasis on core implementation and dependencies."
                    .to_string(),
            ),
            stitch_budget_fraction: Some(0.32),
            stitch_top_n: Some(32),
            rerank_top_k: Some(360),
        },
        _ => unreachable!("unexpected menu index"),
    };

    Ok(plan)
}

fn deep_dive_plan(ranked_files: &[FileInfo]) -> Result<GuidedPlan> {
    let areas = build_focus_areas(ranked_files);
    if areas.is_empty() {
        return Ok(GuidedPlan {
            mode: Some(OutputMode::Both),
            max_tokens: Some(110_000),
            task_query: Some(
                "Deep dive into the repository's most important implementation areas and dependencies."
                    .to_string(),
            ),
            stitch_budget_fraction: Some(0.45),
            stitch_top_n: Some(44),
            rerank_top_k: Some(380),
        });
    }

    let labels: Vec<&str> = areas.iter().map(|a| a.label.as_str()).collect();
    let selected = MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Select one or more repo areas")
        .items(&labels)
        .interact()?;

    let selected_areas: Vec<&FocusArea> = if selected.is_empty() {
        vec![&areas[0]]
    } else {
        selected.into_iter().filter_map(|idx| areas.get(idx)).collect()
    };

    let scope =
        selected_areas.iter().map(|area| area.query_scope.as_str()).collect::<Vec<_>>().join("; ");
    let task_query = format!(
        "Deep dive into these repository areas: {scope}. Explain implementation details, data flow, and dependencies."
    );

    Ok(GuidedPlan {
        mode: Some(OutputMode::Both),
        max_tokens: Some(110_000),
        task_query: Some(task_query),
        stitch_budget_fraction: Some(0.45),
        stitch_top_n: Some(44),
        rerank_top_k: Some(380),
    })
}

fn print_preview(root_path: &Path, stats: &ScanStats, ranked_files: &[FileInfo]) {
    let repo_name = root_path.file_name().and_then(|n| n.to_str()).unwrap_or("repo");
    println!();
    println!("Guided export for '{repo_name}'");
    println!("  Files scanned:  {}", stats.files_scanned);
    println!("  Files included: {}", stats.files_included);

    if !stats.languages_detected.is_empty() {
        let mut langs: Vec<(&String, &usize)> = stats.languages_detected.iter().collect();
        langs.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let top = langs
            .into_iter()
            .take(5)
            .map(|(lang, count)| format!("{lang} ({count})"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("  Languages:      {top}");
    }

    let entrypoints: Vec<&str> = ranked_files
        .iter()
        .filter(|f| f.tags.contains("entrypoint"))
        .take(3)
        .map(|f| f.relative_path.as_str())
        .collect();
    if !entrypoints.is_empty() {
        println!("  Entrypoints:    {}", entrypoints.join(", "));
    }

    let top_files = ranked_files
        .iter()
        .take(5)
        .map(|f| f.relative_path.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    if !top_files.is_empty() {
        println!("  Top files:      {top_files}");
    }

    println!();
}

#[derive(Debug, Clone)]
struct FocusArea {
    label: String,
    query_scope: String,
}

fn build_focus_areas(ranked_files: &[FileInfo]) -> Vec<FocusArea> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    let mut dir_scores: BTreeMap<String, f64> = BTreeMap::new();
    for file in ranked_files.iter().take(200) {
        if let Some((top_dir, _)) = file.relative_path.split_once('/') {
            *dir_scores.entry(top_dir.to_string()).or_insert(0.0) += file.priority.max(0.01);
        }
    }
    let mut dirs: Vec<(String, f64)> = dir_scores.into_iter().collect();
    dirs.sort_by(|a, b| {
        b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0))
    });

    for (dir, _) in dirs.into_iter().take(6) {
        let label = format!("Directory: {dir}/");
        if seen.insert(label.clone()) {
            out.push(FocusArea {
                label,
                query_scope: format!("{dir}/ implementation and module dependencies"),
            });
        }
    }

    for file in ranked_files.iter().filter(|f| f.tags.contains("entrypoint")).take(4) {
        let label = format!("Entrypoint: {}", file.relative_path);
        if seen.insert(label.clone()) {
            out.push(FocusArea {
                label,
                query_scope: format!(
                    "entrypoint {} and its call/dependency flow",
                    file.relative_path
                ),
            });
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::build_focus_areas;
    use crate::domain::FileInfo;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn file(path: &str, priority: f64, tags: &[&str]) -> FileInfo {
        FileInfo {
            path: PathBuf::from(path),
            relative_path: path.to_string(),
            size_bytes: 10,
            extension: ".rs".to_string(),
            language: "rust".to_string(),
            id: path.to_string(),
            priority,
            token_estimate: 10,
            tags: tags.iter().map(|t| t.to_string()).collect::<BTreeSet<_>>(),
            is_readme: false,
            is_config: false,
            is_doc: false,
        }
    }

    #[test]
    fn build_focus_areas_prioritizes_top_directories_and_entrypoints() {
        let files = vec![
            file("src/main.rs", 0.9, &["entrypoint"]),
            file("src/lib.rs", 0.8, &[]),
            file("api/routes.rs", 0.85, &[]),
            file("api/handlers.rs", 0.8, &[]),
            file("docs/guide.md", 0.2, &[]),
        ];

        let areas = build_focus_areas(&files);
        let labels: Vec<&str> = areas.iter().map(|a| a.label.as_str()).collect();

        assert!(labels.contains(&"Directory: src/"));
        assert!(labels.contains(&"Directory: api/"));
        assert!(labels.contains(&"Entrypoint: src/main.rs"));
    }
}

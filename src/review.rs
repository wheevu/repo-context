//! Deterministic change-aware repository review.
//!
//! The review workflow deliberately sits beside the export pipeline. It uses
//! the same scanner, static graph, file classification, and redactor, but it
//! has its own small output contract so existing export artifacts do not move.

use anyhow::{Context, Result};
use git2::{Delta, DiffFormat, DiffLineType, DiffOptions, Oid, Repository, StatusOptions};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::load_config;
use crate::domain::{get_language, Config, FileInfo, RedactionMode};
use crate::fetch::local::find_repo_root;
use crate::godot::resolve_profile;
use crate::module::graph::{self, ImportGraph};
use crate::rank::rank_files;
use crate::redact::Redactor;
use crate::scan::scanner::FileScanner;
use crate::utils::{normalize_path, read_file_safe, write_atomic};

/// Machine-readable review schema identifier.
pub const IMPACT_PACK_SCHEMA: &str = "ImpactPackV1";

const MAX_CHANGED_FILES: usize = 256;
const MAX_CHANGED_LINES_PER_FILE: usize = 256;
const MAX_SNIPPETS_PER_FILE: usize = 48;
const MAX_SYMBOLS_PER_FILE: usize = 2_048;
const MAX_TRAVERSAL_NODES_PER_SEED: usize = 512;
const MAX_REFERENCE_CANDIDATES: usize = 4_096;
const MAX_REFERENCE_BYTES: usize = 64 * 1024;
const MAX_REVIEW_SOURCE_BYTES: usize = 2 * 1024 * 1024;

/// Output format for the review command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewFormat {
    /// Human-readable text.
    Text,
    /// Pretty-printed JSON.
    Json,
    /// Text on stdout and JSON in the output file (or after a separator).
    Both,
}

/// Options for building and rendering an impact pack.
#[derive(Debug, Clone)]
pub struct ReviewOptions {
    /// User-provided repository path.
    pub path: PathBuf,
    /// Base Git ref. Defaults to HEAD.
    pub base: Option<String>,
    /// Head Git ref. Defaults to the working tree when omitted.
    pub head: Option<String>,
    /// Compare the base ref with the current working tree.
    pub working_tree: bool,
    /// Requested output format.
    pub format: ReviewFormat,
    /// Optional output file. JSON is written for json and both.
    pub output: Option<PathBuf>,
    /// Disable the same redaction boundary used by export.
    pub no_redact: bool,
    /// Maximum number of related files emitted in the pack.
    pub max_related_files: usize,
}

/// Versioned, deterministic machine-readable impact pack.
#[derive(Debug, Clone, Serialize)]
pub struct ImpactPackV1 {
    /// Stable schema identifier.
    pub schema: &'static str,
    /// Numeric schema version for consumers that prefer numbers.
    pub schema_version: u32,
    /// Repository name, not an absolute path.
    pub repository: String,
    /// Comparison metadata.
    pub comparison: ComparisonInfo,
    /// Summary counts.
    pub summary: Summary,
    /// Changed files, sorted by display path.
    pub changed_files: Vec<ChangedFile>,
    /// Related files selected by static relationships and repository anchors.
    pub related_files: Vec<RelatedFile>,
    /// Explicit bounds used during analysis.
    pub limits: TraversalLimits,
}

/// Comparison metadata for a review run.
#[derive(Debug, Clone, Serialize)]
pub struct ComparisonInfo {
    /// refs or working_tree.
    pub mode: String,
    /// Base ref label.
    pub base: String,
    /// Head ref label, or WORKTREE.
    pub head: String,
    /// Resolved base tree id.
    pub base_oid: String,
    /// Resolved head tree id when available.
    pub head_oid: Option<String>,
}

/// Summary counts for a pack.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    /// Number of changed files.
    pub changed_files: usize,
    /// Number of changed symbols.
    pub changed_symbols: usize,
    /// Number of related files.
    pub related_files: usize,
    /// Number of related tests.
    pub tests: usize,
    /// Number of related configuration files.
    pub configs: usize,
    /// Number of related documentation files.
    pub docs: usize,
    /// Number of additions across changed text files.
    pub additions: usize,
    /// Number of deletions across changed text files.
    pub deletions: usize,
}

/// A single changed file and its bounded impact details.
#[derive(Debug, Clone, Serialize)]
pub struct ChangedFile {
    /// New path, or old path for a deletion.
    pub path: String,
    /// Old path when a rename/copy changed the path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    /// Git change status.
    pub status: String,
    /// Detected language.
    pub language: String,
    /// SHA-256 of the old content, when readable.
    pub old_sha256: Option<String>,
    /// SHA-256 of the new content, when readable.
    pub new_sha256: Option<String>,
    /// Number of added lines.
    pub additions: usize,
    /// Number of deleted lines.
    pub deletions: usize,
    /// Changed line locations, without source text.
    pub changed_lines: Vec<ChangedLine>,
    /// Changed declarations inferred from syntax-aware parsing.
    pub symbols: Vec<ChangedSymbol>,
    /// Direct static imports from this file.
    pub imports: Vec<String>,
    /// Direct static importers, a conservative caller approximation.
    pub callers: Vec<String>,
    /// Why this file is in the pack.
    pub reasons: Vec<String>,
    /// Bounded, redacted changed-line snippets.
    pub snippets: Vec<DiffSnippet>,
    /// Whether Git marked this change as binary or the text could not be read.
    pub binary: bool,
}

/// One changed line location.
#[derive(Debug, Clone, Serialize)]
pub struct ChangedLine {
    /// old or new.
    pub side: String,
    /// One-based line number when Git supplied one.
    pub line: usize,
}

/// One changed symbol and its old/new span.
#[derive(Debug, Clone, Serialize)]
pub struct ChangedSymbol {
    /// function, method, class, type, or another parser label.
    pub kind: String,
    /// Syntax-level symbol name.
    pub name: String,
    /// added, removed, or changed.
    pub status: String,
    /// Old one-based start line.
    pub old_start_line: Option<usize>,
    /// Old one-based end line.
    pub old_end_line: Option<usize>,
    /// New one-based start line.
    pub new_start_line: Option<usize>,
    /// New one-based end line.
    pub new_end_line: Option<usize>,
}

/// One redacted line excerpt from a changed hunk.
#[derive(Debug, Clone, Serialize)]
pub struct DiffSnippet {
    /// added or removed.
    pub side: String,
    /// One-based line number.
    pub line: usize,
    /// Redacted source excerpt.
    pub content: String,
}

/// A file included because it may be relevant to the change.
#[derive(Debug, Clone, Serialize)]
pub struct RelatedFile {
    /// Relative repository path.
    pub path: String,
    /// dependency, caller, test, config, or documentation.
    pub relation: String,
    /// Static graph distance, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance: Option<usize>,
    /// Explanation suitable for a human reviewer.
    pub reason: String,
}

/// Bounds and truncation policy used for the pack.
#[derive(Debug, Clone, Serialize)]
pub struct TraversalLimits {
    /// Maximum changed files emitted.
    pub max_changed_files: usize,
    /// Maximum related files emitted.
    pub max_related_files: usize,
    /// Maximum graph nodes explored per changed-file seed and direction.
    pub max_traversal_nodes_per_seed: usize,
    /// Maximum changed lines emitted per file.
    pub max_changed_lines_per_file: usize,
    /// Maximum snippets emitted per file.
    pub max_snippets_per_file: usize,
    /// Maximum syntax symbols visited per file.
    pub max_symbols_per_file: usize,
    /// Maximum bytes read when searching docs/config references.
    pub max_reference_bytes: usize,
    /// Whether any output collection was truncated.
    pub truncated: bool,
}

#[derive(Debug, Clone)]
struct RawFileDiff {
    path: String,
    old_path: Option<String>,
    status: String,
    old_id: Option<Oid>,
    new_id: Option<Oid>,
    old_lines: BTreeSet<usize>,
    new_lines: BTreeSet<usize>,
    snippets: Vec<RawSnippet>,
    snippets_truncated: bool,
    additions: usize,
    deletions: usize,
    binary: bool,
}

#[derive(Debug, Clone)]
struct RawSnippet {
    side: &'static str,
    line: usize,
    content: String,
}

#[derive(Debug, Clone)]
struct SymbolSpan {
    kind: String,
    name: String,
    start_line: usize,
    end_line: usize,
}

#[derive(Debug, Clone, Copy)]
enum SymbolSide {
    Old,
    New,
}

/// Build an impact pack without printing or writing anything.
pub fn build(options: &ReviewOptions) -> Result<ImpactPackV1> {
    if options.max_related_files == 0 {
        anyhow::bail!("max_related_files must be greater than zero");
    }

    let requested = options
        .path
        .canonicalize()
        .with_context(|| format!("invalid repository path {}", options.path.display()))?;
    if !requested.is_dir() {
        anyhow::bail!("review path is not a directory: {}", requested.display());
    }
    let root = find_repo_root(&requested);
    let repository = Repository::open(&root)
        .with_context(|| format!("{} is not a Git repository", root.display()))?;

    let use_worktree = options.working_tree || options.head.is_none();
    if use_worktree && options.head.is_some() {
        anyhow::bail!("--head cannot be combined with --working-tree");
    }
    let base_label = options.base.clone().unwrap_or_else(|| "HEAD".to_string());
    let head_label = options.head.clone().unwrap_or_else(|| "WORKTREE".to_string());

    if !use_worktree {
        enforce_ref_review_checkout(
            &repository,
            options.head.as_deref().context("head ref is required for a ref-to-ref review")?,
        )?;
    }

    let (raw_diffs, base_oid, head_oid) =
        collect_diffs(&repository, &root, &base_label, options.head.as_deref(), use_worktree)?;

    let mut config = load_config(&root, None)?;
    let _ = resolve_profile(&mut config, &root);
    let ranked_files = current_ranked_files(&root, &config)?;
    let graph = graph::build(&ranked_files);
    let file_by_path: HashMap<String, FileInfo> = ranked_files
        .iter()
        .cloned()
        .map(|file| (normalize_path(&file.relative_path), file))
        .collect();
    let redactor = build_redactor(&config, options.no_redact);

    let mut changed_files = Vec::new();
    let mut related_candidates = BTreeMap::new();
    let mut truncated = raw_diffs.len() > MAX_CHANGED_FILES;
    for raw in raw_diffs.into_iter().take(MAX_CHANGED_FILES) {
        truncated |= raw.changed_lines_truncated() || raw.snippets_truncated;
        let (changed, changed_truncated) = build_changed_file(
            &root,
            &repository,
            &raw,
            use_worktree,
            &graph,
            &file_by_path,
            redactor.as_ref(),
        )?;
        truncated |= changed_truncated;
        truncated |= collect_graph_related(&root, &graph, &changed, &mut related_candidates);
        changed_files.push(changed);
    }

    truncated |= collect_reference_related(&ranked_files, &changed_files, &mut related_candidates);

    let mut related_files: Vec<RelatedFile> = related_candidates.into_values().collect();
    related_files.sort_by(|left, right| {
        left.path.cmp(&right.path).then_with(|| left.relation.cmp(&right.relation))
    });
    if related_files.len() > options.max_related_files {
        related_files.truncate(options.max_related_files);
        truncated = true;
    }

    let summary = summarize(&changed_files, &related_files);
    let limits = TraversalLimits {
        max_changed_files: MAX_CHANGED_FILES,
        max_related_files: options.max_related_files,
        max_traversal_nodes_per_seed: MAX_TRAVERSAL_NODES_PER_SEED,
        max_changed_lines_per_file: MAX_CHANGED_LINES_PER_FILE,
        max_snippets_per_file: MAX_SNIPPETS_PER_FILE,
        max_symbols_per_file: MAX_SYMBOLS_PER_FILE,
        max_reference_bytes: MAX_REFERENCE_BYTES,
        truncated,
    };

    Ok(ImpactPackV1 {
        schema: IMPACT_PACK_SCHEMA,
        schema_version: 1,
        repository: root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("repository")
            .to_string(),
        comparison: ComparisonInfo {
            mode: if use_worktree { "working_tree" } else { "refs" }.to_string(),
            base: base_label,
            head: head_label,
            base_oid,
            head_oid,
        },
        summary,
        changed_files,
        related_files,
        limits,
    })
}

/// Render a pack as stable human-readable text.
#[must_use]
pub fn render_text(pack: &ImpactPackV1) -> String {
    let mut out = String::new();
    out.push_str("Impact pack: ImpactPackV1\n");
    out.push_str(&format!(
        "Comparison: {} ({}) -> {}\n",
        pack.comparison.mode, pack.comparison.base, pack.comparison.head
    ));
    out.push_str(&format!(
        "Changed files: {} | symbols: {} | related: {}\n",
        pack.summary.changed_files, pack.summary.changed_symbols, pack.summary.related_files
    ));
    if pack.limits.truncated {
        out.push_str("Bound: output was truncated; see limits in JSON.\n");
    }

    if pack.changed_files.is_empty() {
        out.push_str("\nNo changes detected.\n");
    } else {
        out.push_str("\nChanged files\n");
        for file in &pack.changed_files {
            out.push_str(&format!(
                "- {} [{}] +{} -{}\n",
                file.path, file.status, file.additions, file.deletions
            ));
            if let Some(old_path) = &file.old_path {
                out.push_str(&format!("  renamed from: {old_path}\n"));
            }
            if !file.symbols.is_empty() {
                out.push_str("  symbols:\n");
                for symbol in &file.symbols {
                    out.push_str(&format!(
                        "    - {} {} ({})\n",
                        symbol.kind, symbol.name, symbol.status
                    ));
                }
            }
            if !file.imports.is_empty() {
                out.push_str(&format!("  imports: {}\n", file.imports.join(", ")));
            }
            if !file.callers.is_empty() {
                out.push_str(&format!("  callers: {}\n", file.callers.join(", ")));
            }
            for reason in &file.reasons {
                out.push_str(&format!("  why: {reason}\n"));
            }
            for snippet in &file.snippets {
                out.push_str(&format!(
                    "  {} {} | {}\n",
                    snippet.side, snippet.line, snippet.content
                ));
            }
        }
    }

    out.push_str("\nRelated files\n");
    if pack.related_files.is_empty() {
        out.push_str("- none\n");
    } else {
        for related in &pack.related_files {
            let distance = related.distance.map(|value| format!(" d={value}")).unwrap_or_default();
            out.push_str(&format!(
                "- {} [{}{}] — {}\n",
                related.path, related.relation, distance, related.reason
            ));
        }
    }
    out.push_str(&format!(
        "\nTests: {} | configs: {} | docs: {} | additions: {} | deletions: {}\n",
        pack.summary.tests,
        pack.summary.configs,
        pack.summary.docs,
        pack.summary.additions,
        pack.summary.deletions
    ));
    out
}

/// Render according to CLI options, writing only the explicitly requested file.
pub fn run(options: ReviewOptions) -> Result<()> {
    let format = options.format;
    let output = options.output.clone();
    let pack = build(&options)?;
    let json = format!("{}\n", serde_json::to_string_pretty(&pack)?);
    let text = render_text(&pack);

    match format {
        ReviewFormat::Text => {
            if let Some(path) = output {
                write_atomic(&path, text.as_bytes())
                    .with_context(|| format!("failed writing review text to {}", path.display()))?;
            } else {
                print!("{text}");
            }
        }
        ReviewFormat::Json => {
            if let Some(path) = output {
                write_atomic(&path, json.as_bytes())
                    .with_context(|| format!("failed writing impact pack to {}", path.display()))?;
            } else {
                print!("{json}");
            }
        }
        ReviewFormat::Both => {
            print!("{text}");
            if let Some(path) = output {
                write_atomic(&path, json.as_bytes())
                    .with_context(|| format!("failed writing impact pack to {}", path.display()))?;
                println!("\nJSON impact pack: {}", path.display());
            } else {
                println!("\n--- ImpactPackV1 JSON ---\n{json}");
            }
        }
    }
    Ok(())
}

fn collect_diffs(
    repository: &Repository,
    root: &Path,
    base_label: &str,
    head_label: Option<&str>,
    use_worktree: bool,
) -> Result<(Vec<RawFileDiff>, String, Option<String>)> {
    let base_object = repository
        .revparse_single(base_label)
        .with_context(|| format!("could not resolve base ref '{base_label}'"))?;
    let base_tree = base_object
        .peel_to_tree()
        .with_context(|| format!("base ref '{base_label}' does not resolve to a tree"))?;
    let base_oid = base_tree.id().to_string();

    let mut options = DiffOptions::new();
    options
        .context_lines(3)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .ignore_submodules(true);

    if use_worktree {
        let diff =
            repository.diff_tree_to_workdir_with_index(Some(&base_tree), Some(&mut options))?;
        return collect_diff_lines(repository, root, diff, true, base_oid, None);
    }

    let head_ref = head_label.context("head ref is required for a ref-to-ref review")?;
    let head_object = repository
        .revparse_single(head_ref)
        .with_context(|| format!("could not resolve head ref '{head_ref}'"))?;
    let head_tree = head_object
        .peel_to_tree()
        .with_context(|| format!("head ref '{head_ref}' does not resolve to a tree"))?;
    let head_oid = head_tree.id().to_string();
    let diff =
        repository.diff_tree_to_tree(Some(&base_tree), Some(&head_tree), Some(&mut options))?;
    collect_diff_lines(repository, root, diff, false, base_oid, Some(head_oid))
}

fn collect_diff_lines(
    repository: &Repository,
    root: &Path,
    diff: git2::Diff<'_>,
    use_worktree: bool,
    base_oid: String,
    head_oid: Option<String>,
) -> Result<(Vec<RawFileDiff>, String, Option<String>)> {
    let mut files = BTreeMap::<String, RawFileDiff>::new();
    for delta in diff.deltas() {
        let old_path = delta.old_file().path().map(display_path);
        let new_path = delta.new_file().path().map(display_path);
        let path = new_path.clone().or_else(|| old_path.clone());
        let Some(path) = path else { continue };
        let old_path_for_output = old_path.filter(|old| old != &path);
        files.insert(
            path.clone(),
            RawFileDiff {
                path,
                old_path: old_path_for_output,
                status: delta_status(delta.status()).to_string(),
                old_id: nonzero_oid(delta.old_file().id()),
                new_id: nonzero_oid(delta.new_file().id()),
                old_lines: BTreeSet::new(),
                new_lines: BTreeSet::new(),
                snippets: Vec::new(),
                snippets_truncated: false,
                additions: 0,
                deletions: 0,
                binary: delta.old_file().is_binary() || delta.new_file().is_binary(),
            },
        );
    }

    diff.print(DiffFormat::Patch, |delta, _hunk, line| {
        let key = delta.new_file().path().or_else(|| delta.old_file().path()).map(display_path);
        let Some(key) = key else { return true };
        let Some(file) = files.get_mut(&key) else { return true };
        match line.origin_value() {
            DiffLineType::Addition => {
                file.additions = file.additions.saturating_add(1);
                if let Some(number) = line.new_lineno().filter(|number| *number > 0) {
                    let number = number as usize;
                    file.new_lines.insert(number);
                    if file.snippets.len() < MAX_SNIPPETS_PER_FILE {
                        file.snippets.push(RawSnippet {
                            side: "added",
                            line: number,
                            content: String::from_utf8_lossy(line.content()).trim_end().to_string(),
                        });
                    } else {
                        file.snippets_truncated = true;
                    }
                }
            }
            DiffLineType::Deletion => {
                file.deletions = file.deletions.saturating_add(1);
                if let Some(number) = line.old_lineno().filter(|number| *number > 0) {
                    let number = number as usize;
                    file.old_lines.insert(number);
                    if file.snippets.len() < MAX_SNIPPETS_PER_FILE {
                        file.snippets.push(RawSnippet {
                            side: "removed",
                            line: number,
                            content: String::from_utf8_lossy(line.content()).trim_end().to_string(),
                        });
                    } else {
                        file.snippets_truncated = true;
                    }
                }
            }
            DiffLineType::Binary => file.binary = true,
            _ => {}
        }
        true
    })?;

    for file in files.values_mut() {
        if file.status == "added" && file.new_lines.is_empty() {
            if let Some(content) =
                side_content(repository, root, file, SymbolSide::New, use_worktree)?
            {
                file.new_lines.extend(1..=line_count(&content));
            }
        }
        if file.status == "deleted" && file.old_lines.is_empty() {
            if let Some(content) =
                side_content(repository, root, file, SymbolSide::Old, use_worktree)?
            {
                file.old_lines.extend(1..=line_count(&content));
            }
        }
    }

    Ok((files.into_values().collect(), base_oid, head_oid))
}

fn build_changed_file(
    root: &Path,
    repository: &Repository,
    raw: &RawFileDiff,
    use_worktree: bool,
    graph: &ImportGraph,
    file_by_path: &HashMap<String, FileInfo>,
    redactor: Option<&Redactor>,
) -> Result<(ChangedFile, bool)> {
    let language = language_for_path(&raw.path);
    let old_content = side_content(repository, root, raw, SymbolSide::Old, use_worktree)?;
    let new_content = side_content(repository, root, raw, SymbolSide::New, use_worktree)?;
    let (old_symbols, old_symbols_truncated) = old_content
        .as_deref()
        .map(|content| extract_symbols_bounded(&language, content))
        .unwrap_or_default();
    let (new_symbols, new_symbols_truncated) = new_content
        .as_deref()
        .map(|content| extract_symbols_bounded(&language, content))
        .unwrap_or_default();
    let mut symbols = changed_symbols(&old_symbols, &new_symbols, &raw.old_lines, &raw.new_lines);
    let changed_symbols_truncated = symbols.len() > MAX_SYMBOLS_PER_FILE;
    symbols.truncate(MAX_SYMBOLS_PER_FILE);

    let (imports, callers) = if let Some(file) = file_by_path.get(&normalize_path(&raw.path)) {
        let path = file.path.canonicalize().unwrap_or_else(|_| file.path.clone());
        let imports = graph
            .edges
            .get(&path)
            .into_iter()
            .flatten()
            .filter_map(|target| graph.files.get(target).map(|file| file.relative_path.clone()))
            .collect::<Vec<_>>();
        let callers = graph::direct_callers(graph, &path)
            .into_iter()
            .filter_map(|caller| graph.files.get(&caller).map(|file| file.relative_path.clone()))
            .collect::<Vec<_>>();
        (sorted_unique(imports), sorted_unique(callers))
    } else {
        (Vec::new(), Vec::new())
    };

    let snippets = raw
        .snippets
        .iter()
        .map(|snippet| DiffSnippet {
            side: snippet.side.to_string(),
            line: snippet.line,
            content: redactor
                .map(|redactor| {
                    let (filename, extension) = file_name_extension(&raw.path);
                    redactor
                        .redact_with_language_report(
                            &snippet.content,
                            &language,
                            &extension,
                            &filename,
                            &raw.path,
                        )
                        .content
                })
                .unwrap_or_else(|| snippet.content.clone()),
        })
        .collect();

    let mut changed_lines = Vec::new();
    for line in raw.old_lines.iter().take(MAX_CHANGED_LINES_PER_FILE) {
        changed_lines.push(ChangedLine { side: "old".to_string(), line: *line });
    }
    let remaining = MAX_CHANGED_LINES_PER_FILE.saturating_sub(changed_lines.len());
    for line in raw.new_lines.iter().take(remaining) {
        changed_lines.push(ChangedLine { side: "new".to_string(), line: *line });
    }

    let mut reasons = vec![format!("file changed ({})", raw.status)];
    if !symbols.is_empty() {
        reasons.push(format!("{} changed symbol(s) overlap the diff", symbols.len()));
    }
    if !imports.is_empty() {
        reasons.push("direct static imports are listed for context".to_string());
    }
    if !callers.is_empty() {
        reasons.push("direct static importers are listed as conservative callers".to_string());
    }
    if raw.binary {
        reasons.push("binary or unreadable content is represented by metadata only".to_string());
    }

    let changed_file = ChangedFile {
        path: raw.path.clone(),
        old_path: raw.old_path.clone(),
        status: raw.status.clone(),
        language,
        old_sha256: old_content.as_deref().map(sha256_text),
        new_sha256: new_content.as_deref().map(sha256_text),
        additions: raw.additions,
        deletions: raw.deletions,
        changed_lines,
        symbols,
        imports,
        callers,
        reasons,
        snippets,
        binary: raw.binary || (old_content.is_none() && new_content.is_none()),
    };
    Ok((changed_file, old_symbols_truncated || new_symbols_truncated || changed_symbols_truncated))
}

impl RawFileDiff {
    fn changed_lines_truncated(&self) -> bool {
        self.old_lines.len().saturating_add(self.new_lines.len()) > MAX_CHANGED_LINES_PER_FILE
    }
}

fn enforce_ref_review_checkout(repository: &Repository, requested_head: &str) -> Result<()> {
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .exclude_submodules(true);
    let statuses = repository.statuses(Some(&mut options))?;
    if !statuses.is_empty() {
        anyhow::bail!(
            "{}",
            concat!(
                "ref-to-ref review requires a clean worktree; found tracked or untracked changes. ",
                "Use --working-tree to review local changes."
            )
        );
    }

    let requested_commit = repository
        .revparse_single(requested_head)
        .with_context(|| format!("could not resolve head ref '{requested_head}'"))?
        .peel_to_commit()
        .with_context(|| {
            format!(
                "head ref '{requested_head}' does not resolve to a commit for ref-to-ref review"
            )
        })?;
    let checked_out_commit = repository
        .head()
        .context("ref-to-ref review requires a checked-out HEAD")?
        .peel_to_commit()
        .context("checked-out HEAD does not resolve to a commit")?;

    if checked_out_commit.id() != requested_commit.id() {
        anyhow::bail!(
            concat!(
                "ref-to-ref review requires checked-out HEAD ({}) to match requested --head ",
                "'{}' ({}). Check out the requested head commit, or use --working-tree to ",
                "review local changes."
            ),
            checked_out_commit.id(),
            requested_head,
            requested_commit.id()
        );
    }

    Ok(())
}

fn current_ranked_files(root: &Path, config: &Config) -> Result<Vec<FileInfo>> {
    let mut scanner = FileScanner::from_config(root.to_path_buf(), config);
    let files = scanner.scan()?;
    rank_files(root, files)
}

fn collect_graph_related(
    root: &Path,
    graph: &ImportGraph,
    changed: &ChangedFile,
    candidates: &mut BTreeMap<(String, String), RelatedFile>,
) -> bool {
    let path = root.join(&changed.path).canonicalize().unwrap_or_else(|_| root.join(&changed.path));
    if !graph.files.contains_key(&path) {
        return false;
    }

    let (dependencies, dependencies_truncated) = bounded_distances(graph, &path, false);
    let (callers, callers_truncated) = bounded_distances(graph, &path, true);
    for (target, distance) in dependencies {
        let Some(file) = graph.files.get(&target) else { continue };
        insert_related(
            candidates,
            RelatedFile {
                path: file.relative_path.clone(),
                relation: "dependency".to_string(),
                distance: Some(distance),
                reason: format!("statically imported by changed file {}", changed.path),
            },
        );
    }
    for (caller, distance) in callers {
        let Some(file) = graph.files.get(&caller) else { continue };
        let is_test = is_test_path(&file.relative_path) || file.tags.contains("test");
        insert_related(
            candidates,
            RelatedFile {
                path: file.relative_path.clone(),
                relation: if is_test { "test" } else { "caller" }.to_string(),
                distance: Some(distance),
                reason: if is_test {
                    format!("test file statically imports changed file {}", changed.path)
                } else {
                    format!("statically imports changed file {}", changed.path)
                },
            },
        );
    }
    dependencies_truncated || callers_truncated
}

fn collect_reference_related(
    files: &[FileInfo],
    changed_files: &[ChangedFile],
    candidates: &mut BTreeMap<(String, String), RelatedFile>,
) -> bool {
    if changed_files.is_empty() {
        return false;
    }
    let mut truncated = files.len() > MAX_REFERENCE_CANDIDATES;
    let changed_paths: HashSet<&str> =
        changed_files.iter().map(|file| file.path.as_str()).collect();
    let changed_stems: Vec<String> = changed_files
        .iter()
        .filter_map(|file| {
            Path::new(&file.path)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(|stem| stem.to_ascii_lowercase())
        })
        .collect();
    let changed_symbols: Vec<String> = changed_files
        .iter()
        .flat_map(|file| file.symbols.iter().map(|symbol| symbol.name.to_ascii_lowercase()))
        .collect();

    let mut sorted_files = files.to_vec();
    sorted_files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    for file in sorted_files.iter().take(MAX_REFERENCE_CANDIDATES) {
        let path = normalize_path(&file.relative_path);
        if changed_paths.contains(path.as_str()) {
            continue;
        }
        let is_test = file.tags.contains("test") || is_test_path(&path);
        let is_config = file.is_config || is_config_path(&path);
        let is_doc = file.is_doc || file.is_readme || is_documentation_path(&path);
        if !is_test && !is_config && !is_doc {
            continue;
        }
        if fs::metadata(&file.path)
            .map(|metadata| metadata.len() > MAX_REFERENCE_BYTES as u64)
            .unwrap_or(false)
        {
            truncated = true;
        }
        let content = read_file_safe(&file.path, Some(MAX_REFERENCE_BYTES), None)
            .map(|(content, _)| content.to_ascii_lowercase())
            .unwrap_or_default();
        let mentions_change = changed_files.iter().any(|changed| {
            content.contains(&changed.path.to_ascii_lowercase())
                || changed
                    .symbols
                    .iter()
                    .any(|symbol| content.contains(&symbol.name.to_ascii_lowercase()))
        });
        let stem_match = changed_stems.iter().any(|stem| path.contains(stem));
        let symbol_match = changed_symbols.iter().any(|symbol| content.contains(symbol.as_str()));

        let (relation, reason) = if is_test && (mentions_change || stem_match || symbol_match) {
            ("test", "test path or content references a changed file or symbol".to_string())
        } else if is_config && (mentions_change || path_is_root_config(&path)) {
            ("config", "repository configuration is a nearby build or runtime anchor".to_string())
        } else if is_doc && (mentions_change || file.is_readme) {
            (
                "documentation",
                "documentation references the changed area or is the repository overview"
                    .to_string(),
            )
        } else {
            continue;
        };
        insert_related(
            candidates,
            RelatedFile { path, relation: relation.to_string(), distance: None, reason },
        );
    }
    truncated
}

fn insert_related(candidates: &mut BTreeMap<(String, String), RelatedFile>, related: RelatedFile) {
    let key = (related.path.clone(), related.relation.clone());
    candidates.entry(key).or_insert(related);
}

fn bounded_distances(
    graph: &ImportGraph,
    start: &Path,
    reverse: bool,
) -> (Vec<(PathBuf, usize)>, bool) {
    let mut output = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([(start.to_path_buf(), 0usize)]);
    while let Some((path, distance)) = queue.pop_front() {
        if !seen.insert(path.clone()) {
            continue;
        }
        if distance > 0 {
            output.push((path.clone(), distance));
            if output.len() > MAX_TRAVERSAL_NODES_PER_SEED {
                output.truncate(MAX_TRAVERSAL_NODES_PER_SEED);
                return (output, true);
            }
        }
        let mut next = if reverse {
            graph.reverse.get(&path).cloned().unwrap_or_default()
        } else {
            graph.edges.get(&path).cloned().unwrap_or_default()
        };
        next.sort();
        next.dedup();
        for target in next {
            if graph.files.contains_key(&target) && !seen.contains(&target) {
                queue.push_back((target, distance.saturating_add(1)));
            }
        }
    }
    (output, false)
}

fn summarize(changed_files: &[ChangedFile], related_files: &[RelatedFile]) -> Summary {
    Summary {
        changed_files: changed_files.len(),
        changed_symbols: changed_files.iter().map(|file| file.symbols.len()).sum(),
        related_files: related_files.len(),
        tests: related_files.iter().filter(|file| file.relation == "test").count(),
        configs: related_files.iter().filter(|file| file.relation == "config").count(),
        docs: related_files.iter().filter(|file| file.relation == "documentation").count(),
        additions: changed_files.iter().map(|file| file.additions).sum(),
        deletions: changed_files.iter().map(|file| file.deletions).sum(),
    }
}

fn side_content(
    repository: &Repository,
    root: &Path,
    file: &RawFileDiff,
    side: SymbolSide,
    use_worktree: bool,
) -> Result<Option<String>> {
    let (path, oid) = match side {
        SymbolSide::Old => (file.old_path.as_deref().unwrap_or(&file.path), file.old_id),
        SymbolSide::New => (file.path.as_str(), file.new_id),
    };
    if use_worktree && matches!(side, SymbolSide::New) {
        let path = root.join(path);
        if !path.is_file() {
            return Ok(None);
        }
        let metadata = fs::metadata(&path)?;
        if metadata.len() as usize > MAX_REVIEW_SOURCE_BYTES {
            return Ok(None);
        }
        return Ok(read_file_safe(&path, Some(MAX_REVIEW_SOURCE_BYTES), None)
            .ok()
            .map(|(content, _)| content));
    }
    let Some(oid) = oid else { return Ok(None) };
    let blob = repository.find_blob(oid)?;
    if blob.size() > MAX_REVIEW_SOURCE_BYTES {
        return Ok(None);
    }
    let bytes = blob.content();
    if bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(bytes).into_owned()))
}

fn extract_symbols_bounded(language: &str, content: &str) -> (Vec<SymbolSpan>, bool) {
    let mut symbols = match language {
        "python" => tree_symbols(
            content,
            tree_sitter_python::LANGUAGE.into(),
            &["function_definition", "class_definition"],
        ),
        "rust" => tree_symbols(
            content,
            tree_sitter_rust::LANGUAGE.into(),
            &["function_item", "impl_item", "struct_item", "enum_item", "trait_item", "mod_item"],
        ),
        "javascript" => tree_symbols(
            content,
            tree_sitter_javascript::LANGUAGE.into(),
            &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "lexical_declaration",
            ],
        ),
        "typescript" => tree_symbols(
            content,
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            &[
                "function_declaration",
                "class_declaration",
                "method_definition",
                "interface_declaration",
                "type_alias_declaration",
                "lexical_declaration",
            ],
        ),
        "go" => tree_symbols(
            content,
            tree_sitter_go::LANGUAGE.into(),
            &[
                "function_declaration",
                "method_declaration",
                "type_declaration",
                "const_declaration",
                "var_declaration",
            ],
        ),
        "gdscript" => gdscript_symbols(content),
        _ => Vec::new(),
    };
    symbols.sort_by(|left, right| {
        left.start_line
            .cmp(&right.start_line)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.name.cmp(&right.name))
    });
    let truncated = symbols.len() > MAX_SYMBOLS_PER_FILE;
    symbols.truncate(MAX_SYMBOLS_PER_FILE);
    (symbols, truncated)
}

fn tree_symbols(
    content: &str,
    language_impl: tree_sitter::Language,
    definition_kinds: &[&str],
) -> Vec<SymbolSpan> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language_impl).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(content, None) else { return Vec::new() };
    let mut output = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if output.len() > MAX_SYMBOLS_PER_FILE {
            break;
        }
        if definition_kinds.contains(&node.kind()) && output.len() <= MAX_SYMBOLS_PER_FILE {
            let kind = symbol_kind(node.kind()).to_string();
            let name = node
                .child_by_field_name("name")
                .and_then(|name| name.utf8_text(content.as_bytes()).ok())
                .map(clean_symbol)
                .filter(|name| !name.is_empty())
                .or_else(|| first_identifier(content, node));
            if let Some(name) = name {
                output.push(SymbolSpan {
                    kind,
                    name,
                    start_line: node.start_position().row.saturating_add(1),
                    end_line: node.end_position().row.saturating_add(1),
                });
            }
        }
        for index in (0..node.named_child_count()).rev() {
            if let Some(child) = node.named_child(index) {
                stack.push(child);
            }
        }
    }
    output
}

fn gdscript_symbols(content: &str) -> Vec<SymbolSpan> {
    let mut starts = Vec::<(usize, String, String)>::new();
    for (index, line) in content.lines().enumerate() {
        if line.chars().next().is_some_and(|character| character.is_whitespace()) {
            continue;
        }
        let trimmed = line.trim();
        let (kind, rest) = if let Some(rest) = trimmed.strip_prefix("static func ") {
            ("function", rest)
        } else if let Some(rest) = trimmed.strip_prefix("func ") {
            ("function", rest)
        } else if let Some(rest) = trimmed.strip_prefix("class_name ") {
            ("class", rest)
        } else if let Some(rest) = trimmed.strip_prefix("class ") {
            ("class", rest)
        } else if let Some(rest) = trimmed.strip_prefix("signal ") {
            ("signal", rest)
        } else {
            continue;
        };
        let name = rest.split(['(', ':', ' ', '=']).next().map(str::trim).unwrap_or_default();
        if !name.is_empty() {
            starts.push((index + 1, kind.to_string(), name.to_string()));
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(index, (start_line, kind, name))| SymbolSpan {
            kind: kind.clone(),
            name: name.clone(),
            start_line: *start_line,
            end_line: starts
                .get(index + 1)
                .map(|next| next.0.saturating_sub(1))
                .unwrap_or_else(|| content.lines().count().max(*start_line)),
        })
        .collect()
}

fn changed_symbols(
    old_symbols: &[SymbolSpan],
    new_symbols: &[SymbolSpan],
    old_lines: &BTreeSet<usize>,
    new_lines: &BTreeSet<usize>,
) -> Vec<ChangedSymbol> {
    let mut old_by_key = BTreeMap::<(String, String), SymbolSpan>::new();
    let mut new_by_key = BTreeMap::<(String, String), SymbolSpan>::new();
    for symbol in old_symbols.iter().filter(|symbol| overlaps(symbol, old_lines)) {
        old_by_key.insert((symbol.kind.clone(), symbol.name.clone()), symbol.clone());
    }
    for symbol in new_symbols.iter().filter(|symbol| overlaps(symbol, new_lines)) {
        new_by_key.insert((symbol.kind.clone(), symbol.name.clone()), symbol.clone());
    }

    let keys: BTreeSet<(String, String)> =
        old_by_key.keys().chain(new_by_key.keys()).cloned().collect();
    keys.into_iter()
        .map(|(kind, name)| {
            let old = old_by_key.get(&(kind.clone(), name.clone()));
            let new = new_by_key.get(&(kind.clone(), name.clone()));
            ChangedSymbol {
                kind,
                name,
                status: match (old.is_some(), new.is_some()) {
                    (true, true) => "changed",
                    (true, false) => "removed",
                    (false, true) => "added",
                    (false, false) => unreachable!("symbol key must have a side"),
                }
                .to_string(),
                old_start_line: old.map(|symbol| symbol.start_line),
                old_end_line: old.map(|symbol| symbol.end_line),
                new_start_line: new.map(|symbol| symbol.start_line),
                new_end_line: new.map(|symbol| symbol.end_line),
            }
        })
        .collect()
}

fn overlaps(symbol: &SymbolSpan, lines: &BTreeSet<usize>) -> bool {
    lines.range(symbol.start_line..=symbol.end_line).next().is_some()
}

fn first_identifier(content: &str, node: tree_sitter::Node<'_>) -> Option<String> {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind().contains("identifier") {
            if let Ok(text) = current.utf8_text(content.as_bytes()) {
                let clean = clean_symbol(text);
                if !clean.is_empty() {
                    return Some(clean);
                }
            }
        }
        for index in (0..current.named_child_count()).rev() {
            if let Some(child) = current.named_child(index) {
                stack.push(child);
            }
        }
    }
    None
}

fn clean_symbol(value: &str) -> String {
    value
        .trim()
        .trim_matches([':', ';', '(', ')', '{', '}', '[', ']'])
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
}

fn symbol_kind(kind: &str) -> String {
    match kind {
        "function_definition" | "function_item" | "function_declaration" => "function",
        "method_definition" | "method_declaration" => "method",
        "class_definition" | "class_declaration" => "class",
        "struct_item"
        | "enum_item"
        | "trait_item"
        | "interface_declaration"
        | "type_declaration"
        | "type_alias_declaration" => "type",
        "impl_item" => "impl",
        "mod_item" => "module",
        "lexical_declaration" | "const_declaration" | "var_declaration" => "binding",
        other => other,
    }
    .to_string()
}

fn build_redactor(config: &Config, no_redact: bool) -> Option<Redactor> {
    if no_redact || !config.redact_secrets {
        return None;
    }
    let (entropy, paranoid, structure_safe) = match config.redaction_mode {
        RedactionMode::Fast => (false, false, false),
        RedactionMode::Standard => (true, false, false),
        RedactionMode::Paranoid => (true, true, false),
        RedactionMode::StructureSafe => (true, false, true),
    };
    Some(Redactor::from_config(entropy, paranoid, structure_safe, &config.redaction))
}

fn language_for_path(path: &str) -> String {
    let path = Path::new(path);
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{extension}"))
        .unwrap_or_default();
    let filename = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    get_language(&extension, filename)
}

fn file_name_extension(path: &str) -> (String, String) {
    let path = Path::new(path);
    (
        path.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_string(),
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!(".{extension}"))
            .unwrap_or_default(),
    )
}

fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower
        .split('/')
        .any(|part| part == "test" || part == "tests" || part == "spec" || part == "specs")
        || lower.contains("_test.")
        || lower.contains(".test.")
        || lower.contains("_spec.")
        || lower.contains(".spec.")
}

fn is_config_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = Path::new(&lower).file_name().and_then(|name| name.to_str()).unwrap_or_default();
    matches!(
        name,
        "cargo.toml"
            | "go.mod"
            | "go.work"
            | "package.json"
            | "tsconfig.json"
            | "pyproject.toml"
            | "setup.cfg"
            | "tox.ini"
            | "makefile"
            | "dockerfile"
            | "project.godot"
            | "justfile"
            | "taskfile.yml"
            | "taskfile.yaml"
    ) || lower.starts_with(".github/workflows/")
}

fn is_documentation_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".md")
        || lower.ends_with(".rst")
        || lower.ends_with(".adoc")
        || lower.starts_with("docs/")
}

fn path_is_root_config(path: &str) -> bool {
    !path.contains('/') && is_config_path(path)
}

fn line_count(content: &str) -> usize {
    content.lines().count().max(1)
}

fn sha256_text(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

fn sorted_unique(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

fn display_path(path: &Path) -> String {
    normalize_path(path.to_string_lossy().as_ref())
}

fn nonzero_oid(oid: Oid) -> Option<Oid> {
    (!oid.is_zero()).then_some(oid)
}

fn delta_status(status: Delta) -> &'static str {
    match status {
        Delta::Added | Delta::Untracked => "added",
        Delta::Deleted => "deleted",
        Delta::Renamed => "renamed",
        Delta::Copied => "copied",
        Delta::Typechange => "type_changed",
        Delta::Unreadable => "unreadable",
        Delta::Conflicted => "conflicted",
        Delta::Ignored => "ignored",
        Delta::Modified => "modified",
        Delta::Unmodified => "unmodified",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bounded_distances, changed_symbols, collect_reference_related, extract_symbols_bounded,
        ChangedFile, MAX_REFERENCE_CANDIDATES, MAX_TRAVERSAL_NODES_PER_SEED,
    };
    use crate::domain::FileInfo;
    use crate::module::graph::ImportGraph;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    fn file_info(path: &str) -> FileInfo {
        FileInfo {
            path: PathBuf::from(path),
            relative_path: path.to_string(),
            size_bytes: 0,
            extension: ".rs".to_string(),
            language: "rust".to_string(),
            id: path.to_string(),
            priority: 0.0,
            token_estimate: 0,
            tags: BTreeSet::new(),
            is_readme: false,
            is_config: false,
            is_doc: false,
        }
    }

    fn changed_file() -> ChangedFile {
        ChangedFile {
            path: "src/changed.rs".to_string(),
            old_path: None,
            status: "modified".to_string(),
            language: "rust".to_string(),
            old_sha256: None,
            new_sha256: None,
            additions: 0,
            deletions: 0,
            changed_lines: Vec::new(),
            symbols: Vec::new(),
            imports: Vec::new(),
            callers: Vec::new(),
            reasons: Vec::new(),
            snippets: Vec::new(),
            binary: false,
        }
    }

    #[test]
    fn symbol_extractors_cover_representative_languages() {
        let fixtures = [
            ("rust", include_str!("../tests/fixtures/review/rust/lib.rs"), "refresh_token"),
            ("go", include_str!("../tests/fixtures/review/go/auth.go"), "RefreshToken"),
            (
                "typescript",
                include_str!("../tests/fixtures/review/typescript/client.ts"),
                "refreshToken",
            ),
            ("python", include_str!("../tests/fixtures/review/python/auth.py"), "refresh_token"),
            (
                "gdscript",
                include_str!("../tests/fixtures/review/godot/scripts/auth.gd"),
                "refresh_token",
            ),
        ];
        for (language, content, expected) in fixtures {
            let symbols = extract_symbols_bounded(language, content).0;
            assert!(
                symbols.iter().any(|symbol| symbol.name == expected),
                "{language} symbols: {symbols:?}"
            );
        }
    }

    #[test]
    fn changed_symbol_matching_is_limited_to_diff_spans() {
        let old = extract_symbols_bounded("rust", "fn stable() {}\nfn changed() {\n  1\n}\n").0;
        let new = extract_symbols_bounded("rust", "fn stable() {}\nfn changed() {\n  2\n}\n").0;
        let lines = BTreeSet::from([3]);
        let symbols = changed_symbols(&old, &new, &lines, &lines);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "changed");
    }

    #[test]
    fn reference_related_matches_symbol_mentions_case_insensitively() {
        use crate::review::ChangedSymbol;
        use std::fs;
        use tempfile::TempDir;

        let temp = TempDir::new().expect("temp");
        let doc = temp.path().join("docs/guide.md");
        fs::create_dir_all(temp.path().join("docs")).expect("mkdir docs");
        // The document mentions the changed symbol in mixed case.
        fs::write(&doc, "# Guide\n\nUses RefreshTokenProvider in this flow.\n").expect("write doc");
        let mut file = file_info(&doc.to_string_lossy());
        file.is_doc = true;
        let mut changed = changed_file();
        changed.symbols = vec![ChangedSymbol {
            kind: "type".to_string(),
            name: "RefreshTokenProvider".to_string(),
            status: "added".to_string(),
            old_start_line: None,
            old_end_line: None,
            new_start_line: Some(1),
            new_end_line: Some(10),
        }];
        let mut candidates = BTreeMap::new();

        collect_reference_related(&[file], &[changed], &mut candidates);

        assert!(
            candidates.values().any(|related| related.relation == "documentation"),
            "mixed-case symbol mention in a doc must be detected, got: {candidates:?}"
        );
    }

    #[test]
    fn reference_and_traversal_bounds_report_truncation() {
        let files = (0..=MAX_REFERENCE_CANDIDATES)
            .map(|index| file_info(&format!("/synthetic/{index}.rs")))
            .collect::<Vec<_>>();
        let mut candidates = BTreeMap::new();
        assert!(collect_reference_related(&files, &[changed_file()], &mut candidates));

        let start = PathBuf::from("/synthetic/start.rs");
        let mut graph = ImportGraph::default();
        let mut previous = start.clone();
        for index in 0..=MAX_TRAVERSAL_NODES_PER_SEED {
            let current = PathBuf::from(format!("/synthetic/dependency-{index}.rs"));
            graph.files.insert(current.clone(), file_info(&current.to_string_lossy()));
            graph.edges.entry(previous).or_default().push(current.clone());
            previous = current;
        }
        let (related, truncated) = bounded_distances(&graph, &start, false);
        assert_eq!(related.len(), MAX_TRAVERSAL_NODES_PER_SEED);
        assert!(truncated);
    }
}

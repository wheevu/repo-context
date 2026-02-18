//! Info command implementation

use anyhow::Result;
use clap::Args;
use std::collections::HashSet;
use std::path::PathBuf;

use super::utils::parse_csv;
use crate::chunk::code_chunker::supported_tree_sitter_languages;
use crate::rank::rank_files;
use crate::scan::scanner::FileScanner;
use crate::scan::tree::generate_tree;
use crate::utils::format_with_commas;

#[derive(Args)]
pub struct InfoArgs {
    /// Local directory path to analyze
    #[arg(value_name = "PATH")]
    pub path: PathBuf,

    /// Include only these extensions (comma-separated)
    #[arg(short = 'i', long, value_name = "EXTS")]
    pub include_ext: Option<String>,

    /// Exclude paths matching these globs (comma-separated)
    #[arg(short = 'e', long, value_name = "GLOBS")]
    pub exclude_glob: Option<String>,

    /// Skip files larger than this (bytes)
    #[arg(long, value_name = "BYTES")]
    pub max_file_bytes: Option<u64>,

    /// Ignore .gitignore rules
    #[arg(long)]
    pub no_gitignore: bool,

    /// Follow symbolic links when scanning
    #[arg(long)]
    pub follow_symlinks: bool,

    /// Include minified/bundled files
    #[arg(long)]
    pub include_minified: bool,
}

pub fn run(args: InfoArgs) -> Result<()> {
    let root = args.path.canonicalize()?;
    if !root.is_dir() {
        anyhow::bail!("Path is not a directory: {}", root.display());
    }

    let include_ext = parse_csv(&args.include_ext);
    let exclude_glob = parse_csv(&args.exclude_glob);

    let mut scanner = FileScanner::new(root.clone())
        .max_file_bytes(args.max_file_bytes.unwrap_or(1_048_576))
        .respect_gitignore(!args.no_gitignore)
        .follow_symlinks(args.follow_symlinks)
        .skip_minified(!args.include_minified);

    if let Some(extensions) = include_ext {
        scanner = scanner.include_extensions(extensions);
    }
    if let Some(globs) = exclude_glob {
        scanner = scanner.exclude_globs(globs);
    }

    let scanned_files = scanner.scan()?;
    let stats = scanner.stats().clone();

    let ranked_files = rank_files(&root, scanned_files)?;

    // Repository name (just the directory name, matching Python's path.name)
    let repo_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("");
    println!("Repository: {}", repo_name);

    // Languages detected (matching Python cli.py:762-765)
    if !stats.languages_detected.is_empty() {
        let mut langs: Vec<_> = stats.languages_detected.iter().collect();
        langs.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        println!("Languages detected:");
        for (lang, count) in langs {
            println!("  {}: {} files", lang, count);
        }
    }

    // Entrypoints (matching Python cli.py:767-770)
    let entrypoints: Vec<&str> = ranked_files
        .iter()
        .filter(|f| f.tags.contains("entrypoint"))
        .map(|f| f.relative_path.as_str())
        .collect();
    if !entrypoints.is_empty() {
        println!("Entrypoints:");
        for ep in &entrypoints {
            println!("  {}", ep);
        }
    }

    // Top priority files — unconditional top 10 (matching Python cli.py:772-778)
    let top_files: Vec<_> = ranked_files.iter().take(10).collect();
    if !top_files.is_empty() {
        println!("Top priority files:");
        for f in &top_files {
            println!("  {} ({}%)", f.relative_path, (f.priority * 100.0).round() as u64);
        }
    }

    // Statistics block (matching Python cli.py:779-787)
    println!("Statistics:");
    println!("  Total files scanned: {}", stats.files_scanned);
    println!("  Files included: {}", stats.files_included);
    println!("  Files skipped (size): {}", stats.files_skipped_size);
    println!("  Files skipped (binary): {}", stats.files_skipped_binary);
    println!("  Files skipped (extension): {}", stats.files_skipped_extension);
    println!("  Files skipped (gitignore): {}", stats.files_skipped_gitignore);
    println!("  Total bytes: {}", format_with_commas(stats.total_bytes_included));
    println!("  Tree-sitter languages: {}", supported_tree_sitter_languages().join(", "));

    // Directory tree with top-10 files highlighted
    let highlighted: HashSet<String> =
        ranked_files.iter().take(10).map(|f| f.relative_path.clone()).collect();
    let tree = generate_tree(&root, 4, true, &highlighted)?;
    println!("\n{}", tree);

    Ok(())
}

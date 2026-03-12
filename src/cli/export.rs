//! Export command implementation.

use anyhow::Result;
use clap::Args;
use std::path::PathBuf;

use super::utils::parse_csv;
use crate::app::export::{execute, ExportExecutionOptions};
use crate::config::{load_config, merge_cli_with_config, CliOverrides};
use crate::domain::{OutputMode, RedactionMode};

#[derive(Args)]
pub struct ExportArgs {
    /// Local directory path to export.
    #[arg(short, long, value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// GitHub repository URL to clone and export.
    #[arg(short = 'r', long, value_name = "URL")]
    pub repo: Option<String>,

    /// Git ref (branch/tag/SHA) when using --repo.
    #[arg(long, value_name = "REF")]
    pub ref_: Option<String>,

    /// Path to config file (repo-context.toml or .r2p.yml).
    #[arg(short = 'c', long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Include only these extensions (comma-separated, e.g., '.py,.ts').
    #[arg(short = 'i', long, value_name = "EXTS")]
    pub include_ext: Option<String>,

    /// Exclude paths matching these globs (comma-separated).
    #[arg(short = 'e', long, value_name = "GLOBS")]
    pub exclude_glob: Option<String>,

    /// Skip files larger than this (bytes).
    #[arg(long, value_name = "BYTES")]
    pub max_file_bytes: Option<u64>,

    /// Stop after exporting this many bytes total.
    #[arg(long, value_name = "BYTES")]
    pub max_total_bytes: Option<u64>,

    /// Ignore .gitignore rules.
    #[arg(long)]
    pub no_gitignore: bool,

    /// Follow symbolic links when scanning.
    #[arg(long)]
    pub follow_symlinks: bool,

    /// Include minified/bundled files.
    #[arg(long)]
    pub include_minified: bool,

    /// Maximum tokens in output.
    #[arg(short = 't', long, value_name = "TOKENS")]
    pub max_tokens: Option<usize>,

    /// Target tokens per chunk.
    #[arg(long, value_name = "TOKENS")]
    pub chunk_tokens: Option<usize>,

    /// Overlap tokens between adjacent chunks.
    #[arg(long, value_name = "TOKENS")]
    pub chunk_overlap: Option<usize>,

    /// Coalesce chunks smaller than this.
    #[arg(long, value_name = "TOKENS")]
    pub min_chunk_tokens: Option<usize>,

    /// Output format: 'prompt', 'rag', or 'both'.
    #[arg(short = 'm', long, value_name = "MODE")]
    pub mode: Option<String>,

    /// Directory for output files.
    #[arg(short = 'o', long, value_name = "DIR")]
    pub output_dir: Option<PathBuf>,

    /// Omit timestamps for reproducible diffs.
    #[arg(long)]
    pub no_timestamp: bool,

    /// Max depth for directory tree in output.
    #[arg(long, value_name = "DEPTH")]
    pub tree_depth: Option<usize>,

    /// Disable automatic secret/credential redaction.
    #[arg(long)]
    pub no_redact: bool,

    /// Redaction mode: fast|standard|paranoid|structure-safe.
    #[arg(long, value_name = "MODE")]
    pub redaction_mode: Option<String>,
}

pub fn run(args: ExportArgs) -> Result<()> {
    if args.path.is_some() && args.repo.is_some() {
        anyhow::bail!("Cannot specify both --path and --repo");
    }

    let cwd = std::env::current_dir()?;
    let config_anchor = match args.path.as_ref() {
        Some(path) if path.exists() => path.canonicalize().unwrap_or_else(|_| cwd.clone()),
        _ => cwd.clone(),
    };

    let file_config = load_config(&config_anchor, args.config.as_deref())?;
    let include_ext = parse_csv(&args.include_ext).map(|v| v.into_iter().collect());
    let exclude_glob = parse_csv(&args.exclude_glob).map(|v| v.into_iter().collect());
    let mode = if args.mode.is_some() { Some(parse_mode(args.mode.as_deref())?) } else { None };
    let redaction_mode = if args.redaction_mode.is_some() {
        Some(parse_redaction_mode(args.redaction_mode.as_deref())?)
    } else {
        None
    };

    let cli_overrides = CliOverrides {
        path: args.path,
        repo_url: args.repo,
        ref_: args.ref_,
        include_extensions: include_ext,
        exclude_globs: exclude_glob,
        max_file_bytes: args.max_file_bytes,
        max_total_bytes: args.max_total_bytes,
        respect_gitignore: if args.no_gitignore { Some(false) } else { None },
        follow_symlinks: if args.follow_symlinks { Some(true) } else { None },
        skip_minified: if args.include_minified { Some(false) } else { None },
        max_tokens: args.max_tokens,
        chunk_tokens: args.chunk_tokens,
        chunk_overlap: args.chunk_overlap,
        min_chunk_tokens: args.min_chunk_tokens,
        mode,
        output_dir: args.output_dir,
        tree_depth: args.tree_depth,
        redact_secrets: if args.no_redact { Some(false) } else { None },
        redaction_mode,
    };

    let merged = merge_cli_with_config(file_config, cli_overrides);

    if merged.path.is_none() && merged.repo_url.is_none() {
        anyhow::bail!("Either --path or --repo must be specified");
    }

    let outcome =
        execute(merged, ExportExecutionOptions { include_timestamp: !args.no_timestamp })?;

    println!("Export complete:");
    println!("  root: {}", outcome.root_path.display());
    println!("  files: {}", outcome.stats.files_included);
    println!("  chunks: {}", outcome.stats.chunks_created);
    println!("  tokens: {}", outcome.stats.total_tokens_estimated);
    for file in outcome.output_files {
        println!("  wrote: {}", file);
    }

    Ok(())
}

fn parse_mode(mode: Option<&str>) -> Result<OutputMode> {
    match mode.unwrap_or("both").trim().to_ascii_lowercase().as_str() {
        "prompt" => Ok(OutputMode::Prompt),
        "rag" => Ok(OutputMode::Rag),
        "both" => Ok(OutputMode::Both),
        other => anyhow::bail!("Invalid mode '{other}'. Expected one of: prompt, rag, both"),
    }
}

fn parse_redaction_mode(mode: Option<&str>) -> Result<RedactionMode> {
    match mode
        .unwrap_or("standard")
        .trim()
        .to_ascii_lowercase()
        .replace('_', "-")
        .as_str()
    {
        "fast" => Ok(RedactionMode::Fast),
        "standard" => Ok(RedactionMode::Standard),
        "paranoid" => Ok(RedactionMode::Paranoid),
        "structure-safe" => Ok(RedactionMode::StructureSafe),
        other => anyhow::bail!(
            "Invalid redaction mode '{other}'. Expected one of: fast, standard, paranoid, structure-safe"
        ),
    }
}

//! Local index command.

use anyhow::Result;
use clap::Args;
use std::path::PathBuf;

use crate::app::export::build_index;
use crate::config::{load_config, merge_cli_with_config, CliOverrides};
use crate::domain::ProjectProfile;
use crate::index::default_index_path;

#[derive(Args)]
pub struct IndexArgs {
    /// Local repository path to index.
    #[arg(short, long, value_name = "PATH")]
    pub path: PathBuf,

    /// SQLite database path. Defaults to the user cache.
    #[arg(long, value_name = "FILE")]
    pub db: Option<PathBuf>,

    /// Path to an explicit TOML config file.
    #[arg(short = 'c', long, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// Repository profile: auto|generic|godot.
    #[arg(long, value_name = "PROFILE")]
    pub profile: Option<String>,

    /// Ignore .gitignore rules.
    #[arg(long)]
    pub no_gitignore: bool,

    /// Follow symbolic links when scanning.
    #[arg(long)]
    pub follow_symlinks: bool,

    /// Include minified/bundled files.
    #[arg(long)]
    pub include_minified: bool,
}

pub fn run(args: IndexArgs) -> Result<()> {
    let path = args.path.canonicalize().map_err(|error| {
        anyhow::anyhow!("invalid repository path {}: {error}", args.path.display())
    })?;
    if !path.is_dir() {
        anyhow::bail!("index path is not a directory: {}", path.display());
    }

    let file_config = load_config(&path, args.config.as_deref())?;
    let profile = args.profile.as_deref().map(parse_profile).transpose()?;
    let config = merge_cli_with_config(
        file_config,
        CliOverrides {
            path: Some(path.clone()),
            repo_url: None,
            ref_: None,
            profile,
            include_extensions: None,
            exclude_globs: None,
            max_file_bytes: None,
            max_total_bytes: None,
            respect_gitignore: if args.no_gitignore { Some(false) } else { None },
            follow_symlinks: if args.follow_symlinks { Some(true) } else { None },
            skip_minified: if args.include_minified { Some(false) } else { None },
            max_tokens: None,
            chunk_tokens: None,
            chunk_overlap: None,
            min_chunk_tokens: None,
            mode: None,
            output_dir: None,
            tree_depth: None,
            redact_secrets: Some(true),
            redaction_mode: None,
        },
    );

    let db = args
        .db
        .or_else(|| default_index_path(&path))
        .ok_or_else(|| anyhow::anyhow!("could not resolve a user cache directory; pass --db"))?;
    let refresh = build_index(config, &db)?;
    println!("Index refreshed:");
    println!("  database: {}", db.display());
    println!("  reused files: {}", refresh.reused_files);
    println!("  updated files: {}", refresh.updated_files);
    println!("  removed files: {}", refresh.removed_files);
    println!("  chunks: {}", refresh.indexed_chunks);
    Ok(())
}

fn parse_profile(profile: &str) -> Result<ProjectProfile> {
    match profile.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok(ProjectProfile::Auto),
        "generic" => Ok(ProjectProfile::Generic),
        "godot" => Ok(ProjectProfile::Godot),
        other => anyhow::bail!("Invalid profile '{other}'. Expected one of: auto, generic, godot"),
    }
}

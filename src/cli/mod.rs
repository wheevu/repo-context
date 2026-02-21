//! Command-line interface for repo-context
//!
//! Provides `export`, `index`, and `info` subcommands.

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing::Level;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

mod codeintel;
mod diff;
mod export;
mod guided;
mod index;
mod info;
mod query;
mod utils;

/// Convert repositories into LLM-friendly context packs
#[derive(Parser)]
#[command(name = "repo-context")]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose logging (sets log level to DEBUG)
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Export a repository as an LLM-friendly context pack
    Export(Box<export::ExportArgs>),

    /// Display repository information without exporting
    Info(info::InfoArgs),

    /// Build a local SQLite index for query-time retrieval
    Index(index::IndexArgs),

    /// Query a local SQLite index for task-relevant chunks
    Query(query::QueryArgs),

    /// Export portable code-intel JSON (SCIP-lite)
    Codeintel(codeintel::CodeIntelArgs),

    /// Compare two export outputs and show structural diffs
    Diff(diff::DiffArgs),
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();

    // Wire verbose flag to the tracing log level.
    // RUST_LOG in the environment always takes precedence; --verbose falls back to DEBUG.
    let filter = if cli.verbose {
        EnvFilter::from_default_env().add_directive(Level::DEBUG.into())
    } else {
        EnvFilter::from_default_env().add_directive(Level::WARN.into())
    };
    let _ = tracing_subscriber::registry()
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(filter)
        .try_init();

    match cli.command {
        Commands::Export(args) => export::run(*args),
        Commands::Info(args) => info::run(args),
        Commands::Index(args) => index::run(args),
        Commands::Query(args) => query::run(args),
        Commands::Codeintel(args) => codeintel::run(args),
        Commands::Diff(args) => diff::run(args),
    }
}

//! Review command implementation.

use anyhow::Result;
use clap::Args;
use std::path::PathBuf;

use crate::review::{self, ReviewFormat, ReviewOptions};

/// Build a deterministic change-aware impact pack.
#[derive(Args)]
pub struct ReviewArgs {
    /// Local repository path.
    #[arg(short, long, value_name = "PATH")]
    pub path: PathBuf,

    /// Base Git ref. Defaults to HEAD.
    #[arg(long, value_name = "REF")]
    pub base: Option<String>,

    /// Head Git commit/ref. Ref mode requires a clean checkout at this exact commit.
    #[arg(long, value_name = "REF", conflicts_with = "working_tree")]
    pub head: Option<String>,

    /// Explicitly compare the base ref with the current working tree.
    #[arg(long, conflicts_with = "head")]
    pub working_tree: bool,

    /// Output format: text, json, or both.
    #[arg(long, default_value = "text", value_name = "FORMAT")]
    pub format: String,

    /// Write the selected output (JSON for both/json) atomically to this path.
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Disable secret redaction in changed-line snippets.
    #[arg(long)]
    pub no_redact: bool,

    /// Maximum related files emitted in the impact pack.
    #[arg(long, default_value_t = 128, value_name = "COUNT")]
    pub max_related_files: usize,
}

pub fn run(args: ReviewArgs) -> Result<()> {
    let format = match args.format.trim().to_ascii_lowercase().as_str() {
        "text" => ReviewFormat::Text,
        "json" => ReviewFormat::Json,
        "both" => ReviewFormat::Both,
        other => {
            anyhow::bail!("Invalid review format '{other}'. Expected one of: text, json, both")
        }
    };
    review::run(ReviewOptions {
        path: args.path,
        base: args.base,
        head: args.head,
        working_tree: args.working_tree,
        format,
        output: args.output,
        no_redact: args.no_redact,
        max_related_files: args.max_related_files,
    })
}

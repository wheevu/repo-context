//! Repository fetching (local, GitHub, HuggingFace)

use anyhow::Result;
use std::path::Path;

pub mod context;
pub mod github;
pub mod huggingface;
pub mod local;
pub mod workspace;

pub use context::RepoContext;

/// Fetch a repository from local path or remote URL.
///
/// Dispatches to the appropriate fetcher based on the URL host:
/// - `github.com` → [`github::clone_repository`]
/// - `huggingface.co` / `hf.co` → [`huggingface::clone_repository`]
/// - Local path → [`local::validate_local_path`]
pub fn fetch_repository(
    path: Option<&Path>,
    repo_url: Option<&str>,
    ref_: Option<&str>,
) -> Result<RepoContext> {
    if let Some(p) = path {
        local::validate_local_path(p)
    } else if let Some(url) = repo_url {
        if huggingface::is_huggingface_url(url) {
            huggingface::clone_repository(url, ref_)
        } else {
            // Default: GitHub (handles both HTTPS and SSH)
            github::clone_repository(url, ref_)
        }
    } else {
        anyhow::bail!("Either path or repo_url must be specified")
    }
}

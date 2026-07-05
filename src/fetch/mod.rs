//! Repository fetching (local, GitHub, HuggingFace)

use anyhow::Result;
use std::path::Path;

pub mod context;
pub mod github;
pub mod huggingface;
pub mod local;

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
        // Only allow https:// and git@ (SSH) schemes to prevent local file access.
        if url.starts_with("file://") {
            anyhow::bail!("Local file:// URLs are not supported; use --path instead");
        }
        if !(url.starts_with("https://") || url.starts_with("http://") || url.starts_with("git@")) {
            anyhow::bail!(
                "Unsupported URL scheme in '{}'. Use https:// or git@ URLs, or --path for local repos",
                url
            );
        }
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

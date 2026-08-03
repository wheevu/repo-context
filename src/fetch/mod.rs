//! Repository fetching (local, GitHub, HuggingFace)

use anyhow::Result;
use std::path::Path;

pub mod context;
pub mod github;
pub mod huggingface;
pub mod local;

pub use context::RepoContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteKind {
    Github,
    HuggingFace,
}

pub(crate) fn classify_remote_url(url: &str) -> Result<RemoteKind> {
    let url = url.trim();
    if url.is_empty() {
        anyhow::bail!("Repository URL cannot be empty");
    }
    if url.contains('?') || url.contains('#') {
        anyhow::bail!("Repository URLs must not contain query parameters or fragments");
    }
    if url.starts_with("git@github.com:") {
        let path = url.trim_start_matches("git@github.com:");
        validate_repo_path(path, "GitHub")?;
        if path.trim_matches('/').split('/').filter(|part| !part.is_empty()).count() != 2 {
            anyhow::bail!("GitHub repository URL must contain exactly owner/repository");
        }
        return Ok(RemoteKind::Github);
    }
    let (host, path) = https_host_and_path(url)?;
    let kind = match host {
        "github.com" => RemoteKind::Github,
        "huggingface.co" | "hf.co" => RemoteKind::HuggingFace,
        _ => anyhow::bail!(
            "Unsupported repository host '{host}'; only GitHub and HuggingFace HTTPS URLs are supported"
        ),
    };
    validate_repo_path(
        path,
        match kind {
            RemoteKind::Github => "GitHub",
            RemoteKind::HuggingFace => "HuggingFace",
        },
    )?;
    if kind == RemoteKind::Github
        && path.trim_matches('/').split('/').filter(|part| !part.is_empty()).count() != 2
    {
        anyhow::bail!("GitHub repository URL must contain exactly owner/repository");
    }
    Ok(kind)
}

pub(crate) fn https_host_and_path(url: &str) -> Result<(&str, &str)> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| anyhow::anyhow!("Only HTTPS repository URLs are supported"))?;
    let (authority, path) =
        rest.split_once('/').ok_or_else(|| anyhow::anyhow!("Repository URL is missing a path"))?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    if host.is_empty() || host.contains(':') || host.contains('\\') {
        anyhow::bail!("Repository URL has an invalid host");
    }
    Ok((host, path))
}

fn validate_repo_path(path: &str, provider: &str) -> Result<()> {
    let parts =
        path.trim_matches('/').split('/').filter(|part| !part.is_empty()).collect::<Vec<_>>();
    if parts.len() < 2
        || parts.iter().any(|part| *part == "." || *part == ".." || part.contains('\\'))
    {
        anyhow::bail!("{provider} repository URL has an invalid owner/repository path");
    }
    if parts.iter().any(|part| part.chars().any(char::is_control)) {
        anyhow::bail!("{provider} repository URL contains control characters");
    }
    Ok(())
}

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
        match classify_remote_url(url)? {
            RemoteKind::HuggingFace => huggingface::clone_repository(url, ref_),
            RemoteKind::Github => github::clone_repository(url, ref_),
        }
    } else {
        anyhow::bail!("Either path or repo_url must be specified")
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_remote_url, RemoteKind};

    #[test]
    fn accepts_only_supported_remote_hosts_and_schemes() {
        assert_eq!(
            classify_remote_url("https://github.com/org/repo").expect("GitHub URL"),
            RemoteKind::Github
        );
        assert_eq!(
            classify_remote_url("git@github.com:org/repo.git").expect("SSH URL"),
            RemoteKind::Github
        );
        assert_eq!(
            classify_remote_url("https://huggingface.co/org/model").expect("HF URL"),
            RemoteKind::HuggingFace
        );
    }

    #[test]
    fn rejects_unsafe_remote_urls() {
        for url in [
            "http://github.com/org/repo",
            "file:///tmp/repo",
            "https://evil.example/org/repo",
            "https://github.com/org/repo?token=secret",
            "https://github.com/org/repo/tree/main",
            "git@evil.example:org/repo.git",
        ] {
            assert!(classify_remote_url(url).is_err(), "URL should be rejected: {url}");
        }
    }
}

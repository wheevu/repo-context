//! GitHub repository cloning

use crate::fetch::RepoContext;
use anyhow::{Context, Result};
use git2::{FetchOptions, ObjectType, Repository};
use std::env;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn clone_repository(url: &str, ref_: Option<&str>) -> Result<RepoContext> {
    let temp_dir = build_temp_repo_dir();
    std::fs::create_dir_all(&temp_dir)
        .with_context(|| format!("Failed creating temp directory: {}", temp_dir.display()))?;

    // Normalize GitHub URLs: strip trailing slash, append .git if missing.
    // Matches Python fetcher.py behavior which normalizes to
    // https://github.com/{owner}/{repo}.git form.
    let normalized = normalize_github_url(url);
    let url = normalized.as_str();

    if let Some(reference) = ref_ {
        // Specific ref: try shallow clone targeting the branch first, fall back to full clone.
        let repo = try_shallow_clone_with_branch(url, &temp_dir, reference).or_else(|_| {
            Repository::clone(url, &temp_dir)
                .with_context(|| format!("Failed cloning repository from {url}"))
        })?;
        checkout_ref(&repo, reference)?;
        Ok(RepoContext::new(temp_dir, true))
    } else {
        // No specific ref: shallow clone (depth=1) the default branch.
        let repo = shallow_clone(url, &temp_dir).or_else(|_| {
            Repository::clone(url, &temp_dir)
                .with_context(|| format!("Failed cloning repository from {url}"))
        })?;
        let _ = repo; // drop
        Ok(RepoContext::new(temp_dir, true))
    }
}

/// Normalize a GitHub URL to the canonical HTTPS `.git` form.
///
/// Examples:
/// - `https://github.com/owner/repo`    → `https://github.com/owner/repo.git`
/// - `https://github.com/owner/repo/`   → `https://github.com/owner/repo.git`
/// - `https://github.com/owner/repo.git`→ unchanged
/// - non-GitHub URLs                    → unchanged
fn normalize_github_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    if trimmed.contains("github.com") && !trimmed.ends_with(".git") {
        format!("{}.git", trimmed)
    } else {
        trimmed.to_string()
    }
}

/// Attempt a shallow clone (depth=1) targeting a specific branch name.
fn try_shallow_clone_with_branch(url: &str, dest: &Path, branch: &str) -> Result<Repository> {
    let mut builder = git2::build::RepoBuilder::new();
    builder.branch(branch);

    let mut fo = FetchOptions::new();
    fo.depth(1);
    builder.fetch_options(fo);

    builder.clone(url, dest).with_context(|| format!("Shallow clone with branch {branch} failed"))
}

/// Shallow clone (depth=1) the default branch.
fn shallow_clone(url: &str, dest: &Path) -> Result<Repository> {
    let mut fo = FetchOptions::new();
    fo.depth(1);

    let mut builder = git2::build::RepoBuilder::new();
    builder.fetch_options(fo);

    builder.clone(url, dest).with_context(|| format!("Shallow clone from {url} failed"))
}

fn checkout_ref(repo: &Repository, reference: &str) -> Result<()> {
    let object = repo
        .revparse_single(reference)
        .with_context(|| format!("Failed to resolve ref: {reference}"))?;

    repo.checkout_tree(&object, None)
        .with_context(|| format!("Failed to checkout tree for ref: {reference}"))?;

    if object.kind() == Some(ObjectType::Commit) {
        repo.set_head_detached(object.id())
            .with_context(|| format!("Failed to set detached HEAD for ref: {reference}"))?;
    }

    Ok(())
}

fn build_temp_repo_dir() -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let pid = std::process::id();
    env::temp_dir().join(format!("repo-to-prompt-{pid}-{nanos}"))
}

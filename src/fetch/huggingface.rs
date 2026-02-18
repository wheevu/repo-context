//! HuggingFace repository cloning (Spaces, models, datasets)

use crate::fetch::RepoContext;
use anyhow::{Context, Result};
use git2::{ObjectType, Repository};
use std::env;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Recognised HuggingFace repo types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HfRepoType {
    Spaces,
    Datasets,
    Models,
}

/// Parsed components from a HuggingFace URL.
pub struct HfParsed {
    pub owner: String,
    pub repo_name: String,
    pub repo_type: HfRepoType,
    pub ref_: Option<String>,
}

/// Returns `true` if `url` looks like a HuggingFace URL.
pub fn is_huggingface_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains("huggingface.co") || lower.contains("hf.co")
}

/// Parse a HuggingFace URL into its components.
///
/// Supported formats:
/// - `https://huggingface.co/spaces/owner/repo[/tree/<ref>]`
/// - `https://huggingface.co/datasets/owner/repo[/tree/<ref>]`
/// - `https://huggingface.co/owner/repo[/tree/<ref>]`  (model)
/// - `https://hf.co/…` (same rules)
pub fn parse_huggingface_url(url: &str) -> Result<HfParsed> {
    // Strip scheme / host to get path segments.
    let path = if let Some(pos) = url.find("://") {
        let after_scheme = &url[pos + 3..];
        // skip the host component
        if let Some(slash) = after_scheme.find('/') {
            &after_scheme[slash..]
        } else {
            anyhow::bail!("Invalid HuggingFace URL (no path): {url}");
        }
    } else {
        anyhow::bail!("Invalid HuggingFace URL (no scheme): {url}");
    };

    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    if parts.is_empty() {
        anyhow::bail!("Invalid HuggingFace URL (empty path): {url}");
    }

    let (repo_type, owner, repo_name, ref_offset) = match parts[0] {
        "spaces" => {
            if parts.len() < 3 {
                anyhow::bail!("Invalid HuggingFace spaces URL (missing owner/repo): {url}");
            }
            (HfRepoType::Spaces, parts[1], parts[2], 3usize)
        }
        "datasets" => {
            if parts.len() < 3 {
                anyhow::bail!("Invalid HuggingFace datasets URL (missing owner/repo): {url}");
            }
            (HfRepoType::Datasets, parts[1], parts[2], 3usize)
        }
        owner => {
            if parts.len() < 2 {
                anyhow::bail!("Invalid HuggingFace model URL (missing repo): {url}");
            }
            (HfRepoType::Models, owner, parts[1], 2usize)
        }
    };

    // Look for `/tree/<ref>` after the owner/repo segments.
    let ref_ = if parts.len() >= ref_offset + 2 && parts[ref_offset] == "tree" {
        Some(parts[ref_offset + 1].to_string())
    } else {
        None
    };

    Ok(HfParsed { owner: owner.to_string(), repo_name: repo_name.to_string(), repo_type, ref_ })
}

/// Clone a HuggingFace repository to a temporary directory.
pub fn clone_repository(url: &str, ref_: Option<&str>) -> Result<RepoContext> {
    let parsed = parse_huggingface_url(url)?;

    // Resolve the ref: prefer explicit argument, then URL-embedded ref.
    let resolved_ref = ref_.map(str::to_string).or(parsed.ref_);

    // Build canonical clone URL.
    let clone_url = match parsed.repo_type {
        HfRepoType::Spaces => {
            format!("https://huggingface.co/spaces/{}/{}", parsed.owner, parsed.repo_name)
        }
        HfRepoType::Datasets => {
            format!("https://huggingface.co/datasets/{}/{}", parsed.owner, parsed.repo_name)
        }
        HfRepoType::Models => {
            format!("https://huggingface.co/{}/{}", parsed.owner, parsed.repo_name)
        }
    };

    let temp_dir = build_temp_repo_dir(&parsed.repo_name);
    std::fs::create_dir_all(&temp_dir)
        .with_context(|| format!("Failed creating temp directory: {}", temp_dir.display()))?;

    let repo = Repository::clone(&clone_url, &temp_dir)
        .with_context(|| format!("Failed cloning HuggingFace repository from {clone_url}"))?;

    if let Some(ref reference) = resolved_ref {
        checkout_ref(&repo, reference)?;
    }

    Ok(RepoContext::new(temp_dir, true))
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

fn build_temp_repo_dir(repo_name: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let pid = std::process::id();
    env::temp_dir().join(format!("repo-to-prompt-hf-{repo_name}-{pid}-{nanos}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_huggingface_url() {
        assert!(is_huggingface_url("https://huggingface.co/spaces/owner/repo"));
        assert!(is_huggingface_url("https://hf.co/owner/model"));
        assert!(!is_huggingface_url("https://github.com/owner/repo"));
    }

    #[test]
    fn test_parse_spaces_url() {
        let parsed = parse_huggingface_url("https://huggingface.co/spaces/gradio/demo").unwrap();
        assert_eq!(parsed.owner, "gradio");
        assert_eq!(parsed.repo_name, "demo");
        assert_eq!(parsed.repo_type, HfRepoType::Spaces);
        assert!(parsed.ref_.is_none());
    }

    #[test]
    fn test_parse_spaces_url_with_ref() {
        let parsed =
            parse_huggingface_url("https://huggingface.co/spaces/gradio/demo/tree/main").unwrap();
        assert_eq!(parsed.ref_, Some("main".to_string()));
    }

    #[test]
    fn test_parse_datasets_url() {
        let parsed = parse_huggingface_url("https://huggingface.co/datasets/owner/mydata").unwrap();
        assert_eq!(parsed.owner, "owner");
        assert_eq!(parsed.repo_name, "mydata");
        assert_eq!(parsed.repo_type, HfRepoType::Datasets);
    }

    #[test]
    fn test_parse_model_url() {
        let parsed = parse_huggingface_url("https://huggingface.co/meta-llama/Llama-2-7b").unwrap();
        assert_eq!(parsed.owner, "meta-llama");
        assert_eq!(parsed.repo_name, "Llama-2-7b");
        assert_eq!(parsed.repo_type, HfRepoType::Models);
        assert!(parsed.ref_.is_none());
    }

    #[test]
    fn test_parse_model_url_with_ref() {
        let parsed = parse_huggingface_url("https://huggingface.co/owner/model/tree/dev").unwrap();
        assert_eq!(parsed.ref_, Some("dev".to_string()));
    }

    #[test]
    fn test_parse_hf_co_url() {
        let parsed = parse_huggingface_url("https://hf.co/spaces/owner/repo").unwrap();
        assert_eq!(parsed.owner, "owner");
        assert_eq!(parsed.repo_name, "repo");
        assert_eq!(parsed.repo_type, HfRepoType::Spaces);
    }

    #[test]
    fn test_clone_url_spaces() {
        let parsed = parse_huggingface_url("https://huggingface.co/spaces/gradio/demo").unwrap();
        let clone_url = match parsed.repo_type {
            HfRepoType::Spaces => {
                format!("https://huggingface.co/spaces/{}/{}", parsed.owner, parsed.repo_name)
            }
            _ => panic!("wrong type"),
        };
        assert_eq!(clone_url, "https://huggingface.co/spaces/gradio/demo");
    }

    #[test]
    fn test_clone_url_datasets() {
        let parsed = parse_huggingface_url("https://huggingface.co/datasets/owner/mydata").unwrap();
        let clone_url = match parsed.repo_type {
            HfRepoType::Datasets => {
                format!("https://huggingface.co/datasets/{}/{}", parsed.owner, parsed.repo_name)
            }
            _ => panic!("wrong type"),
        };
        assert_eq!(clone_url, "https://huggingface.co/datasets/owner/mydata");
    }

    #[test]
    fn test_clone_url_model() {
        let parsed = parse_huggingface_url("https://huggingface.co/meta-llama/Llama-2-7b").unwrap();
        let clone_url = match parsed.repo_type {
            HfRepoType::Models => {
                format!("https://huggingface.co/{}/{}", parsed.owner, parsed.repo_name)
            }
            _ => panic!("wrong type"),
        };
        assert_eq!(clone_url, "https://huggingface.co/meta-llama/Llama-2-7b");
    }
}

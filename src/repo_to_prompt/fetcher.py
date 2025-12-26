"""
Repository fetcher module.

Handles fetching repositories from local paths and GitHub URLs.
"""

from __future__ import annotations

import os
import re
import shutil
import tempfile
from pathlib import Path
from urllib.parse import urlparse

from rich.console import Console

console = Console()


class FetchError(Exception):
    """Error during repository fetching."""
    pass


def parse_github_url(url: str) -> tuple[str, str, str | None]:
    """
    Parse a GitHub URL into owner, repo, and optional ref.

    Supports formats:
    - https://github.com/owner/repo
    - https://github.com/owner/repo.git
    - https://github.com/owner/repo/tree/branch
    - git@github.com:owner/repo.git

    Returns:
        Tuple of (owner, repo_name, ref or None)
    """
    # Handle SSH URLs
    if url.startswith("git@"):
        match = re.match(r"git@github\.com:([^/]+)/([^/]+?)(?:\.git)?$", url)
        if match:
            return match.group(1), match.group(2), None
        raise FetchError(f"Invalid GitHub SSH URL: {url}")

    # Handle HTTPS URLs
    parsed = urlparse(url)
    if parsed.netloc not in ("github.com", "www.github.com"):
        raise FetchError(f"Not a GitHub URL: {url}")

    # Split path
    path_parts = [p for p in parsed.path.split("/") if p]

    if len(path_parts) < 2:
        raise FetchError(f"Invalid GitHub URL (missing owner/repo): {url}")

    owner = path_parts[0]
    repo = path_parts[1].removesuffix(".git")

    # Check for branch/tag in URL
    ref = None
    if len(path_parts) >= 4 and path_parts[2] in ("tree", "blob", "commit"):
        ref = path_parts[3]

    return owner, repo, ref


def clone_github_repo(
    url: str,
    ref: str | None = None,
    target_dir: Path | None = None,
    shallow: bool = True,
) -> Path:
    """
    Clone a GitHub repository.

    Args:
        url: GitHub repository URL
        ref: Branch, tag, or commit SHA to checkout
        target_dir: Directory to clone into (temp dir if None)
        shallow: Whether to do a shallow clone

    Returns:
        Path to the cloned repository
    """
    try:
        import git
    except ImportError as exc:
        raise FetchError(
            "GitPython is required for GitHub cloning. Install with: pip install gitpython"
        ) from exc

    # Parse URL to get components
    owner, repo_name, url_ref = parse_github_url(url)

    # Use ref from URL if not explicitly provided
    if ref is None:
        ref = url_ref

    # Normalize URL to HTTPS
    clone_url = f"https://github.com/{owner}/{repo_name}.git"

    # Create target directory
    if target_dir is None:
        target_dir = Path(tempfile.mkdtemp(prefix="repo-to-prompt-"))
    else:
        target_dir = Path(target_dir)
        target_dir.mkdir(parents=True, exist_ok=True)

    repo_path = target_dir / repo_name

    console.print(f"[cyan]Cloning {owner}/{repo_name}...[/cyan]")

    try:
        # Clone options
        clone_kwargs: dict = {"depth": 1} if shallow and ref is None else {}

        if ref:
            # For specific refs, we need to clone without depth first
            # then checkout the ref
            if shallow:
                # Try shallow clone with specific branch
                try:
                    repo = git.Repo.clone_from(
                        clone_url,
                        repo_path,
                        branch=ref,
                        depth=1,
                    )
                except git.GitCommandError:
                    # Fall back to full clone if shallow with ref fails
                    repo = git.Repo.clone_from(clone_url, repo_path)
                    repo.git.checkout(ref)
            else:
                repo = git.Repo.clone_from(clone_url, repo_path)
                repo.git.checkout(ref)
        else:
            repo = git.Repo.clone_from(clone_url, repo_path, **clone_kwargs)

        console.print(f"[green]✓ Cloned to {repo_path}[/green]")
        return repo_path

    except git.GitCommandError as e:
        raise FetchError(f"Failed to clone repository: {e}") from e
    except Exception as e:
        raise FetchError(f"Unexpected error during clone: {e}") from e


def validate_local_path(path: Path) -> Path:
    """
    Validate and resolve a local repository path.

    Args:
        path: Path to validate

    Returns:
        Resolved absolute path

    Raises:
        FetchError: If path is invalid
    """
    resolved = path.resolve()

    if not resolved.exists():
        raise FetchError(f"Path does not exist: {resolved}")

    if not resolved.is_dir():
        raise FetchError(f"Path is not a directory: {resolved}")

    # Check if it's readable
    if not os.access(resolved, os.R_OK):
        raise FetchError(f"Path is not readable: {resolved}")

    return resolved


def get_repo_root(path: Path) -> Path:
    """
    Find the root of a git repository.

    Walks up the directory tree looking for .git directory.
    Returns the input path if no .git found.
    """
    current = path.resolve()

    while current != current.parent:
        if (current / ".git").exists():
            return current
        current = current.parent

    # No .git found, return original path
    return path.resolve()


def fetch_repository(
    path: Path | None = None,
    repo_url: str | None = None,
    ref: str | None = None,
    target_dir: Path | None = None,
) -> tuple[Path, bool]:
    """
    Fetch a repository from local path or GitHub URL.

    Args:
        path: Local path to repository
        repo_url: GitHub repository URL
        ref: Branch/tag/SHA for GitHub repos
        target_dir: Target directory for cloned repos

    Returns:
        Tuple of (repo_path, is_temp) where is_temp indicates if cleanup is needed
    """
    if path is not None:
        validated_path = validate_local_path(path)
        return validated_path, False

    if repo_url is not None:
        cloned_path = clone_github_repo(repo_url, ref, target_dir)
        return cloned_path, target_dir is None  # is_temp if no target specified

    raise FetchError("Either path or repo_url must be provided")


def cleanup_temp_repo(path: Path) -> None:
    """
    Clean up a temporary cloned repository.

    Args:
        path: Path to the temporary repository
    """
    try:
        if path.exists():
            shutil.rmtree(path)
    except Exception as e:
        console.print(f"[yellow]Warning: Failed to clean up temp directory {path}: {e}[/yellow]")


class RepoContext:
    """
    Context manager for repository fetching.

    Handles automatic cleanup of temporary cloned repositories.
    """

    def __init__(
        self,
        path: Path | None = None,
        repo_url: str | None = None,
        ref: str | None = None,
    ):
        self.path = path
        self.repo_url = repo_url
        self.ref = ref
        self._repo_path: Path | None = None
        self._is_temp: bool = False

    def __enter__(self) -> Path:
        self._repo_path, self._is_temp = fetch_repository(
            path=self.path,
            repo_url=self.repo_url,
            ref=self.ref,
        )
        return self._repo_path

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        if self._is_temp and self._repo_path is not None:
            cleanup_temp_repo(self._repo_path.parent)

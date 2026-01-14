"""
Repository fetcher module.

Handles fetching repositories from local paths, GitHub URLs, and HuggingFace Spaces.
"""

from __future__ import annotations

import os
import re
import shutil
import tempfile
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

from rich.console import Console

console = Console()


class FetchError(Exception):
    """Error during repository fetching."""

    pass


def is_github_url(url: str) -> bool:
    """Check if a URL is a GitHub repository URL.

    Args:
        url: URL to check.

    Returns:
        True if the URL is a GitHub URL, False otherwise.
    """
    if url.startswith("git@github.com:"):
        return True
    parsed = urlparse(url)
    return parsed.netloc in ("github.com", "www.github.com")


def is_huggingface_url(url: str) -> bool:
    """Check if a URL is a HuggingFace Spaces or repository URL.

    Args:
        url: URL to check.

    Returns:
        True if the URL is a HuggingFace URL, False otherwise.
    """
    parsed = urlparse(url)
    return parsed.netloc in ("huggingface.co", "hf.co", "www.huggingface.co")


def parse_github_url(url: str) -> tuple[str, str, str | None]:
    """Parse a GitHub URL into `(owner, repo_name, ref)`.

    Supports common formats:
    - `https://github.com/owner/repo`
    - `https://github.com/owner/repo.git`
    - `https://github.com/owner/repo/tree/<ref>`
    - `git@github.com:owner/repo.git`

    Args:
        url: GitHub repository URL (HTTPS or SSH).

    Returns:
        A tuple `(owner, repo_name, ref)` where `ref` is optional and derived from the URL
        when present (e.g., `/tree/<ref>`).

    Raises:
        FetchError: If the URL is not a recognized GitHub repository URL.
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


def parse_huggingface_url(url: str) -> tuple[str, str, str, str | None]:
    """Parse a HuggingFace URL into `(owner, repo_name, repo_type, ref)`.

    Supports common formats:
    - `https://huggingface.co/spaces/owner/space-name`
    - `https://hf.co/spaces/owner/space-name`
    - `https://huggingface.co/spaces/owner/space-name/tree/<ref>`
    - `https://huggingface.co/owner/model-name` (models)
    - `https://huggingface.co/datasets/owner/dataset-name` (datasets)

    Args:
        url: HuggingFace repository URL.

    Returns:
        A tuple `(owner, repo_name, repo_type, ref)` where `repo_type` is one of
        'spaces', 'models', or 'datasets', and `ref` is optional.

    Raises:
        FetchError: If the URL is not a recognized HuggingFace repository URL.
    """
    parsed = urlparse(url)
    if parsed.netloc not in ("huggingface.co", "hf.co", "www.huggingface.co"):
        raise FetchError(f"Not a HuggingFace URL: {url}")

    # Split path
    path_parts = [p for p in parsed.path.split("/") if p]

    if len(path_parts) < 2:
        raise FetchError(f"Invalid HuggingFace URL (missing owner/repo): {url}")

    # Determine repo type based on URL structure
    if path_parts[0] in ("spaces", "datasets"):
        # Format: /spaces/owner/repo or /datasets/owner/repo
        repo_type = path_parts[0]
        if len(path_parts) < 3:
            raise FetchError(f"Invalid HuggingFace URL (missing owner/repo): {url}")
        owner = path_parts[1]
        repo_name = path_parts[2]
        # Check for branch/tag in URL
        ref = None
        if len(path_parts) >= 5 and path_parts[3] == "tree":
            ref = path_parts[4]
    else:
        # Format: /owner/repo (models are the default)
        repo_type = "models"
        owner = path_parts[0]
        repo_name = path_parts[1]
        # Check for branch/tag in URL
        ref = None
        if len(path_parts) >= 4 and path_parts[2] == "tree":
            ref = path_parts[3]

    return owner, repo_name, repo_type, ref


def clone_huggingface_repo(
    url: str,
    ref: str | None = None,
    target_dir: Path | None = None,
    shallow: bool = True,
) -> Path:
    """Clone a HuggingFace repository (Spaces, models, or datasets) to a local directory.

    Args:
        url: HuggingFace repository URL.
        ref: Optional branch/tag/SHA to checkout. If omitted, uses the default branch
            (or any ref encoded in the URL).
        target_dir: Parent directory to clone into. If None, a temporary directory is created.
        shallow: Whether to attempt a shallow clone (`depth=1`) when possible.

    Returns:
        Path to the cloned repository root directory.

    Raises:
        FetchError: If GitPython is unavailable or the clone/checkout fails.
    """
    try:
        import git
    except ImportError as exc:
        raise FetchError(
            "GitPython is required for HuggingFace cloning. Install with: pip install gitpython"
        ) from exc

    # Parse URL to get components
    owner, repo_name, repo_type, url_ref = parse_huggingface_url(url)

    # Use ref from URL if not explicitly provided
    if ref is None:
        ref = url_ref

    # Build clone URL based on repo type
    if repo_type == "spaces":
        clone_url = f"https://huggingface.co/spaces/{owner}/{repo_name}"
    elif repo_type == "datasets":
        clone_url = f"https://huggingface.co/datasets/{owner}/{repo_name}"
    else:
        # models
        clone_url = f"https://huggingface.co/{owner}/{repo_name}"

    # Create target directory
    if target_dir is None:
        target_dir = Path(tempfile.mkdtemp(prefix="repo-to-prompt-"))
    else:
        target_dir = Path(target_dir)
        target_dir.mkdir(parents=True, exist_ok=True)

    repo_path = target_dir / repo_name

    console.print(f"[cyan]Cloning HuggingFace {repo_type}: {owner}/{repo_name}...[/cyan]")

    try:
        # Clone options
        clone_kwargs: dict[str, Any] = {"depth": 1} if shallow and ref is None else {}

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
        raise FetchError(f"Failed to clone HuggingFace repository: {e}") from e
    except Exception as e:
        raise FetchError(f"Unexpected error during clone: {e}") from e


def clone_github_repo(
    url: str,
    ref: str | None = None,
    target_dir: Path | None = None,
    shallow: bool = True,
) -> Path:
    """Clone a GitHub repository to a local directory.

    Args:
        url: GitHub repository URL (HTTPS or SSH).
        ref: Optional branch/tag/SHA to checkout. If omitted, uses the default branch
            (or any ref encoded in the URL).
        target_dir: Parent directory to clone into. If None, a temporary directory is created.
        shallow: Whether to attempt a shallow clone (`depth=1`) when possible.

    Returns:
        Path to the cloned repository root directory.

    Raises:
        FetchError: If GitPython is unavailable or the clone/checkout fails.
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
        clone_kwargs: dict[str, Any] = {"depth": 1} if shallow and ref is None else {}

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
    """Validate and resolve a local repository path.

    Args:
        path: Local path to validate.

    Returns:
        Resolved absolute path to a readable directory.

    Raises:
        FetchError: If the path does not exist, is not a directory, or is not readable.
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
    """Find the git repository root for a path.

    Walks upward until a `.git` directory is found. If none is found, returns the resolved
    input path.

    Args:
        path: Any path inside (or near) a repository.

    Returns:
        The repository root path, or the resolved input path if no `.git` is found.
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
    """Fetch a repository from a local path or by cloning a GitHub/HuggingFace URL.

    Exactly one of `path` or `repo_url` must be provided.

    Args:
        path: Local path to an existing repository directory.
        repo_url: GitHub or HuggingFace repository URL to clone.
        ref: Optional branch/tag/SHA when cloning.
        target_dir: Optional target directory for clones. If None, a temp directory is used.

    Returns:
        Tuple `(repo_path, is_temp)` where `is_temp` indicates whether the returned repository
        should be cleaned up by the caller.

    Raises:
        FetchError: If neither input source is provided, URL is unsupported, or fetch fails.
    """
    if path is not None:
        validated_path = validate_local_path(path)
        return validated_path, False

    if repo_url is not None:
        # Detect URL type and dispatch to appropriate clone function
        if is_github_url(repo_url):
            cloned_path = clone_github_repo(repo_url, ref, target_dir)
        elif is_huggingface_url(repo_url):
            cloned_path = clone_huggingface_repo(repo_url, ref, target_dir)
        else:
            raise FetchError(f"Unsupported repository URL: {repo_url}")
        return cloned_path, target_dir is None  # is_temp if no target specified

    raise FetchError("Either path or repo_url must be provided")


def cleanup_temp_repo(path: Path) -> None:
    """Delete a temporary clone directory.

    Args:
        path: Path to the temporary directory to remove.
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
    ) -> None:
        """Initialize the context manager.

        Args:
            path: Local repository path (mutually exclusive with `repo_url`).
            repo_url: GitHub repository URL to clone (mutually exclusive with `path`).
            ref: Optional git ref to checkout when cloning.
        """
        self.path = path
        self.repo_url = repo_url
        self.ref = ref
        self._repo_path: Path | None = None
        self._is_temp: bool = False

    def __enter__(self) -> Path:
        """Enter the context, fetching the repository.

        Returns:
            The path to the fetched repository root.

        Raises:
            FetchError: If fetching fails.
        """
        self._repo_path, self._is_temp = fetch_repository(
            path=self.path,
            repo_url=self.repo_url,
            ref=self.ref,
        )
        return self._repo_path

    def __exit__(
        self, exc_type: type[BaseException] | None, exc_val: BaseException | None, exc_tb: Any
    ) -> None:
        """Exit the context and clean up any temporary clone."""
        if self._is_temp and self._repo_path is not None:
            cleanup_temp_repo(self._repo_path.parent)

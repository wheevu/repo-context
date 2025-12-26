from __future__ import annotations

from pathlib import Path

from repo_to_prompt.cli import get_repo_output_dir


def test_get_repo_output_dir_namespaces_by_repo_name() -> None:
    base = Path("out")
    repo_root = Path("/tmp/my-repo")
    assert get_repo_output_dir(base, repo_root) == Path("out") / "my-repo"


def test_get_repo_output_dir_avoids_double_nesting() -> None:
    base = Path("out") / "my-repo"
    repo_root = Path("/tmp/my-repo")
    assert get_repo_output_dir(base, repo_root) == base

"""Tests for the fetcher module."""

import pytest

from repo_to_prompt.fetcher import (
    FetchError,
    is_github_url,
    is_huggingface_url,
    parse_github_url,
    parse_huggingface_url,
    validate_local_path,
)


class TestParseGitHubUrl:
    """Tests for GitHub URL parsing."""

    def test_simple_https_url(self):
        """Test parsing simple HTTPS URL."""
        owner, repo, ref = parse_github_url("https://github.com/owner/repo")

        assert owner == "owner"
        assert repo == "repo"
        assert ref is None

    def test_https_url_with_git_suffix(self):
        """Test parsing HTTPS URL with .git suffix."""
        owner, repo, ref = parse_github_url("https://github.com/owner/repo.git")

        assert owner == "owner"
        assert repo == "repo"
        assert ref is None

    def test_url_with_branch(self):
        """Test parsing URL with branch."""
        owner, repo, ref = parse_github_url("https://github.com/owner/repo/tree/develop")

        assert owner == "owner"
        assert repo == "repo"
        assert ref == "develop"

    def test_url_with_tag(self):
        """Test parsing URL with tag."""
        owner, repo, ref = parse_github_url("https://github.com/owner/repo/tree/v1.0.0")

        assert owner == "owner"
        assert repo == "repo"
        assert ref == "v1.0.0"

    def test_ssh_url(self):
        """Test parsing SSH URL."""
        owner, repo, ref = parse_github_url("git@github.com:owner/repo.git")

        assert owner == "owner"
        assert repo == "repo"
        assert ref is None

    def test_invalid_url(self):
        """Test that invalid URL raises error."""
        with pytest.raises(FetchError):
            parse_github_url("https://gitlab.com/owner/repo")

    def test_incomplete_url(self):
        """Test that incomplete URL raises error."""
        with pytest.raises(FetchError):
            parse_github_url("https://github.com/owner")


class TestIsGitHubUrl:
    """Tests for GitHub URL detection."""

    def test_https_url(self):
        """Test detecting HTTPS GitHub URL."""
        assert is_github_url("https://github.com/owner/repo") is True

    def test_ssh_url(self):
        """Test detecting SSH GitHub URL."""
        assert is_github_url("git@github.com:owner/repo.git") is True

    def test_non_github_url(self):
        """Test non-GitHub URL returns False."""
        assert is_github_url("https://gitlab.com/owner/repo") is False

    def test_huggingface_url(self):
        """Test HuggingFace URL returns False."""
        assert is_github_url("https://huggingface.co/spaces/owner/space") is False


class TestIsHuggingFaceUrl:
    """Tests for HuggingFace URL detection."""

    def test_huggingface_url(self):
        """Test detecting HuggingFace URL."""
        assert is_huggingface_url("https://huggingface.co/spaces/owner/space") is True

    def test_hf_short_url(self):
        """Test detecting hf.co short URL."""
        assert is_huggingface_url("https://hf.co/spaces/owner/space") is True

    def test_www_huggingface_url(self):
        """Test detecting www.huggingface.co URL."""
        assert is_huggingface_url("https://www.huggingface.co/owner/model") is True

    def test_github_url(self):
        """Test GitHub URL returns False."""
        assert is_huggingface_url("https://github.com/owner/repo") is False


class TestParseHuggingFaceUrl:
    """Tests for HuggingFace URL parsing."""

    def test_simple_spaces_url(self):
        """Test parsing simple Spaces URL."""
        owner, repo, repo_type, ref = parse_huggingface_url(
            "https://huggingface.co/spaces/owner/space-name"
        )

        assert owner == "owner"
        assert repo == "space-name"
        assert repo_type == "spaces"
        assert ref is None

    def test_hf_short_spaces_url(self):
        """Test parsing hf.co short Spaces URL."""
        owner, repo, repo_type, ref = parse_huggingface_url("https://hf.co/spaces/owner/space")

        assert owner == "owner"
        assert repo == "space"
        assert repo_type == "spaces"
        assert ref is None

    def test_spaces_url_with_branch(self):
        """Test parsing Spaces URL with branch."""
        owner, repo, repo_type, ref = parse_huggingface_url(
            "https://huggingface.co/spaces/owner/space/tree/main"
        )

        assert owner == "owner"
        assert repo == "space"
        assert repo_type == "spaces"
        assert ref == "main"

    def test_model_url(self):
        """Test parsing model URL (default type)."""
        owner, repo, repo_type, ref = parse_huggingface_url(
            "https://huggingface.co/meta-llama/Llama-2-7b"
        )

        assert owner == "meta-llama"
        assert repo == "Llama-2-7b"
        assert repo_type == "models"
        assert ref is None

    def test_model_url_with_ref(self):
        """Test parsing model URL with ref."""
        owner, repo, repo_type, ref = parse_huggingface_url(
            "https://huggingface.co/owner/model/tree/v1.0"
        )

        assert owner == "owner"
        assert repo == "model"
        assert repo_type == "models"
        assert ref == "v1.0"

    def test_dataset_url(self):
        """Test parsing dataset URL."""
        owner, repo, repo_type, ref = parse_huggingface_url(
            "https://huggingface.co/datasets/squad/squad"
        )

        assert owner == "squad"
        assert repo == "squad"
        assert repo_type == "datasets"
        assert ref is None

    def test_dataset_url_with_ref(self):
        """Test parsing dataset URL with ref."""
        owner, repo, repo_type, ref = parse_huggingface_url(
            "https://huggingface.co/datasets/owner/dataset/tree/dev"
        )

        assert owner == "owner"
        assert repo == "dataset"
        assert repo_type == "datasets"
        assert ref == "dev"

    def test_invalid_url_non_huggingface(self):
        """Test that non-HuggingFace URL raises error."""
        with pytest.raises(FetchError):
            parse_huggingface_url("https://github.com/owner/repo")

    def test_invalid_url_missing_repo(self):
        """Test that incomplete URL raises error."""
        with pytest.raises(FetchError):
            parse_huggingface_url("https://huggingface.co/spaces/owner")

    def test_invalid_url_missing_owner(self):
        """Test that URL with only one path component raises error."""
        with pytest.raises(FetchError):
            parse_huggingface_url("https://huggingface.co/owner")


class TestValidateLocalPath:
    """Tests for local path validation."""

    def test_valid_directory(self, tmp_path):
        """Test validating existing directory."""
        result = validate_local_path(tmp_path)

        assert result == tmp_path.resolve()

    def test_nonexistent_path(self):
        """Test that nonexistent path raises error."""
        from pathlib import Path

        with pytest.raises(FetchError) as exc_info:
            validate_local_path(Path("/nonexistent/path"))

        assert "does not exist" in str(exc_info.value)

    def test_file_instead_of_directory(self, tmp_path):
        """Test that file path raises error."""
        file_path = tmp_path / "test.txt"
        file_path.write_text("test")

        with pytest.raises(FetchError) as exc_info:
            validate_local_path(file_path)

        assert "not a directory" in str(exc_info.value)

"""Tests for the fetcher module."""

import pytest

from repo_to_prompt.fetcher import FetchError, parse_github_url, validate_local_path


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

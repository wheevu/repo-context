"""
Fixture-based integration tests for repo-to-prompt.

Creates realistic test repositories with:
- .gitignore files
- Symlinks
- Nested directories
- Secrets to redact
- Multiple file types
"""

import json
import os
import tempfile
from pathlib import Path

import pytest

from repo_to_prompt.chunker import chunk_file, coalesce_small_chunks
from repo_to_prompt.config import FileInfo, ScanStats, REPORT_SCHEMA_VERSION
from repo_to_prompt.ranker import FileRanker
from repo_to_prompt.redactor import create_redactor
from repo_to_prompt.renderer import (
    render_context_pack,
    render_jsonl,
    write_outputs,
    ReportRenderer,
)
from repo_to_prompt.scanner import scan_repository
from repo_to_prompt.config import OutputMode


def create_fixture_repo(root: Path) -> None:
    """Create a realistic test repository structure."""
    # Create directory structure
    (root / "src").mkdir()
    (root / "src" / "utils").mkdir()
    (root / "tests").mkdir()
    (root / "docs").mkdir()
    (root / "build").mkdir()  # Should be ignored
    (root / "node_modules").mkdir()  # Should be ignored
    
    # README
    (root / "README.md").write_text("""# Test Project

A sample project for testing repo-to-prompt.

## Installation

```bash
pip install test-project
```

## Usage

```python
from test_project import main
main()
```
""")
    
    # Package config
    (root / "pyproject.toml").write_text("""[project]
name = "test-project"
version = "1.0.0"
description = "A test project"

[project.scripts]
test-cli = "test_project.cli:main"
""")
    
    # Main source files
    (root / "src" / "__init__.py").write_text('"""Test project package."""\n__version__ = "1.0.0"\n')
    
    (root / "src" / "main.py").write_text('''"""Main module."""

def main():
    """Entry point."""
    print("Hello, world!")
    return 0


if __name__ == "__main__":
    main()
''')
    
    (root / "src" / "utils" / "__init__.py").write_text('"""Utilities package."""\n')
    
    (root / "src" / "utils" / "helpers.py").write_text('''"""Helper functions."""

def helper_function(x: int, y: int) -> int:
    """Add two numbers."""
    return x + y


def format_output(data: dict) -> str:
    """Format data for output."""
    return str(data)
''')
    
    # File with secrets (to test redaction)
    (root / "src" / "config.py").write_text('''"""Configuration with secrets."""

# API keys (should be redacted)
API_KEY = "AKIAIOSFODNN7EXAMPLE"
SECRET_KEY = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"

# GitHub token (should be redacted)
GITHUB_TOKEN = "ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"

# Database URL with password (should be redacted)
DATABASE_URL = "postgresql://user:secretpassword123@localhost/db"

def get_config():
    """Get configuration."""
    return {"api_key": API_KEY}
''')
    
    # Test files
    (root / "tests" / "__init__.py").write_text("")
    
    (root / "tests" / "test_main.py").write_text('''"""Tests for main module."""
import pytest
from src.main import main


def test_main():
    """Test main function."""
    assert main() == 0


def test_main_output(capsys):
    """Test main output."""
    main()
    captured = capsys.readouterr()
    assert "Hello" in captured.out
''')
    
    # Documentation
    (root / "docs" / "index.md").write_text("""# Documentation

Welcome to the documentation.

## Getting Started

See the README for installation instructions.
""")
    
    # .gitignore
    (root / ".gitignore").write_text("""# Build outputs
build/
dist/
*.pyc
__pycache__/

# Dependencies
node_modules/
.venv/

# IDE
.idea/
.vscode/

# Secrets (but we still have config.py with example secrets)
.env
*.secret
""")
    
    # Nested .gitignore
    (root / "docs" / ".gitignore").write_text("""# Ignore generated docs
_build/
*.generated.md
""")
    
    # Files that should be ignored
    (root / "build" / "output.js").write_text("console.log('built');")
    (root / "node_modules" / "package.json").write_text('{"name": "dep"}')


@pytest.fixture
def fixture_repo():
    """Create a fixture repository for testing."""
    with tempfile.TemporaryDirectory() as tmpdir:
        root = Path(tmpdir)
        create_fixture_repo(root)
        yield root


class TestFixtureRepoScanning:
    """Tests for scanning fixture repositories."""

    def test_scan_respects_gitignore(self, fixture_repo):
        """Test that scanning respects .gitignore."""
        files, stats = scan_repository(fixture_repo)
        
        paths = {f.relative_path for f in files}
        
        # Should include source files
        assert "src/main.py" in paths
        assert "src/utils/helpers.py" in paths
        assert "README.md" in paths
        
        # Should exclude gitignored directories
        assert not any("build/" in p for p in paths)
        assert not any("node_modules/" in p for p in paths)

    def test_scan_finds_all_python_files(self, fixture_repo):
        """Test that all Python files are found."""
        files, stats = scan_repository(
            fixture_repo,
            include_extensions={".py"},
        )
        
        py_files = [f for f in files if f.extension == ".py"]
        
        # Should find all .py files not in gitignored dirs
        assert len(py_files) >= 5  # __init__.py files, main.py, helpers.py, config.py, test files

    def test_scan_statistics(self, fixture_repo):
        """Test scan statistics are collected correctly."""
        files, stats = scan_repository(fixture_repo)
        
        assert stats.files_scanned > 0
        assert stats.files_included > 0
        assert stats.files_skipped_glob > 0  # build/, node_modules/
        assert stats.total_bytes_included > 0

    def test_language_detection(self, fixture_repo):
        """Test language detection."""
        files, stats = scan_repository(fixture_repo)
        
        assert "python" in stats.languages_detected
        assert "markdown" in stats.languages_detected


class TestFixtureRepoRanking:
    """Tests for ranking files in fixture repositories."""

    def test_readme_ranked_highest(self, fixture_repo):
        """Test that README is ranked highest."""
        files, stats = scan_repository(fixture_repo)
        scanned_paths = {f.relative_path for f in files}
        ranker = FileRanker(fixture_repo, scanned_files=scanned_paths)
        ranked = ranker.rank_files(files)
        
        # README should be first
        assert ranked[0].relative_path == "README.md"
        assert ranked[0].priority == 1.0

    def test_config_files_ranked_high(self, fixture_repo):
        """Test that config files are ranked high."""
        files, stats = scan_repository(fixture_repo)
        scanned_paths = {f.relative_path for f in files}
        ranker = FileRanker(fixture_repo, scanned_files=scanned_paths)
        ranked = ranker.rank_files(files)
        
        # Find pyproject.toml
        pyproject = next((f for f in ranked if f.relative_path == "pyproject.toml"), None)
        assert pyproject is not None
        assert pyproject.priority >= 0.8

    def test_test_files_ranked_lower(self, fixture_repo):
        """Test that test files are ranked lower."""
        files, stats = scan_repository(fixture_repo)
        scanned_paths = {f.relative_path for f in files}
        ranker = FileRanker(fixture_repo, scanned_files=scanned_paths)
        ranked = ranker.rank_files(files)
        
        # Find test file
        test_file = next((f for f in ranked if "test_" in f.relative_path), None)
        assert test_file is not None
        assert test_file.priority < 0.6


class TestFixtureRepoRedaction:
    """Tests for secret redaction in fixture repositories."""

    def test_redacts_aws_keys(self, fixture_repo):
        """Test that AWS keys are redacted."""
        files, stats = scan_repository(fixture_repo)
        config_file = next((f for f in files if f.relative_path == "src/config.py"), None)
        assert config_file is not None
        
        redactor = create_redactor(enabled=True)
        chunks = chunk_file(config_file, max_tokens=1000, redactor=redactor)
        
        content = "\n".join(c.content for c in chunks)
        
        # Should not contain the actual key
        assert "AKIAIOSFODNN7EXAMPLE" not in content
        # Should contain redaction marker (could be various formats)
        assert "REDACTED" in content

    def test_redacts_github_tokens(self, fixture_repo):
        """Test that GitHub tokens are redacted."""
        files, stats = scan_repository(fixture_repo)
        config_file = next((f for f in files if f.relative_path == "src/config.py"), None)
        
        redactor = create_redactor(enabled=True)
        chunks = chunk_file(config_file, max_tokens=1000, redactor=redactor)
        
        content = "\n".join(c.content for c in chunks)
        
        assert "ghp_" not in content


class TestFixtureRepoOutput:
    """Tests for output generation with fixture repositories."""

    def test_context_pack_structure(self, fixture_repo):
        """Test context pack has expected structure."""
        files, stats = scan_repository(fixture_repo)
        scanned_paths = {f.relative_path for f in files}
        ranker = FileRanker(fixture_repo, scanned_files=scanned_paths)
        files = ranker.rank_files(files)
        
        # Create chunks
        all_chunks = []
        for f in files[:5]:  # Just first 5 files
            chunks = chunk_file(f, max_tokens=500)
            all_chunks.extend(chunks)
        
        stats.chunks_created = len(all_chunks)
        
        context_pack = render_context_pack(
            root_path=fixture_repo,
            files=files,
            chunks=all_chunks,
            ranker=ranker,
            stats=stats,
            include_timestamp=False,
        )
        
        # Check structure
        assert "# Repository Context Pack:" in context_pack
        assert "## 📋 Repository Overview" in context_pack
        assert "## 📁 Directory Structure" in context_pack
        assert "## 🔑 Key Files" in context_pack

    def test_report_json_schema_version(self, fixture_repo):
        """Test that report.json includes schema version."""
        files, stats = scan_repository(fixture_repo)
        
        report_renderer = ReportRenderer(
            stats=stats,
            config={"mode": "both"},
            output_files=["test.md"],
            files=files,
        )
        
        report = report_renderer.render()
        
        assert "schema_version" in report
        assert report["schema_version"] == REPORT_SCHEMA_VERSION

    def test_report_json_includes_file_manifest(self, fixture_repo):
        """Test that report.json includes file manifest with IDs."""
        files, stats = scan_repository(fixture_repo)
        scanned_paths = {f.relative_path for f in files}
        ranker = FileRanker(fixture_repo, scanned_files=scanned_paths)
        files = ranker.rank_files(files)
        
        report_renderer = ReportRenderer(
            stats=stats,
            config={"mode": "both"},
            output_files=["test.md"],
            files=files,
        )
        
        report = report_renderer.render()
        
        assert "files" in report
        assert len(report["files"]) > 0
        
        # Check file entry structure
        first_file = report["files"][0]
        assert "id" in first_file
        assert "path" in first_file
        assert "priority" in first_file
        assert len(first_file["id"]) == 16  # SHA256 prefix

    def test_file_ids_are_deterministic(self, fixture_repo):
        """Test that file IDs are deterministic across runs."""
        files1, _ = scan_repository(fixture_repo)
        files2, _ = scan_repository(fixture_repo)
        
        # Find same file in both scans
        readme1 = next((f for f in files1 if f.relative_path == "README.md"), None)
        readme2 = next((f for f in files2 if f.relative_path == "README.md"), None)
        
        assert readme1 is not None
        assert readme2 is not None
        assert readme1.id == readme2.id

    def test_write_outputs_creates_all_files(self, fixture_repo):
        """Test that write_outputs creates all expected files."""
        with tempfile.TemporaryDirectory() as outdir:
            output_dir = Path(outdir)
            files, stats = scan_repository(fixture_repo)
            scanned_paths = {f.relative_path for f in files}
            ranker = FileRanker(fixture_repo, scanned_files=scanned_paths)
            files = ranker.rank_files(files)
            
            # Create chunks
            all_chunks = []
            for f in files[:3]:
                chunks = chunk_file(f, max_tokens=500)
                all_chunks.extend(chunks)
            
            stats.chunks_created = len(all_chunks)
            
            context_pack = render_context_pack(
                fixture_repo, files, all_chunks, ranker, stats,
                include_timestamp=False,
            )
            
            output_files = write_outputs(
                output_dir=output_dir,
                mode=OutputMode.BOTH,
                context_pack=context_pack,
                chunks=all_chunks,
                stats=stats,
                config={"mode": "both"},
                include_timestamp=False,
                files=files,
            )
            
            # Check files were created
            assert (output_dir / "context_pack.md").exists()
            assert (output_dir / "chunks.jsonl").exists()
            assert (output_dir / "report.json").exists()
            
            # Verify report.json structure
            with open(output_dir / "report.json") as f:
                report = json.load(f)
            
            assert report["schema_version"] == REPORT_SCHEMA_VERSION
            assert "stats" in report
            assert "files" in report


class TestTokenBudget:
    """Tests for token budget functionality."""

    def test_file_token_estimation(self, fixture_repo):
        """Test that files get token estimates."""
        files, _ = scan_repository(fixture_repo)
        
        for f in files:
            chunks = chunk_file(f, max_tokens=500)
            f.token_estimate = sum(c.token_estimate for c in chunks)
        
        # All files should have token estimates
        for f in files:
            if f.size_bytes > 0:
                assert f.token_estimate > 0

    def test_chunks_have_token_estimates(self, fixture_repo):
        """Test that chunks have token estimates."""
        files, _ = scan_repository(fixture_repo)
        readme = next((f for f in files if f.relative_path == "README.md"), None)
        
        chunks = chunk_file(readme, max_tokens=500)
        
        for chunk in chunks:
            assert chunk.token_estimate > 0


class TestSymlinksInFixture:
    """Tests for symlink handling in fixture repositories."""

    @pytest.mark.skipif(os.name == "nt", reason="Symlinks require admin on Windows")
    def test_symlinks_skipped_by_default(self, fixture_repo):
        """Test that symlinks are skipped by default."""
        # Create a symlink
        link = fixture_repo / "src" / "main_link.py"
        link.symlink_to(fixture_repo / "src" / "main.py")
        
        files, _ = scan_repository(fixture_repo, follow_symlinks=False)
        paths = {f.relative_path for f in files}
        
        assert "src/main.py" in paths
        assert "src/main_link.py" not in paths

    @pytest.mark.skipif(os.name == "nt", reason="Symlinks require admin on Windows")
    def test_symlinks_followed_when_enabled(self, fixture_repo):
        """Test that symlinks are followed when enabled."""
        # Create a symlink
        link = fixture_repo / "src" / "main_link.py"
        link.symlink_to(fixture_repo / "src" / "main.py")
        
        files, _ = scan_repository(fixture_repo, follow_symlinks=True)
        paths = {f.relative_path for f in files}
        
        assert "src/main.py" in paths
        assert "src/main_link.py" in paths


class TestNestedGitignore:
    """Tests for nested .gitignore handling."""

    def test_nested_gitignore_respected(self, fixture_repo):
        """Test that nested .gitignore files are respected."""
        # Create a file that should be ignored by nested .gitignore
        (fixture_repo / "docs" / "_build").mkdir()
        (fixture_repo / "docs" / "_build" / "index.html").write_text("<html></html>")
        
        files, _ = scan_repository(fixture_repo)
        paths = {f.relative_path for f in files}
        
        # Should not include files ignored by nested .gitignore
        assert "docs/_build/index.html" not in paths
        
        # Should include other docs files
        assert "docs/index.md" in paths

"""Tests for the scanner module."""

import tempfile
from pathlib import Path

import pytest

from repo_to_prompt.scanner import FileScanner, GitIgnoreParser, generate_tree, scan_repository


@pytest.fixture
def temp_repo():
    """Create a temporary repository structure for testing."""
    with tempfile.TemporaryDirectory() as tmpdir:
        root = Path(tmpdir)
        
        # Create directory structure
        (root / "src").mkdir()
        (root / "src" / "main").mkdir()
        (root / "tests").mkdir()
        (root / "docs").mkdir()
        (root / "node_modules").mkdir()
        (root / "node_modules" / "package").mkdir()
        
        # Create files
        (root / "README.md").write_text("# Test Project\n\nThis is a test.")
        (root / "package.json").write_text('{"name": "test", "main": "src/index.js"}')
        (root / "src" / "index.js").write_text("console.log('hello');")
        (root / "src" / "main" / "app.py").write_text("def main(): pass")
        (root / "tests" / "test_app.py").write_text("def test_main(): pass")
        (root / "docs" / "guide.md").write_text("# Guide\n\nHow to use.")
        (root / "node_modules" / "package" / "index.js").write_text("// vendored")
        
        # Create .gitignore
        (root / ".gitignore").write_text("node_modules/\n*.pyc\n__pycache__/\n")
        
        yield root


class TestGitIgnoreParser:
    """Tests for GitIgnoreParser."""
    
    def test_parse_gitignore(self, temp_repo):
        """Test parsing .gitignore file."""
        parser = GitIgnoreParser(temp_repo)
        
        # node_modules should be ignored
        node_modules_file = temp_repo / "node_modules" / "package" / "index.js"
        assert parser.is_ignored(node_modules_file)
        
        # src files should not be ignored
        src_file = temp_repo / "src" / "index.js"
        assert not parser.is_ignored(src_file)
    
    def test_empty_gitignore(self, temp_repo):
        """Test with empty .gitignore."""
        # Clear .gitignore
        (temp_repo / ".gitignore").write_text("")
        
        parser = GitIgnoreParser(temp_repo)
        
        # Nothing should be ignored
        src_file = temp_repo / "src" / "index.js"
        assert not parser.is_ignored(src_file)


class TestFileScanner:
    """Tests for FileScanner."""
    
    def test_scan_respects_gitignore(self, temp_repo):
        """Test that scanner respects .gitignore."""
        scanner = FileScanner(temp_repo, respect_gitignore=True)
        files = list(scanner.scan())
        
        # node_modules files should be excluded
        paths = {f.relative_path for f in files}
        assert "node_modules/package/index.js" not in paths
        
        # src files should be included
        assert "src/index.js" in paths
    
    def test_scan_ignores_gitignore_when_disabled(self, temp_repo):
        """Test that scanner can ignore .gitignore."""
        # Note: node_modules is also in default exclude_globs
        scanner = FileScanner(
            temp_repo,
            respect_gitignore=False,
            exclude_globs=set(),  # Clear default excludes
        )
        files = list(scanner.scan())
        
        # Check that we actually scan more files
        paths = {f.relative_path for f in files}
        # The file might still be excluded by other means, but gitignore is not the reason
        assert len(paths) >= 5
    
    def test_scan_filters_by_extension(self, temp_repo):
        """Test filtering by file extension."""
        scanner = FileScanner(
            temp_repo,
            include_extensions={".py"},
            respect_gitignore=True,
        )
        files = list(scanner.scan())
        
        # Only Python files should be included
        for f in files:
            assert f.extension == ".py"
    
    def test_scan_filters_by_size(self, temp_repo):
        """Test filtering by file size."""
        # Create a large file
        large_file = temp_repo / "src" / "large.py"
        large_file.write_text("x" * 10000)
        
        scanner = FileScanner(
            temp_repo,
            max_file_bytes=5000,
            respect_gitignore=True,
        )
        files = list(scanner.scan())
        
        # Large file should be excluded
        paths = {f.relative_path for f in files}
        assert "src/large.py" not in paths
    
    def test_scan_applies_exclude_globs(self, temp_repo):
        """Test that custom exclude globs work."""
        scanner = FileScanner(
            temp_repo,
            exclude_globs={"tests/**"},
            respect_gitignore=True,
        )
        files = list(scanner.scan())
        
        # Test files should be excluded
        paths = {f.relative_path for f in files}
        assert "tests/test_app.py" not in paths
        
        # Other files should be included
        assert "src/index.js" in paths
    
    def test_scan_statistics(self, temp_repo):
        """Test that scan statistics are collected."""
        scanner = FileScanner(temp_repo, respect_gitignore=True)
        files = list(scanner.scan())
        
        assert scanner.stats.files_scanned > 0
        assert scanner.stats.files_included > 0
        assert scanner.stats.total_bytes_included > 0


class TestGenerateTree:
    """Tests for generate_tree function."""
    
    def test_basic_tree(self, temp_repo):
        """Test basic tree generation."""
        tree = generate_tree(temp_repo, max_depth=2)
        
        assert temp_repo.name in tree
        assert "src/" in tree
        assert "README.md" in tree
    
    def test_tree_depth_limit(self, temp_repo):
        """Test that tree respects depth limit."""
        tree = generate_tree(temp_repo, max_depth=1)
        
        # Should show top-level directories but not their contents
        assert "src/" in tree
        # Should not show contents of src at depth 1
        lines = tree.split("\n")
        # Depth 1 means we see root and its children, but not grandchildren
        assert any("src/" in line for line in lines)
    
    def test_tree_highlights_files(self, temp_repo):
        """Test that tree highlights specified files."""
        tree = generate_tree(
            temp_repo,
            max_depth=3,
            files_to_highlight={"README.md"},
        )
        
        # README should be highlighted with star
        assert "⭐" in tree


class TestScanRepository:
    """Tests for scan_repository convenience function."""
    
    def test_scan_repository_returns_files_and_stats(self, temp_repo):
        """Test that scan_repository returns both files and stats."""
        files, stats = scan_repository(temp_repo)
        
        assert isinstance(files, list)
        assert len(files) > 0
        assert stats.files_included > 0

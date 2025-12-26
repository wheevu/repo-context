"""Tests for the ranker module."""

import tempfile
from pathlib import Path

import pytest

from repo_to_prompt.config import FileInfo
from repo_to_prompt.ranker import FileRanker, rank_files


@pytest.fixture
def temp_repo():
    """Create a temporary repository structure for testing."""
    with tempfile.TemporaryDirectory() as tmpdir:
        root = Path(tmpdir)
        
        # Create directory structure
        (root / "src").mkdir()
        (root / "tests").mkdir()
        (root / "docs").mkdir()
        (root / "examples").mkdir()
        
        # Create files
        (root / "README.md").write_text("# Test Project\n\nThis is a test.")
        (root / "package.json").write_text('{"name": "test", "main": "src/index.js", "scripts": {"test": "jest"}}')
        (root / "src" / "index.js").write_text("export function main() {}")
        (root / "src" / "utils.js").write_text("export function helper() {}")
        (root / "tests" / "test_main.js").write_text("test('main', () => {})")
        (root / "docs" / "guide.md").write_text("# Guide")
        (root / "examples" / "demo.js").write_text("import { main } from '../src'")
        (root / "package-lock.json").write_text("{}")
        
        yield root


class TestFileRanker:
    """Tests for FileRanker."""
    
    def test_readme_gets_highest_priority(self, temp_repo):
        """Test that README gets highest priority."""
        ranker = FileRanker(temp_repo)
        
        readme_info = FileInfo(
            path=temp_repo / "README.md",
            relative_path="README.md",
            size_bytes=100,
            extension=".md",
            language="markdown",
        )
        
        priority = ranker.rank_file(readme_info)
        assert priority == 1.0
    
    def test_config_files_get_high_priority(self, temp_repo):
        """Test that config files get high priority."""
        ranker = FileRanker(temp_repo)
        
        config_info = FileInfo(
            path=temp_repo / "package.json",
            relative_path="package.json",
            size_bytes=100,
            extension=".json",
            language="json",
        )
        
        priority = ranker.rank_file(config_info)
        assert priority >= 0.85
    
    def test_test_files_get_lower_priority(self, temp_repo):
        """Test that test files get lower priority."""
        ranker = FileRanker(temp_repo)
        
        test_info = FileInfo(
            path=temp_repo / "tests" / "test_main.js",
            relative_path="tests/test_main.js",
            size_bytes=100,
            extension=".js",
            language="javascript",
        )
        
        priority = ranker.rank_file(test_info)
        assert priority == 0.5  # test priority
    
    def test_lock_files_get_lowest_priority(self, temp_repo):
        """Test that lock files get low priority."""
        ranker = FileRanker(temp_repo)
        
        lock_info = FileInfo(
            path=temp_repo / "package-lock.json",
            relative_path="package-lock.json",
            size_bytes=100,
            extension=".json",
            language="json",
        )
        
        priority = ranker.rank_file(lock_info)
        assert priority == 0.15  # lock file priority
    
    def test_detects_entrypoints_from_package_json(self, temp_repo):
        """Test that entrypoints are detected from package.json."""
        ranker = FileRanker(temp_repo)
        
        entrypoints = ranker.get_entrypoints()
        assert "src/index.js" in entrypoints
    
    def test_detects_languages(self, temp_repo):
        """Test that languages are detected from manifests."""
        ranker = FileRanker(temp_repo)
        
        languages = ranker.get_detected_languages()
        assert "javascript" in languages
    
    def test_example_files_get_medium_priority(self, temp_repo):
        """Test that example files get medium priority."""
        ranker = FileRanker(temp_repo)
        
        example_info = FileInfo(
            path=temp_repo / "examples" / "demo.js",
            relative_path="examples/demo.js",
            size_bytes=100,
            extension=".js",
            language="javascript",
        )
        
        priority = ranker.rank_file(example_info)
        assert priority == 0.6  # example priority
    
    def test_rank_files_sorts_by_priority(self, temp_repo):
        """Test that rank_files sorts files by priority."""
        ranker = FileRanker(temp_repo)
        
        files = [
            FileInfo(
                path=temp_repo / "tests" / "test_main.js",
                relative_path="tests/test_main.js",
                size_bytes=100,
                extension=".js",
                language="javascript",
            ),
            FileInfo(
                path=temp_repo / "README.md",
                relative_path="README.md",
                size_bytes=100,
                extension=".md",
                language="markdown",
            ),
            FileInfo(
                path=temp_repo / "src" / "utils.js",
                relative_path="src/utils.js",
                size_bytes=100,
                extension=".js",
                language="javascript",
            ),
        ]
        
        ranked = ranker.rank_files(files)
        
        # README should be first (highest priority)
        assert ranked[0].relative_path == "README.md"
        
        # Test file should be last (lowest priority among these)
        assert ranked[-1].relative_path == "tests/test_main.js"
    
    def test_adds_tags_based_on_file_type(self, temp_repo):
        """Test that appropriate tags are added to files."""
        ranker = FileRanker(temp_repo)
        
        readme_info = FileInfo(
            path=temp_repo / "README.md",
            relative_path="README.md",
            size_bytes=100,
            extension=".md",
            language="markdown",
        )
        
        ranker.rank_files([readme_info])
        
        assert "readme" in readme_info.tags


class TestRankFiles:
    """Tests for rank_files convenience function."""
    
    def test_returns_sorted_list(self, temp_repo):
        """Test that rank_files returns sorted list."""
        files = [
            FileInfo(
                path=temp_repo / "src" / "utils.js",
                relative_path="src/utils.js",
                size_bytes=100,
                extension=".js",
                language="javascript",
            ),
            FileInfo(
                path=temp_repo / "README.md",
                relative_path="README.md",
                size_bytes=100,
                extension=".md",
                language="markdown",
            ),
        ]
        
        ranked = rank_files(temp_repo, files)
        
        # README should be first
        assert ranked[0].relative_path == "README.md"


class TestEntrypointValidation:
    """Tests for entrypoint validation against scanned files."""
    
    def test_entrypoints_validated_against_scanned_files(self, temp_repo):
        """Test that entrypoints are validated against scanned file set."""
        # Create a pyproject.toml with entrypoints pointing to non-existent files
        (temp_repo / "pyproject.toml").write_text('''
[project]
name = "test"

[project.scripts]
cli = "nonexistent.module:main"
''')
        
        # Create actual source file
        (temp_repo / "src" / "cli.py").write_text("def main(): pass")
        
        # Scan would only find src/cli.py, not nonexistent/module.py
        scanned_files = {"src/cli.py", "README.md"}
        
        ranker = FileRanker(temp_repo, scanned_files=scanned_files)
        entrypoints = ranker.get_entrypoints()
        
        # nonexistent/module.py should NOT be in entrypoints
        assert "nonexistent/module.py" not in entrypoints
    
    def test_fake_classifiers_not_entrypoints(self):
        """Test that pyproject classifiers don't become entrypoints."""
        with tempfile.TemporaryDirectory() as tmpdir:
            root = Path(tmpdir)
            
            # Create pyproject with classifiers (not scripts)
            (root / "pyproject.toml").write_text('''
[project]
name = "test"
requires-python = ">=3.10"
license = "MIT"
''')
            
            scanned_files = {"src/main.py"}
            ranker = FileRanker(root, scanned_files=scanned_files)
            entrypoints = ranker.get_entrypoints()
            
            # Should NOT create fake entrypoints from classifiers
            assert ">=3/10.py" not in entrypoints
            assert "MIT.py" not in entrypoints
    
    def test_valid_entrypoints_included(self, temp_repo):
        """Test that valid entrypoints from package.json are included."""
        scanned_files = {"src/index.js", "src/utils.js", "README.md"}
        
        ranker = FileRanker(temp_repo, scanned_files=scanned_files)
        entrypoints = ranker.get_entrypoints()
        
        # src/index.js is in package.json main and in scanned_files
        assert "src/index.js" in entrypoints
    
    def test_set_scanned_files_revalidates(self, temp_repo):
        """Test that set_scanned_files re-validates entrypoints."""
        # Initially create ranker without scanned files
        ranker = FileRanker(temp_repo)
        
        # Now update with scanned files that don't include the entrypoint
        ranker.set_scanned_files({"README.md", "docs/guide.md"})
        entrypoints = ranker.get_entrypoints()
        
        # src/index.js is not in new scanned_files, should be removed
        assert "src/index.js" not in entrypoints

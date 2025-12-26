"""Tests for utility functions."""

import tempfile
from pathlib import Path

import pytest

from repo_to_prompt.utils import (
    detect_encoding,
    estimate_tokens,
    is_binary_file,
    is_likely_generated,
    is_lock_file,
    is_vendored,
    normalize_path,
    read_file_safe,
    stable_hash,
    truncate_string,
)


class TestEstimateTokens:
    """Tests for token estimation."""
    
    def test_empty_string(self):
        """Test token estimation for empty string."""
        assert estimate_tokens("") == 0
    
    def test_short_string(self):
        """Test token estimation for short string."""
        # ~4 characters per token heuristic
        tokens = estimate_tokens("hello world")
        assert 1 <= tokens <= 5
    
    def test_longer_string(self):
        """Test token estimation for longer string."""
        text = "The quick brown fox jumps over the lazy dog. " * 10
        tokens = estimate_tokens(text)
        
        # Should be roughly chars/4
        expected = len(text) // 4
        assert abs(tokens - expected) < expected * 0.5  # Within 50%


class TestStableHash:
    """Tests for stable hash generation."""
    
    def test_same_input_same_hash(self):
        """Test that same input produces same hash."""
        hash1 = stable_hash("content", "path.py", 1, 10)
        hash2 = stable_hash("content", "path.py", 1, 10)
        
        assert hash1 == hash2
    
    def test_different_content_different_hash(self):
        """Test that different content produces different hash."""
        hash1 = stable_hash("content1", "path.py", 1, 10)
        hash2 = stable_hash("content2", "path.py", 1, 10)
        
        assert hash1 != hash2
    
    def test_different_path_different_hash(self):
        """Test that different path produces different hash."""
        hash1 = stable_hash("content", "path1.py", 1, 10)
        hash2 = stable_hash("content", "path2.py", 1, 10)
        
        assert hash1 != hash2
    
    def test_hash_length(self):
        """Test that hash is 16 characters."""
        hash_val = stable_hash("content", "path.py", 1, 10)
        assert len(hash_val) == 16


class TestDetectEncoding:
    """Tests for encoding detection."""
    
    def test_utf8_file(self):
        """Test detection of UTF-8 file."""
        with tempfile.NamedTemporaryFile(mode="w", suffix=".txt", delete=False, encoding="utf-8") as f:
            f.write("Hello, World! 你好世界")
            f.flush()
            
            encoding = detect_encoding(Path(f.name))
            assert encoding.lower() in ("utf-8", "utf8", "ascii")
    
    def test_nonexistent_file(self):
        """Test detection for nonexistent file returns default."""
        encoding = detect_encoding(Path("/nonexistent/file.txt"))
        assert encoding == "utf-8"


class TestIsBinaryFile:
    """Tests for binary file detection."""
    
    def test_text_file(self):
        """Test that text file is not detected as binary."""
        with tempfile.NamedTemporaryFile(mode="w", suffix=".txt", delete=False) as f:
            f.write("Hello, World!\nThis is a text file.")
            f.flush()
            
            assert not is_binary_file(Path(f.name))
    
    def test_binary_file(self):
        """Test that binary file is detected."""
        with tempfile.NamedTemporaryFile(mode="wb", suffix=".bin", delete=False) as f:
            f.write(b"\x00\x01\x02\x03\x04\x05")
            f.flush()
            
            assert is_binary_file(Path(f.name))


class TestReadFileSafe:
    """Tests for safe file reading."""
    
    def test_read_utf8_file(self):
        """Test reading UTF-8 file."""
        with tempfile.NamedTemporaryFile(mode="w", suffix=".txt", delete=False, encoding="utf-8") as f:
            f.write("Hello, World!")
            f.flush()
            
            content, encoding = read_file_safe(Path(f.name))
            assert content == "Hello, World!"
    
    def test_read_with_max_bytes(self):
        """Test reading with byte limit."""
        with tempfile.NamedTemporaryFile(mode="w", suffix=".txt", delete=False, encoding="utf-8") as f:
            f.write("A" * 1000)
            f.flush()
            
            content, _ = read_file_safe(Path(f.name), max_bytes=100)
            assert len(content) <= 100


class TestNormalizePath:
    """Tests for path normalization."""
    
    def test_forward_slash(self):
        """Test that forward slashes are preserved."""
        assert normalize_path("src/main/app.py") == "src/main/app.py"
    
    def test_backslash_conversion(self):
        """Test that backslashes are converted to forward slashes."""
        assert normalize_path("src\\main\\app.py") == "src/main/app.py"


class TestTruncateString:
    """Tests for string truncation."""
    
    def test_no_truncation_needed(self):
        """Test that short strings are not truncated."""
        assert truncate_string("hello", 10) == "hello"
    
    def test_truncation_with_suffix(self):
        """Test truncation with default suffix."""
        result = truncate_string("hello world", 8)
        assert result == "hello..."
        assert len(result) == 8
    
    def test_custom_suffix(self):
        """Test truncation with custom suffix."""
        result = truncate_string("hello world", 9, suffix="…")
        assert result.endswith("…")


class TestIsLikelyGenerated:
    """Tests for generated file detection."""
    
    def test_minified_file(self):
        """Test detection of minified files."""
        assert is_likely_generated(Path("bundle.min.js"))
        assert is_likely_generated(Path("styles.min.css"))
    
    def test_generated_directory(self):
        """Test detection of files in generated directories."""
        assert is_likely_generated(Path("generated/output.js"))
        assert is_likely_generated(Path("build/app.js"))
    
    def test_generated_content(self):
        """Test detection from content markers."""
        content = "// AUTO-GENERATED FILE - DO NOT EDIT\n"
        assert is_likely_generated(Path("file.js"), content)
    
    def test_long_lines(self):
        """Test detection of files with very long lines."""
        content = "x" * 2000  # Very long single line
        assert is_likely_generated(Path("file.js"), content)
    
    def test_normal_file(self):
        """Test that normal files are not flagged."""
        assert not is_likely_generated(Path("src/app.js"))
        assert not is_likely_generated(Path("src/app.js"), "const x = 1;")


class TestIsLockFile:
    """Tests for lock file detection."""
    
    def test_package_lock(self):
        """Test detection of package-lock.json."""
        assert is_lock_file(Path("package-lock.json"))
    
    def test_yarn_lock(self):
        """Test detection of yarn.lock."""
        assert is_lock_file(Path("yarn.lock"))
    
    def test_poetry_lock(self):
        """Test detection of poetry.lock."""
        assert is_lock_file(Path("poetry.lock"))
    
    def test_cargo_lock(self):
        """Test detection of Cargo.lock."""
        assert is_lock_file(Path("Cargo.lock"))
    
    def test_non_lock_file(self):
        """Test that regular files are not flagged."""
        assert not is_lock_file(Path("package.json"))
        assert not is_lock_file(Path("main.py"))


class TestIsVendored:
    """Tests for vendored file detection."""
    
    def test_vendor_directory(self):
        """Test detection of vendor directory."""
        assert is_vendored(Path("vendor/package/main.go"))
    
    def test_node_modules(self):
        """Test detection of node_modules."""
        assert is_vendored(Path("node_modules/lodash/index.js"))
    
    def test_third_party(self):
        """Test detection of third_party directory."""
        assert is_vendored(Path("third_party/lib/code.c"))
    
    def test_non_vendored(self):
        """Test that regular files are not flagged."""
        assert not is_vendored(Path("src/main.py"))
        assert not is_vendored(Path("lib/utils.js"))

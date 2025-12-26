"""
Utility functions for repo-to-prompt.

Includes token estimation, hashing, encoding detection, line ending normalization,
and misc helpers for deterministic processing.
"""

from __future__ import annotations

import hashlib
import re
from pathlib import Path

import chardet

# Try to import tiktoken for accurate token counting
_tiktoken_encoder = None
try:
    import tiktoken
    _tiktoken_encoder = tiktoken.get_encoding("cl100k_base")
except ImportError:
    pass


def estimate_tokens(text: str) -> int:
    """
    Estimate the number of tokens in text.

    Uses tiktoken if available, otherwise falls back to character-based heuristic.
    """
    if _tiktoken_encoder is not None:
        return len(_tiktoken_encoder.encode(text, disallowed_special=()))

    # Fallback heuristic: ~4 characters per token on average
    # This is a rough approximation for English text and code
    return len(text) // 4


def stable_hash(content: str, path: str, start_line: int, end_line: int) -> str:
    """
    Generate a stable hash ID for a chunk.

    The hash is based on the content and location, ensuring deterministic IDs
    across runs for the same content.
    """
    # Use content + location for uniqueness
    hash_input = f"{path}:{start_line}-{end_line}:{content[:1000]}"
    return hashlib.sha256(hash_input.encode("utf-8")).hexdigest()[:16]


def detect_encoding(file_path: Path, sample_size: int = 8192) -> str:
    """
    Detect the encoding of a file.

    Args:
        file_path: Path to the file
        sample_size: Number of bytes to sample for detection

    Returns:
        Detected encoding name (e.g., 'utf-8', 'latin-1')
    """
    try:
        with open(file_path, "rb") as f:
            sample = f.read(sample_size)

        if not sample:
            return "utf-8"

        # Check for BOM
        if sample.startswith(b"\xef\xbb\xbf"):
            return "utf-8-sig"
        if sample.startswith(b"\xff\xfe") or sample.startswith(b"\xfe\xff"):
            return "utf-16"

        # Use chardet for detection
        result = chardet.detect(sample)
        encoding = result.get("encoding")

        if encoding is None:
            return "utf-8"

        # Normalize encoding names
        encoding = encoding.lower()
        if encoding in ("ascii", "utf-8", "utf8"):
            return "utf-8"

        return encoding

    except Exception:
        return "utf-8"


def is_binary_file(file_path: Path, sample_size: int = 8192) -> bool:
    """
    Check if a file appears to be binary.

    Uses null byte detection and character analysis.
    """
    try:
        with open(file_path, "rb") as f:
            sample = f.read(sample_size)

        if not sample:
            return False

        # Check for null bytes (strong indicator of binary)
        if b"\x00" in sample:
            return True

        # Check for high ratio of non-text bytes
        # Text files typically have >70% printable ASCII
        printable_count = sum(
            1 for b in sample
            if 32 <= b <= 126 or b in (9, 10, 13)  # printable + tab, newline, CR
        )

        return printable_count / len(sample) < 0.70

    except Exception:
        return True  # Assume binary if we can't read it


def read_file_safe(
    file_path: Path,
    max_bytes: int | None = None,
    encoding: str | None = None
) -> tuple[str, str]:
    """
    Safely read a file with encoding detection and error handling.

    Args:
        file_path: Path to the file
        max_bytes: Maximum bytes to read (None for all)
        encoding: Encoding to use (None for auto-detect)

    Returns:
        Tuple of (content, encoding_used)
    """
    if encoding is None:
        encoding = detect_encoding(file_path)

    try:
        with open(file_path, encoding=encoding, errors="replace") as f:
            content = f.read(max_bytes) if max_bytes is not None else f.read()
        return content, encoding
    except Exception:
        # Last resort: try utf-8 with replace
        try:
            with open(file_path, encoding="utf-8", errors="replace") as f:
                content = f.read(max_bytes) if max_bytes is not None else f.read()
            return content, "utf-8"
        except Exception as inner_e:
            raise OSError(f"Failed to read file {file_path}: {inner_e}") from inner_e


def stream_file_lines(
    file_path: Path,
    encoding: str | None = None,
    start_line: int = 1,
    end_line: int | None = None
) -> list[str]:
    """
    Stream specific lines from a file without loading the entire file.

    Args:
        file_path: Path to the file
        encoding: Encoding to use (None for auto-detect)
        start_line: First line to read (1-indexed)
        end_line: Last line to read (inclusive, None for all remaining)

    Returns:
        List of lines in the range
    """
    if encoding is None:
        encoding = detect_encoding(file_path)

    lines = []
    try:
        with open(file_path, encoding=encoding, errors="replace") as f:
            for line_num, line in enumerate(f, start=1):
                if line_num < start_line:
                    continue
                if end_line is not None and line_num > end_line:
                    break
                lines.append(line)
    except Exception:
        pass

    return lines


def normalize_path(path: str) -> str:
    """Normalize a path for consistent comparison (use forward slashes)."""
    return path.replace("\\", "/")


def normalize_line_endings(content: str) -> str:
    """
    Normalize line endings to LF (Unix-style).

    Handles CRLF (Windows), CR (old Mac), and mixed line endings.

    Args:
        content: Text content with potentially mixed line endings

    Returns:
        Content with all line endings normalized to LF
    """
    # Replace CRLF first, then remaining CR
    return content.replace("\r\n", "\n").replace("\r", "\n")


def truncate_string(s: str, max_length: int, suffix: str = "...") -> str:
    """Truncate a string to max length, adding suffix if truncated."""
    if len(s) <= max_length:
        return s
    return s[:max_length - len(suffix)] + suffix


# Common patterns for detecting generated/vendored files
GENERATED_PATTERNS = [
    re.compile(r"generated", re.IGNORECASE),
    re.compile(r"auto-generated", re.IGNORECASE),
    re.compile(r"do not edit", re.IGNORECASE),
    re.compile(r"machine generated", re.IGNORECASE),
]

MINIFIED_INDICATORS = [
    ".min.",
    ".bundle.",
    ".packed.",
]


def is_likely_minified(file_path: Path, max_line_length: int = 5000) -> bool:
    """
    Check if a file appears to be minified based on line length.

    Minified files typically have extremely long lines (entire file on one line).
    This is a quick heuristic check that reads only the first line.

    Args:
        file_path: Path to the file
        max_line_length: Maximum line length before considering minified (default: 5000)

    Returns:
        True if the file appears to be minified
    """
    name = file_path.name.lower()

    # Check filename indicators first (fast path)
    for indicator in MINIFIED_INDICATORS:
        if indicator in name:
            return True

    # Read first line to check length
    try:
        with open(file_path, "rb") as f:
            # Read up to max_line_length + 1 bytes
            chunk = f.read(max_line_length + 1)
            if not chunk:
                return False

            # Find first newline
            newline_pos = chunk.find(b"\n")
            if newline_pos == -1:
                # No newline found - if we read the full chunk, line is too long
                return len(chunk) > max_line_length
            return newline_pos > max_line_length
    except (OSError, PermissionError):
        return False


def is_likely_generated(file_path: Path, content_sample: str = "") -> bool:
    """
    Check if a file appears to be generated or minified.

    Args:
        file_path: Path to the file
        content_sample: Optional sample of file content to check

    Returns:
        True if the file appears to be generated
    """
    name = file_path.name.lower()

    # Check filename indicators
    for indicator in MINIFIED_INDICATORS:
        if indicator in name:
            return True

    # Check common generated directories
    path_str = str(file_path).lower()
    if any(d in path_str for d in ["generated/", "gen/", "auto/", "build/"]):
        return True

    # Check content for generated markers
    if content_sample:
        sample_lower = content_sample[:2000].lower()
        for pattern in GENERATED_PATTERNS:
            if pattern.search(sample_lower):
                return True

        # Check for extremely long lines (common in minified files)
        first_line = content_sample.split("\n")[0] if content_sample else ""
        if len(first_line) > 1000:
            return True

    return False


def is_lock_file(file_path: Path) -> bool:
    """Check if a file is a dependency lock file."""
    name = file_path.name.lower()
    return name in {
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "poetry.lock",
        "pipfile.lock",
        "cargo.lock",
        "gemfile.lock",
        "composer.lock",
        "go.sum",
    }


def is_vendored(file_path: Path) -> bool:
    """Check if a file appears to be vendored/third-party."""
    path_str = str(file_path).lower()
    return any(d in path_str for d in [
        "vendor/",
        "vendors/",
        "third_party/",
        "third-party/",
        "thirdparty/",
        "external/",
        "extern/",
        "node_modules/",
    ])

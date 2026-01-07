"""
Utility functions for repo-to-prompt.

Includes token estimation, hashing, encoding detection, line ending normalization,
and misc helpers for deterministic processing.
"""

from __future__ import annotations

import hashlib
import re
from pathlib import Path
from typing import Any

import chardet

# Try to import tiktoken for accurate token counting
_tiktoken_encoder: Any | None = None
try:
    import tiktoken

    try:
        # Some environments (sandboxed CI, restricted containers) may have `tiktoken`
        # installed but unable to initialize its resources. In that case, gracefully
        # fall back to the heuristic estimator rather than failing at import time.
        _tiktoken_encoder = tiktoken.get_encoding("cl100k_base")
    except Exception:
        _tiktoken_encoder = None
except ImportError:
    pass


def estimate_tokens(text: str) -> int:
    """Estimate token count for a string.

    Uses `tiktoken` when available for a closer approximation to OpenAI-style tokenization;
    otherwise falls back to a lightweight heuristic.

    Args:
        text: Input text to estimate tokens for.

    Returns:
        Estimated number of tokens in `text`.
    """
    if _tiktoken_encoder is not None:
        return len(_tiktoken_encoder.encode(text, disallowed_special=()))

    # Fallback heuristic (~4 chars/token) keeps the tool usable without optional deps.
    return len(text) // 4


def stable_hash(content: str, path: str, start_line: int, end_line: int) -> str:
    """Generate a deterministic short hash for a content chunk.

    The ID is intentionally stable across runs for the same content and location so that
    downstream pipelines (diffing, caching, embeddings) can reference chunks consistently.

    Args:
        content: Chunk content (text).
        path: File path associated with the chunk (typically repo-relative).
        start_line: 1-indexed start line for the chunk.
        end_line: 1-indexed end line (inclusive) for the chunk.

    Returns:
        A short, deterministic hex string identifier.
    """
    # Include location + a capped content prefix to reduce collisions without hashing entire blobs.
    hash_input = f"{path}:{start_line}-{end_line}:{content[:1000]}"
    return hashlib.sha256(hash_input.encode("utf-8")).hexdigest()[:16]


def detect_encoding(file_path: Path, sample_size: int = 8192) -> str:
    """Detect a likely text encoding for a file.

    The implementation deliberately prefers UTF-8 and only uses `chardet` when strict UTF-8
    decoding fails. This reduces false positives where UTF-8 is misdetected as Latin-1/CP1252,
    which would produce mojibake and destabilize diffs.

    Args:
        file_path: Path to the file to inspect.
        sample_size: Number of bytes to sample from the start of the file.

    Returns:
        A normalized encoding label (e.g., `"utf-8"`, `"utf-8-sig"`, `"utf-16-le"`).
    """
    try:
        with open(file_path, "rb") as f:
            sample = f.read(sample_size)

        if not sample:
            return "utf-8"

        # Check for BOM markers first (most reliable)
        if sample.startswith(b"\xef\xbb\xbf"):
            return "utf-8-sig"
        if sample.startswith(b"\xff\xfe"):
            return "utf-16-le"
        if sample.startswith(b"\xfe\xff"):
            return "utf-16-be"

        # Try UTF-8 first - most source files are UTF-8
        # If the content decodes cleanly as UTF-8, use it
        try:
            sample.decode("utf-8")
            return "utf-8"
        except UnicodeDecodeError:
            pass

        # Fall back to chardet for non-UTF-8 files
        result = chardet.detect(sample)
        encoding_any = result.get("encoding")

        if not isinstance(encoding_any, str) or not encoding_any:
            return "utf-8"

        # Normalize encoding names
        encoding = encoding_any.lower()
        if encoding in ("ascii", "utf-8", "utf8"):
            return "utf-8"

        return encoding

    except Exception:
        return "utf-8"


def is_binary_file(file_path: Path, sample_size: int = 8192) -> bool:
    """Heuristically determine whether a file is binary.

    Uses a fast null-byte check first (strong binary signal), then falls back to a ratio of
    printable ASCII bytes. This intentionally biases toward *treating unreadable files as
    binary* to avoid crashing the pipeline on weird encodings or permissions.

    Args:
        file_path: Path to the file to test.
        sample_size: Number of bytes to sample from the file start.

    Returns:
        True if the file is likely binary, otherwise False.
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
            1
            for b in sample
            if 32 <= b <= 126 or b in (9, 10, 13)  # printable + tab, newline, CR
        )

        return printable_count / len(sample) < 0.70

    except Exception:
        return True  # Assume binary if we can't read it


def read_file_safe(
    file_path: Path, max_bytes: int | None = None, encoding: str | None = None
) -> tuple[str, str]:
    """Read a file robustly with encoding detection and safe error handling.

    Strategy:
    - If `encoding` is explicitly provided, use it.
    - Otherwise, try strict UTF-8 first (most modern repos are UTF-8).
    - If UTF-8 fails, detect encoding and retry using `errors="replace"` to avoid crashes.

    Args:
        file_path: Path to the file to read.
        max_bytes: Optional maximum number of characters to read (None reads entire file).
        encoding: Optional explicit encoding to use (None enables auto-detection).

    Returns:
        A tuple `(content, encoding_used)`.

    Raises:
        OSError: If the file cannot be read even with fallback strategies.
    """
    # If encoding specified, use it directly
    if encoding is not None:
        try:
            with open(file_path, encoding=encoding, errors="replace") as f:
                content = f.read(max_bytes) if max_bytes is not None else f.read()
            return content, encoding
        except LookupError:
            # Unknown encoding, fall through to auto-detect
            pass

    # Try UTF-8 first (strict mode to detect issues)
    try:
        with open(file_path, encoding="utf-8", errors="strict") as f:
            content = f.read(max_bytes) if max_bytes is not None else f.read()
        return content, "utf-8"
    except UnicodeDecodeError:
        # UTF-8 failed, try detecting encoding
        pass
    except Exception:
        # Other error (file not found, permission, etc.)
        pass

    # Fall back to encoding detection
    detected = detect_encoding(file_path)
    try:
        with open(file_path, encoding=detected, errors="replace") as f:
            content = f.read(max_bytes) if max_bytes is not None else f.read()
        return content, detected
    except Exception:
        # Last resort: UTF-8 with replacement
        try:
            with open(file_path, encoding="utf-8", errors="replace") as f:
                content = f.read(max_bytes) if max_bytes is not None else f.read()
            return content, "utf-8"
        except Exception as inner_e:
            raise OSError(f"Failed to read file {file_path}: {inner_e}") from inner_e


def stream_file_lines(
    file_path: Path, encoding: str | None = None, start_line: int = 1, end_line: int | None = None
) -> list[str]:
    """Read a line range from a file without loading it entirely.

    Args:
        file_path: Path to the file to read.
        encoding: Optional encoding (None triggers auto-detection).
        start_line: 1-indexed first line to include.
        end_line: 1-indexed last line to include (inclusive). None reads to EOF.

    Returns:
        A list of lines (including original line endings) in the requested range.
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
    """Normalize a path for consistent cross-platform comparisons.

    Args:
        path: Path string that may contain platform-specific separators.

    Returns:
        Normalized path using forward slashes.
    """
    return path.replace("\\", "/")


def normalize_line_endings(content: str) -> str:
    """Normalize line endings to LF (Unix-style).

    Args:
        content: Input text that may contain CRLF/CR/mixed endings.

    Returns:
        Content with all line endings normalized to LF.
    """
    # Replace CRLF first, then remaining CR, to avoid double-transforming CRLF.
    return content.replace("\r\n", "\n").replace("\r", "\n")


def truncate_string(s: str, max_length: int, suffix: str = "...") -> str:
    """Truncate a string to a maximum length, appending a suffix if needed.

    Args:
        s: Input string.
        max_length: Maximum length of the returned string.
        suffix: Suffix to append when truncation occurs.

    Returns:
        The original string if it fits, otherwise a truncated version ending with `suffix`.
    """
    if len(s) <= max_length:
        return s
    return s[: max_length - len(suffix)] + suffix


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
    """Heuristically detect minified/bundled files by line length.

    Minified files often collapse large amounts of code into a single extremely long line.
    This function reads only a small prefix to keep scanning fast.

    Args:
        file_path: Path to the file.
        max_line_length: Threshold line length used to classify a file as minified.

    Returns:
        True if the file appears to be minified, otherwise False.
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
    """Heuristically detect generated or machine-produced files.

    Uses a combination of filename hints, directory location, and common “generated” markers
    in the file header. The goal is to deprioritize noisy artifacts (bundles, build outputs)
    without requiring heavyweight parsing.

    Args:
        file_path: Path to the file.
        content_sample: Optional content snippet used for header-marker checks.

    Returns:
        True if the file appears to be generated/minified, otherwise False.
    """
    name = file_path.name.lower()

    # Check filename indicators
    for indicator in MINIFIED_INDICATORS:
        if indicator in name:
            return True

    # Check common generated directories - normalize path for cross-platform
    path_str = normalize_path(str(file_path)).lower()
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
    """Check whether a path is a dependency lock file.

    Args:
        file_path: Path to check.

    Returns:
        True if the filename matches a known lock-file name.
    """
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
    """Check whether a path likely belongs to vendored/third-party code.

    Args:
        file_path: Path to check.

    Returns:
        True if the path contains a known vendor directory segment.
    """
    # Normalize path for cross-platform compatibility
    path_str = normalize_path(str(file_path)).lower()
    return any(
        d in path_str
        for d in [
            "vendor/",
            "vendors/",
            "third_party/",
            "third-party/",
            "thirdparty/",
            "external/",
            "extern/",
            "node_modules/",
        ]
    )

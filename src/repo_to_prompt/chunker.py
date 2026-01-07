"""
Chunking module for repo-to-prompt.

Provides language-aware and line-based chunking strategies.
"""

from __future__ import annotations

import re
from abc import ABC, abstractmethod
from collections.abc import Generator
from dataclasses import dataclass
from pathlib import Path

from .config import Chunk, FileInfo
from .redactor import Redactor
from .utils import estimate_tokens, read_file_safe, stable_hash


@dataclass
class ChunkBoundary:
    """Represents a potential chunk boundary within a file.

    Attributes:
        line_number: 0-indexed line number within the file content.
        boundary_type: Category label for the boundary (e.g., "definition", "blank").
        weight: Relative desirability of breaking at this boundary (higher is better).
    """

    line_number: int
    boundary_type: str  # "function", "class", "block", "paragraph"
    weight: float  # Higher weight = better break point


class BaseChunker(ABC):
    """Abstract base class for chunkers."""

    @abstractmethod
    def chunk(
        self,
        file_info: FileInfo,
        content: str,
        max_tokens: int,
        overlap_tokens: int,
    ) -> Generator[Chunk, None, None]:
        """Generate chunks from file content.

        Args:
            file_info: Metadata about the file being chunked.
            content: Full file content (already loaded and optionally redacted).
            max_tokens: Target maximum tokens per chunk.
            overlap_tokens: Approximate token overlap between adjacent chunks.

        Yields:
            `Chunk` objects describing contiguous regions of the input content.
        """
        raise NotImplementedError


class LineBasedChunker(BaseChunker):
    """
    Simple line-based chunker with awareness of natural boundaries.

    Tries to break at paragraph boundaries, blank lines, and logical divisions.
    """

    # Patterns that indicate good break points
    BREAK_PATTERNS = [
        (re.compile(r"^\s*$"), 1.0),  # Blank lines
        (re.compile(r"^#+\s"), 0.9),  # Markdown headers
        (re.compile(r"^(def |async def |class |function |const |let |var |export |import )"), 0.85),
        (re.compile(r"^(public |private |protected |static )"), 0.8),
        (re.compile(r"^(\s*)\}"), 0.7),  # Closing braces
        (re.compile(r"^//|^#|^/\*"), 0.6),  # Comments
    ]

    def find_boundaries(self, lines: list[str]) -> list[ChunkBoundary]:
        """Find likely chunk boundaries based on regex heuristics.

        Args:
            lines: File content split into lines (with or without line endings).

        Returns:
            A list of `ChunkBoundary` candidates with weights indicating break quality.
        """
        boundaries = []

        for i, line in enumerate(lines):
            for pattern, weight in self.BREAK_PATTERNS:
                if pattern.match(line):
                    boundaries.append(
                        ChunkBoundary(
                            line_number=i,
                            boundary_type="pattern",
                            weight=weight,
                        )
                    )
                    break

        return boundaries

    def chunk(
        self,
        file_info: FileInfo,
        content: str,
        max_tokens: int,
        overlap_tokens: int,
    ) -> Generator[Chunk, None, None]:
        """Chunk content using a line-based strategy with soft boundary selection.

        This chunker aims for deterministic, stable chunk boundaries by selecting the best
        boundary near an "ideal" chunk end.

        Args:
            file_info: Metadata about the file being chunked.
            content: Full content of the file.
            max_tokens: Target maximum tokens per chunk.
            overlap_tokens: Approximate overlap tokens between chunks.

        Yields:
            Chunks covering the file content in order.
        """
        lines = content.splitlines(keepends=True)
        if not lines:
            return

        boundaries = self.find_boundaries(lines)
        boundary_lines = {b.line_number: b for b in boundaries}

        # Estimate tokens per line (rough average)
        total_tokens = estimate_tokens(content)
        avg_tokens_per_line = max(1, total_tokens / len(lines)) if lines else 1

        # Target lines per chunk
        target_lines = int(max_tokens / avg_tokens_per_line)
        overlap_lines = int(overlap_tokens / avg_tokens_per_line)

        current_start = 0

        while current_start < len(lines):
            # Calculate ideal end position
            ideal_end = min(current_start + target_lines, len(lines))

            # Find best boundary near ideal end
            best_end = ideal_end
            best_boundary_weight = 0.0

            # Look for boundaries in the last 20% of the chunk
            search_start = int(current_start + target_lines * 0.8)
            for line_num in range(search_start, min(ideal_end + 10, len(lines))):
                if line_num in boundary_lines:
                    boundary = boundary_lines[line_num]
                    if boundary.weight > best_boundary_weight:
                        best_boundary_weight = boundary.weight
                        best_end = line_num

            # Ensure we make progress
            if best_end <= current_start:
                best_end = min(current_start + target_lines, len(lines))

            # Extract chunk content
            chunk_lines = lines[current_start:best_end]
            chunk_content = "".join(chunk_lines)

            if chunk_content.strip():
                chunk_id = stable_hash(
                    chunk_content,
                    file_info.relative_path,
                    current_start + 1,
                    best_end,
                )

                yield Chunk(
                    id=chunk_id,
                    path=file_info.relative_path,
                    language=file_info.language,
                    start_line=current_start + 1,  # 1-indexed
                    end_line=best_end,
                    content=chunk_content,
                    priority=file_info.priority,
                    tags=file_info.tags.copy(),
                    token_estimate=estimate_tokens(chunk_content),
                )

            # Move to next chunk with overlap
            current_start = max(current_start + 1, best_end - overlap_lines)


class MarkdownChunker(BaseChunker):
    """
    Markdown-aware chunker.

    Respects heading structure and tries to keep sections together.
    """

    HEADING_PATTERN = re.compile(r"^(#{1,6})\s+(.+)$", re.MULTILINE)

    def chunk(
        self,
        file_info: FileInfo,
        content: str,
        max_tokens: int,
        overlap_tokens: int,
    ) -> Generator[Chunk, None, None]:
        """Chunk Markdown content while respecting section headers.

        Args:
            file_info: Metadata about the file being chunked.
            content: Markdown content to split into sections/chunks.
            max_tokens: Target maximum tokens per chunk.
            overlap_tokens: Approximate overlap tokens between chunks.

        Yields:
            Chunks that try to preserve heading-bounded sections.
        """
        lines = content.splitlines(keepends=True)
        if not lines:
            return

        # Find all headings
        sections = []
        current_section_start = 0
        current_heading = ""

        for i, line in enumerate(lines):
            match = self.HEADING_PATTERN.match(line.strip())
            if match:
                # Save previous section
                if i > current_section_start:
                    sections.append(
                        (
                            current_section_start,
                            i,
                            current_heading,
                        )
                    )
                current_section_start = i
                current_heading = match.group(2)

        # Don't forget the last section
        if current_section_start < len(lines):
            sections.append(
                (
                    current_section_start,
                    len(lines),
                    current_heading,
                )
            )

        # If no sections found, fall back to line-based chunking
        if not sections:
            line_chunker = LineBasedChunker()
            yield from line_chunker.chunk(file_info, content, max_tokens, overlap_tokens)
            return

        # Process sections, combining small ones and splitting large ones
        for start, end, heading in sections:
            section_content = "".join(lines[start:end])
            section_tokens = estimate_tokens(section_content)

            if section_tokens <= max_tokens:
                # Section fits in one chunk
                if section_content.strip():
                    chunk_id = stable_hash(
                        section_content,
                        file_info.relative_path,
                        start + 1,
                        end,
                    )

                    tags = file_info.tags.copy()
                    if heading:
                        tags.append(f"section:{heading[:30]}")

                    yield Chunk(
                        id=chunk_id,
                        path=file_info.relative_path,
                        language=file_info.language,
                        start_line=start + 1,
                        end_line=end,
                        content=section_content,
                        priority=file_info.priority,
                        tags=tags,
                        token_estimate=section_tokens,
                    )
            else:
                # Section too large, use line-based chunking
                line_chunker = LineBasedChunker()
                section_file_info = FileInfo(
                    path=file_info.path,
                    relative_path=file_info.relative_path,
                    size_bytes=len(section_content),
                    extension=file_info.extension,
                    language=file_info.language,
                    priority=file_info.priority,
                    tags=file_info.tags.copy(),
                )

                for chunk in line_chunker.chunk(
                    section_file_info,
                    section_content,
                    max_tokens,
                    overlap_tokens,
                ):
                    # Adjust line numbers
                    chunk.start_line += start
                    chunk.end_line += start
                    chunk.id = stable_hash(
                        chunk.content,
                        file_info.relative_path,
                        chunk.start_line,
                        chunk.end_line,
                    )
                    yield chunk


class CodeChunker(BaseChunker):
    """
    Code-aware chunker using Tree-sitter when available.

    Falls back to pattern-based chunking if Tree-sitter is not installed.
    """

    # Patterns for detecting function/class definitions
    DEFINITION_PATTERNS = {
        "python": [
            re.compile(r"^(async\s+)?def\s+\w+"),
            re.compile(r"^class\s+\w+"),
        ],
        "javascript": [
            re.compile(r"^(export\s+)?(async\s+)?function\s+\w+"),
            re.compile(r"^(export\s+)?class\s+\w+"),
            re.compile(r"^(export\s+)?(const|let|var)\s+\w+\s*=\s*(async\s+)?\("),
            re.compile(r"^(export\s+)?(const|let|var)\s+\w+\s*=\s*\{"),
        ],
        "typescript": [
            re.compile(r"^(export\s+)?(async\s+)?function\s+\w+"),
            re.compile(r"^(export\s+)?class\s+\w+"),
            re.compile(r"^(export\s+)?(const|let|var)\s+\w+\s*=\s*(async\s+)?\("),
            re.compile(r"^(export\s+)?interface\s+\w+"),
            re.compile(r"^(export\s+)?type\s+\w+"),
        ],
        "go": [
            re.compile(r"^func\s+(\([^)]+\)\s+)?\w+"),
            re.compile(r"^type\s+\w+\s+(struct|interface)"),
        ],
        "java": [
            re.compile(r"^(public|private|protected)?\s*(static\s+)?\w+\s+\w+\s*\("),
            re.compile(r"^(public|private|protected)?\s*(abstract\s+)?class\s+\w+"),
            re.compile(r"^(public\s+)?interface\s+\w+"),
        ],
        "rust": [
            re.compile(r"^(pub\s+)?fn\s+\w+"),
            re.compile(r"^(pub\s+)?struct\s+\w+"),
            re.compile(r"^(pub\s+)?enum\s+\w+"),
            re.compile(r"^(pub\s+)?trait\s+\w+"),
            re.compile(r"^impl\s+"),
        ],
    }

    def find_code_boundaries(self, lines: list[str], language: str) -> list[ChunkBoundary]:
        """Find boundaries in code based on lightweight pattern matching.

        This is intentionally heuristic: it avoids requiring AST/Tree-sitter while still
        preferring breaks near definitions.

        Args:
            lines: File content split into lines.
            language: Normalized language label from `FileInfo.language`.

        Returns:
            A list of candidate boundaries.
        """
        boundaries = []
        patterns = self.DEFINITION_PATTERNS.get(language, [])

        for i, line in enumerate(lines):
            stripped = line.lstrip()

            # Check language-specific patterns
            for pattern in patterns:
                if pattern.match(stripped):
                    boundaries.append(
                        ChunkBoundary(
                            line_number=i,
                            boundary_type="definition",
                            weight=0.95,
                        )
                    )
                    break

            # Generic boundaries
            if stripped == "":
                boundaries.append(
                    ChunkBoundary(
                        line_number=i,
                        boundary_type="blank",
                        weight=0.5,
                    )
                )
            elif stripped.startswith("//") or stripped.startswith("#") or stripped.startswith("/*"):
                boundaries.append(
                    ChunkBoundary(
                        line_number=i,
                        boundary_type="comment",
                        weight=0.6,
                    )
                )

        return boundaries

    def chunk(
        self,
        file_info: FileInfo,
        content: str,
        max_tokens: int,
        overlap_tokens: int,
    ) -> Generator[Chunk, None, None]:
        """Chunk source code, preferring breaks at definitions.

        Args:
            file_info: Metadata about the file being chunked.
            content: File content to chunk.
            max_tokens: Target maximum tokens per chunk.
            overlap_tokens: Approximate overlap tokens between chunks.

        Yields:
            Chunks covering the source code in order.
        """
        lines = content.splitlines(keepends=True)
        if not lines:
            return

        boundaries = self.find_code_boundaries(lines, file_info.language)

        # Group boundaries by type for smarter chunking
        definition_lines = {b.line_number for b in boundaries if b.boundary_type == "definition"}

        # Estimate tokens per line
        total_tokens = estimate_tokens(content)
        avg_tokens_per_line = max(1, total_tokens / len(lines)) if lines else 1
        target_lines = int(max_tokens / avg_tokens_per_line)
        overlap_lines = int(overlap_tokens / avg_tokens_per_line)

        current_start = 0

        while current_start < len(lines):
            ideal_end = min(current_start + target_lines, len(lines))

            # Prefer ending at a definition boundary
            best_end = ideal_end

            # Look ahead for definition boundaries
            for line_num in range(ideal_end, min(ideal_end + 20, len(lines))):
                if line_num in definition_lines:
                    best_end = line_num
                    break

            # Look back if we didn't find one ahead
            if best_end == ideal_end:
                for line_num in range(ideal_end - 1, max(current_start, ideal_end - 30), -1):
                    if line_num in definition_lines:
                        best_end = line_num
                        break

            # Ensure progress
            if best_end <= current_start:
                best_end = min(current_start + target_lines, len(lines))

            chunk_lines = lines[current_start:best_end]
            chunk_content = "".join(chunk_lines)

            if chunk_content.strip():
                chunk_id = stable_hash(
                    chunk_content,
                    file_info.relative_path,
                    current_start + 1,
                    best_end,
                )

                yield Chunk(
                    id=chunk_id,
                    path=file_info.relative_path,
                    language=file_info.language,
                    start_line=current_start + 1,
                    end_line=best_end,
                    content=chunk_content,
                    priority=file_info.priority,
                    tags=file_info.tags.copy(),
                    token_estimate=estimate_tokens(chunk_content),
                )

            current_start = max(current_start + 1, best_end - overlap_lines)


class ChunkerFactory:
    """Factory for creating appropriate chunkers based on file type."""

    CODE_LANGUAGES = {
        "python",
        "javascript",
        "typescript",
        "go",
        "java",
        "rust",
        "c",
        "cpp",
        "csharp",
        "ruby",
        "php",
        "swift",
        "kotlin",
        "scala",
    }

    MARKDOWN_LANGUAGES = {
        "markdown",
        "restructuredtext",
        "asciidoc",
    }

    @classmethod
    def get_chunker(cls, language: str) -> BaseChunker:
        """Select a chunker implementation based on a language label.

        Args:
            language: Normalized language label (e.g., `"python"`, `"markdown"`).

        Returns:
            A `BaseChunker` implementation.
        """
        if language in cls.MARKDOWN_LANGUAGES:
            return MarkdownChunker()
        elif language in cls.CODE_LANGUAGES:
            return CodeChunker()
        else:
            return LineBasedChunker()


def chunk_file(
    file_info: FileInfo,
    max_tokens: int = 800,
    overlap_tokens: int = 120,
    redactor: Redactor | None = None,
) -> list[Chunk]:
    """Chunk a file into pieces suitable for prompting and retrieval.

    Args:
        file_info: File metadata including path, language, and ranking priority.
        max_tokens: Target maximum tokens per chunk.
        overlap_tokens: Approximate overlap tokens between adjacent chunks.
        redactor: Optional redactor used to remove secrets from content prior to chunking.

    Returns:
        A list of chunks produced from the file content.

    Raises:
        OSError: If the file cannot be read.
    """
    # Read file content
    content, _ = read_file_safe(file_info.path)

    # Apply redaction if enabled
    if redactor:
        content = redactor.redact(content)

    # Get appropriate chunker
    chunker = ChunkerFactory.get_chunker(file_info.language)

    # Generate chunks
    chunks = list(chunker.chunk(file_info, content, max_tokens, overlap_tokens))

    return chunks


def chunk_content(
    content: str,
    path: str,
    language: str,
    max_tokens: int = 800,
    overlap_tokens: int = 120,
    priority: float = 0.5,
    tags: list[str] | None = None,
) -> list[Chunk]:
    """Chunk an in-memory content string directly.

    Useful for tests or cases where file I/O is already handled upstream.

    Args:
        content: Content to chunk.
        path: Pseudo-path used for chunk metadata (often repo-relative).
        language: Language label to select chunking strategy.
        max_tokens: Target maximum tokens per chunk.
        overlap_tokens: Approximate overlap tokens between adjacent chunks.
        priority: Priority score to propagate into chunks.
        tags: Optional tags to propagate into chunks.

    Returns:
        A list of chunks produced from `content`.
    """
    file_info = FileInfo(
        path=Path(path),
        relative_path=path,
        size_bytes=len(content),
        extension=Path(path).suffix,
        language=language,
        priority=priority,
        tags=tags or [],
    )

    chunker = ChunkerFactory.get_chunker(language)
    return list(chunker.chunk(file_info, content, max_tokens, overlap_tokens))


def coalesce_small_chunks(
    chunks: list[Chunk],
    min_tokens: int = 200,
    max_tokens: int = 800,
) -> list[Chunk]:
    """
    Coalesce small adjacent chunks from the same file.

    This reduces chunk explosion by merging tiny chunks (below min_tokens)
    with their neighbors, as long as the result doesn't exceed max_tokens.

    Args:
        chunks: List of chunks to coalesce
        min_tokens: Minimum tokens below which chunks are merged
        max_tokens: Maximum tokens for merged chunks

    Returns:
        New list of coalesced chunks.
    """
    if not chunks:
        return []

    # Group chunks by file path to only merge within same file
    from collections import defaultdict

    by_file: dict[str, list[Chunk]] = defaultdict(list)
    for chunk in chunks:
        by_file[chunk.path].append(chunk)

    result: list[Chunk] = []

    for _path, file_chunks in by_file.items():
        # Sort by start line to ensure correct order
        file_chunks.sort(key=lambda c: c.start_line)

        coalesced: list[Chunk] = []
        current: Chunk | None = None

        for chunk in file_chunks:
            if current is None:
                current = chunk
                continue

            # Check if we should merge
            combined_tokens = current.token_estimate + chunk.token_estimate
            can_merge = (
                current.token_estimate < min_tokens or chunk.token_estimate < min_tokens
            ) and combined_tokens <= max_tokens

            # Also ensure chunks are adjacent or overlapping
            is_adjacent = chunk.start_line <= current.end_line + 1

            if can_merge and is_adjacent:
                # Merge chunks
                merged_content = current.content

                # If there's a gap or overlap, handle it
                if chunk.start_line > current.end_line:
                    # Small gap - just concatenate
                    merged_content = current.content + chunk.content
                elif chunk.start_line <= current.end_line:
                    # Overlapping - need to deduplicate
                    # Split content into lines and merge
                    current_lines = current.content.splitlines(keepends=True)
                    # We intentionally keep `chunk.content` as the authoritative tail; we only
                    # need the current-lines prefix to avoid duplicating overlapped lines.

                    # Calculate overlap
                    overlap_start = chunk.start_line - current.start_line
                    if overlap_start < len(current_lines):
                        # Take current's content up to overlap, then all of chunk
                        merged_content = "".join(current_lines[:overlap_start]) + chunk.content
                    else:
                        merged_content = current.content + chunk.content

                # Create merged chunk
                current = Chunk(
                    id=stable_hash(
                        merged_content,
                        current.path,
                        current.start_line,
                        chunk.end_line,
                    ),
                    path=current.path,
                    language=current.language,
                    start_line=current.start_line,
                    end_line=chunk.end_line,
                    content=merged_content,
                    priority=max(current.priority, chunk.priority),
                    tags=list(set(current.tags) | set(chunk.tags)),
                    token_estimate=estimate_tokens(merged_content),
                )
            else:
                # Can't merge, save current and start new
                coalesced.append(current)
                current = chunk

        # Don't forget the last chunk
        if current is not None:
            coalesced.append(current)

        result.extend(coalesced)

    # Restore original order by sorting by path then start_line
    result.sort(key=lambda c: (c.path, c.start_line))

    return result

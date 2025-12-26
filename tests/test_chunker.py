"""Tests for the chunker module."""

import pytest

from repo_to_prompt.chunker import (
    ChunkerFactory,
    CodeChunker,
    LineBasedChunker,
    MarkdownChunker,
    chunk_content,
)


class TestLineBasedChunker:
    """Tests for LineBasedChunker."""
    
    def test_chunks_simple_content(self):
        """Test chunking simple content."""
        content = "\n".join([f"Line {i}" for i in range(100)])
        
        chunks = chunk_content(
            content=content,
            path="test.txt",
            language="text",
            max_tokens=50,
            overlap_tokens=10,
        )
        
        assert len(chunks) > 1
        
        # Each chunk should have valid line numbers
        for chunk in chunks:
            assert chunk.start_line >= 1
            assert chunk.end_line >= chunk.start_line
            assert chunk.content.strip()
    
    def test_chunks_have_overlap(self):
        """Test that chunks have overlap."""
        content = "\n".join([f"Line {i}" for i in range(100)])
        
        chunks = chunk_content(
            content=content,
            path="test.txt",
            language="text",
            max_tokens=100,
            overlap_tokens=20,
        )
        
        # Check for overlapping line ranges in consecutive chunks
        for i in range(len(chunks) - 1):
            current = chunks[i]
            next_chunk = chunks[i + 1]
            # Next chunk should start before or at current chunk's end
            # (accounting for overlap)
            assert next_chunk.start_line <= current.end_line + 10
    
    def test_single_chunk_for_small_content(self):
        """Test that small content results in single chunk."""
        content = "Hello world"
        
        chunks = chunk_content(
            content=content,
            path="test.txt",
            language="text",
            max_tokens=1000,
            overlap_tokens=100,
        )
        
        assert len(chunks) == 1
        assert chunks[0].content.strip() == "Hello world"
    
    def test_chunk_ids_are_stable(self):
        """Test that chunk IDs are deterministic."""
        content = "Hello\nWorld\nTest"
        
        chunks1 = chunk_content(content, "test.txt", "text", 1000, 100)
        chunks2 = chunk_content(content, "test.txt", "text", 1000, 100)
        
        assert chunks1[0].id == chunks2[0].id
    
    def test_chunk_ids_differ_for_different_content(self):
        """Test that different content produces different IDs."""
        chunks1 = chunk_content("Hello", "test.txt", "text", 1000, 100)
        chunks2 = chunk_content("World", "test.txt", "text", 1000, 100)
        
        assert chunks1[0].id != chunks2[0].id


class TestMarkdownChunker:
    """Tests for MarkdownChunker."""
    
    def test_chunks_by_heading(self):
        """Test that markdown is chunked by headings."""
        content = """# Introduction

This is the intro section.
It has multiple paragraphs.

More intro text here.

# Features

This is the features section.
It describes features.

## Sub-feature

A sub-feature description.

# Conclusion

Final thoughts.
"""
        
        chunks = chunk_content(
            content=content,
            path="test.md",
            language="markdown",
            max_tokens=500,
            overlap_tokens=50,
        )
        
        # Should have multiple chunks based on sections
        assert len(chunks) >= 1
        
        # First chunk should contain Introduction
        assert "Introduction" in chunks[0].content or "intro" in chunks[0].content.lower()
    
    def test_preserves_heading_in_chunk(self):
        """Test that headings are preserved in chunks."""
        content = """# My Heading

Some content under the heading.
"""
        
        chunks = chunk_content(
            content=content,
            path="test.md",
            language="markdown",
            max_tokens=500,
            overlap_tokens=50,
        )
        
        assert "# My Heading" in chunks[0].content


class TestCodeChunker:
    """Tests for CodeChunker."""
    
    def test_chunks_python_by_functions(self):
        """Test that Python code is chunked at function boundaries."""
        content = '''def function_one():
    """First function."""
    x = 1
    y = 2
    return x + y


def function_two():
    """Second function."""
    a = 10
    b = 20
    return a * b


def function_three():
    """Third function."""
    pass


class MyClass:
    """A class."""
    
    def method_one(self):
        pass
    
    def method_two(self):
        pass
'''
        
        chunks = chunk_content(
            content=content,
            path="test.py",
            language="python",
            max_tokens=100,
            overlap_tokens=20,
        )
        
        # Should create multiple chunks
        assert len(chunks) >= 1
        
        # Each chunk should have valid structure
        for chunk in chunks:
            assert chunk.language == "python"
            assert chunk.path == "test.py"
    
    def test_chunks_javascript_by_functions(self):
        """Test that JavaScript code is chunked appropriately."""
        content = '''function greet(name) {
    console.log("Hello, " + name);
}

const add = (a, b) => {
    return a + b;
};

class Calculator {
    constructor() {
        this.value = 0;
    }
    
    add(x) {
        this.value += x;
        return this;
    }
}

export function main() {
    const calc = new Calculator();
    calc.add(5);
}
'''
        
        chunks = chunk_content(
            content=content,
            path="test.js",
            language="javascript",
            max_tokens=100,
            overlap_tokens=20,
        )
        
        assert len(chunks) >= 1
        for chunk in chunks:
            assert chunk.language == "javascript"


class TestChunkerFactory:
    """Tests for ChunkerFactory."""
    
    def test_returns_code_chunker_for_python(self):
        """Test that factory returns CodeChunker for Python."""
        chunker = ChunkerFactory.get_chunker("python")
        assert isinstance(chunker, CodeChunker)
    
    def test_returns_code_chunker_for_javascript(self):
        """Test that factory returns CodeChunker for JavaScript."""
        chunker = ChunkerFactory.get_chunker("javascript")
        assert isinstance(chunker, CodeChunker)
    
    def test_returns_markdown_chunker_for_markdown(self):
        """Test that factory returns MarkdownChunker for Markdown."""
        chunker = ChunkerFactory.get_chunker("markdown")
        assert isinstance(chunker, MarkdownChunker)
    
    def test_returns_line_chunker_for_unknown(self):
        """Test that factory returns LineBasedChunker for unknown languages."""
        chunker = ChunkerFactory.get_chunker("unknown")
        assert isinstance(chunker, LineBasedChunker)


class TestChunkContent:
    """Tests for chunk_content convenience function."""
    
    def test_assigns_priority(self):
        """Test that priority is assigned to chunks."""
        chunks = chunk_content(
            content="test content",
            path="test.txt",
            language="text",
            max_tokens=1000,
            overlap_tokens=100,
            priority=0.9,
        )
        
        assert chunks[0].priority == 0.9
    
    def test_assigns_tags(self):
        """Test that tags are assigned to chunks."""
        chunks = chunk_content(
            content="test content",
            path="test.txt",
            language="text",
            max_tokens=1000,
            overlap_tokens=100,
            tags=["important", "readme"],
        )
        
        assert "important" in chunks[0].tags
        assert "readme" in chunks[0].tags
    
    def test_estimates_tokens(self):
        """Test that token count is estimated."""
        content = "word " * 100  # ~100 words
        
        chunks = chunk_content(
            content=content,
            path="test.txt",
            language="text",
            max_tokens=1000,
            overlap_tokens=100,
        )
        
        # Token estimate should be reasonable (roughly chars/4)
        assert chunks[0].token_estimate > 0
        assert chunks[0].token_estimate < len(content)


class TestCoalesceSmallChunks:
    """Tests for chunk coalescing."""
    
    def test_coalesces_small_chunks_from_same_file(self):
        """Test that small adjacent chunks are merged."""
        from repo_to_prompt.chunker import coalesce_small_chunks
        from repo_to_prompt.config import Chunk
        
        # Create several small chunks from the same file
        chunks = [
            Chunk(
                id="chunk1",
                path="test.py",
                language="python",
                start_line=1,
                end_line=5,
                content="# Line 1\n# Line 2\n# Line 3\n# Line 4\n# Line 5\n",
                priority=0.5,
                tags=[],
                token_estimate=20,  # Small
            ),
            Chunk(
                id="chunk2",
                path="test.py",
                language="python",
                start_line=6,
                end_line=10,
                content="# Line 6\n# Line 7\n# Line 8\n# Line 9\n# Line 10\n",
                priority=0.5,
                tags=[],
                token_estimate=20,  # Small
            ),
        ]
        
        result = coalesce_small_chunks(chunks, min_tokens=50, max_tokens=800)
        
        # Should merge into one chunk
        assert len(result) == 1
        assert result[0].start_line == 1
        assert result[0].end_line == 10
    
    def test_does_not_coalesce_large_chunks(self):
        """Test that chunks above min_tokens are not merged."""
        from repo_to_prompt.chunker import coalesce_small_chunks
        from repo_to_prompt.config import Chunk
        
        chunks = [
            Chunk(
                id="chunk1",
                path="test.py",
                language="python",
                start_line=1,
                end_line=50,
                content="x" * 1000,
                priority=0.5,
                tags=[],
                token_estimate=300,  # Above min_tokens
            ),
            Chunk(
                id="chunk2",
                path="test.py",
                language="python",
                start_line=51,
                end_line=100,
                content="y" * 1000,
                priority=0.5,
                tags=[],
                token_estimate=300,  # Above min_tokens
            ),
        ]
        
        result = coalesce_small_chunks(chunks, min_tokens=200, max_tokens=800)
        
        # Should NOT merge since both are above min_tokens
        assert len(result) == 2
    
    def test_does_not_merge_across_files(self):
        """Test that chunks from different files are not merged."""
        from repo_to_prompt.chunker import coalesce_small_chunks
        from repo_to_prompt.config import Chunk
        
        chunks = [
            Chunk(
                id="chunk1",
                path="file1.py",
                language="python",
                start_line=1,
                end_line=5,
                content="# File 1",
                priority=0.5,
                tags=[],
                token_estimate=10,
            ),
            Chunk(
                id="chunk2",
                path="file2.py",  # Different file
                language="python",
                start_line=1,
                end_line=5,
                content="# File 2",
                priority=0.5,
                tags=[],
                token_estimate=10,
            ),
        ]
        
        result = coalesce_small_chunks(chunks, min_tokens=50, max_tokens=800)
        
        # Should NOT merge across files
        assert len(result) == 2
    
    def test_respects_max_tokens(self):
        """Test that merged chunks don't exceed max_tokens."""
        from repo_to_prompt.chunker import coalesce_small_chunks
        from repo_to_prompt.config import Chunk
        
        # Create chunks that would exceed max_tokens if merged
        chunks = [
            Chunk(
                id=f"chunk{i}",
                path="test.py",
                language="python",
                start_line=i * 10 + 1,
                end_line=(i + 1) * 10,
                content=f"# Block {i}\n" * 10,
                priority=0.5,
                tags=[],
                token_estimate=100,
            )
            for i in range(5)
        ]
        
        result = coalesce_small_chunks(chunks, min_tokens=150, max_tokens=250)
        
        # Should merge some but not all (max 250 tokens)
        assert len(result) < len(chunks)
        for chunk in result:
            assert chunk.token_estimate <= 250
    
    def test_empty_input(self):
        """Test coalescing empty list."""
        from repo_to_prompt.chunker import coalesce_small_chunks
        
        result = coalesce_small_chunks([], min_tokens=200, max_tokens=800)
        assert result == []

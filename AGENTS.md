# Agent Guide for repo-to-prompt

This guide is for AI coding agents working in the `repo-to-prompt` codebase. It contains build commands, code style conventions, and project-specific guidelines.

## Project Overview

**repo-to-prompt** is a Python CLI tool that converts code repositories into LLM-friendly context packs for prompting and RAG (Retrieval-Augmented Generation) workflows.

- **Language**: Python 3.10+ (supports 3.10, 3.11, 3.12)
- **Build System**: Hatchling (modern Python packaging)
- **CLI Framework**: Typer with Rich for progress UI
- **Validation**: Pydantic v2 for config models
- **License**: MIT

## Build & Development Commands

### Setup

```bash
# Create virtual environment
python -m venv .venv
source .venv/bin/activate  # Windows: .venv\Scripts\activate

# Install in development mode with dev dependencies
pip install -e ".[dev]"

# Install with all optional dependencies (tiktoken, tree-sitter)
pip install -e ".[all]"

# Install pre-commit hooks (recommended)
pip install pre-commit && pre-commit install
```

### Testing

```bash
# Run all tests
pytest

# Run tests with verbose output
pytest -v

# Run tests with coverage report
pytest --cov=repo_to_prompt --cov-report=xml --cov-report=term-missing

# Run a single test file
pytest tests/test_ranker.py

# Run a single test function
pytest tests/test_ranker.py::TestFileRanker::test_readme_gets_highest_priority

# Run tests matching a pattern
pytest -k "test_readme"

# Run tests with short traceback (default in config)
pytest --tb=short

# Stop on first failure
pytest -x

# Run tests quietly (minimal output)
pytest -q
```

### Linting & Formatting

```bash
# Run ruff linter (with auto-fix)
ruff check src tests --fix

# Run ruff linter (check only, no fix)
ruff check src tests

# Format code with ruff
ruff format src tests

# Check formatting without changes
ruff format --check src tests
```

### Type Checking

```bash
# Run mypy type checker
mypy src --ignore-missing-imports --no-error-summary

# Run mypy with strict mode (configured in pyproject.toml)
mypy src
```

### Running the CLI

```bash
# Export a local repository
repo-to-prompt export --path /path/to/repo
r2p export -p /path/to/repo  # Short alias

# Export from GitHub
repo-to-prompt export --repo https://github.com/owner/repo

# View repository info
repo-to-prompt info /path/to/repo

# Run from source (during development)
python -m repo_to_prompt.cli export --path /path/to/repo
```

### Pre-commit Hooks

```bash
# Run all pre-commit hooks manually
pre-commit run --all-files

# Run specific hook
pre-commit run ruff --all-files
pre-commit run mypy --all-files
```

## Code Style Guidelines

### General Conventions

- **Line length**: 100 characters (enforced by ruff)
- **Python version**: Target 3.10+ (use modern syntax)
- **Docstrings**: Use Google-style docstrings for all public functions, classes, and modules
- **Type hints**: Required for all function signatures (mypy strict mode enabled)
- **String quotes**: Double quotes preferred (enforced by ruff format)

### Imports

**Order** (managed by ruff, isort-compatible):
1. `from __future__ import annotations` (always first in modules)
2. Standard library imports
3. Third-party imports
4. Local/relative imports

**Example**:
```python
from __future__ import annotations

import sys
from pathlib import Path
from typing import Any

import typer
from rich.console import Console

from .config import OutputMode
from .utils import estimate_tokens
```

### Naming Conventions

- **Modules**: `snake_case` (e.g., `config_loader.py`, `file_scanner.py`)
- **Classes**: `PascalCase` (e.g., `FileRanker`, `RedactionConfig`)
- **Functions/Methods**: `snake_case` (e.g., `scan_repository`, `chunk_file`)
- **Constants**: `UPPER_SNAKE_CASE` (e.g., `DEFAULT_CHUNK_TOKENS`, `REPORT_SCHEMA_VERSION`)
- **Private**: Prefix with `_` (e.g., `_internal_helper`)
- **Type variables**: `PascalCase` with `T` suffix (e.g., `ConfigT`)

### Type Hints

- Use `from __future__ import annotations` for forward references
- Always annotate function parameters and return types
- Use modern syntax: `list[str]` not `List[str]`, `dict[str, Any]` not `Dict[str, Any]`
- Use `Path` from `pathlib` for file paths, not `str`
- Use `| None` instead of `Optional[...]` (PEP 604)

**Example**:
```python
def scan_repository(
    root: Path,
    include_extensions: set[str] | None = None,
    exclude_globs: list[str] | None = None,
) -> list[FileInfo]:
    """Scan repository and return file info."""
    ...
```

### Error Handling

- Use custom exceptions for domain-specific errors (e.g., `FetchError`, `ConfigError`)
- Provide clear, actionable error messages
- Use Typer's `Exit` for CLI errors with appropriate exit codes
- Log errors with Rich console for user-facing messages
- Never swallow exceptions silently

**Example**:
```python
from .fetcher import FetchError

try:
    repo_ctx = fetch_repo(repo_url)
except FetchError as e:
    console.print(f"[red]Error:[/red] {e}")
    raise typer.Exit(1)
```

### Configuration & Data Classes

- Use Pydantic models for validated configuration
- Use `@dataclass` for simple data containers
- Use `Enum` for fixed choices (e.g., `OutputMode`)
- Provide sensible defaults

**Example**:
```python
from enum import Enum
from pydantic import BaseModel, Field

class OutputMode(str, Enum):
    PROMPT = "prompt"
    RAG = "rag"
    BOTH = "both"

class ScanConfig(BaseModel):
    max_file_bytes: int = Field(default=1_048_576, ge=0)
    include_extensions: set[str] = Field(default_factory=set)
```

### Testing Conventions

- Use pytest fixtures for test setup
- Class-based test organization: `class TestFeatureName:`
- Descriptive test names: `test_<what>_<condition>` (e.g., `test_readme_gets_highest_priority`)
- One assertion per test when possible
- Use `pytest.mark.parametrize` for multiple test cases
- Use `tempfile.TemporaryDirectory()` for file system tests

**Example**:
```python
import pytest
from pathlib import Path

@pytest.fixture
def temp_repo():
    """Create a temporary repository for testing."""
    with tempfile.TemporaryDirectory() as tmpdir:
        root = Path(tmpdir)
        (root / "README.md").write_text("# Test")
        yield root

class TestFileRanker:
    """Tests for FileRanker."""

    def test_readme_gets_highest_priority(self, temp_repo):
        """Test that README files receive priority 1.0."""
        ranker = FileRanker(temp_repo)
        priority = ranker.rank_file(readme_info)
        assert priority == 1.0
```

## Architecture & Design Principles

### Pipeline Architecture

The tool follows a modular pipeline:
1. **Fetch** (`fetcher.py`) → Get repository (local or GitHub)
2. **Scan** (`scanner.py`) → Discover files with .gitignore respect
3. **Rank** (`ranker.py`) → Prioritize important files
4. **Chunk** (`chunker.py`) → Split content into model-friendly sizes
5. **Redact** (`redactor.py`) → Remove secrets safely
6. **Render** (`renderer.py`) → Generate outputs (Markdown, JSONL)

### Key Design Principles

- **Deterministic**: Same input → same output (stable sorting, reproducible hashes)
- **High signal over volume**: Prioritize READMEs, configs, entrypoints over tests/locks
- **Language-aware**: Structure-based chunking for Python, JS/TS, Go, Java, Rust
- **UTF-8 fidelity**: Preserve emojis, smart quotes, international characters
- **Structure-safe redaction**: Never break syntax (AST-validated for Python)
- **Concurrent I/O**: Use thread pools for file scanning performance
- **Cross-platform**: Support macOS, Linux, Windows

### Performance Considerations

- Use `pathlib.Path` for cross-platform path handling
- Use `ThreadPoolExecutor` for concurrent file I/O (see `scanner.py`)
- Respect size limits (`max_file_bytes`, `max_total_bytes`)
- Cache expensive operations (e.g., encoding detection)

## Common Patterns

### Progress UI with Rich

```python
from rich.progress import Progress, SpinnerColumn, TextColumn

with Progress(SpinnerColumn(), TextColumn("[progress.description]{task.description}")) as progress:
    task = progress.add_task("Scanning files...", total=len(files))
    for file in files:
        # Process file
        progress.advance(task)
```

### File Path Handling

```python
from pathlib import Path

# Always use Path, not strings
repo_root = Path("/path/to/repo").resolve()
relative_path = file_path.relative_to(repo_root)
```

### Configuration Loading

```python
from .config_loader import load_config, merge_cli_with_config

# Load from file
config = load_config(Path("repo-to-prompt.toml"))

# Merge CLI overrides
final_config = merge_cli_with_config(cli_args, config)
```

## Ruff Configuration

- **Selected rules**: E, F, I, N, W, UP, B, C4, SIM
- **Ignored rules**:
  - `E501`: Line too long (handled by formatter)
  - `B008`: Function call in default argument (standard Typer pattern for `typer.Option()`)

## When to Run Tests

- Before committing (pre-commit hook runs pytest on push)
- After making changes to core logic
- When fixing bugs (write a failing test first)
- When adding new features (write tests alongside)

## Project Structure

```
repo-to-prompt/
├── src/repo_to_prompt/    # Source code (11 modules)
│   ├── cli.py             # CLI entry point with Typer
│   ├── config.py          # Configuration models
│   ├── config_loader.py   # Config file loading
│   ├── fetcher.py         # Repository fetching
│   ├── scanner.py         # File discovery
│   ├── chunker.py         # Content chunking
│   ├── ranker.py          # File ranking
│   ├── renderer.py        # Output generation
│   ├── redactor.py        # Secret redaction
│   └── utils.py           # Shared utilities
├── tests/                 # Test suite (15 test modules)
├── .github/workflows/     # CI/CD (ci.yml, release.yml)
├── pyproject.toml         # Project config and dependencies
├── .pre-commit-config.yaml # Pre-commit hooks
└── README.md              # User documentation
```

## Additional Resources

- **CI Pipeline**: `.github/workflows/ci.yml` - Tests across Python 3.10-3.12 and OS (Ubuntu, macOS, Windows)
- **Pre-commit config**: `.pre-commit-config.yaml` - Ruff, mypy, trailing whitespace, YAML/TOML validation
- **Dependencies**: Listed in `pyproject.toml` under `[project.dependencies]` and `[project.optional-dependencies]`

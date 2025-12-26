# repo-to-prompt

Convert repositories into LLM-friendly context packs for prompting and RAG.

*Because LLMs are smart… but they still can’t read your repo through vibes.* 🙎🏻‍♂️

[![CI](https://github.com/wheevu/repo-to-prompt/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/wheevu/repo-to-prompt/actions/workflows/ci.yml)
[![Python 3.10+](https://img.shields.io/badge/python-3.10+-blue.svg)](https://www.python.org/downloads/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

## Overview

`repo-to-prompt` is a CLI tool that transforms codebases into high-signal text bundles optimized for LLM prompting and retrieval-augmented generation (RAG). It scans your repo, ranks the “actually important” files first, chunks content into model-friendly pieces, and produces structured outputs ready for AI consumption.

If you’ve ever pasted a whole repository into a prompt and immediately regretted it: same. This tool is the fix.

### Key Features

- **Smart File Ranking**: Prioritizes READMEs, configs, entrypoints over tests and generated files
- **Language-Aware Chunking**: Uses code structure (functions, classes) for Python, JS/TS, Go, Java, Rust
- **Secret Redaction**: Automatically detects and redacts API keys, tokens, and credentials
- **Gitignore Respect**: Honors `.gitignore` patterns by default
- **GitHub Support**: Clone and process remote repositories directly
- **Deterministic Output**: Stable ordering and chunk IDs for reproducible results
- **Cross-Platform**: Works on macOS, Linux, and Windows

### Design Philosophy

- **High signal > high volume**: READMEs and entrypoints first, `node_modules` never.
- **Deterministic**: If you run it twice, you shouldn’t get two different realities.
- **Language-aware**: Code is a language; treat it like one.

## Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/your-org/repo-to-prompt.git
cd repo-to-prompt

# Create virtual environment
python -m venv .venv
source .venv/bin/activate  # On Windows: .venv\Scripts\activate

# Install in development mode
pip install -e ".[dev]"

# Or install with all optional dependencies
pip install -e ".[all]"
```

### Optional Dependencies

- **tiktoken**: More accurate token counting (uses OpenAI's tokenizer)
- **tree-sitter**: Enhanced code parsing (optional; currently the chunker is pattern-based)

```bash
pip install -e ".[tiktoken]"

# Optional: install tree-sitter language packages
pip install -e ".[treesitter]"
```

## Quick Start

### Export a Local Repository

```bash
# Basic export (produces both markdown and JSONL)
repo-to-prompt export --path /path/to/your/repo

# Or use the short alias
r2p export -p /path/to/your/repo
```

### Export from GitHub

```bash
# Export from GitHub URL
repo-to-prompt export --repo https://github.com/owner/repo

# Export specific branch
repo-to-prompt export --repo https://github.com/owner/repo --ref develop
```

### View Repository Info

```bash
# Get repo statistics without exporting
repo-to-prompt info /path/to/your/repo
```

Tip: `info` supports the same include/exclude knobs as `export`, so your counts don’t drift depending on which command you used.

## Usage

### Command: `export`

The main command for converting repositories to context packs.

```bash
repo-to-prompt export [OPTIONS]
```

#### Input Options

| Option | Short | Description |
| ------ | ----- | ----------- |
| `--path PATH` | `-p` | Local path to the repository |
| `--repo URL` | `-r` | GitHub repository URL |
| `--ref REF` | | Git ref (branch/tag/SHA) for GitHub repos |

#### Filter Options

| Option | Short | Default | Description |
| ------ | ----- | ------- | ----------- |
| `--include-ext EXT` | `-i` | (many) | Comma-separated extensions to include |
| `--exclude-glob GLOB` | `-e` | (many) | Comma-separated glob patterns to exclude |
| `--max-file-bytes N` | | 1048576 | Max size per file (1 MB) |
| `--max-total-bytes N` | | 20000000 | Max total export size (20 MB) |
| `--no-gitignore` | | false | Don't respect .gitignore files |

#### Chunking Options

| Option | Default | Description |
| ------ | ------- | ----------- |
| `--chunk-tokens N` | 800 | Target tokens per chunk |
| `--chunk-overlap N` | 120 | Token overlap between chunks |
| `--min-chunk-tokens N` | 200 | Minimum chunk size; smaller chunks are coalesced (`0` disables) |

#### Output Options

| Option | Short | Default | Description |
| ------ | ----- | ------- | ----------- |
| `--mode MODE` | `-m` | both | Output mode: `prompt`, `rag`, or `both` |
| `--output-dir DIR` | `-o` | ./out | Base output directory (outputs go into `DIR/<repo-name>/`) |
| `--tree-depth N` | | 4 | Max depth for directory tree |
| `--no-redact` | | false | Disable secret redaction |

### Command: `info`

Display repository information without exporting.

```bash
repo-to-prompt info PATH [OPTIONS]
```

`info` uses the same scanner as `export`, so you can pass filters for consistent statistics:

| Option | Short | Default | Description |
| ------ | ----- | ------- | ----------- |
| `--include-ext EXT` | `-i` | (all) | Comma-separated extensions to include |
| `--exclude-glob GLOB` | `-e` | (none) | Comma-separated glob patterns to exclude |
| `--max-file-bytes N` | | 1048576 | Max size per file (1 MB) |
| `--no-gitignore` | | false | Don't respect .gitignore files |

## Examples

### 1. Basic Local Export

```bash
# Export current directory
repo-to-prompt export -p .

# Output:
# ./out/<repo-name>/context_pack.md  - Markdown context pack
# ./out/<repo-name>/chunks.jsonl     - JSONL chunks for RAG
# ./out/<repo-name>/report.json      - Processing statistics
```

### 2. GitHub Repository Export

```bash
# Export a public GitHub repo
repo-to-prompt export --repo https://github.com/pallets/flask

# Export specific version
repo-to-prompt export --repo https://github.com/pallets/flask --ref 3.0.0
```

### 3. Python Project with Custom Filters

```bash
# Only Python and Markdown files, skip tests
repo-to-prompt export \
  -p ./my-python-project \
  --include-ext ".py,.md,.rst,.toml" \
  --exclude-glob "tests/**,test_*"
```

### 4. RAG Mode Only (JSONL Output)

```bash
# Generate only JSONL chunks for embedding
repo-to-prompt export -p ./repo --mode rag -o ./embeddings
```

### 5. Large Monorepo with Size Limits

```bash
# Process large repo with limits
repo-to-prompt export \
  -p ./monorepo \
  --max-file-bytes 500000 \
  --max-total-bytes 10000000 \
  --tree-depth 3
```

### 6. Disable Secret Redaction

```bash
# Skip secret redaction (use with caution!)
repo-to-prompt export -p ./repo --no-redact
```

## Output Structure

Outputs are written to `--output-dir/<repo-name>/`.

### context_pack.md

A structured Markdown document containing:

1. **Repository Overview** - Project summary, description, detected languages, entrypoints, available commands
2. **Directory Structure** - Visual tree with important files highlighted
3. **Key Files** - Categorized list of documentation, configs, and entrypoints
4. **Code Map** - Per-language module listing
5. **File Contents** - Chunked content with file paths and line numbers

Example:

```markdown
# Repository Context Pack: my-project

> Generated by repo-to-prompt on 2024-01-15 10:30:00
> Files: 45 | Chunks: 128 | Size: 234,567 bytes

---

## 📋 Repository Overview

**Project:** my-project
**Description:** A sample Python project
**Languages:** python (35), markdown (8), yaml (2)

**Entrypoints:**
- `src/my_project/cli.py`
- `src/my_project/__main__.py`

...
```

### chunks.jsonl

JSONL file with one chunk per line:

```json
{"id": "a1b2c3d4e5f67890", "path": "src/main.py", "lang": "python", "start_line": 1, "end_line": 45, "content": "...", "priority": 0.85, "tags": ["entrypoint", "core"]}
{"id": "b2c3d4e5f6789012", "path": "src/utils.py", "lang": "python", "start_line": 1, "end_line": 30, "content": "...", "priority": 0.75, "tags": ["core"]}
```

### report.json

Processing statistics:

```json
{
  "generated_at": "2024-01-15T10:30:00",
  "stats": {
    "files_scanned": 150,
    "files_included": 45,
    "files_skipped": {
      "size": 3,
      "binary": 12,
      "extension": 85,
      "gitignore": 5
    },
    "total_bytes_included": 234567,
    "chunks_created": 128,
    "processing_time_seconds": 2.34,
    "languages_detected": {
      "python": 35,
      "markdown": 8,
      "yaml": 2
    }
  }
}
```

## Secret Redaction

By default, `repo-to-prompt` detects and redacts common secrets:

- AWS access keys and secret keys
- GitHub tokens (ghp_, gho_, ghu_, ghr_)
- GitLab tokens
- Slack tokens and webhooks
- Stripe API keys
- Google API keys
- JWT tokens
- Private keys (RSA, DSA, EC, OpenSSH)
- Generic patterns (api_key, secret_key, password, etc.)
- Connection string passwords

Redacted content is replaced with descriptive placeholders like `[AWS_ACCESS_KEY_REDACTED]`.

## File Priority Ranking

Files are ranked by importance (highest to lowest):

| Priority | Category | Examples |
| -------- | -------- | -------- |
| 1.00 | README | README.md, README.rst |
| 0.95 | Main docs | CONTRIBUTING.md, CHANGELOG.md |
| 0.90 | Config | pyproject.toml, package.json, Dockerfile |
| 0.85 | Entrypoints | main.py, index.js, cli.py |
| 0.80 | API definitions | types.ts, models.py, schema.graphql |
| 0.75 | Core source | src/**, lib/** |
| 0.60 | Examples | examples/**, samples/** |
| 0.50 | Tests | tests/**, *_test.py |
| 0.20 | Generated | *.min.js, auto-generated |
| 0.15 | Lock files | package-lock.json, poetry.lock |
| 0.10 | Vendored | vendor/**, node_modules/** |

## Architecture

```text
src/repo_to_prompt/
├── cli.py          # CLI entry point (typer)
├── config.py       # Configuration and data models
├── fetcher.py      # Repository fetching (local/GitHub)
├── scanner.py      # File discovery and filtering
├── chunker.py      # Language-aware content chunking
├── ranker.py       # File importance ranking
├── renderer.py     # Output generation (MD, JSONL, JSON)
├── redactor.py     # Secret detection and redaction
└── utils.py        # Token estimation, hashing, encoding
```

## Development

### Setup

```bash
# Clone and install dev dependencies
git clone https://github.com/your-org/repo-to-prompt.git
cd repo-to-prompt
pip install -e ".[dev]"
```

### Running Tests

```bash
# Run all tests
pytest

# Run with coverage
pytest --cov=repo_to_prompt --cov-report=html

# Run specific test file
pytest tests/test_chunker.py -v
```

### Code Quality

```bash
# Lint with ruff
ruff check src tests

# Type check with mypy
mypy src
```

## License

MIT License.

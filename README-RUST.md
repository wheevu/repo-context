# repo-to-prompt 🦀

**Turn repositories into LLM-friendly context packs for prompting and RAG.**

[![CI](https://github.com/wheevu/repo-to-prompt/actions/workflows/ci.yml/badge.svg)](https://github.com/wheevu/repo-to-prompt/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

> **🚧 Rust Rewrite In Progress**: This project is being rewritten from Python to Rust for improved performance, reliability, and cross-platform distribution. The Python version is still functional and available in the `src/repo_to_prompt/` directory.

## Overview

`repo-to-prompt` is a CLI tool that converts code repositories into high-signal text bundles optimized for LLM prompting and Retrieval-Augmented Generation (RAG) workflows. It intelligently scans repositories, ranks files by importance, chunks content into model-friendly sizes, and exports structured outputs ready for AI workflows.

### Key Features

- **Smart File Ranking**: Prioritizes READMEs, configs, and entrypoints over tests and generated files
- **Language-Aware Chunking**: Structure-aware chunking for Python, JS/TS, Go, Java, Rust, and Markdown
- **Advanced Secret Redaction**: 25+ patterns, entropy detection, paranoid mode, and allowlists
- **Structure-Safe Redaction**: Never breaks code syntax (AST-validated for Python)
- **UTF-8 Fidelity**: Preserves emojis, smart quotes, and international characters
- **Gitignore Respect**: Honors `.gitignore` using Git as source of truth
- **GitHub Support**: Clone and process remote repositories directly
- **Deterministic Output**: Stable ordering and chunk IDs for reproducible results
- **Concurrent Scanning**: Fast I/O with thread pools (Rayon in Rust, ThreadPoolExecutor in Python)
- **Cross-Platform**: Works on macOS, Linux, and Windows

### Design Philosophy

- **High signal > high volume**: READMEs and entrypoints first, `node_modules` never
- **Deterministic**: Running twice produces identical results
- **Language-aware**: Code is structured text; treat it accordingly

## Installation

### Rust Version (Recommended)

The Rust version offers better performance and easier distribution. Currently in development.

```bash
# From source
git clone https://github.com/wheevu/repo-to-prompt.git
cd repo-to-prompt
cargo build --release

# The binary will be at target/release/repo-to-prompt
# Optionally, install it to your PATH
cargo install --path .
```

### Python Version (Stable)

```bash
# Clone and install
git clone https://github.com/wheevu/repo-to-prompt.git
cd repo-to-prompt

# Create virtual environment
python -m venv .venv
source .venv/bin/activate  # Windows: .venv\Scripts\activate

# Install with development dependencies
pip install -e ".[dev]"

# Or install with all optional dependencies
pip install -e ".[all]"
```

## Quick Start

### Rust CLI (In Development)

```bash
# Export a local repository
repo-to-prompt export --path /path/to/your/repo

# View help
repo-to-prompt --help
repo-to-prompt export --help
```

### Python CLI (Current)

```bash
# Basic export (produces both markdown and JSONL)
repo-to-prompt export --path /path/to/your/repo

# Or use the short alias
r2p export -p /path/to/your/repo

# Export from GitHub
repo-to-prompt export --repo https://github.com/owner/repo

# View repository info without exporting
repo-to-prompt info /path/to/your/repo
```

For complete Python CLI documentation, see the original README sections below.

## Development

### Rust Development

```bash
# Run tests
cargo test

# Run with verbose logging
RUST_LOG=debug cargo run -- export --path .

# Format code
cargo fmt

# Lint
cargo clippy --all-targets --all-features

# Build optimized release
cargo build --release
```

### Python Development

```bash
# Run tests
pytest

# Run tests with coverage
pytest --cov=repo_to_prompt --cov-report=xml --cov-report=term-missing

# Lint and format
ruff check src tests --fix
ruff format src tests

# Type check
mypy src --ignore-missing-imports
```

## Architecture

### Rust Stack (Target)

- **CLI**: `clap` v4 with derive macros
- **Errors**: `anyhow` + `thiserror` + `tracing`
- **Config**: `serde`, `toml`, `serde_yaml`, `figment`
- **File Discovery**: `ignore` (gitignore) + `walkdir`
- **Concurrency**: `rayon` for parallel processing
- **Git**: `git2` (libgit2 bindings)
- **Testing**: `cargo test`, `insta` snapshots, `assert_cmd`

### Python Stack (Current)

- **CLI**: Typer + Rich for progress UI
- **Validation**: Pydantic v2
- **File Discovery**: pathspec + GitPython
- **Concurrency**: ThreadPoolExecutor
- **Testing**: pytest + pytest-cov

### Module Structure

Both implementations follow the same pipeline architecture:

1. **Fetch** → Get repository (local or GitHub)
2. **Scan** → Discover files with gitignore respect
3. **Rank** → Prioritize important files
4. **Chunk** → Split content into model-friendly sizes
5. **Redact** → Remove secrets safely
6. **Render** → Generate outputs (Markdown, JSONL, report)

## Migration Status

| Component | Python | Rust | Status |
|-----------|--------|------|--------|
| CLI Framework | ✅ Typer | ✅ clap | Complete |
| Domain Types | ✅ Pydantic | ✅ serde | Complete |
| Config Loading | ✅ | 🚧 | In Progress |
| File Scanner | ✅ | 🚧 | In Progress |
| Ranking | ✅ | 🚧 | Planned |
| Chunking | ✅ | 🚧 | Planned |
| Redaction | ✅ | 🚧 | Planned |
| Rendering | ✅ | 🚧 | Planned |
| GitHub Fetch | ✅ | 🚧 | Planned |
| Tests | ✅ 15 test files | ✅ Integration | Expanding |

## Contributing

We welcome contributions! Whether you're interested in:

- **Rust implementation**: Help port Python modules to Rust
- **Python maintenance**: Bug fixes and improvements to the current version
- **Documentation**: Improving guides and examples
- **Testing**: Adding test cases and improving coverage

Please see `AGENTS.md` for detailed architecture notes and contribution guidelines.

## License

MIT License - see [LICENSE](LICENSE) file for details.

## Acknowledgments

Built with best-in-class Rust libraries:
- [clap](https://github.com/clap-rs/clap) for CLI parsing
- [serde](https://github.com/serde-rs/serde) for serialization
- [rayon](https://github.com/rayon-rs/rayon) for parallelism
- [ignore](https://github.com/BurntSushi/ripgrep/tree/master/crates/ignore) for gitignore handling
- [git2](https://github.com/rust-lang/git2-rs) for Git operations

---

**Python Version Full Documentation**

_The sections below document the current Python implementation. Rust equivalents are being developed._

[Rest of original Python README would go here...]

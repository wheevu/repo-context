# Agent Guide for repo-context

This guide is for AI coding agents working in the `repo-context` codebase. It contains build commands, code style conventions, and project-specific guidelines.

## Project Overview

**repo-context** is a Rust CLI tool that converts code repositories into LLM-friendly context packs for prompting and RAG (Retrieval-Augmented Generation) workflows.

- **Language**: Rust (edition 2021)
- **Build System**: Cargo
- **CLI Framework**: clap v4 (derive macros)
- **Serialization**: serde + toml + serde_yaml
- **Error handling**: anyhow + thiserror + tracing
- **License**: MIT

## Build & Development Commands

### Setup

```bash
# Requires Rust stable (install via https://rustup.rs)
rustup update stable

# Build in debug mode
cargo build

# Build optimized release binary
cargo build --release
# Binary: target/release/repo-context

# Install to ~/.cargo/bin
cargo install --path .
```

### Testing

```bash
# Run all tests (189 tests)
cargo test

# Run tests with output shown
cargo test -- --nocapture

# Run a single test file
cargo test --test cli_tests

# Run a single test by name
cargo test test_readme_ranks_higher_than_test

# Run tests matching a pattern
cargo test redact

# Stop on first failure
cargo test -- --test-threads=1

# Run unit tests only (no integration tests)
cargo test --lib
```

### Golden Snapshot Tests

Golden snapshot tests use [insta](https://insta.rs). To update snapshots after intentional output changes:

```bash
# Update all golden snapshots
INSTA_UPDATE=always cargo test -- golden

# Review snapshots interactively
cargo insta review
```

### Linting & Formatting

```bash
# Format code (enforced in CI)
cargo fmt

# Check formatting without making changes
cargo fmt -- --check

# Lint with clippy (CI uses -D warnings — all warnings are errors)
cargo clippy --all-targets --all-features -- -D warnings

# Lint check only
cargo clippy --all-targets --all-features
```

### Running the CLI

```bash
# Export a local repository
cargo run -- export --path /path/to/repo

# Export from GitHub
cargo run -- export --repo https://github.com/owner/repo

# View repository info
cargo run -- info /path/to/repo

# With debug logging
RUST_LOG=debug cargo run -- export --path /path/to/repo
```

## Code Style Guidelines

### General Conventions

- **Line length**: 100 characters (enforced by `rustfmt.toml`: `max_width = 100`)
- **Edition**: Rust 2021
- **Formatting**: `cargo fmt` (enforced in CI)
- **Linting**: `cargo clippy -- -D warnings` (all warnings are errors in CI)
- **Comments**: Use `//!` for module-level docs, `///` for item-level docs

### Imports

```rust
// Standard library first
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

// Third-party next
use anyhow::Result;
use serde::{Deserialize, Serialize};

// Crate-local last
use crate::domain::{Chunk, FileInfo};
use crate::utils::estimate_tokens;
```

### Naming Conventions

- **Modules**: `snake_case` (e.g., `config_loader`, `file_scanner`)
- **Types/Structs/Enums**: `PascalCase` (e.g., `FileRanker`, `OutputMode`)
- **Functions/Methods**: `snake_case` (e.g., `scan_repository`, `chunk_file`)
- **Constants**: `UPPER_SNAKE_CASE` (e.g., `DEFAULT_CHUNK_TOKENS`)
- **Private items**: no prefix convention needed (use `pub` explicitly)

### Error Handling

- Use `anyhow::Result` for fallible functions throughout
- Use `thiserror` for domain-specific error types (e.g., `FetchError`)
- Provide clear, actionable error messages
- Never silently swallow errors

```rust
use anyhow::{Context, Result};

fn load_config(path: &Path) -> Result<Config> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read config at {}", path.display()))?;
    toml::from_str(&content).context("Invalid TOML in config file")
}
```

### Domain Types

- Use `BTreeSet<String>` (not `HashSet`) for `FileInfo.tags` and `Chunk.tags` — required for deterministic output
- Use `PathBuf` for owned paths, `&Path` for borrowed paths
- Use `serde` derives for any type that is serialized/deserialized

### Testing Conventions

- Unit tests live in `#[cfg(test)]` modules at the bottom of each source file
- Integration tests live in `tests/*.rs`
- Golden snapshot tests use `insta` (see `tests/golden_export_tests.rs`)
- Use `tempfile::TempDir` for filesystem tests
- Test names follow `test_<what>_<condition>` (e.g., `test_readme_ranks_higher_than_test`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_readme_ranks_higher_than_test() {
        // ...
    }
}
```

## Architecture & Design Principles

### Pipeline Architecture

The tool follows a modular pipeline:
1. **Fetch** (`fetch/`) → Get repository (local path, GitHub, or HuggingFace)
2. **Scan** (`scan/`) → Discover files with `.gitignore` respect via the `ignore` crate
3. **Rank** (`rank/`) → Prioritize important files (READMEs, configs, entrypoints)
4. **Chunk** (`chunk/`) → Split content into model-friendly sizes
5. **Redact** (`redact/`) → Remove secrets safely
6. **Render** (`render/`) → Generate outputs (Markdown, JSONL, JSON report)

### Key Design Principles

- **Deterministic**: Same input → same output (use `BTreeSet`/`BTreeMap` for stable ordering)
- **High signal over volume**: Prioritize READMEs, configs, entrypoints over tests/locks
- **Language-aware chunking**: Tree-sitter for Python, Rust, JS/TS, Go; regex fallback otherwise
- **UTF-8 fidelity**: Preserve emojis, smart quotes, international characters
- **Structure-safe redaction**: Never break syntax (AST-validated for Python via `rustpython-parser`)
- **Cross-platform**: Support macOS, Linux, Windows

### Performance Considerations

- Use `rayon` for parallel file I/O (`scan/scanner.rs`)
- Respect size limits (`max_file_bytes`, `max_total_bytes`)
- Use `PathBuf`/`Path` for all path handling (not raw strings)

## Common Patterns

### Returning structured results with anyhow

```rust
pub fn run(args: ExportArgs) -> Result<()> {
    let root = args.path.canonicalize()
        .context("Failed to resolve repository path")?;
    // ...
    Ok(())
}
```

### Progress UI with indicatif

```rust
use indicatif::{ProgressBar, ProgressStyle};

let pb = ProgressBar::new(files.len() as u64);
pb.set_style(ProgressStyle::default_bar()
    .template("{spinner:.green} [{bar:40}] {pos}/{len} {msg}")?);
for file in &files {
    // process
    pb.inc(1);
}
pb.finish_with_message("done");
```

### Config Loading

```rust
use crate::config::loader::load_config;
use crate::config::merge::merge_cli_with_config;

let config = load_config(Some(config_path))?;
let final_config = merge_cli_with_config(cli_args, config);
```

## Supported Tree-sitter Languages

Structure-aware chunking via tree-sitter is available for:

- `python`
- `rust`
- `javascript`
- `typescript`
- `go`

All other languages fall back to the line-based chunker. Java and Kotlin have extension detection for language statistics but no tree-sitter chunking support.

## When to Run Tests

- After making changes to any pipeline stage
- After fixing a bug (write a failing test first)
- After adding a new feature (tests alongside)
- Before every commit — CI runs `cargo test` on all 3 platforms

## Project Structure

```
repo-context/
├── src/
│   ├── main.rs              # Binary entry point
│   ├── lib.rs               # Library entry point (re-exports modules)
│   ├── cli/
│   │   ├── mod.rs           # CLI root (clap App, tracing init)
│   │   ├── export.rs        # `export` command implementation
│   │   ├── info.rs          # `info` command implementation
│   │   └── utils.rs         # CSV argument parsing helper
│   ├── config/
│   │   ├── mod.rs           # Re-exports
│   │   ├── loader.rs        # TOML/YAML config file loading
│   │   └── merge.rs         # CLI flag override merging
│   ├── domain/
│   │   └── mod.rs           # Core types: FileInfo, Chunk, Config, ScanStats, etc.
│   ├── fetch/
│   │   ├── mod.rs           # Dispatch: local vs GitHub vs HuggingFace
│   │   ├── context.rs       # RepoContext + temp dir cleanup
│   │   ├── local.rs         # Local path validation + git root detection
│   │   ├── github.rs        # GitHub shallow clone via git2
│   │   └── huggingface.rs   # HuggingFace Spaces/Models/Datasets clone
│   ├── scan/
│   │   ├── mod.rs           # Re-exports
│   │   ├── scanner.rs       # File discovery with gitignore (ignore crate)
│   │   └── tree.rs          # Directory tree generation (Unicode box-drawing)
│   ├── rank/
│   │   ├── mod.rs           # rank_files() public entry point
│   │   └── ranker.rs        # FileRanker with manifest-aware entrypoint detection
│   ├── chunk/
│   │   ├── mod.rs           # Chunking dispatch + coalesce logic
│   │   ├── code_chunker.rs  # Tree-sitter code chunker (Python/Rust/JS/TS/Go)
│   │   ├── line_chunker.rs  # Line-based fallback chunker
│   │   └── markdown_chunker.rs  # Heading-aware Markdown chunker
│   ├── redact/
│   │   ├── mod.rs           # Re-exports
│   │   ├── redactor.rs      # Main redactor + Python AST validation
│   │   ├── rules.rs         # 25+ built-in secret patterns
│   │   └── entropy.rs       # Shannon entropy calculator
│   ├── render/
│   │   ├── mod.rs           # Re-exports
│   │   ├── context_pack.rs  # Markdown context_pack.md renderer
│   │   ├── jsonl.rs         # JSONL chunks.jsonl renderer
│   │   └── report.rs        # JSON report.json writer
│   └── utils/
│       ├── mod.rs           # Re-exports + format_with_commas
│       ├── classify.rs      # is_vendored, is_lock_file, is_generated, is_minified
│       ├── encoding.rs      # UTF-8/binary detection, safe file reading
│       ├── hashing.rs       # Stable SHA-256 chunk IDs
│       ├── paths.rs         # Path normalization utilities
│       └── tokens.rs        # Token estimation (char/4 heuristic)
├── tests/
│   ├── cli_tests.rs             # CLI integration tests (assert_cmd)
│   ├── export_output_tests.rs   # Export pipeline tests
│   ├── golden_export_tests.rs   # Golden snapshot tests (insta)
│   └── snapshots/               # Insta snapshot files
├── Cargo.toml               # Package manifest and dependencies
├── Cargo.lock               # Locked dependency versions (committed)
├── rustfmt.toml             # Formatter config (max_width=100, edition=2021)
└── README.md                # User documentation
```

## Additional Resources

- **CI Pipeline**: `.github/workflows/ci.yml` — fmt + clippy + test + release build on Ubuntu/macOS/Windows
- **Release Pipeline**: `.github/workflows/release.yml` — cross-compiles for 5 targets and attaches binaries to GitHub Releases on version tags
- **Dependencies**: `Cargo.toml` under `[dependencies]` and `[dev-dependencies]`
- **Snapshot testing**: `tests/snapshots/` — managed by `cargo insta`

# repo-context

*(Was previously 'repo-to-prompt', written in python, which was okay. But now the thing's charged with the Speed Force ⚡️🏃🏻)*

Turn a code repository into a tidy "context pack" you can paste into an LLM — or feed into a RAG pipeline.

[![CI](https://github.com/wheevu/repo-context/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/wheevu/repo-context/actions/workflows/ci.yml)

![CLI Demo](assets/cli.gif)


## What it does

`repo-context` scans a repository and exports **high-signal text bundles**:

-   **`context_pack.md`** — a structured markdown doc you can paste into ChatGPT/Claude/etc.
-   **`chunks.jsonl`** — one chunk per line (great for embeddings + retrieval)
-   **`report.json`** — stats + what got included/skipped

It tries to keep the *important* stuff (READMEs, configs, entrypoints, core source) and skip the less impactful resources (generated files, vendor folders, giant binaries).

## Why you'd use it

-   You want an LLM to help with a repo **without** dumping your whole codebase into chat. 🫩
-   You want **repeatable** outputs (stable ordering + stable chunk IDs).
-   You want basic protection against accidentally leaking secrets (optional redaction).

## Features

-   **Picks important files first** (docs + entrypoints beat tests + build artifacts)
-   **Chunks code in a sane way** (tries to split at functions/classes/modules when possible)
-   **Respects `.gitignore`** by default
-   **Can clone remote repos** (GitHub / HuggingFace)
-   **Optional secret redaction** (tokens/keys/password-y strings)
-   **Task-aware retrieval** (`--task`) with symbol/dependency expansion
-   **Two-phase retrieval** (BM25 + semantic rerank) with audit tags in output
-   **Module thread stitching** with reserved token budget for related definitions/import-neighbors
-   **PR-focused mode** (`--mode pr-context`) with Touch Points / Entrypoints / Invariants
-   **SQLite symbol graph** export (`symbol_graph.db`) for local graph-aware workflows
-   **Guardrails in context packs** (Claims Index + Missing Pieces heuristics)
-   **Local index workflow** (`index` / `query`) + portable code-intel export (`codeintel`)
-   **Context diff mode** (`diff`) with text/markdown/json output formats

## Install

### Pre-built binaries (recommended)

Grab the latest release from the GitHub Releases page and put `repo-context` somewhere on your `PATH`.

### Build from source

```bash
git clone https://github.com/wheevu/repo-context.git
cd repo-context
cargo build --release
# The binary will be at: target/release/repo-context

# Or install to ~/.cargo/bin
cargo install --path .
```

## Quick start

Export a local repo:


![CLI local 2](assets/2.png)

```bash
repo-context export --path .
```

Export from a remote repo:

![CLI repo 3](assets/4.png)


```bash
repo-context export --repo https://github.com/owner/repo
```

Show repo stats only (no export files):
```bash
repo-context info .
```

## Guided mode (default)

![CLI local 1](assets/1.png)


`repo-context export` is interactive by default in terminals:

-   **Quick scan** (fast, high-signal defaults)
-   **Architecture overview** (stronger dependency/system context)
-   **Deep dive specific areas** (repo-specific focus selection)
-   **Full context** (largest practical context bundle)

![CLI repo 1](assets/3.png)


In non-interactive sessions (CI/pipes), it automatically falls back to quick defaults.

Skip prompts explicitly:
```bash
repo-context export --path . --quick
```

## Use-case playbook

**Small + high-signal export (non-interactive)**
```bash
repo-context export -p . \
  --quick \
  --include-ext ".rs,.toml,.md" \
  --exclude-glob "tests/**,target/**"
```

**Architecture understanding for an unfamiliar repo**
```bash
repo-context export -p . --task "overall architecture and dependencies" --mode both
```

**Deep dive into a feature path**
```bash
repo-context export -p . --task "trace auth refresh and token validation flow"
```

**RAG-only output**
```bash
repo-context export -p . --mode rag -o ./embeddings
```

**Reproducible outputs for version-to-version diffs**
```bash
repo-context export -p . --no-timestamp
```

**Strict token-budget handling with always-include files**
```bash
# hard-error if always-include files alone exceed --max-tokens
repo-context export -p . --max-tokens 50000

# explicitly allow always-include overflow
repo-context export -p . --max-tokens 50000 --allow-over-budget
```

**Best stitching quality (index first, export second)**
```bash
repo-context index -p .
repo-context export -p . --task "trace auth refresh flow"
```

**Build local index and query it repeatedly**
```bash
repo-context index -p .
repo-context query --task "where are retries and backoff handled?" --expand
```

**Portable code-intel output from local index**
```bash
repo-context codeintel --db .repo-context/index.sqlite --out .repo-context/codeintel.json
```

**Compare two exports**
```bash
repo-context diff out/repo-a out/repo-b
repo-context diff out/repo-a out/repo-b --format markdown
repo-context diff out/repo-a out/repo-b --format json
```

## Command manual

### Top-level commands

-   `export` - create context artifacts (`context_pack`, `chunks`, report, graph)
-   `info` - scan + ranking summary only
-   `index` - build local SQLite retrieval/symbol index
-   `query` - retrieve task-relevant chunks from index
-   `codeintel` - export portable SCIP-like JSON from index
-   `diff` - compare two exports

### `export` options

**Input source**
-   `-p, --path <PATH>` local repository path
-   `-r, --repo <URL>` remote repository URL (GitHub/HuggingFace)
-   `--ref <REF>` branch/tag/SHA when using `--repo`
-   `-c, --config <FILE>` config file path

**Scope and filtering**
-   `-i, --include-ext <EXTS>` extension allowlist (`.rs,.toml,.md`)
-   `-e, --exclude-glob <GLOBS>` exclude globs
-   `--max-file-bytes <BYTES>` per-file size cap
-   `--max-total-bytes <BYTES>` total scan byte cap
-   `--no-gitignore` ignore `.gitignore`
-   `--follow-symlinks` follow symlinks
-   `--include-minified` include minified/bundled files

**Retrieval and ranking**
-   `-t, --max-tokens <TOKENS>` output token budget
-   `--allow-over-budget` allow always-include overflow
-   `--task <TEXT>` task-aware reranking query
-   `--no-semantic-rerank` disable semantic rerank stage
-   `--semantic-model <MODEL>` semantic model identifier
-   `--rerank-top-k <N>` number of chunks for semantic reranking
-   `--stitch-budget-fraction <FLOAT>` reserved budget for stitched context
-   `--stitch-top-n <N>` top-ranked seed chunks for stitching

**Chunking**
-   `--chunk-tokens <TOKENS>` target chunk size
-   `--chunk-overlap <TOKENS>` chunk overlap
-   `--min-chunk-tokens <TOKENS>` coalescing threshold

**Output and rendering**
-   `-m, --mode <MODE>` `prompt|rag|contribution|pr-context|both`
-   `-o, --output-dir <DIR>` output base directory
-   `--no-timestamp` reproducible output (no timestamp fields)
-   `--tree-depth <DEPTH>` tree depth in rendered context pack
-   `--no-graph` skip `symbol_graph.db` output
-   `--quick` skip guided menu and run non-interactive defaults

**Redaction**
-   `--no-redact` disable secret redaction
-   `--redaction-mode <MODE>` `fast|standard|paranoid|structure-safe`

### `info` options

-   `<PATH>` repo path to inspect
-   `-i, --include-ext <EXTS>` extension allowlist
-   `-e, --exclude-glob <GLOBS>` exclude globs
-   `--max-file-bytes <BYTES>` per-file size cap
-   `--no-gitignore` ignore `.gitignore`
-   `--follow-symlinks` follow symlinks
-   `--include-minified` include minified/bundled files

### `index` options

-   `-p, --path <PATH>` local path to index
-   `-r, --repo <URL>` remote URL to clone and index
-   `--ref <REF>` branch/tag/SHA for `--repo`
-   `-c, --config <FILE>` config file path
-   `--db <FILE>` SQLite output path (default: `.repo-context/index.sqlite`)
-   `-i, --include-ext <EXTS>` extension allowlist
-   `-e, --exclude-glob <GLOBS>` exclude globs
-   `--max-file-bytes <BYTES>` per-file size cap
-   `--max-total-bytes <BYTES>` total scan byte cap
-   `--no-gitignore` ignore `.gitignore`
-   `--follow-symlinks` follow symlinks
-   `--include-minified` include minified/bundled files
-   `--chunk-tokens <TOKENS>` chunk size target
-   `--chunk-overlap <TOKENS>` chunk overlap
-   `--min-chunk-tokens <TOKENS>` coalescing threshold
-   `--lsp` enrich with rust-analyzer symbol references

### `query` options

-   `--db <FILE>` index database path
-   `--task <TEXT>` required retrieval query text
-   `-n, --limit <COUNT>` max hits to show
-   `--lsp-backend <MODE>` `off|auto|rust-analyzer`
-   `--expand` include definitions/callers/tests/docs expansions

### `codeintel` options

-   `--db <FILE>` index database path
-   `--out <FILE>` output JSON path

### `diff` options

-   `<BEFORE> <AFTER>` directories containing prior/current exports
-   `--format <FORMAT>` `text|markdown|json`

### Global options

-   `-v, --verbose` set log level to DEBUG
-   `-h, --help` and `-V, --version`

## Output (what you get)

Outputs go to: `<output-dir>/<repo-name>/`

**Files:**
-   `<repo-name>_context_pack.md` — overview + tree + key files + chunked content
-   `<repo-name>_chunks.jsonl` — `{ id, path, lang, start_line, end_line, content, ... }`
-   `<repo-name>_report.json` — scan/export stats + skip reasons
-   `<repo-name>_symbol_graph.db` — persisted symbol/import graph (unless `--no-graph`)

## Configuration

By default, it looks for one of these files in the repository root:
-   `repo-context.toml`, `.repo-context.toml`
-   `r2p.toml`, `.r2p.toml`
-   `r2p.yml`/`.yaml`, `.r2p.yml`/`.yaml`

CLI flags override config values.

<details>
<summary>Example config (`r2p.toml`)</summary>

```toml
[repo-context]
include_extensions = [".rs", ".toml", ".md"]
exclude_globs      = ["tests/**", "target/**"]
chunk_tokens       = 800
chunk_overlap      = 120
min_chunk_tokens   = 200
output_dir         = "./out"
mode               = "both"
tree_depth         = 4
respect_gitignore  = true
redact_secrets     = true
```
</details>

## Secret redaction (optional)

By default, `repo-context` can detect and replace common secrets with placeholders like:
`[AWS_ACCESS_KEY_REDACTED]`

You can also allowlist paths/strings or add your own patterns via config.

## Development

```bash
cargo test
cargo fmt
cargo clippy --all-targets --all-features
cargo build --release
```

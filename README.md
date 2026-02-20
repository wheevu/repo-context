# repo-context

*(Was previously 'repo-to-prompt', written in python, which was okay. But now the thing's charged with the Speed Force ⚡️🏃🏻)*

Turn a code repository into a tidy "context pack" you can paste into an LLM — or feed into a RAG pipeline.

[![CI](https://github.com/wheevu/repo-context/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/wheevu/repo-context/actions/workflows/ci.yml)

## What it does

`repo-context` scans a repository and exports **high-signal text bundles**:

-   **`context_pack.md`** — a structured markdown doc you can paste into ChatGPT/Claude/etc.
-   **`chunks.jsonl`** — one chunk per line (great for embeddings + retrieval)
-   **`report.json`** — stats + what got included/skipped

It tries to keep the *important* stuff (READMEs, configs, entrypoints, core source) and skip the noise (generated files, vendor folders, giant binaries).

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
```bash
repo-context export --path .
```

Export from a remote repo:
```bash
repo-context export --repo https://github.com/owner/repo
```

Just show stats (no output files):
```bash
repo-context info .
```

Build a local retrieval index once:
```bash
repo-context index --path .
```

Query the local index:
```bash
repo-context query --task "where is auth token refresh handled?"
```

Export portable code-intel JSON:
```bash
repo-context codeintel
```

## Common recipes

**"Small + high signal" export**
```bash
repo-context export -p . \
  --include-ext ".rs,.toml,.md" \
  --exclude-glob "tests/**,target/**"
```

**RAG-only output to a custom folder**
```bash
repo-context export -p . --mode rag -o ./embeddings
```

**Reproducible output (nice for diffs)**
```bash
repo-context export -p . --no-timestamp
```

## Output (what you get)

Outputs go to: `<output-dir>/<repo-name>/`

**Files:**
-   `context_pack.md` — overview + tree + "key files" + chunked content
-   `chunks.jsonl` — `{ id, path, lang, start_line, end_line, content, ... }`
-   `report.json` — scan/export stats + skip reasons
-   `symbol_graph.db` — persisted symbol/import graph (unless `--no-graph`)

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

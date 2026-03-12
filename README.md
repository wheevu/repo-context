# repo-context

<p align="center">
<img src=assets/title.svg width=70%>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white">
  <img src="https://img.shields.io/badge/CLI-111111">
  <img src="https://img.shields.io/badge/MIT-111111">
</p>

`repo-context` is a Rust CLI that turns a repo into high-signal
context artifacts for LLM workflows.

It focuses on: package source code into clean prompt & retrieval inputs with
predictable output.

## Commands

- `export` - build context artifacts
- `info` - inspect repository composition without exporting

## What `export` writes

For each export run, the tool writes:

- `<repo>_context_pack.md` (prompt-friendly repository narrative)
- `<repo>_chunks.jsonl` (chunked records for retrieval pipelines)
- `<repo>_report.json` (selection stats, skips, drops, and run metadata)

Artifact generation depends on `--mode`:

- `--mode prompt` -> context pack + report
- `--mode rag` -> chunks + report
- `--mode both` (default) -> context pack + chunks + report

By default outputs are written under `./out/<repo>/`.

## Export pipeline (contract)

`export` follows this deterministic flow:

1. fetch repository (local path or remote URL)
2. scan candidate files
3. rank high-signal files
4. redact file content (enabled by default)
5. chunk redacted content
6. render artifacts and report


## Quick start

```bash
git clone https://github.com/wheevu/repo-context.git
cd repo-context
cargo build --release

# Export current repository
cargo run -- export --path .

# Inspect repository only
cargo run -- info .
```

## Export examples

```bash
# Prompt + RAG outputs (default)
repo-context export --path . --mode both

# Prompt-only
repo-context export --path . --mode prompt

# RAG-only
repo-context export --path . --mode rag

# Reproducible output
repo-context export --path . --no-timestamp

# Disable secret redaction (not recommended)
repo-context export --path . --no-redact
```

## Development

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

## License

MIT

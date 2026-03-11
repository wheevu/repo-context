# repo-context

`repo-context` is a Rust CLI that turns a code repository into deterministic, high-signal
context artifacts for LLM workflows.

It focuses on one core job: package source code into clean prompt and retrieval inputs with
predictable output.

## What it outputs

For each export run, the tool writes:

- `<repo>_context_pack.md` (prompt-friendly repository narrative)
- `<repo>_chunks.jsonl` (chunked records for retrieval pipelines)
- `<repo>_report.json` (selection stats, skips, drops, and run metadata)

## Stable commands

- `export` - build context artifacts
- `info` - inspect repository composition without exporting

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
```

## Product boundaries

This tool is intentionally scoped.

It does **not** claim semantic code understanding, full code intelligence, or automated PR review.
Report fields are designed to describe what was selected, skipped, dropped, and emitted.

## Development

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

## License

MIT

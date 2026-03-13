# repo-context

<p align="center">
  <img src="assets/title.svg" width="70%">
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white">
  <img src="https://img.shields.io/badge/CLI-111111">
  <img src="https://img.shields.io/badge/MIT-111111">
</p>

`repo-context` is a Rust CLI that turns repositories into high-signal context artifacts for LLM workflows.

It exports clean, predictable prompt and retrieval inputs from local or remote codebases.

## Performance

Originally built in Python, later rewritten in Rust.

Local benchmarks showed **~82–83% lower export time** (**~5.5–5.8× faster**) on representative repositories.

| Repository | Python | Rust | Speedup |
|---|---:|---:|---:|
| `repo-context` | 5.896s | 1.074s | 5.49× |
| `dora-rs` | 2.079s | 0.357s | 5.82× |

Benchmarked with `hyperfine` using the same export workflow and `--no-timestamp`.

## Commands

- `export` — build context artifacts
- `info` — inspect repository composition without exporting

## Output

`export` writes artifacts under `./out/<repo>/`:

- `<repo>_context_pack.md` — prompt-friendly repository context
- `<repo>_chunks.jsonl` — retrieval chunks for embedding/indexing
- `<repo>_report.json` — selection stats and run metadata

By mode:

- `prompt` → context pack + report
- `rag` → chunks + report
- `both` → context pack + chunks + report

## Export flow

1. fetch repository
2. scan candidate files
3. rank high-signal files
4. redact secrets by default
5. chunk content
6. render artifacts and report

## Quick start

```bash
git clone https://github.com/wheevu/repo-context.git
cd repo-context
cargo build --release
```

Export current repository
```bash
cargo run -- export --path .
```
Inspect repository only
```bash
cargo run -- info .
```
##  Examples
Prompt + RAG outputs (default)
```
repo-context export --path . --mode both
```
Prompt-only
```
repo-context export --path . --mode prompt
```
RAG-only
```
repo-context export --path . --mode rag
```
Reproducible output
```
repo-context export --path . --no-timestamp
```
Disable secret redaction
```
repo-context export --path . --no-redact
```
## Development
```
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

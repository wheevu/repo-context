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

## Demo

Full scope against `tokio-rs/tokio`:

```bash
repo-context export --repo https://github.com/tokio-rs/tokio --scan-mode full --no-timestamp
```

```text
Export complete:
  root: /private/var/.../repo-context-...
  files: 846
  chunks: 2877
  tokens: 1508244
  wrote: ~/rc-output/tokio/tokio_context_pack.md
  wrote: ~/rc-output/tokio/tokio_chunks.jsonl
  wrote: ~/rc-output/tokio/tokio_report.json
```

Focused module (`tokio/src/lib.rs`):

```bash
repo-context export --repo https://github.com/tokio-rs/tokio --scan-mode focused --focus-file tokio/src/lib.rs --no-timestamp
```

```text
Export complete:
  root: /private/var/.../repo-context-...
  files: 314
  chunks: 1400
  tokens: 809090
  wrote: ~/rc-output/tokio/focus_lib/tokio_focus_lib_context_pack.md
  wrote: ~/rc-output/tokio/focus_lib/tokio_focus_lib_chunks.jsonl
  wrote: ~/rc-output/tokio/focus_lib/tokio_focus_lib_report.json
```

The focused context pack starts with the selected module, then follows the relevant Rust graph:

````markdown
### `tokio/src/lib.rs`
*Priority: 100% | Language: rust | Chunks: 9*

```rust
cfg_rt! {
    pub mod runtime;
}
```

### `tokio/src/runtime/runtime.rs`
*Priority: 90% | Language: rust | Chunks: 8*

```rust
use crate::task::JoinHandle;

/// The Tokio runtime.
///
/// The runtime provides an I/O driver, task scheduler, [timer], and
/// blocking pool, necessary for running asynchronous tasks.
```
````

## Performance

Originally built in Python, later rewritten in Rust.

No benchmark results are checked in yet. To collect local, reproducible evidence, run:

```bash
./bench/bench.sh          # benchmark this repository
./bench/bench.sh <repo>   # benchmark another local repository
```

The script builds the release binary with `--locked`, runs `repo-context export --no-timestamp --mode rag` with `hyperfine`, and writes raw timing JSON plus run metadata under `bench/results/`. See `bench/README.md` for the methodology.

## Commands

- `export` — build context artifacts
- `info` — inspect repository composition without exporting

## Output

`export` writes artifacts under `~/rc-output/<repo>/`:

- `<repo>_context_pack.md` — prompt-friendly repository context
- `<repo>_chunks.jsonl` — retrieval chunks for embedding/indexing
- `<repo>_report.json` — selection stats and run metadata

By mode:

- `prompt` → context pack + report
- `rag` → chunks + report
- `both` → context pack + chunks + report

Interactive exports can run in **focused mode**. Small repos show individual files; large repos show module groups. File focus includes the selected file plus its callers, dependencies, tests, and entry path. Module focus emits the entry's full dependency graph.

<details>
<summary>Export flow</summary>

1. fetch repository
2. scan candidate files
3. rank high-signal files
4. redact secrets by default
5. chunk content
6. render artifacts and report

</details>

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
<details>
<summary>Examples</summary>

Prompt + RAG outputs (default, maximum useful coverage)
```
repo-context export --path .
```
Budgeted output
```
repo-context export --path . --max-tokens 12000
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
Focused export (interactive)
```
repo-context export --path .
# choose "Focused", then pick a file or module
```
Focused export (non-interactive)
```
repo-context export --path . --scan-mode focused --focus-file src/main.rs
```
Disable secret redaction
```
repo-context export --path . --no-redact
```

</details>

## Development
```
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

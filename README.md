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

It exports clean, predictable prompt and retrieval inputs from local directories, GitHub repositories, and Hugging Face repositories.

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
  selected content tokens: 1508244
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
  selected content tokens: 809090
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
- `index` — build or refresh a local redacted SQLite retrieval index
- `review` — build a deterministic change-aware ImpactPackV1

## Configuration

Configuration is TOML-only.
Auto-discovery checks `repo-context.toml`, `.repo-context.toml`, `r2p.toml`, and `.r2p.toml` in the repository root.
Use `--config <FILE>` to load an explicit TOML file.

## Output

`export` writes artifacts under `~/rc-output/<repo>/`:

- `<repo>_context_pack.md` — prompt-friendly repository context
- `<repo>_chunks.jsonl` — retrieval chunks for embedding/indexing
- `<repo>_report.json` — selection stats and run metadata

Task exports add an explainable `retrieval` object to the report and to JSONL chunks with task evidence.
The report stores only a SHA-256 task fingerprint, never the raw task text.

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
The token selector initially reserves 40% for source and 60% for documentation/configuration.
Unused space may be borrowed by either pool; the report records the actual
`source_tokens_selected` and `context_tokens_selected` totals. The cap applies separately to each
rendered prompt and JSONL artifact, including its formatting and metadata.

Task-aware output (full coverage, task-ranked)
```
repo-context export --path . --task "trace OAuth refresh failures" --no-timestamp
```
Task-aware budgeted output
```
repo-context export --path . --task "trace OAuth refresh failures" --max-tokens 12000
```
Task retrieval is deterministic and local: BM25 lexical matching seeds a
bounded traversal of the existing static import graph, related tests, and
repository anchors. It uses no network, hosted model, embedding service, or
runtime execution. `--no-index` keeps the same retrieval behavior in memory.

Build an explicit local index
```
repo-context index --path . --db /tmp/repo-context.sqlite
```
When `--db` is omitted, the index lives in the platform user cache. On macOS
that is `~/Library/Caches/repo-context/indexes/<root-key>/index.sqlite`; Unix
uses `$XDG_CACHE_HOME` or `~/.cache`, and Windows uses `%LOCALAPPDATA%`.
The database contains relative paths, metadata, static imports, symbols, and
redacted chunks only. It is an advisory cache: exports re-scan and re-redact
the current repository before rendering. An unavailable implicit cache falls
back to memory; an explicit `--index-db` failure is an error.

Review the current working tree

```bash
repo-context review --path . --working-tree --format both --output /tmp/impact-pack.json
```
Review two Git refs

```bash
repo-context review --path . --base main --head HEAD --format json
```
Ref-to-ref review scans the checked-out repository to build its static import
graph and related-file set. For deterministic output, the worktree must be
clean and the checked-out `HEAD` commit must exactly match the commit resolved
by `--head`. Otherwise the command exits with an actionable error: check out
the requested head commit, or use `--working-tree` to review local changes.

Impact packs are versioned as `ImpactPackV1`. They contain changed
files, syntax-aware changed symbols, direct static imports and conservative
caller relationships, relevant tests/configuration/documentation, bounded
redacted changed-line snippets, and stable content hashes. The default
comparison is `HEAD` to the working tree; `--base` and
`--head` select two refs instead.

Review is intentionally static: import edges are not runtime call graphs, and
caller relationships are import-based approximations. Binary or unreadable
files produce metadata without source excerpts. Traversal is bounded per
changed-file seed, changed files and snippets are capped, and
`--max-related-files` bounds the emitted related-file set; the JSON
`limits` object records those bounds and whether output was truncated.

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

Export a Godot 4 project (auto-detected)
```
repo-context export --path /path/to/game --no-timestamp
```

Force or disable repository-specific behavior
```
repo-context export --path /path/to/game --profile godot
repo-context export --path /path/to/repo --profile generic
```

</details>

<details>
<summary>Godot 4 repositories</summary>

Godot support is auto-selected when the repository contains `project.godot`, a `.godot/` directory, or a `.gd`, `.tscn`, `.tres`, or `.gdshader` file.
`project.godot` is the strongest signal.
Set `profile = "godot"` or `profile = "generic"` in `repo-context.toml`, or use `--profile`, to override detection.
The Godot profile is layered onto the normal extension and ignore configuration.

The scanner understands GDScript (`.gd`), text scenes (`.tscn`), text resources (`.tres`), shaders (`.gdshader`, `.gdshaderinc`), Godot configuration (`.godot`, including `project.godot`), and `.cfg` files. It extracts GDScript declarations and resource paths, scene hierarchy and connections, project settings, shader declarations, standalone Godot test scripts, and the main scene/autoload/load relationships used by the context pack and JSON report.

Generated `.uid` and `.import` files are inventory-only, and `.godot/` editor/import caches are excluded. Images, audio, models, and binary `.res`/`.scn` resources are recorded with path, type, and size but are never read as text or emitted as chunks. Large scenes are structurally summarized; detailed node batches are retrieval-only when needed.

Valid JSON is parsed before minification checks. The prompt receives a concise top-level schema, while logical object/array batches remain available in RAG chunks. Invalid JSON safely falls back to normal text chunking. Minified-code protection remains active for JavaScript and similar source formats.

Known limitations: the Godot parsers are intentionally lightweight rather than compiler-complete. Dynamic resource paths, runtime-created node hierarchies, computed input action names, and binary asset internals cannot always be resolved statically. Malformed or unusual text formats degrade to source/text chunks instead of failing an export. Task retrieval makes static dependency/importer claims only; it does not claim runtime call reachability, compiler semantics, or behavioral correctness. Remote task exports use ephemeral retrieval and never persist a remote repository index. `--no-redact` also bypasses persistent indexing.

Godot analysis remains additive in report schema `1.5.0`: reports may contain a top-level `godot` object with resolved relationship metadata, and file dispositions may use `inventory_only` for generated metadata and binary assets that were indexed without textual chunks. Task reports additionally contain stable retrieval counts and relation labels; raw task text and cache paths are never emitted.

</details>

## Development
```
cargo fmt
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
cargo build --release --locked
```

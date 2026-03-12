# DESIGN.md

Internal design contract for `repo-context` after the scope reset.

This document is for maintainers and coding agents. It defines what this project is, what it is not, and how to evolve it without regressing into architecture sprawl.

This file can be committed when explicitly requested by a maintainer.

## 1. Product Thesis (Non-negotiable)

`repo-context` is a deterministic repository-to-context packager.

It exists to do one job well:
- scan a repository
- select high-signal files
- redact sensitive text heuristically
- chunk content for LLM workflows
- render stable artifacts

Stable CLI commands:
- `export`
- `info`

Everything else must justify itself against this thesis.

## 2. What We Deliberately Removed

We removed these because they diluted product clarity and inflated maintenance cost:

- multi-surface CLI (`index`, `query`, `codeintel`, `diff`)
- guided/interactive mode and preset numerology
- contribution/PR context modes
- separate graph persistence paths
- LSP/rust-analyzer enrichment as core behavior
- fake semantic reranking layer
- snapshot-heavy test surface tied to removed modes

Reason: those features looked sophisticated but produced weak leverage and inconsistent trust guarantees.

## 3. Architectural Decisions (And Why)

### Decision A: Thin CLI boundary
CLI modules parse args and dispatch; they do not own product logic.

Why:
- keeps command contracts stable
- makes behavior testable without shell ceremony
- prevents god files from reappearing

### Decision B: Keep the core pipeline
Core path remains:
- fetch -> scan -> rank -> redact -> chunk -> render -> report

Implementation detail that must remain true:
- redaction happens on full file content before chunking so structure-safe behavior is evaluated on full source.

Why:
- this is the actual user value path
- these stages already had real quality

### Decision C: Trustworthy reporting over ambitious reporting
`report.json` should prioritize factual fields (selected/skipped/dropped/output) and avoid claims that sound stronger than implementation.

Why:
- misleading analytics are worse than missing analytics
- downstream users build automation on this report

### Decision D: Fewer outputs, clearer contract
Supported outputs:
- `<repo>_context_pack.md`
- `<repo>_chunks.jsonl`
- `<repo>_report.json`

Why:
- clearer user expectations
- less hidden coupling

### Decision E: Scope over optionality
No speculative extensibility layers unless there is active demand and a validated consumer.

Why:
- optionality is expensive when unvalidated
- this project previously paid heavy complexity tax for low-usage paths

## 4. Current Module Map (Expected)

- `src/cli/`
  - `mod.rs` command wiring
  - `export.rs` argument parsing and dispatch only
  - `info.rs` info command
  - `utils.rs` small parse helpers

- `src/app/`
  - `export.rs` core export execution pipeline

- `src/fetch/`
  - repository acquisition only

- `src/scan/`
  - file discovery
  - tree generation

- `src/rank/`
  - file priority ranking

- `src/chunk/`
  - chunk generation and coalescing

- `src/redact/`
  - redaction rules and modes

- `src/render/`
  - markdown/jsonl/report rendering

- `src/config/`
  - config loading and CLI merge

- `src/domain/`
  - focused type modules (`config`, `file`, `chunk`, `ranking`, `redaction`, `stats`, `output`, `language`)

- `src/utils/`
  - encoding, hashing, path, classification, token helpers

## 5. Invariants Future Changes Must Preserve

1. Determinism
- `--no-timestamp` should produce stable outputs for the same input.
- sorting and IDs must remain stable.

2. Honest reporting
- report fields must represent observable behavior.
- no "semantic" or "coverage" claims without robust backing.

3. Stable command surface
- do not silently add new top-level commands.
- new command proposals require a thesis-level justification.

4. Pipeline readability
- no single file should become orchestration soup again.
- if a file feels like a control tower, split responsibilities.

5. Redaction safety posture
- keep default redaction on.
- preserve explicit opt-out (`--no-redact`).

6. Stage ownership clarity
- CLI files parse and dispatch.
- App layer owns orchestration.
- Domain types stay modular, not monolithic.

## 6. Design Philosophy for Future Agents

### 6.1 Depth-first, not breadth-first
When improving the system:
- first harden the core export path
- then simplify internals
- only then consider feature expansion

Do not add product surface while internals are still incoherent.

### 6.2 Kill-first discipline
When a change requests flexibility:
- ask what can be removed instead
- collapse duplicate code paths before adding new ones
- reject "maybe useful later" abstractions

### 6.3 Truth over vibes
Do not add labels like "semantic", "intelligent", "code-intel" unless the implementation genuinely earns them.

### 6.4 Contract-first modifications
Before major edits:
- identify which user contract is changing
- add/update tests for that contract
- then edit implementation

## 7. Agent Execution Protocol (Required)

When an agent works on this repo, follow this order:

1. Clarify scope against thesis
- Is the request directly improving `export` or `info`?
- If not, document why this belongs in core.

2. Locate contract tests
- existing integration tests in `tests/cli_tests.rs` and `tests/export_output_tests.rs`
- add/adjust tests before broad refactors

3. Make smallest coherent change set
- avoid mechanical churn
- avoid style-only rewrites unrelated to behavior

4. Validate fully
- `cargo fmt`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test`

5. Report impact
- what changed in behavior
- what was removed/simplified
- what invariants were checked

## 8. Rules For Reintroducing Any Removed Capability

A removed capability can return only if all checks pass:

- clear user/problem statement
- measurable value over current core
- no duplication with existing modules
- explicit trust model (what it can/cannot guarantee)
- isolated architecture boundary
- deterministic output implications understood
- test plan and maintenance owner identified

If these are not satisfied, do not reintroduce.

## 9. Anti-Patterns To Reject Immediately

- "just add one command" without thesis impact
- hidden optional branches inside `export` that behave like separate products
- giant argument structs with mostly speculative knobs
- report fields that are heuristic but look authoritative
- maintaining parallel persistence models for similar purpose
- comments that justify complexity with old parity goals instead of current product goals

## 10. Change Budget Guidance

Prefer these kinds of changes:
- improve ranking signal quality
- improve chunk quality and determinism
- improve redaction precision/false-positive tradeoff
- improve scan correctness and ignore handling
- improve report clarity and machine-readability
- improve test precision and speed

Be skeptical of these kinds of changes:
- generalized retrieval systems
- language-server integration in core path
- PR review/explainer automation
- feature presets and interactive control planes

## 11. Testing Strategy Contract

Expected test layers:

- Unit tests in module files for local logic
- Integration tests for stable CLI behavior:
  - `tests/cli_tests.rs`
  - `tests/export_output_tests.rs`

Required checks for export changes:
- deterministic behavior without timestamp
- artifact existence per mode
- redaction on/off behavior
- redaction correctness before chunking (including boundary-sensitive cases)
- report field shape and trust boundaries

Do not add brittle snapshots for noisy fields unless normalization is explicit.

## 12. Documentation Contract

Whenever behavior changes:
- update `README.md` examples and boundaries
- ensure CLI `--help` text matches README claims
- keep docs conservative and implementation-backed

No README theater:
- avoid unsubstantiated performance slogans
- avoid broad platform claims for narrow implementations

## 13. Practical Checklist For Any Non-Trivial PR

- [ ] Does this change strengthen `export` or `info` directly?
- [ ] Does it reduce complexity somewhere else?
- [ ] Are new flags/branches truly necessary?
- [ ] Are claims in outputs/docs still honest?
- [ ] Did we keep deterministic behavior?
- [ ] Did we run fmt, clippy (`-D warnings`), and tests?
- [ ] Did we avoid introducing duplicate architecture paths?

## 14. If You Are Unsure, Default To This

Choose the option that:
- keeps the command surface smaller
- keeps behavior more deterministic
- keeps reports more honest
- keeps the code easier to explain in one page

If a feature cannot pass that filter, it probably does not belong in core.

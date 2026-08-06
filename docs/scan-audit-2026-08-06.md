# Scanner module audit - 2026-08-06

Deep-dive audit of `src/scan/scanner.rs` (the pipeline entry point: gitignore-aware file discovery, filtering, disposition inventory, and stats).
All fixes landed in one session and are covered by tests in `src/scan/scanner.rs` and `src/utils/classify.rs`.

## Findings and fixes

### 1. HIGH, security: external files leaked through file symlinks under the default config

**Before:** the symlink containment check (`is_path_within_root`) only ran when `canonical_root` was set, which only happened when `follow_symlinks=true`.
Walkdir yields symlink entries even when `follow_links=false` (it only refuses to descend into directory links).
So with default settings, a repo containing `link.rs -> ~/.ssh/id_rsa` had the external file's content read, scanned, and embedded in the context pack.

**Fix:** the root is now canonicalized unconditionally, and every symlink entry whose resolved target escapes the root is rejected with a `SkippedSymlink` disposition.
If the root cannot be canonicalized, all symlink entries are rejected closed.
Entries under a followed link report themselves as symlinks, so files reached through an escaping ancestor link are caught too.

### 2. MEDIUM: unfollowed directory symlinks vanished from the inventory

**Before:** with `follow_symlinks=false`, a directory symlink was silently skipped (`continue`) with no disposition and no stats, so the report claimed `"complete": true` while those paths were absent.

**Fix:** unfollowed directory symlinks are now recorded as `SkippedSymlink` dispositions and counted in `files_discovered`/`files_skipped_symlink`, keeping the `dispositions.len() == files_discovered` invariant that the report relies on.

### 3. MEDIUM: duplicate content via symlinked directory aliases

**Before:** with `follow_symlinks=true`, `real/a.rs` and `linked/a.rs` (link to `real`) were both included - duplicated tokens, chunks, and stats, with distinct IDs.
The old test only asserted `any(...)` so it could not catch the duplicate.

**Fix:** when the walk observed any symlink, included files are deduplicated by canonical path.
The second alias is recorded as `SkippedSymlink` with a note naming the path that was kept.
The pre-existing test was strengthened to assert the dedup behavior.

### 4. MEDIUM, perf: wasted full second tree walk when gitignore was disabled

**Before:** `record_unseen_files()` always ran a second traversal (up to 50k examined files).
With `respect_gitignore=false` nothing can ever be unseen (the main walker yields every regular file, and both traversals prune hidden/noise directories identically), so the walk was pure waste on every `--no-gitignore` scan.

**Fix:** the reconciliation traversal is skipped entirely when `respect_gitignore=false`.

### 5. LOW: false `truncated`/`complete: false` report claim

**Before:** the unseen-inventory cap counted files *walked* (`examined`), not files *found*.
A repository with more than 50k files and nothing unseen reported a truncated inventory.

**Fix:** the cap now bounds the inventory size (unseen files found).
A repo with nothing unseen reports `complete: true` even past 50k files.
The existing cap test still passes unchanged because its fixture hits the cap with found files.

### 6. LOW: inconsistent disposition labels for identical situations

**Before:** a gitignored file under `playwright-report/` was labeled `ExcludedNoiseDir` while the same situation under `dist/`, `build/`, or `out/` was labeled `SkippedGitignore`.

**Fix:** every file found by the reconciliation traversal is now uniformly labeled `SkippedGitignore` (gitignore is the only hiding mechanism left after both traversals prune identically).
`is_excluded_noise_path` was removed; the `ExcludedNoiseDir` variant remains in `FileDispositionReason` for report-format compatibility but is no longer emitted.

### 7. LOW, perf: whole-file JSON reads during minified detection

**Before:** `is_likely_minified` called `read_to_string` on every `.json` file (up to 1MB each) just to decide the "one-line JSON" exception, duplicating the export read.

**Fix:** the JSON parse check now samples a bounded 64KB prefix (`JSON_SAMPLE_SIZE`).
One behavior consequence: a JSON file whose first 64KB is not self-contained valid JSON (e.g. a single string literal longer than 64KB) falls through to the line-length check.
This is a deliberate, documented heuristic trade.

## Files touched

- `src/scan/scanner.rs` - fixes 1-6, new tests
- `src/utils/classify.rs` - fix 7

## Tests

- `default_scanner_rejects_external_file_symlinks` (unix) - fix 1
- `default_scanner_records_unfollowed_directory_symlinks` (unix) - fix 2
- `follow_symlinks_allows_directory_targets_inside_repository` (unix) - strengthened to assert fix 3
- `unseen_inventory_is_complete_when_nothing_is_unseen` - fix 5
- `gitignore_disabled_skips_unseen_reconciliation_walk` - fix 4
- Full suite: 250 tests pass; `cargo clippy --all-targets` and `cargo fmt --check` clean.

## Notes for future agents

- The report invariant `dispositions.len() == files_discovered` in `tests/export_output_tests.rs::export_report_has_one_disposition_per_discovered_file` is load-bearing; keep it in mind when changing scanner accounting.
- Old reports may still contain `excluded_noise_dir` dispositions; do not remove the enum variant or its `as_str` arm without a schema bump.
- The containment check canonicalizes only symlink entries; regular files pay no extra syscalls.
- Dedup only activates when a symlink was observed, so repos without symlinks pay nothing.
- Walkdir's `path_is_symlink` is also true for entries reached *through* a followed link (the `follow_link` flag), which is why the containment check covers ancestor-link escapes.
- Deliberately untouched: hidden-dir files (`.cache/`, etc.) are still never inventoried - the directory filter prunes them from both traversals by design, matching the Python heritage.

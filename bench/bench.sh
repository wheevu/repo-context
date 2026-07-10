#!/usr/bin/env bash
# Reproducible performance benchmark for repo-context.
#
# Usage:
#   ./bench/bench.sh          # benchmark this repo
#   ./bench/bench.sh <repo>   # benchmark another local repo
#
# Requires: hyperfine, cargo (with --release), python3
# Outputs: raw hyperfine JSON, run metadata JSON, and export artifacts under bench/results/.

set -euo pipefail

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$BENCH_DIR/.." && pwd)"
RESULTS_DIR="$BENCH_DIR/results"

TARGET_REPO="${1:-$REPO_ROOT}"
TARGET_NAME="$(basename "$(cd "$TARGET_REPO" && pwd)")"
TARGET_ABS="$(cd "$TARGET_REPO" && pwd)"
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)"
RUN_DIR="$RESULTS_DIR/$TARGET_NAME/$RUN_ID"
HYPERFINE_JSON="$RUN_DIR/hyperfine.json"
METADATA_JSON="$RUN_DIR/metadata.json"

COMMAND_ARGS=(export --path "$TARGET_ABS" --no-timestamp --mode rag --output-dir "$RUN_DIR/export")

mkdir -p "$RUN_DIR/export"
echo "=== repo-context benchmark on $TARGET_NAME ==="
echo "  Target: $TARGET_ABS"
echo "  Binary: release mode (cargo build --release --locked)"
echo "  Run: $RUN_ID"
echo

# Build release binary
echo "[1/3] Building release binary..."
cargo build --release --locked --manifest-path "$REPO_ROOT/Cargo.toml" 2>&1 | tail -1
BIN="$REPO_ROOT/target/release/repo-context"
COMMAND_DISPLAY="$BIN export --path \"$TARGET_ABS\" --no-timestamp --mode rag --output-dir \"$RUN_DIR/export\""

echo "[2/3] Warming up (single run)..."
"$BIN" "${COMMAND_ARGS[@]}" 2>/dev/null

echo "[3/3] Running hyperfine (5 warmup, 10 timed runs)..."
hyperfine \
  --warmup 5 \
  --runs 10 \
  --export-json "$HYPERFINE_JSON" \
  --show-output \
  --command-name "repo-context export" \
  "$BIN export --path \"$TARGET_ABS\" --no-timestamp --mode rag --output-dir \"$RUN_DIR/export\""

echo "[metadata] Capturing run metadata..."
REPO_REVISION="$(git -C "$REPO_ROOT" rev-parse HEAD 2>/dev/null || true)"
REPO_DIRTY="$(test -n "$(git -C "$REPO_ROOT" status --porcelain 2>/dev/null)" && echo true || echo false)"
TARGET_REVISION="$(git -C "$TARGET_ABS" rev-parse HEAD 2>/dev/null || true)"
TARGET_DIRTY="$(test -n "$(git -C "$TARGET_ABS" status --porcelain 2>/dev/null)" && echo true || echo false)"
FIXTURE_SHA256="$(python3 - "$TARGET_ABS" <<'PY'
import hashlib
import os
import sys

root = os.path.abspath(sys.argv[1])
skip_dirs = {'.git', 'target', 'node_modules', 'dist', '.svelte-kit'}
hasher = hashlib.sha256()
file_count = 0
for current, dirs, files in os.walk(root):
    dirs[:] = sorted(d for d in dirs if d not in skip_dirs)
    if os.path.relpath(current, root).split(os.sep)[:2] == ['bench', 'results']:
        dirs[:] = []
        continue
    for name in sorted(files):
        path = os.path.join(current, name)
        rel = os.path.relpath(path, root).replace(os.sep, '/')
        if rel.startswith('bench/results/'):
            continue
        try:
            with open(path, 'rb') as fh:
                data = fh.read()
        except OSError:
            continue
        hasher.update(rel.encode())
        hasher.update(b'\0')
        hasher.update(data)
        hasher.update(b'\0')
        file_count += 1
print(f'{hasher.hexdigest()} {file_count}')
PY
)"
export REPO_ROOT TARGET_ABS TARGET_NAME RUN_ID RUN_DIR HYPERFINE_JSON COMMAND_DISPLAY
export REPO_REVISION REPO_DIRTY TARGET_REVISION TARGET_DIRTY FIXTURE_SHA256
python3 - <<'PY' > "$METADATA_JSON"
import json
import os
import platform
import shutil
import subprocess

def cmd(args):
    try:
        return subprocess.check_output(args, text=True, stderr=subprocess.STDOUT).strip()
    except Exception as exc:
        return f'unavailable: {exc}'

fixture_hash, _, fixture_files = os.environ['FIXTURE_SHA256'].partition(' ')
metadata = {
    'schema_version': 1,
    'benchmark': 'repo-context export rag',
    'run_id': os.environ['RUN_ID'],
    'methodology': 'Release binary built with cargo build --release --locked; hyperfine runs repo-context export with --no-timestamp --mode rag against a local fixture repository.',
    'command': os.environ['COMMAND_DISPLAY'],
    'hyperfine_raw_json': os.path.relpath(os.environ['HYPERFINE_JSON'], os.environ['RUN_DIR']),
    'repo_context': {
        'root': os.environ['REPO_ROOT'],
        'revision': os.environ['REPO_REVISION'],
        'dirty': os.environ['REPO_DIRTY'] == 'true',
    },
    'fixture': {
        'path': os.environ['TARGET_ABS'],
        'name': os.environ['TARGET_NAME'],
        'revision': os.environ['TARGET_REVISION'],
        'dirty': os.environ['TARGET_DIRTY'] == 'true',
        'content_sha256': fixture_hash,
        'file_count': int(fixture_files or 0),
    },
    'environment': {
        'os': platform.platform(),
        'machine': platform.machine(),
        'processor': platform.processor(),
        'python': platform.python_version(),
        'rustc': cmd(['rustc', '--version', '--verbose']),
        'cargo': cmd(['cargo', '--version']),
        'hyperfine': cmd(['hyperfine', '--version']) if shutil.which('hyperfine') else 'unavailable',
    },
}
print(json.dumps(metadata, indent=2, sort_keys=True))
PY

echo
echo "Results saved to: $RUN_DIR"
echo "  hyperfine: $HYPERFINE_JSON"
echo "  metadata:  $METADATA_JSON"

# Print summary
MEAN=$(python3 -c "
import json, sys
with open('$HYPERFINE_JSON') as f:
    d = json.load(f)
results = d['results']
for r in results:
    mean = r['mean']
    print(f\"  Mean: {mean:.3f}s (n={len(r['times'])} runs)\")
    print(f\"  Min:  {r['min']:.3f}s\")
    print(f\"  Max:  {r['max']:.3f}s\")
" 2>/dev/null || true)

echo
echo "=== System info ==="
uname -a
echo "Target fixture: $FIXTURE_SHA256"
echo "Binary version: $("$BIN" --version 2>/dev/null || echo "unknown")"

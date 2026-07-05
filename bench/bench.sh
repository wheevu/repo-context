#!/usr/bin/env bash
# Reproducible performance benchmark for repo-context.
#
# Usage:
#   ./bench/bench.sh          # benchmark this repo
#   ./bench/bench.sh <repo>   # benchmark another local repo
#
# Requires: hyperfine, cargo (with --release)
# Outputs: bench/results/ repo-root relative

set -euo pipefail

BENCH_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$BENCH_DIR/.." && pwd)"
RESULTS_DIR="$BENCH_DIR/results"

TARGET_REPO="${1:-$REPO_ROOT}"
TARGET_NAME="$(basename "$(cd "$TARGET_REPO" && pwd)")"

mkdir -p "$RESULTS_DIR"
echo "=== repo-context benchmark on $TARGET_NAME ==="
echo "  Target: $TARGET_REPO"
echo "  Binary: release mode (cargo build --release)"
echo

# Build release binary
echo "[1/3] Building release binary..."
cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml" 2>&1 | tail -1
BIN="$REPO_ROOT/target/release/repo-context"

echo "[2/3] Warming up (single run)..."
"$BIN" export --path "$TARGET_REPO" --no-timestamp --mode rag \
  --output-dir "$RESULTS_DIR" 2>/dev/null

echo "[3/3] Running hyperfine (5 warmup, 10 timed runs)..."
hyperfine \
  --warmup 5 \
  --runs 10 \
  --export-json "$RESULTS_DIR/${TARGET_NAME}_bench.json" \
  --show-output \
  --command-name "repo-context export" \
  "$BIN export --path \"$TARGET_REPO\" --no-timestamp --mode rag --output-dir \"$RESULTS_DIR\""

echo
echo "Results saved to: $RESULTS_DIR/${TARGET_NAME}_bench.json"

# Print summary
MEAN=$(python3 -c "
import json, sys
with open('$RESULTS_DIR/${TARGET_NAME}_bench.json') as f:
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
echo "Target repo files: $(find "$TARGET_REPO" -type f -not -path '*/target/*' -not -path '*/.git/*' -not -path '*/node_modules/*' | wc -l | tr -d ' ')"
echo "Binary version: $("$BIN" --version 2>/dev/null || echo "unknown")"

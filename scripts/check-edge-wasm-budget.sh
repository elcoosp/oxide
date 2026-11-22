#!/usr/bin/env bash
# V2-E09 / §V2-E09: enforce the 400 KB gzip wasm budget on the committed edge
# artifact (`crates/cli/resources/edge.wasm`). Runs `wasm-opt -Oz` when the
# optimizer is on PATH (spec: "wasm-opt -Oz, budget 400 KB gzip"); when it is
# absent the script measures the committed artifact's gzip size directly and
# reports — the gate itself always runs.
#
# Exit 0: the gzip size is within the budget.
# Exit 1: the gzip size exceeds the budget (or the artifact is missing/empty).
#
# The budget is 400 KB = 409600 bytes (binary kilobytes). The current
# artifact (router's `scan`/`regex` features omitted from the wasm build)
# ships at ~148 KB gzip — well under.
set -euo pipefail

WASM="${1:-crates/cli/resources/edge.wasm}"
BUDGET=409600  # 400 KB in bytes (400 * 1024).

if [ ! -s "$WASM" ]; then
    echo "ERROR: edge wasm artifact missing or empty at $WASM" >&2
    echo "  run: scripts/regen-edge-wasm.sh" >&2
    exit 1
fi

# Work on a copy so the committed artifact is never mutated by the gate.
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
cp "$WASM" "$TMP/edge.wasm"

if command -v wasm-opt >/dev/null; then
    wasm-opt -Oz "$TMP/edge.wasm" -o "$TMP/edge.opt.wasm"
    MEASURE="$TMP/edge.opt.wasm"
    MODE="wasm-opt -Oz"
else
    MEASURE="$TMP/edge.wasm"
    MODE="raw (wasm-opt not on PATH; the regen script applies -Oz when present)"
fi

GZIP_SIZE=$(gzip -c "$MEASURE" | wc -c | tr -d ' ')
RAW_SIZE=$(wc -c < "$MEASURE" | tr -d ' ')

echo "edge wasm budget gate ($MODE)"
echo "  artifact: $WASM"
echo "  raw size:   $RAW_SIZE bytes"
echo "  gzip size:  $GZIP_SIZE bytes"
echo "  budget:     $BUDGET bytes (400 KB gzip)"

if [ "$GZIP_SIZE" -le "$BUDGET" ]; then
    echo "PASS: gzip size $GZIP_SIZE ≤ budget $BUDGET"
    exit 0
else
    echo "FAIL: gzip size $GZIP_SIZE exceeds budget $BUDGET" >&2
    echo "  lean the wasm dep graph (gate router's scan/regex features off)" >&2
    echo "  for the edge build, then rerun scripts/regen-edge-wasm.sh." >&2
    exit 1
fi

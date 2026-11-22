#!/usr/bin/env bash
# M1(a) — the honest RED baseline. Runs clippy per-crate over the 8 divergent
# crates (bundler, cache, cli, dev, graph, jshost, plugin, server) plus assets
# (opted-in but the 2026-11-21 postscript records pre-existing pedantic debt).
#
# Feature gating keeps every run V8/nasm-free in this sandbox:
#   jshost, cli : --no-default-features (drops the `v8` feature; the V8-gated
#                 test/example targets carry `required-features = ["v8"]` so
#                 --all-targets skips them — clippy sees the Node-worker half)
#   assets      : default features (avif default-off → no nasm/rav1e)
#   others      : default features (pure Rust, no native toolchain)
#
# Each crate gets its own transcript under scripts/gates/clippy_red_<crate>.log;
# the consolidated counts roll up into scripts/gates/clippy_red.log.
set -uo pipefail

CRATES=(
  "graph:oxide-fw-graph:"
  "bundler:oxide-fw-bundler:"
  "cache:oxide-fw-cache:"
  "dev:oxide-fw-dev:"
  "plugin:oxide-fw-plugin:"
  "server:oxide-fw-server:"
  "jshost:oxide-fw-jshost:--no-default-features"
  "cli:oxide-fw-cli:--no-default-features"
  "assets:oxide-fw-assets:"
)

GATES_DIR="$(dirname "$0")"
cd "$(git rev-parse --show-toplevel)"

ROLLUP="$GATES_DIR/clippy_red.log"
{
  echo "# M1(a) RED baseline — per-crate pedantic finding counts"
  echo "# Captured: $(date -u +%Y-%m-%dT%H:%M:%SZ)  rustc $(rustc --version)"
  echo "# Command shape: cargo clippy -p <crate> [--no-default-features] --all-targets -- -D warnings -W clippy::pedantic"
  echo
} > "$ROLLUP"

for entry in "${CRATES[@]}"; do
  IFS=':' read -r name pkg extra <<< "$entry"
  flags=()
  [ -n "$extra" ] && flags+=("$extra")
  log="$GATES_DIR/clippy_red_${name}.log"
  echo "=== $name ($pkg ${flags[*]}) ===" | tee -a "$ROLLUP"
  # cargo clean -p keeps the shared dep rlibs (diagnostics etc.) but drops the
  # crate's own artifacts — the chunked-clearance disk discipline.
  cargo clean -p "$pkg" >/dev/null 2>&1 || true
  cargo clippy -p "$pkg" "${flags[@]}" --all-targets -- -D warnings -W clippy::pedantic \
    > "$log" 2>&1
  status=$?
  # Count finding lines: clippy emits "error: ..." / "warning: ..." per finding.
  finds=$(grep -cE '^(error|warning):' "$log" || true)
  # The trailing "could not compile ... due to N previous errors" line carries
  # the authoritative error count for a -D run.
  prev=$(grep -oE 'due to [0-9]+ previous errors?' "$log" | grep -oE '[0-9]+' || true)
  echo "findings=$finds  previous_errors=${prev:-0}  exit=$status" | tee -a "$ROLLUP"
  echo | tee -a "$ROLLUP"
done

echo "# rollup complete" >> "$ROLLUP"
cat "$ROLLUP"

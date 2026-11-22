#!/usr/bin/env bash
# M1(a) rollup — read the per-crate RED transcripts (clippy_red_<crate>.log)
# and emit a clean finding count per crate into scripts/gates/clippy_red.log.
# The authoritative count is the number of `^error:` lines that are NOT the
# "could not compile" summary (those are real clippy findings; the trailing
# summary and the Cargo.toml manifest warnings are excluded).
set -uo pipefail

GATES_DIR="$(dirname "$0")"
cd "$(git rev-parse --show-toplevel)"

CRATES=(graph bundler cache dev plugin server jshost cli assets)

{
  echo "# M1(a) RED baseline — per-crate pedantic finding counts"
  echo "# Captured: $(date -u +%Y-%m-%dT%H:%M:%SZ)  rustc $(rustc --version 2>/dev/null)"
  echo "# Command: cargo clippy -p <crate> [--no-default-features] --all-targets -- -D warnings -W clippy::pedantic"
  echo "# (jshost + cli run --no-default-features: V8-free; assets default: avif off, nasm-free)"
  echo "# The count is 'error:' lines minus the 'could not compile' summary"
  echo "# (excludes the Cargo.toml manifest warning and the build-failed notice)."
  echo
  printf '%-10s %-10s %s\n' "crate" "findings" "exit"
  printf '%-10s %-10s %s\n' "-----" "--------" "----"
  total=0
  for c in "${CRATES[@]}"; do
    log="$GATES_DIR/clippy_red_${c}.log"
    [ -f "$log" ] || { printf '%-10s %-10s %s\n' "$c" "MISSING" "-"; continue; }
    # Real findings: `^error:` lines that are NOT the "could not compile" summary.
    n=$(grep -cE '^error:' "$log" 2>/dev/null || true)
    # Subtract the "could not compile ... due to N previous errors" summary lines.
    summary=$(grep -cE '^error: could not compile' "$log" 2>/dev/null || true)
    findings=$((n - summary))
    [ "$findings" -lt 0 ] && findings=0
    exit_line=$(grep -oE 'exit=[0-9]+' "$log" 2>/dev/null | tail -1 || true)
    printf '%-10s %-10s %s\n' "$c" "$findings" "${exit_line:-?}"
    total=$((total + findings))
  done
  printf '%-10s %-10s\n' "TOTAL" "$total"
} > "$GATES_DIR/clippy_red.log"

cat "$GATES_DIR/clippy_red.log"

#!/usr/bin/env bash
# Single source of truth for the README's test badges.
#
# The README re-staled on arrival twice (r9, r10) because its counts were
# maintained by hand. This prints the measured numbers so the badge can be
# generated, and so `check-launch-content.sh` can compare against them.
#
#   ./scripts/count-tests.sh          human-readable
#   ./scripts/count-tests.sh --badge  the two badge values, space-separated
set -uo pipefail
cd "$(dirname "$0")/.."

# Rust: static count of test attributes (cheap, no toolchain run needed —
# the badge is a count of test *functions*, not a run result).
rust_count() {
  local bare tokio
  bare=$(grep -rh '#\[test\]' crates/ --include='*.rs' 2>/dev/null | wc -l | tr -d ' ')
  tokio=$(grep -rh '#\[tokio::test\]' crates/ --include='*.rs' 2>/dev/null | wc -l | tr -d ' ')
  echo $((bare + tokio))
}

# TypeScript: run the suites and sum the pass counts. Slow (~15 s), so the
# badge call caches per invocation; the gate calls it once.
ts_counts() {
  local total=0 files
  for d in packages/react packages/vue packages/svelte packages/solid \
           packages/oxide packages/create-oxide; do
    local n
    n=$(pnpm -C "$d" test 2>/dev/null | grep -E '^# pass' | tail -1 | awk '{print $3}')
    total=$((total + ${n:-0}))
  done
  if [ -d conformance/node_modules ]; then
    local c
    c=$(cd conformance && node --test 'test/*.test.ts' 2>/dev/null | grep -E '^# pass' | tail -1 | awk '{print $3}')
    total=$((total + ${c:-0}))
  fi
  files=$(ls packages/*/test/*.test.ts conformance/test/*.test.ts 2>/dev/null | wc -l | tr -d ' ')
  echo "$total $files"
}

RUST=$(rust_count)

if [ "${1:-}" = "--badge" ]; then
  read -r TS TS_FILES <<< "$(ts_counts)"
  echo "$RUST $TS $TS_FILES"
  exit 0
fi

read -r TS TS_FILES <<< "$(ts_counts)"
DEV=$(grep -c '^## ' docs/deviations.md)
echo "Rust test functions : $RUST"
echo "TypeScript tests    : $TS across $TS_FILES files"
echo "Deviations          : $DEV"

#!/usr/bin/env bash
# M0: the Tier-1 verification battery for the recovered tree (residuals fixed first).
# Results append to scripts/m0-battery.log; every gate's exit code is recorded.
set -uo pipefail
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
cd /home/z/my-project
LOG=scripts/m0-battery.log
: > "$LOG"

gate() { # gate <name> <cmd...>
  local name="$1"; shift
  echo "=== GATE $name: $* ===" >> "$LOG"
  "$@" >> "$LOG" 2>&1
  local rc=$?
  echo "=== GATE $name EXIT=$rc ===" >> "$LOG"
  df -h / | tail -1 >> "$LOG"
  return $rc
}

gate fmt        cargo fmt --all --check
rc=$?
if [ $rc -ne 0 ]; then echo "FMT FAILED — stopping battery" >> "$LOG"; exit 1; fi

gate check      cargo check --workspace --all-targets
rc=$?
echo "=== CHECK_EXIT=$rc ===" >> "$LOG"

gate clippy     cargo clippy -p oxide-fw-compiler -p oxide-fw-bundler -p oxide-fw-cli --all-targets --no-deps -- -D warnings
rc=$?
echo "=== CLIPPY_EXIT=$rc ===" >> "$LOG"

gate tests      cargo test -p oxide-fw-compiler -p oxide-fw-bundler --all-features --no-fail-fast
rc=$?
echo "=== TESTS_EXIT=$rc ===" >> "$LOG"

echo "=== BATTERY DONE ===" >> "$LOG"
grep -E "GATE .* EXIT|CHECK_EXIT|CLIPPY_EXIT|TESTS_EXIT" "$LOG"

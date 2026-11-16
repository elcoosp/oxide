#!/usr/bin/env bash
# V8-free compile + test battery for the touched crates.
set -uo pipefail
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
export CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
cd /home/z/my-project
LOG=scripts/v8free-battery.log
: > "$LOG"
echo "=== CHECK (V8-free touched crates) ===" >> "$LOG"
cargo check -p oxide-fw-compiler -p oxide-fw-bundler -p oxide-fw-server -p oxide-fw-dispatch -p oxide-fw-router -p oxide-fw-cache --all-targets >> "$LOG" 2>&1
echo "=== CHECK_EXIT=$? ===" >> "$LOG"
df -h / | tail -1 >> "$LOG"
echo "=== TESTS ===" >> "$LOG"
cargo test -p oxide-fw-compiler -p oxide-fw-bundler -p oxide-fw-server -p oxide-fw-dispatch -p oxide-fw-router -p oxide-fw-cache --all-features --no-fail-fast >> "$LOG" 2>&1
echo "=== TESTS_EXIT=$? ===" >> "$LOG"
df -h / | tail -1 >> "$LOG"
echo "=== V8FREE DONE ===" >> "$LOG"
grep -E "CHECK_EXIT|TESTS_EXIT|test result:" "$LOG" | tail -20

#!/usr/bin/env bash
# scripts/bisect_smoke.sh — M2 zero-API local bisect harness.
#
# Walks BISECT_GOOD..HEAD (default 8bcc462e..HEAD, the recorded regression
# window) one commit at a time. Every step rebuilds `oxide-fw-cli` and
# re-runs the smoke build against examples/smoke-app-react:
#
#   cargo build -p oxide-fw-cli && ./target/debug/oxide build --root examples/smoke-app-react
#
# A compile break counts as BAD (a commit that cannot be built cannot be
# good). Exit-code contract expected by `git bisect run`:
#   0 = good (build + smoke pass), 1 = bad, 125 = skip (e.g. checkout fail).
#
# All git operations are strictly LOCAL — no GitHub API/CI lookups anywhere.
#
# Usage:
#   bash scripts/bisect_smoke.sh            # orchestrator: starts and runs
#                                           # the bisect, prints the first
#                                           # bad SHA + the bisect log
#   bash scripts/bisect_smoke.sh --step     # one-step probe (used by
#                                           # `git bisect run` internally)
#
# Environment:
#   BISECT_GOOD  base commit (default 8bcc462e86a52c608b97f7720c7b0e3429db3e46)
#   LIBCLANG_PATH dir holding libclang.so for the v8/bindgen chain
set -u

cd "$(git rev-parse --show-toplevel)" || exit 125

# Toolchain + the vendored native chain (the M1 sandbox recipe).
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0
: "${LIBCLANG_PATH:=$HOME/.venv/lib/python3.12/site-packages/clang/native}"
export LIBCLANG_PATH

GOOD="${BISECT_GOOD:-8bcc462e86a52c608b97f7720c7b0e3429db3e46}"
STEP_LOG=/tmp/bisect_step.log

step() {
    echo "=== bisect step @ $(git rev-parse --short HEAD) ==="
    # The CLI must build for the commit to be testable at all.
    if ! cargo build -p oxide-fw-cli >"$STEP_LOG" 2>&1; then
        echo "BUILD-BROKEN (counts as bad): tail of build log:"
        tail -20 "$STEP_LOG"
        exit 1
    fi
    if ! ./target/debug/oxide build --root examples/smoke-app-react \
        >"$STEP_LOG" 2>&1; then
        echo "SMOKE-BROKEN (counts as bad): tail of smoke log:"
        tail -20 "$STEP_LOG"
        exit 1
    fi
    echo "GOOD"
    exit 0
}

orchestrate() {
    # The repro must actually be red at HEAD, or there is nothing to bisect.
    if ! cargo build -p oxide-fw-cli >/dev/null 2>&1; then
        echo "HEAD does not build — fix the environment before bisecting" >&2
        exit 2
    fi
    if ./target/debug/oxide build --root examples/smoke-app-react \
        >/dev/null 2>&1; then
        echo "HEAD smoke build PASSES — regression not reproduced; no bisect" >&2
        exit 2
    fi

    git bisect start HEAD "$GOOD" || exit 2
    git bisect run "$0" --step
    STATUS=$?
    echo "=== git bisect run exit: $STATUS ==="
    git bisect log
    # Keep the session OUT of bisect state for follow-up work.
    git bisect reset
    exit "$STATUS"
}

case "${1:-}" in
    --step) step ;;
    *) orchestrate ;;
esac

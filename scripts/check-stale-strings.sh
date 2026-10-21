#!/usr/bin/env bash
# §22 / L5 — grep gate for stale strings that no longer match the code.
#
# The three strings below have been fixed at least once each:
#   - "only transform is merged"      -> J4 (resolveId and load also merged)
#   - "not yet merged"                -> J4 (same)
#   - "not wired into the pipeline"   -> J2 (enables() reports it)
#   - "conservative landing"          -> L5 (per-chunk reuse is the design)
#
# A future commit that reintroduces any of these rots the docs the same way
# it did before. Fail the build. The doc-comment on the flag itself is fine
# (an explanatory `not yet merged — see WP-X` would be caught here too, so
# keep the language honest or remove the string).
#
# Excluded:
#   - test files (`*tests*.rs`, `/tests/`) — they may legitimately assert
#     the absence of the string (grep would otherwise match the assertion).
#   - docs/*.md and benchmarks/report.md — historical records legitimately
#     mention the strings when describing what was fixed.
set -uo pipefail
cd "$(dirname "$0")/.."

FAIL=0
check() {
    local pattern="$1"
    local why="$2"
    local hits
    hits=$(grep -rnE "$pattern" --include='*.rs' crates/*/src/ 2>/dev/null \
        | grep -vE '(tests\.rs|/tests/)' \
        || true)
    if [ -n "$hits" ]; then
        echo "STALE: '$pattern' ($why)"
        echo "$hits" | sed 's/^/  /'
        FAIL=1
    fi
}

check 'only transform is merged'      'J4: resolveId and load are merged too'
check 'not yet merged'                'J4: hooks are merged; check the merge table'
check 'not wired into the pipeline'   'J2: enables() reports it'
check 'conservative landing'          'L5: per-chunk reuse is the design now'

if [ "$FAIL" -eq 0 ]; then
    echo "stale-strings: PASS (no stale strings in src/)"
    exit 0
fi
echo "stale-strings: FAIL — see above"
exit 1

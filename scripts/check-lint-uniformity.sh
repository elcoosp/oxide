#!/usr/bin/env bash
# V2-A02 closing leg — the CI-callable workspace lint-uniformity guard.
#
# The shell form of `crates/diagnostics/tests/lint_uniformity.rs`: every
# workspace member's Cargo.toml must carry exactly
#
#   [lints]
#   workspace = true
#
# and nothing else. A bare `[lints]` header (opt-out) or a `[lints.clippy]` /
# `[lints.rust]` override (divergence) fails the check. The rust/lint CI job
# can invoke this before `cargo clippy --workspace` so a re-introduced
# divergent table is caught even when the offending crate's own clippy pass
# is environment-blocked (no libclang/nasm on the runner).
#
# Exit 0 = uniform; exit 1 = drift (names every offender with a one-line fix).
set -uo pipefail

root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"

# Enumerate every workspace member the root Cargo.toml declares. The members
# array uses explicit `crates/<name>` paths (no globs), one quoted path per
# line. Extract each quoted path between `members = [` and the closing `]`.
mapfile -t members < <(
    awk '
        /members[[:space:]]*=/ { in_members = 1 }
        in_members && /^[[:space:]]*\]/ { in_members = 0; next }
        in_members {
            if (match($0, /"[^"]+"/)) {
                print substr($0, RSTART+1, RLENGTH-2)
            }
        }
    ' "$root/Cargo.toml"
)

if [ "${#members[@]}" -eq 0 ]; then
    echo "check-lint-uniformity: FAILED to parse workspace members from $root/Cargo.toml" >&2
    exit 1
fi

fail=0
count=${#members[@]}

echo "check-lint-uniformity: scanning $count workspace members..." >&2

for member in "${members[@]}"; do
    manifest="$root/$member/Cargo.toml"
    if [ ! -f "$manifest" ]; then
        printf '  - %s: Cargo.toml missing at %s\n' "$member" "$manifest" >&2
        fail=1
        continue
    fi
    # Extract the [lints] table body: lines from the bare `[lints]` header to
    # the next top-level `[<table>]` header that is NOT a `[lints.*]` sub-table.
    # A `[lints.*]` sub-table is part of the lints table (an override) and is
    # surfaced as a body line so the divergent-own-table verdict catches it.
    body=$(awk '
        $0 ~ /^[[:space:]]*\[lints\][[:space:]]*$/ { in_lints = 1; next }
        in_lints && $0 ~ /^[[:space:]]*\[/ {
            if ($0 ~ /^[[:space:]]*\[lints\./) { print; next }
            in_lints = 0
            next
        }
        in_lints {
            line = $0
            sub(/^[[:space:]]+/, "", line)
            sub(/[[:space:]]+$/, "", line)
            if (line == "" || line ~ /^#/) next
            print line
        }
    ' "$manifest")

    if [ -z "$body" ]; then
        printf '  - %s: no [lints] table or bare header (no `workspace = true`) — add:\n' "$member" >&2
        printf '      [lints]\n      workspace = true\n' >&2
        fail=1
        continue
    fi

    # The accepted body is exactly `workspace = true` (spaces optional). The
    # norm (spaces stripped) equals `workspace=true` iff the body carries
    # nothing else — any override sub-table or extra key changes the norm.
    # (Command substitution strips the trailing newline awk appends, so a
    # line count is unreliable here; the norm is the authoritative check.)
    norm=$(printf '%s' "$body" | tr -d ' ')
    if [ "$norm" = "workspace=true" ]; then
        continue
    fi
    # Divergent: an override sub-table or an extra key alongside workspace = true.
    printf '  - %s: divergent [lints] table — expected exactly `workspace = true`, got:\n' "$member" >&2
    while IFS= read -r line; do
        printf '      | %s\n' "$line" >&2
    done <<< "$body"
    fail=1
done

if [ "$fail" -eq 0 ]; then
    echo "check-lint-uniformity: PASS — all $count workspace members inherit [lints] workspace = true"
    exit 0
fi

{
    echo "check-lint-uniformity: FAIL — workspace lint-policy drift."
    echo "Every member must carry exactly:"
    echo "  [lints]"
    echo "  workspace = true"
    echo "The drift guard test (crates/diagnostics/tests/lint_uniformity.rs) names the same offenders."
} >&2
exit 1

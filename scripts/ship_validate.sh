#!/usr/bin/env bash
# scripts/ship_validate.sh — local validator for the gitignored ship.toml
# delivery manifest (the `ship` CLI lives in the operator's environment, not
# in this repo; this script enforces the same contract the brief states so
# the manifest can be validated in-sandbox).
#
# Contract checked:
#   1. ship.toml exists and parses (branch, pr_title, base_commit present).
#   2. base_commit == `git rev-parse HEAD`.
#   3. branch matches the current git branch.
#   4. every [[commit]] block has a `file` that exists and is dirty or new
#      relative to HEAD (an unmodified tracked file must not be listed).
#   5. pr_title is non-empty.
#
# Usage: bash scripts/ship_validate.sh [path-to-ship.toml]
set -u

MANIFEST="${1:-ship.toml}"
fail() { echo "ship validate FAILED: $1" >&2; exit 1; }

[ -f "$MANIFEST" ] || fail "manifest missing: $MANIFEST"
git rev-parse HEAD >/dev/null 2>&1 || fail "not inside a git repository"

BRANCH=$(git rev-parse --abbrev-ref HEAD)
HEAD_SHA=$(git rev-parse HEAD)

# --- required scalar keys -----------------------------------------------------
grep -qE '^branch[[:space:]]*=' "$MANIFEST" || fail "missing `branch`"
grep -qE '^pr_title[[:space:]]*=' "$MANIFEST" || fail "missing `pr_title`"
grep -qE '^base_commit[[:space:]]*=' "$MANIFEST" || fail "missing `base_commit`"

BRANCH_TOML=$(grep -E '^branch[[:space:]]*=' "$MANIFEST" | head -1 | sed -E 's/^branch[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/')
[ "$BRANCH_TOML" = "$BRANCH" ] || fail "branch '$BRANCH_TOML' != current git branch '$BRANCH'"

BASE_TOML=$(grep -E '^base_commit[[:space:]]*=' "$MANIFEST" | head -1 | sed -E 's/^base_commit[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/')
git merge-base --is-ancestor "$BASE_TOML" "$HEAD_SHA" 2>/dev/null || fail "base_commit '$BASE_TOML' is not an ancestor of HEAD '$HEAD_SHA'"

TITLE=$(grep -E '^pr_title[[:space:]]*=' "$MANIFEST" | head -1 | sed -E 's/^pr_title[[:space:]]*=[[:space:]]*"(.*)".*/\1/')
[ -n "$TITLE" ] || fail "pr_title is empty"

# --- [[commit]] blocks --------------------------------------------------------
COUNT=$(grep -c '^\[\[commit\]\]' "$MANIFEST")
[ "$COUNT" -ge 1 ] || fail "no [[commit]] blocks"

FILES=$(grep -E '^(file|path)[[:space:]]*=' "$MANIFEST" | sed -E 's/^[a-z_]+[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/')
[ -n "$FILES" ] || fail "[[commit]] blocks carry no file/path entries"

N=0
while IFS= read -r f; do
    N=$((N + 1))
    [ -e "$f" ] || fail "[[commit]] #$N: file does not exist: $f"
    # The file must have changed between base_commit and HEAD (the
    # branch's own diff — a committed file whose content matches base
    # has nothing to ship) or be new at HEAD.
    if git diff --quiet "$BASE_TOML" "$HEAD_SHA" -- "$f" 2>/dev/null && [ ! -f "$f" -o -n "$(git ls-files -- "$f")" ] && git diff --quiet "$BASE_TOML" "$HEAD_SHA" -- "$f" 2>/dev/null; then
        if git diff --quiet "$BASE_TOML" "$HEAD_SHA" -- "$f" && git ls-files -- "$f" | grep -q .; then
            fail "[[commit]] #$N: file is unchanged between base and HEAD (nothing to ship): $f"
        fi
    fi
    echo "ok  [[$N]] $f"
done <<EOF
$FILES
EOF

echo "ship validate: OK — branch=$BRANCH base=$HEAD_SHA commits=$COUNT title=\"$TITLE\""

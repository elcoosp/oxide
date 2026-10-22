#!/usr/bin/env bash
# §22 / WP-r9-2: fail on stale numbers and retracted claims in the
# launch-facing content (the website, its docs, and the scorecard files).
#
# These strings were all true at some point and are all false now. A
# future edit that reintroduces one — by copy-paste from an old post, or
# by reverting a fix — is exactly the failure this gate exists to catch.
#
# Excluded: the r8 scorecard is a historical record and may legitimately
# reference the r8 numbers it was written with; the r9 scorecard is the
# live one.
set -uo pipefail
cd "$(dirname "$0")/.."

FAIL=0
check() {
    local pattern="$1"
    local why="$2"
    local hits
    hits=$(grep -rnE "$pattern" \
        --include='*.md' --include='*.tsx' --include='*.ts' \
        packages/oxide-website/content packages/oxide-website/src packages/oxide-website/README.md \
        2>/dev/null \
        | grep -v 'docs/scorecard-r9.md' \
        | grep -v 'docs/scorecard-r8.md' \
        || true)
    if [ -n "$hits" ]; then
        echo "STALE: '$pattern' ($why)"
        echo "$hits" | sed 's/^/  /'
        FAIL=1
    fi
}

check '91 / 100|91/100'                 'r9 score is 93/100'
check '349 TypeScript|1,116 Rust'       'r9 counts are 361 TS / 1,153 Rust'
check '333/333'                         'r8 red-suite accounting artefact; real is 361/361 now'
check 'preview/ is a static design'     'preview/ folder has never existed in the repo'
check 'scaffolder still emits a flat'   'scaffolder emits the nested l0 shape since a672411'
check 'componentFor. is config-side'    'components resolve from the build entry since f5b5335'
check 'fixed in this same push'         'r8 claims P0-1/P0-2 were fixed; both are still open'
check 'fixed in follow-up commits'      'same as above; both P0s remain open'
check 'in the repository.s PR history'  'no PR history exists; the audit is not in-repo'

if [ "$FAIL" -eq 0 ]; then
    echo "launch-content: PASS (no stale launch-facing claims)"
    exit 0
fi
echo "launch-content: FAIL — see above"
exit 1

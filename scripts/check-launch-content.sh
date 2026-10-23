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
        | grep -v 'auditing-our-own-framework' \
        || true)
    if [ -n "$hits" ]; then
        echo "STALE: '$pattern' ($why)"
        echo "$hits" | sed 's/^/  /'
        FAIL=1
    fi
}

check '91 / 100|91/100'                 'the audited score moved past 91'
check '93 / 100|93/100'                 'r10 scores 95/100'
check '349 TypeScript|1,116 Rust'       'stale counts; see the README badge for the current numbers'
check '361 TypeScript|1,153 Rust'       'stale counts; see the README badge for the current numbers'
check '333/333'                         'r8 red-suite accounting artefact; the suite is green now'
check 'twelve gates|12 gates|12 CI'     'the workflow runs fourteen gates'
check 'preview/ is a static design'     'preview/ folder has never existed in the repo'
check 'scaffolder still emits a flat'   'scaffolder emits the nested l0 shape since a672411'
check 'componentFor. is config-side'    'components resolve from the build entry since f5b5335'
check 'fixed in this same push'         'r8 claims P0-1/P0-2 were fixed; both are still open'
check 'fixed in follow-up commits'      'same as above; both P0s remain open'
check 'in the repository.s PR history'  'no PR history exists; the audit is not in-repo'

# ---------------------------------------------------------------------------
# README accuracy (WP-1). The README re-staled on arrival twice (r9, r10)
# because its counts were hand-maintained. These checks compare it against
# measured reality, so a stale number fails the same gate as a stale claim.
# ---------------------------------------------------------------------------
README=README.md
if [ -f "$README" ]; then
    # (a) The Rust test count in the badge and the prose must agree with the
    #     static count. No toolchain run needed — it is an attribute count.
    rust_measured=$(grep -rh '#\[test\]' crates/ --include='*.rs' 2>/dev/null | wc -l | tr -d ' ')
    rust_tokio=$(grep -rh '#\[tokio::test\]' crates/ --include='*.rs' 2>/dev/null | wc -l | tr -d ' ')
    rust_total=$((rust_measured + rust_tokio))
    rust_badge=$(grep -oE 'Tests-[0-9]+%20Rust' "$README" | head -1 | sed -E 's/Tests-([0-9]+).*/\1/')
    if [ -n "$rust_badge" ] && [ "$rust_badge" != "$rust_total" ]; then
        echo "STALE: README badge says ${rust_badge} Rust tests; measured ${rust_total}"
        FAIL=1
    fi

    # (b) The deviation count must match the headings in deviations.md.
    dev_count=$(grep -c '^## ' docs/deviations.md 2>/dev/null || echo 0)
    dev_word=$(grep -oE '(six|seven|eight|nine|ten) documented' "$README" | head -1 | awk '{print $1}')
    case "$dev_word" in
        six)   dev_claim=6 ;;  seven) dev_claim=7 ;;
        eight) dev_claim=8 ;;  nine)  dev_claim=9 ;;  ten) dev_claim=10 ;;
        *)     dev_claim=0 ;;
    esac
    if [ "$dev_claim" -ne 0 ] && [ "$dev_claim" != "$dev_count" ]; then
        echo "STALE: README says '${dev_word} documented' deviations; deviations.md has ${dev_count}"
        FAIL=1
    fi

    # (c) The collections-contradiction guard: the README must not claim no
    #     shipped loader calls the API while one does.
    if grep -q 'no shipped loader calls it yet' "$README" 2>/dev/null && \
       grep -rq 'getCollection' packages/oxide-website/src/lib/md.server.ts 2>/dev/null; then
        echo "STALE: README says no collections caller; md.server.ts calls getCollection"
        FAIL=1
    fi
fi

# A "K1, wired" claim is only honest while a shipped loader actually calls
# the collections API. The grep above cannot express that condition, so it is
# checked here: if any launch-facing file says collections are wired, at
# least one non-test, non-generated source file must import and call
# `getCollection`/`getEntry` from `oxide/server`.
if grep -rqE 'K1, wired|collections (are|is) wired' \
    --include='*.md' --include='*.tsx' --include='*.ts' \
    packages/oxide-website/content packages/oxide-website/src packages/oxide-website/README.md \
    packages/oxide-website/oxide.config.ts 2>/dev/null; then
    callers=$(grep -rlE '\bget(Collection|Entry)\(' \
        --include='*.ts' --include='*.tsx' \
        packages/oxide-website/src 2>/dev/null \
        | grep -vE '\.oxide\.ts$' \
        | grep -v '/content\.ts$' \
        | wc -l | tr -d ' ')
    if [ "$callers" -eq 0 ]; then
        echo "STALE: 'K1, wired' claimed but no shipped loader calls getCollection/getEntry"
        FAIL=1
    else
        echo "k1-wired: PASS ($callers caller file(s))"
    fi
fi

if [ "$FAIL" -eq 0 ]; then
    echo "launch-content: PASS (no stale launch-facing claims)"
    exit 0
fi
echo "launch-content: FAIL — see above"
exit 1

#!/usr/bin/env bash
# Verify that the prose ranges in benchmarks/report.md match the table.
#
# report.md's header used to claim "the prose narratives summarise them and
# are kept in sync by CI's gate (which compares the main table only)". That
# was false in both halves: gate.sh compares a *fresh* table against the
# committed one and never reads the prose. This script is the missing half.
#
# Checked claims (each must equal the min-max of its table column):
#   **`ttfb(ms)` = LO-HI ms**
#   **`start(ms)` = LO-HI ms**
#   **`cold(ms)` = LO-HI**
# and the per-fixture server bytes line:
#   **`server(B)`**: react N, vue N, svelte N, solid N
#
# Anything else in the prose is descriptive and out of scope.
set -uo pipefail

REPORT="${1:-benchmarks/report.md}"
FAIL=0

# --- parse the main table into "fixture cold warm client server pages start rss ttfb" ---
TABLE=$(awk '
    !in_t && /^\|/ && /fixture/ { in_t = 1; next }
    in_t && /^\|/ {
        gsub(/^ *\| *| *\| *$/, "")
        n = split($0, c, / *\| */)
        # Skip the header and the `| -------: |` separator: the separator
        # starts with `|`, so a `/^-/` line test never catches it, and its
        # `-------:` cells coerce to 0 and poison every min.
        if (n >= 9 && c[1] != "fixture" && c[1] !~ /^[-:]+$/) {
            printf "%s %s %s %s %s %s %s %s %s\n", c[1], c[2], c[3], c[4], c[5], c[6], c[7], c[8], c[9]
        }
        next
    }
    in_t { exit }
' "$REPORT")

if [ -z "$TABLE" ]; then
    echo "prose-sync: FAIL — could not parse the main table from $REPORT"
    exit 1
fi

# column index in the normalized row: 1=fixture 2=cold 3=warm 4=client 5=server 6=pages 7=start 8=rss 9=ttfb
col_min_max() {
    local col="$1"
    echo "$TABLE" | awk -v c="$col" '
        {
            n++
            val = $c + 0
            if (n == 1 || val < mn) mn = val
            if (n == 1 || val > mx) mx = val
        }
        END { printf "%d-%d\n", mn, mx }
    '
}

check_range() {
    local metric="$1" col="$2"
    local actual expected
    actual=$(col_min_max "$col")
    # the prose line: **`metric` = LO-HI
    # `metric` contains parentheses (`ttfb(ms)`), which are regex
    # metacharacters — escape every non-alphanumeric before matching.
    local esc
    esc=$(printf '%s' "$metric" | sed 's/[^A-Za-z0-9]/\\&/g')
    expected=$(grep -oE "\*\*\`${esc}\` = [0-9]+-[0-9]+" "$REPORT" | head -1 | sed -E "s/.*= //")
    if [ -z "$expected" ]; then
        echo "prose-sync: FAIL — no \`${metric}\` = LO-HI claim found in the prose"
        FAIL=1
        return
    fi
    if [ "$actual" != "$expected" ]; then
        echo "prose-sync: FAIL — \`${metric}\` prose says $expected, table is $actual"
        FAIL=1
    else
        echo "prose-sync: ok   \`${metric}\` $expected"
    fi
}

check_range "ttfb(ms)"  9
check_range "start(ms)" 7
check_range "cold(ms)"  2

# --- per-fixture server bytes: react 17787, vue 714, svelte 1627, solid 570 ---
SERVER_LINE=$(grep -oE '\*\*`server\(B\)`\*\*:.*' "$REPORT" | head -1)
if [ -z "$SERVER_LINE" ]; then
    echo "prose-sync: FAIL — no \`server(B)\` per-fixture claim found"
    FAIL=1
else
    for fx in react vue svelte solid; do
        prose=$(echo "$SERVER_LINE" | grep -oE "${fx} [0-9]+" | head -1 | awk '{print $2}')
        actual=$(echo "$TABLE" | awk -v f="smoke-app-${fx}" '$1 == f { print $5 }')
        if [ -z "$prose" ]; then
            echo "prose-sync: FAIL — server(B) prose has no ${fx} value"
            FAIL=1
        elif [ "$prose" != "$actual" ]; then
            echo "prose-sync: FAIL — server(B) ${fx} prose says $prose, table is $actual"
            FAIL=1
        else
            echo "prose-sync: ok   \`server(B)\` ${fx} ${actual}"
        fi
    done
fi

if [ "$FAIL" -eq 0 ]; then
    echo "prose-sync: PASS (prose ranges match the table)"
    exit 0
fi
echo "prose-sync: FAIL — see above"
exit 1

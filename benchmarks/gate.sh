#!/usr/bin/env bash
# §21 CI regression gate. Compare a fresh `run.sh` table against the
# committed `benchmarks/report.md` table and fail when any tracked metric
# regresses by more than the gate threshold (default 5%).
#
# Usage: gate.sh <fresh-table> <report.md> [threshold-percent]
#
# Tracked metrics (higher is worse):
#   cold build ms, warm build ms, client B, server B, start ms, ttfb ms
# Lower-is-better metrics are compared the same way (a +x% move is a
# regression). RSS is tracked too; page count is not (a fixture adding a
# route is not a regression).
set -uo pipefail

FRESH="${1:?usage: gate.sh <fresh-table> <report.md> [threshold]}"
REPORT="${2:?}"
THRESHOLD="${3:-5}"

# Extract the first markdown table from report.md (the run.sh table lives
# under "## The table" and starts with `| fixture`).
# Detect the table by any line whose first cell contains "fixture".
# The header row is column-padded, so an exact `/^\| fixture \|/` regex
# never matches. Fixed 2026-10-21 during Wave M — the old regex made the
# gate silently PASS on arbitrary regressions.
extract_table() {
    awk '
        !in_table && /^\|/ && /fixture/ { in_table = 1; print; next }
        in_table && /^\|/ { print; next }
        in_table { exit }
    ' "$1"
}

# Fresh table is plain `run.sh` output (space-separated, no pipes).
# Normalize both to "fixture cold warm client server pages start rss ttfb".
# Only accept rows whose first field looks like a fixture name. The
# harness's stderr (interleaved `Killed: 9` job-control notices from
# run.sh's per-fixture kill) would otherwise pass the NF >= 9 test and
# then just be dropped at base-lookup. Filtering at the source keeps
# TMP_F clean. Fixed 2026-10-21 during Wave M.
normalize_fresh() {
    awk '
        NR > 2 && NF >= 9 && $1 ~ /^smoke-app-/ {
            printf "%s %s %s %s %s %s %s %s %s\n", $1, $2, $3, $4, $5, $6, $7, $8, $9
        }
    ' "$1"
}

normalize_report() {
    extract_table "$1" | awk -F'|' '
        NF >= 10 {
            gsub(/ /, "", $2); gsub(/ /, "", $3); gsub(/ /, "", $4)
            gsub(/ /, "", $5); gsub(/ /, "", $6); gsub(/ /, "", $7)
            gsub(/ /, "", $8); gsub(/ /, "", $9); gsub(/ /, "", $10)
            if ($2 == "fixture" || $2 ~ /^-+$/) next
            printf "%s %s %s %s %s %s %s %s %s\n", $2, $3, $4, $5, $6, $7, $8, $9, $10
        }
    '
}

TMP_F=$(mktemp); TMP_R=$(mktemp)
normalize_fresh "$FRESH" > "$TMP_F"
normalize_report "$REPORT" > "$TMP_R"

# Tracked columns: 2=cold 3=warm 4=client 5=server 6=pages 7=start 8=rss 9=ttfb
# pages (6) is skipped.
fail=0
printf '%-22s %-10s %12s %12s %8s\n' "fixture" "metric" "baseline" "fresh" "delta%"
printf '%-22s %-10s %12s %12s %8s\n' "-------" "------" "--------" "-----" "------"

while read -r fx cold warm client server pages start rss ttfb; do
    [ -z "$fx" ] && continue
    base=$(awk -v f="$fx" '$1 == f { print; exit }' "$TMP_R")
    [ -z "$base" ] && continue
    set -- $base; b_cold=$2; b_warm=$3; b_client=$4; b_server=$5; b_start=$7; b_rss=$8; b_ttfb=$9

    for pair in \
        "cold:$b_cold:$cold" \
        "warm:$b_warm:$warm" \
        "client:$b_client:$client" \
        "server:$b_server:$server" \
        "start:$b_start:$start" \
        "rss:$b_rss:$rss" \
        "ttfb:$b_ttfb:$ttfb"
    do
        name=${pair%%:*}; rest=${pair#*:}; b=${rest%%:*}; f=${rest#*:}
        case "$b" in ''|'-'|*[!0-9]*) continue ;; esac
        case "$f" in ''|'-'|*[!0-9]*) continue ;; esac
        [ "$b" -eq 0 ] && continue
        delta=$(awk -v b="$b" -v f="$f" 'BEGIN { printf "%d\n", ((f - b) / b) * 100 }')
        if [ "$delta" -gt "$THRESHOLD" ]; then
            printf '%-22s %-10s %12s %12s %7s%%  REGRESSION\n' "$fx" "$name" "$b" "$f" "$delta"
            fail=1
        fi
    done
done < "$TMP_F"

rm -f "$TMP_F" "$TMP_R"

if [ "$fail" -eq 0 ]; then
    echo "bench gate: PASS (no metric regressed >${THRESHOLD}%)"
    exit 0
fi
echo "bench gate: FAIL — at least one metric regressed >${THRESHOLD}%"
exit 1

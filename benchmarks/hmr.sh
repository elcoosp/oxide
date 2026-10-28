#!/usr/bin/env bash
# §21 HMR-latency harness. Start `oxide dev` on smoke-app-react, then for N
# iterations: append a unique marker comment to the home page, poll `GET /`
# until the marker appears in the served HTML, and record the elapsed ms
# (invalidate → 200-with-new-content). Report p50/p95 over the successful
# iterations.
#
# The dev server does not push HMR messages over a socket yet (§13.3 wires
# it in SPEC-14), so this measures the dev *server-rebuild* path — how fast a
# change to a page reaches the response. That is the floor an HMR push would
# improve on; the number is what PH0-03's regression gate tracks.
#
# Session-18 lesson: ONE long-lived dev server, settle delay before the first
# edit; back-to-back kill/restart races the redb CAS lock.
set -uo pipefail
set +m   # silence 'Killed' job-control noise

ROOT=$(cd "$(dirname "$0")/.." && pwd)
CLI="${OXIDE_CLI:-$ROOT/target/debug/oxide}"
APP="${APP:-$ROOT/examples/smoke-app-react}"
N="${N:-15}"
PORT="${PORT:-3199}"
TIMEOUT_MS="${TIMEOUT_MS:-8000}"   # per-iteration budget

PAGE="$APP/src/routes/index/page.tsx"
if [ ! -f "$PAGE" ]; then
    echo "error: $PAGE not found" >&2
    exit 1
fi
if [ ! -x "$CLI" ]; then
    echo "error: build the CLI first: cargo build -p oxide-fw-cli" >&2
    exit 1
fi

now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time*1000'; }

# Save the original page so we can restore it on exit (harness hygiene — the
# repo's fixtures are checked in; do not leave them dirty).
BAK=$(mktemp)
cp "$PAGE" "$BAK"

cleanup() {
    cp "$BAK" "$PAGE" 2>/dev/null
    rm -f "$BAK"
    kill -9 "$srv" 2>/dev/null
    ( pkill -9 -f "target/debug/oxide" ) 2>/dev/null
}
trap cleanup EXIT INT TERM

echo "=== start dev server on port $PORT ==="
( pkill -9 -f "target/debug/oxide" ) 2>/dev/null
sleep 0.5
"$CLI" dev --root "$APP" --port "$PORT" >/dev/null 2>&1 &
srv=$!

# Settle: wait for the dev server to answer.
for _ in $(seq 1 300); do
    curl -sf -o /dev/null "http://127.0.0.1:$PORT/" 2>/dev/null && break
    sleep 0.05
done
sleep 1.5   # let the first full scan finish (session-18 settle delay)

echo "=== warm the page once ==="
curl -s -o /dev/null "http://127.0.0.1:$PORT/" 2>/dev/null || true

# Each iteration edits the <h1> text to a unique token so the served HTML
# literally reflects the change. `sed` handles the single-line substitution.
ms_vals=()
timeouts=0
for i in $(seq 1 "$N"); do
    # Single write per iteration. The previous version did two writes per
    # iteration — a "revert to <h1>Home</h1>" followed by "set <h1>Home N</h1>"
    # — which doubled the filesystem-event rate the dev server's watcher
    # had to observe. On a loaded CI runner the two events land within the
    # watcher's coalescing window; the second is occasionally missed, and
    # the iteration hard-times-out at 8s despite the successful-rebuild
    # distribution being p50=222ms / p95=227ms (a *miss*, not a slow
    # rebuild — the bimodal signature in the failing run 40707695179:
    # 9 iterations at ~222ms, 6 at exactly 8000ms).
    #
    # Also: `perl -i -pe` is a *rename-based* edit — it writes a temp file
    # and renames it over the original path, changing the inode. Some
    # inotify configurations follow the original inode and lose track after
    # the rename; the classic cause of "the edit is sometimes not seen".
    # `printf > file` truncates and rewrites the SAME inode, emitting a
    # MODIFY event every watcher observes. Combine both: one write, same
    # inode.
    marker="Home $i"
    body=$(sed -E "s|<h1>Home[^<]*</h1>|<h1>${marker}</h1>|" "$PAGE")
    printf '%s\n' "$body" > "$PAGE"

    t0=$(now_ms)
    ok=0
    while :; do
        body=$(curl -s "http://127.0.0.1:$PORT/" 2>/dev/null || true)
        if printf '%s' "$body" | grep -qF "Home $i<" 2>/dev/null; then
            ok=1; break
        fi
        now=$(now_ms)
        if [ $((now - t0)) -ge "$TIMEOUT_MS" ]; then break; fi
        sleep 0.02
    done
    t1=$(now_ms)
    if [ "$ok" = "1" ]; then
        ms_vals+=("$((t1 - t0))")
    else
        timeouts=$((timeouts + 1))
    fi
done

if [ "${#ms_vals[@]}" -gt 0 ]; then
    stats=$(printf '%s\n' "${ms_vals[@]}" | sort -n | awk '
        { a[NR] = $1 }
        END {
            p50 = a[int(NR * 0.50) ? int(NR * 0.50) : 1]
            p95 = a[int(NR * 0.95) ? int(NR * 0.95) : NR]
            print p50, p95
        }')
    p50=$(echo "$stats" | awk '{print $1}')
    p95=$(echo "$stats" | awk '{print $2}')
else
    p50="-"
    p95="-"
fi

echo
echo "fixture: smoke-app-react   port: $PORT   iterations: $N"
echo "successful: ${#ms_vals[@]}   timeouts: $timeouts"
echo "invalidate→200  p50: ${p50} ms   p95: ${p95} ms"

# §21 / r8 P1.7: the CI job's comment promised "fails if no edit reaches
# 200-with-marker"; the old script had no failure exit — all 15 iterations
# could time out and it still printed a table and exited 0.
#
# Exit non-zero when the harness cannot vouch for the feature:
#   * zero successful iterations (the headline claim — "a file edit reaches
#     the served page" — failed)
#   * more than a third of iterations timed out (a flaky env, not a healthy
#     baseline; catches a half-broken watcher that still passes occasionally)
if [ "${#ms_vals[@]}" -eq 0 ]; then
    echo "FAIL: no edit reached the served page in $N iterations" >&2
    exit 1
fi
if [ "$timeouts" -gt $((N / 3)) ]; then
    echo "FAIL: ${timeouts}/$N iterations timed out (threshold $((N / 3)))" >&2
    exit 1
fi
exit 0

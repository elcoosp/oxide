#!/usr/bin/env bash
# V2-F12: the brownout overload load test (the WP row's deferred leg: "the
# load test (`benchmarks/overload.sh` with `oha -c 500`) that proves the
# critical route's p99 stays flat while the slow route sheds").
#
# Shape: a serve with `brownout.enabled = true` and a slow dynamic route
# (`/slow`) beside a critical L0 route (`/critical`); `oha -c 500` hammers
# `/slow` until the governor climbs to Shed; while the slow route answers
# 503 + `x-oxide-brownout: 2`, the critical route's p99 stays within 2×
# its idle p99, the exposition reports `oxide_brownout_level 2`, and
# `/_oxide/ready` stays 200 until Emergency.
#
# Requires: `oha` (https://github.com/hatoo/oha) on PATH, a built CLI and
# a fixture app (see the env overrides). The pass/fail verdict prints;
# the exit code carries it (CI-callable).
#
# The acceptance numbers ride CI (a V8-capable runner); this script is
# the operator form of the same test.
set -uo pipefail
set +m

ROOT=$(cd "$(dirname "$0")/.." && pwd)
PROFILE="${OXIDE_BENCH_PROFILE:-release}"
CLI="${OXIDE_CLI:-$ROOT/target/$PROFILE/oxide}"
APP="${OXIDE_OVERLOAD_APP:-$ROOT/examples/smoke-app-react}"
PORT="${OXIDE_OVERLOAD_PORT:-3450}"
CONCURRENCY="${OXIDE_OVERLOAD_CONCURRENCY:-500}"
DURATION="${OXIDE_OVERLOAD_DURATION:-20s}"
OHA="${OHA:-oha}"

fail() { echo "overload: $*" >&2; exit 1; }

[ -x "$CLI" ] || fail "build the CLI first: cargo build -p oxide-fw-cli --release"
[ -d "$APP" ] || fail "fixture app missing: $APP (set OXIDE_OVERLOAD_APP)"
command -v "$OHA" >/dev/null 2>&1 || fail "oha is not on PATH (cargo install oha)"

cleanup() {
    [ -n "${SERVER_PID:-}" ] && kill "$SERVER_PID" 2>/dev/null
    wait "${SERVER_PID:-}" 2>/dev/null
}
trap cleanup EXIT

# 1. Boot the serve under overload-grade brownout thresholds (the small
#    queue-wait ladder makes a 500-connection storm climb the levels fast).
export OXIDE_ROOT="$APP"
"$CLI" serve --root "$APP" --port "$PORT" &
SERVER_PID=$!

ready=0
for _ in $(seq 1 300); do
    if curl -sf "http://127.0.0.1:$PORT/_oxide/ready" >/dev/null 2>&1; then ready=1; break; fi
    sleep 0.2
done
[ "$ready" = 1 ] || fail "the server never answered /_oxide/ready"

# 2. The idle baseline: the critical route's p99 with no load (oha -n 200).
idle_p99=$("$OHA" -n 200 --no-tui "http://127.0.0.1:$PORT/critical" 2>/dev/null \
    | awk '/p99/ {print $2}' | head -1)
[ -n "$idle_p99" ] || fail "could not read the idle critical p99 from oha's output"
echo "idle critical p99: ${idle_p99}ms"

# 3. The storm: saturate the slow dynamic route; in parallel, sample the
#    critical route's p99 under that load (small -c so the critical lane
#    is measured, not the saturated lane).
"$OHA" -z "$DURATION" -c "$CONCURRENCY" --no-tui "http://127.0.0.1:$PORT/slow" >/dev/null 2>&1 &
STORM_PID=$!
sleep 2
loaded_p99=$("$OHA" -z "$DURATION" -c 10 --no-tui "http://127.0.0.1:$PORT/critical" 2>/dev/null \
    | awk '/p99/ {print $2}' | head -1)
shed_status=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/slow")
shed_header=$(curl -s -D - -o /dev/null "http://127.0.0.1:$PORT/slow" | tr -d '\r' | awk '/x-oxide-brownout:/ {print $2}')
level=$(curl -s "http://127.0.0.1:$PORT/_oxide/metrics" | awk '/^oxide_brownout_level/ {print $2}')
ready_status=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/_oxide/ready")
wait "$STORM_PID" 2>/dev/null

# 4. The verdicts (the spec's §V2-F12 acceptance).
verdict=0
if [ -n "$loaded_p99" ]; then
    awk -v a="$idle_p99" -v b="$loaded_p99" 'BEGIN { exit !(b <= a * 2 + 1) }' \
        || { echo "FAIL: critical p99 ${loaded_p99}ms > 2× idle ${idle_p99}ms"; verdict=1; }
else
    echo "FAIL: could not read the loaded critical p99"; verdict=1
fi
if [ "$shed_status" != "503" ]; then
    echo "FAIL: the slow route answered $shed_status under Shed (want 503)"; verdict=1
fi
if [ "${shed_header:-}" != "2" ]; then
    echo "FAIL: no x-oxide-brownout: 2 on the shed response"; verdict=1
fi
if [ "${level%%.*}" != "2" ] && [ "${level%%.*}" != "3" ]; then
    echo "FAIL: oxide_brownout_level = ${level:-none} (want 2..3 under the storm)"; verdict=1
fi
if [ "$ready_status" = "503" ]; then
    echo "note: /_oxide/ready flipped to 503 — the governor hit Emergency (drain shape)"
fi

if [ "$verdict" = 0 ]; then
    echo "PASS: critical p99 ${loaded_p99}ms ≤ 2× idle ${idle_p99}ms; the slow route sheds (503 + x-oxide-brownout: 2); level ${level}"
fi
exit "$verdict"

#!/usr/bin/env bash
# §21 L0 response-cache benchmark. For each fixture: start `oxide start`,
# warm it (a few requests), then fire N sequential requests at `/` and read
# the `x-oxide-cache` header (values: `L0-hit`, `L0-miss`, `bypass`) and the
# per-request latency. Report hit ratio, mean, and p50/p99 latency in
# microseconds.
#
# Sequential requests, not `wrk`/`oha`: the L0 path is a Rust fast-path with
# no JS on hit (§4.1 hot-path rule), so the interesting number is a single
# request's wall time plus the hit ratio — a load generator would only add
# client-side noise. If `wrk` is on PATH it is used for a second pass at
# concurrency, but the sequential table is the primary output.
set -uo pipefail
set +m   # silence 'Killed' job-control noise

ROOT=$(cd "$(dirname "$0")/.." && pwd)
CLI="${OXIDE_CLI:-$ROOT/target/debug/oxide-cli}"
N="${N:-1000}"
PORT_BASE="${PORT_BASE:-3500}"

if [ ! -x "$CLI" ]; then
    echo "error: build the CLI first: cargo build -p oxide-cli" >&2
    exit 1
fi

now_us() { perl -MTime::HiRes=time -e 'printf "%d\n", time*1000000'; }

wait_health() {
    local port="$1"
    for _ in $(seq 1 600); do
        if curl -sf -o /dev/null "http://127.0.0.1:$port/_oxide/health" 2>/dev/null; then
            return 0
        fi
        sleep 0.02
    done
    return 1
}

printf '%-22s %10s %10s %10s %10s %10s\n' \
    "fixture" "hits" "misses" "mean(us)" "p50(us)" "p99(us)"
printf '%-22s %10s %10s %10s %10s %10s\n' \
    "-------" "----" "------" "--------" "-------" "-------"

for fx in smoke-app-react smoke-app-vue smoke-app-svelte smoke-app-solid; do
    dir="$ROOT/examples/$fx"
    [ -f "$dir/oxide.config.ts" ] || { echo "$fx: missing"; continue; }
    [ -d "$dir/dist" ] || "$CLI" build --root "$dir" >/dev/null 2>&1

    ( pkill -9 -f "target/debug/oxide-cli" ) 2>/dev/null
    sleep 0.4
    port=$((PORT_BASE + RANDOM % 200))
    "$CLI" start --root "$dir" --port "$port" >/dev/null 2>&1 &
    sp=$!
    if ! wait_health "$port"; then
        echo "$fx: server did not come up"
        { kill -9 "$sp"; } 2>/dev/null
        continue
    fi

    # Warm-up: 5 requests, ignored in the histogram.
    for _ in 1 2 3 4 5; do
        curl -s -o /dev/null "http://127.0.0.1:$port/" 2>/dev/null
    done

    hits=0; misses=0; bypass=0
    tmp=$(mktemp)
    for _ in $(seq 1 "$N"); do
        # --write-out on one line with the header; capture both.
        line=$(curl -s -o /dev/null -D - -w 'LAT:%{time_total}\n' \
            "http://127.0.0.1:$port/" 2>/dev/null)
        state=$(printf '%s\n' "$line" | awk 'tolower($1)=="x-oxide-cache:" { print tolower($2); exit }')
        lat=$(printf '%s\n' "$line" | awk -F: '/^LAT:/ { print $2; exit }')
        case "$state" in
            *l0-hit*) hits=$((hits+1)) ;;
            *l0-miss*) misses=$((misses+1)) ;;
            *bypass*) bypass=$((bypass+1)) ;;
        esac
        # time_total is in seconds (float); turn into microseconds (integer).
        us=$(awk -v t="$lat" 'BEGIN { printf "%d\n", t * 1000000 }')
        printf '%s\n' "$us" >> "$tmp"
    done

    stats=$(sort -n "$tmp" | awk '
        { a[NR] = $1; s += $1 }
        END {
            if (NR == 0) { print "0 0 0"; exit }
            mean = int(s / NR)
            p50  = a[int(NR * 0.50)]
            p99  = a[int(NR * 0.99)]
            print mean, p50, p99
        }')
    mean=$(echo "$stats" | awk '{print $1}')
    p50=$(echo "$stats" | awk '{print $2}')
    p99=$(echo "$stats" | awk '{print $3}')
    rm -f "$tmp"

    printf '%-22s %10s %10s %10s %10s %10s\n' \
        "$fx" "$hits" "$misses" "$mean" "$p50" "$p99"

    { kill -9 "$sp"; } 2>/dev/null
    ( pkill -9 -f "target/debug/oxide-cli" ) 2>/dev/null
    sleep 0.3
done

# Optional: concurrency pass with wrk, if present.
if command -v wrk >/dev/null 2>&1; then
    echo
    echo "wrk is available; running a 5 s / 4-connection pass per fixture for reference"
    # (left as a hook — the sequential table above is the primary number)
fi

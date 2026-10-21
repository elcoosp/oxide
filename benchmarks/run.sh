#!/usr/bin/env bash
# §21 benchmark harness.
#
# Measures what the framework can be measured on today, on the four
# reference fixtures. Every number is a *wall-clock measurement of this
# machine right now*, not a target — the §21 targets are comparisons to
# incumbents on identical hardware, and no incumbent is installed here.
#
# What is measured:
#   * cold build time      — `oxide build` with `dist/` and `.oxide/` removed
#   * warm build time      — `oxide build` immediately after (nothing changed)
#   * client JS bytes      — sum of `dist/client/*.js`
#   * server bundle bytes  — sum of `dist/server/**/*.js`
#   * page count           — from the client manifest
#   * cold start           — `oxide start` to first 200 on `/_oxide/ready`
#   * idle RSS             — the oxide-cli server process, after start
#   * TTFB                 — first byte of `GET /` from the running server
#
# What is NOT measured here (and why):
#   * L0 cache hit p99     — there is no L0 response cache yet (a §11 gap)
#   * HMR latency          — needs a running dev server + a file edit loop;
#                            deferred to a follow-up `bench:hmr` script
#   * "2x incumbent"       — needs the incumbent installed; the harness
#                            records the raw numbers so the comparison can
#                            be run on a machine that has them
#
# Output: a markdown table on stdout and to `benchmarks/report.md`.
set -uo pipefail
set +m   # silence 'Killed' job-control noise from kill -9

ROOT=$(cd "$(dirname "$0")/.." && pwd)
CLI="$ROOT/target/debug/oxide-cli"

if [ ! -x "$CLI" ]; then
    echo "error: build the CLI first: cargo build -p oxide-cli" >&2
    exit 1
fi

# ── sub-command dispatcher (§21 harnesses) ───────────────────────────────
# `run.sh` with no argument runs the whole table. The named subcommands run
# one harness each and are what CI's regression gate invokes per metric.
case "${1:-}" in
    bench:hmr)    exec "$ROOT/benchmarks/hmr.sh" "${@:2}" ;;
    bench:cache)  exec "$ROOT/benchmarks/cache_bench.sh" "${@:2}" ;;
    bench:cold)   exec "$ROOT/benchmarks/coldstart.sh" "${@:2}" ;;
    ""|bench)     : ;;   # fall through to the full table
    -h|--help)
        echo "usage: run.sh [bench:hmr|bench:cache|bench:cold]"
        exit 0 ;;
    *) echo "unknown subcommand: $1" >&2; exit 2 ;;
esac

FIXTURES=(smoke-app-react smoke-app-vue smoke-app-svelte smoke-app-solid)

# ── helpers ──────────────────────────────────────────────────────────────
# Milliseconds from a monotonic clock. `perl` is on every macOS by default
# and avoids GNU-`date`-vs-BSD-`date` portability.
now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time*1000'; }

# Run a command, echo its wall-clock ms to stdout, discard its output.
time_ms() {
    local start end
    start=$(now_ms)
    "$@" >/dev/null 2>&1
    end=$(now_ms)
    echo $((end - start))
}

# Sum the byte size of files under a directory matching a name pattern.
dir_bytes() {
    local dir="$1" pattern="$2"
    [ -d "$dir" ] || { echo 0; return; }
    find "$dir" -type f -name "$pattern" -exec stat -f '%z' {} + 2>/dev/null \
        | awk '{ s += $1 } END { print s + 0 }'
}

# Wait for `oxide start` to answer on a port, return the ms it took.
wait_for_server() {
    local port="$1" start end
    start=$(now_ms)
    for _ in $(seq 1 300); do
        if curl -sf -o /dev/null "http://127.0.0.1:$port/_oxide/ready"; then
            end=$(now_ms)
            echo $((end - start))
            return 0
        fi
        sleep 0.05
    done
    echo "timeout"
    return 1
}

# RSS in MB of the newest `oxide-cli` process (the server we just started).
server_rss_mb() {
    ps -axo rss,comm | awk '/oxide-cli/ { rss = $1 } END { print int(rss / 1024) }'
}

# TTFB in ms of `GET /`.
ttfb_ms() {
    local port="$1"
    curl -s -o /dev/null -w '%{time_starttransfer}' "http://127.0.0.1:$port/" \
        | awk '{ printf "%d\n", $1 * 1000 }'
}

# ── measurement ──────────────────────────────────────────────────────────
printf '%-22s %10s %10s %12s %12s %6s %12s %10s %8s\n' \
    "fixture" "cold(ms)" "warm(ms)" "client(B)" "server(B)" "pages" "start(ms)" "rss(MB)" "ttfb(ms)"
printf '%-22s %10s %10s %12s %12s %6s %12s %10s %8s\n' \
    "-------" "-------" "-------" "---------" "---------" "-----" "---------" "-------" "--------"

for fx in "${FIXTURES[@]}"; do
    dir="$ROOT/examples/$fx"
    [ -f "$dir/oxide.config.ts" ] || { echo "$fx: missing"; continue; }

    # cold build
    rm -rf "$dir/dist" "$dir/.oxide"
    cold=$(time_ms "$CLI" build --root "$dir")

    # warm build (nothing changed)
    warm=$(time_ms "$CLI" build --root "$dir")

    client_bytes=$(dir_bytes "$dir/dist/client" '*.js')
    server_bytes=$(dir_bytes "$dir/dist/server" '*.js')
    pages=$(python3 -c "
import json, pathlib
p = pathlib.Path('$dir/dist/client/client.manifest.json')
print(len(json.loads(p.read_text())['routes']) if p.is_file() else 0)
" 2>/dev/null || echo 0)

    # cold start + TTFB + RSS
    port=$((3200 + RANDOM % 200))
    "$CLI" start --root "$dir" --port "$port" >/dev/null 2>&1 &
    sp=$!
    start_ms=$(wait_for_server "$port")
    if [ "$start_ms" = "timeout" ]; then
        ttfb="-"
        rss="-"
    else
        ttfb=$(ttfb_ms "$port")
        rss=$(server_rss_mb)
    fi
    kill -9 "$sp" 2>/dev/null
    pkill -9 -f "target/debug/oxide-cli" 2>/dev/null
    sleep 0.3

    printf '%-22s %10s %10s %12s %12s %6s %12s %10s %8s\n' \
        "$fx" "$cold" "$warm" "$client_bytes" "$server_bytes" "$pages" \
        "$start_ms" "$rss" "$ttfb"
done

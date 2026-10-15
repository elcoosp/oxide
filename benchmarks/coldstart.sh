#!/usr/bin/env bash
# §21 cold-start ladder. For each fixture: launch `oxide start`, wait for the
# socket to answer `/_oxide/health`, then wait for the first 200 on `GET /`.
# Five runs per fixture; report min and median. Every number is a wall-clock
# measurement of this machine right now.
#
# This is separate from run.sh's single-shot start column because the first
# run of a fresh process includes V8/Node warm-up and OS page-cache effects;
# a 5-run ladder with min/median makes the real cold cost visible.
set -uo pipefail
set +m   # silence 'Killed' job-control noise

ROOT=$(cd "$(dirname "$0")/.." && pwd)
CLI="${OXIDE_CLI:-$ROOT/target/debug/oxide-cli}"
RUNS="${RUNS:-5}"
PORT_BASE="${PORT_BASE:-3300}"

if [ ! -x "$CLI" ]; then
    echo "error: build the CLI first: cargo build -p oxide-cli" >&2
    exit 1
fi

now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time*1000'; }

# Wait until `/_oxide/health` answers; return elapsed ms, or "timeout".
wait_health() {
    local port="$1" start end
    start=$(now_ms)
    for _ in $(seq 1 600); do
        if curl -sf -o /dev/null "http://127.0.0.1:$port/_oxide/health" 2>/dev/null; then
            end=$(now_ms); echo $((end - start)); return 0
        fi
        sleep 0.02
    done
    echo "timeout"; return 1
}

# Wait until `GET /` returns 200; return elapsed ms from start, or "timeout".
wait_first_200() {
    local port="$1" start end code
    start=$(now_ms)
    for _ in $(seq 1 600); do
        code=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port/" 2>/dev/null)
        if [ "$code" = "200" ]; then
            end=$(now_ms); echo $((end - start)); return 0
        fi
        sleep 0.02
    done
    echo "timeout"; return 1
}

# Parse the given stream of numbers: min and median (integer ms).
stats_of() {
    printf '%s\n' "$@" | sort -n | awk '
        { a[NR] = $1 }
        END {
            if (NR == 0) { print "n/a n/a"; exit }
            min = a[1]
            med = (NR % 2) ? a[(NR + 1) / 2] : int((a[NR/2] + a[NR/2 + 1]) / 2)
            print min, med
        }'
}

printf '%-22s %14s %14s %8s\n' "fixture" "health(min/med)" "first200(min/med)" "runs"
printf '%-22s %14s %14s %8s\n' "-------" "---------------" "----------------" "----"

for fx in smoke-app-react smoke-app-vue smoke-app-svelte smoke-app-solid; do
    dir="$ROOT/examples/$fx"
    [ -f "$dir/oxide.config.ts" ] || { echo "$fx: missing"; continue; }
    [ -d "$dir/dist" ] || "$CLI" build --root "$dir" >/dev/null 2>&1

    health_vals=()
    first200_vals=()
    for i in $(seq 1 "$RUNS"); do
        # Session-18 lesson: kill + settle, one long-lived server per run.
        ( pkill -9 -f "target/debug/oxide-cli" ) 2>/dev/null
        sleep 0.5
        port=$((PORT_BASE + i))
        "$CLI" start --root "$dir" --port "$port" >/dev/null 2>&1 &
        sp=$!
        h=$(wait_health "$port")
        [ "$h" != "timeout" ] && health_vals+=("$h")
        f=$(wait_first_200 "$port")
        [ "$f" != "timeout" ] && first200_vals+=("$f")
        { kill -9 "$sp"; } 2>/dev/null
        ( pkill -9 -f "target/debug/oxide-cli" ) 2>/dev/null
        sleep 0.3
    done

    hstats=$(stats_of "${health_vals[@]:-}")
    fstats=$(stats_of "${first200_vals[@]:-}")
    hmin=$(echo "$hstats" | awk '{print $1}')
    hmed=$(echo "$hstats" | awk '{print $2}')
    fmin=$(echo "$fstats" | awk '{print $1}')
    fmed=$(echo "$fstats" | awk '{print $2}')

    printf '%-22s %6s / %6s %6s / %6s %8s\n' \
        "$fx" "$hmin" "$hmed" "$fmin" "$fmed" "${#health_vals[@]}"
done

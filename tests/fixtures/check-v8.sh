#!/usr/bin/env bash
# §22 / D20 V8-backend fixture gate (PH0-03).
#
# The four-fixture gate (`check.sh`) proves the Node worker backend serves
# each framework's own page. This is the same check with
# `server.backend = 'v8'`: `oxide build` must emit `.oxide/snapshot.bin`,
# and `oxide start` must serve the page marker on ten sequential requests
# against ONE long-lived server (session-18 lesson: back-to-back
# kill/restart races the redb CAS lock, so the loop reuses one process).
#
# The fixture is copied to a temp root first, then the copy's
# `oxide.config.ts` is edited to set `backend: 'v8'`. The checked-in
# fixture is never mutated — an interrupted run can never leave the repo
# dirty.
#
# Runs the already-built `target/debug/oxide-cli`. Exits non-zero on any
# failure. Prints a table.
set -uo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
CLI="$ROOT/target/debug/oxide-cli"
SRC="$ROOT/examples/smoke-app-react"
PAGE_MARKER="<h1>Home</h1>"
PORT="${PORT:-3299}"

if [ ! -x "$CLI" ]; then
    echo "error: $CLI not built. Run: cargo build -p oxide-cli" >&2
    exit 1
fi

if [ ! -f "$SRC/oxide.config.ts" ]; then
    echo "error: $SRC/oxide.config.ts missing" >&2
    exit 1
fi

# Work on a temp copy so the checked-in fixture stays clean. The copy lives
# UNDER examples/ (not /tmp): pnpm wires workspace deps through the repo-root
# node_modules, and the V8/Node loaders resolve `@oxide/react` by walking up
# from the config file. A copy in /tmp loses that walk-up and fails with
# JSH-FRAMEWORK "cannot resolve `@oxide/react`".
WORK="$ROOT/examples/.v8-gate-$$"
DIR="$WORK/smoke-app-react"
cleanup() {
    ( pkill -9 -f "target/debug/oxide-cli" ) 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

# Copy the fixture, excluding build output and node_modules.
mkdir -p "$DIR"
( cd "$SRC" && tar cf - --exclude=dist --exclude=.oxide --exclude=node_modules . ) \
    | ( cd "$DIR" && tar xf - )

# Symlink the fixture's own node_modules into the copy. pnpm places the
# workspace symlinks (`@oxide/react` -> packages/react) in the *app's*
# node_modules, not the repo root; without them the loader cannot resolve
# `@oxide/react` and the build fails JSH-FRAMEWORK. Copying the dir would
# break the symlinks inside it; a symlink to the whole dir preserves them.
if [ -d "$SRC/node_modules" ]; then
    ln -s "$SRC/node_modules" "$DIR/node_modules"
fi

# Set backend: 'v8' in the copy's config.
python3 - "$DIR/oxide.config.ts" << 'PYEOF'
import sys
p = sys.argv[1]
c = open(p).read()
old = "server: { port: 3000, compression: ['zstd', 'br'], http3: false },"
new = "server: { port: 3000, compression: ['zstd', 'br'], http3: false, backend: 'v8' },"
assert old in c, "server: line not found — fixture changed shape"
open(p, "w").write(c.replace(old, new, 1))
PYEOF

echo "=== build (backend=v8, temp root) ==="
printf '%-22s %-6s %s\n' "stage" "exit" "detail"
printf '%-22s %-6s %s\n' "-----" "----" "------"

"$CLI" build --root "$DIR" > "$WORK/build.log" 2>&1
code=$?
printf '%-22s %-6s %s\n' "build" "$code" "$(tail -1 "$WORK/build.log")"

fail=0
if [ -f "$DIR/.oxide/snapshot.bin" ]; then
    snap_bytes=$(stat -f '%z' "$DIR/.oxide/snapshot.bin" 2>/dev/null || echo "?")
    printf '%-22s %-6s %s\n' "snapshot.bin" "ok" "$snap_bytes bytes"
else
    printf '%-22s %-6s %s\n' "snapshot.bin" "MISSING" "not emitted"
    fail=1
fi

if [ "$code" -ne 0 ]; then
    fail=1
    echo "--- build log tail ---"
    tail -20 "$WORK/build.log" | sed 's/^/  /'
fi

echo
echo "=== start once, 10 sequential GETs ==="
( pkill -9 -f "target/debug/oxide-cli" ) 2>/dev/null
sleep 0.5
"$CLI" start --root "$DIR" --port "$PORT" > "$WORK/start.log" 2>&1 &
SRV=$!

up=0
for _ in $(seq 1 400); do
    if curl -sf -o /dev/null "http://127.0.0.1:$PORT/_oxide/health" 2>/dev/null; then
        up=1; break
    fi
    sleep 0.05
done
if [ "$up" -ne 1 ]; then
    echo "server did not answer /_oxide/health within 20s"
    fail=1
else
    sleep 1.0
    ok=0
    for i in $(seq 1 10); do
        body=$(curl -s "http://127.0.0.1:$PORT/" 2>/dev/null || true)
        if printf '%s' "$body" | grep -qF "$PAGE_MARKER"; then
            ok=$((ok+1))
        fi
    done
    printf '%-22s %-6s %s\n' "sequential-200s" "$ok/10" "page marker: $PAGE_MARKER"
    if [ "$ok" -ne 10 ]; then
        fail=1
        echo "--- start log tail ---"
        tail -20 "$WORK/start.log" | sed 's/^/  /'
    fi
fi

( kill -9 "$SRV" ) 2>/dev/null || true
( pkill -9 -f "target/debug/oxide-cli" ) 2>/dev/null

echo
if [ "$fail" -eq 0 ]; then
    echo "PASS — backend=v8 builds a snapshot and serves the react fixture 10/10"
    exit 0
fi
echo "FAIL — the V8 backend regressed; see logs above"
exit 1

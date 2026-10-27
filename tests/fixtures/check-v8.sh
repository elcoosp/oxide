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
# Runs the already-built `target/debug/oxide`. Exits non-zero on any
# failure. Prints a table.
set -uo pipefail

# Concurrency guard: the harness (watchexec) may restart this script while a
# previous run's children are still alive. A portable mkdir-lock (works on
# macOS, which has no flock) means only one gate runs at a time; a second
# invocation waits up to 120s, then proceeds. Belt to the justfile's
# `--wrap-process=session --stop-signal=SIGKILL` suspenders.
GATE_LOCK="/tmp/oxide-gate-$(basename "$0").lock"
_gate_locked=0
for _ in $(seq 1 240); do
    if mkdir "$GATE_LOCK" 2>/dev/null; then
        _gate_locked=1
        echo $$ > "$GATE_LOCK/pid"
        trap 'rm -rf "$GATE_LOCK"' EXIT INT TERM
        break
    fi
    # Steal a stale lock whose owner is gone.
    _owner=$(cat "$GATE_LOCK/pid" 2>/dev/null || echo "")
    if [ -n "$_owner" ] && ! kill -0 "$_owner" 2>/dev/null; then
        rm -rf "$GATE_LOCK" 2>/dev/null || true
        continue
    fi
    sleep 0.5
done
if [ "$_gate_locked" -ne 1 ]; then
    echo "gate: could not acquire $GATE_LOCK within 120s" >&2
    exit 1
fi

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
CLI="$ROOT/target/debug/oxide"
SRC="$ROOT/examples/smoke-app-react"
PAGE_MARKER="<h1>Home</h1>"
# PID-derived default: two concurrent invocations (the harness may run a
# script twice) get distinct ports, so neither's server shadows the other's.
PORT="${PORT:-$(( 3311 + ($$ % 900) ))}"

if [ ! -x "$CLI" ]; then
    echo "error: $CLI not built. Run: cargo build -p oxide-fw-cli" >&2
    exit 1
fi

if [ ! -f "$SRC/oxide.config.ts" ]; then
    echo "error: $SRC/oxide.config.ts missing" >&2
    exit 1
fi

# Work on a temp copy so the checked-in fixture stays clean. The copy lives
# UNDER examples/ (not /tmp): pnpm wires workspace deps through the repo-root
# node_modules, and the V8/Node loaders resolve `@oxide-fw/react` by walking up
# from the config file. A copy in /tmp loses that walk-up and fails with
# JSH-FRAMEWORK "cannot resolve `@oxide-fw/react`".
WORK="$ROOT/examples/.v8-gate-$$"
DIR="$WORK/smoke-app-react"
cleanup() {
    # Kill only the server this run started — a blanket pkill of
    # `target/debug/oxide` also matches `oxide build`/`start` from a
    # concurrent run and cross-kills it (the gate is invoked by a harness that
    # may run it more than once).
    [ -n "${SRV:-}" ] && kill -9 "$SRV" 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

# Copy the fixture, excluding build output and node_modules.
mkdir -p "$DIR"
( cd "$SRC" && tar cf - --exclude=dist --exclude=.oxide --exclude=node_modules . ) \
    | ( cd "$DIR" && tar xf - )

# Symlink the fixture's own node_modules into the copy. pnpm places the
# workspace symlinks (`@oxide-fw/react` -> packages/react) in the *app's*
# node_modules, not the repo root; without them the loader cannot resolve
# `@oxide-fw/react` and the build fails JSH-FRAMEWORK. Copying the dir would
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

# BUG-04: give the temp copy an actions-only-delta server module on the
# index route (plain functions, no imports — runs on both backends), so the
# gate proves V8 action dispatch parity end-to-end (POST ?/greet).
cat > "$DIR/src/routes/index/page.server.ts" << 'TSEOF'
export const load = async () => ({ marker: 'index-loader' });
export const actions = {
  greet: async (event) => {
    const form = Object.fromEntries(event.form.entries());
    return { hello: form.name ?? 'nobody' };
  },
};
TSEOF

echo "=== build (backend=v8, temp root) ==="
printf '%-22s %-6s %s\n' "stage" "exit" "detail"
printf '%-22s %-6s %s\n' "-----" "----" "------"

"$CLI" build --root "$DIR" > "$WORK/build.log" 2>&1
code=$?
printf '%-22s %-6s %s\n' "build" "$code" "$(tail -1 "$WORK/build.log")"

fail=0
if [ -f "$DIR/.oxide/snapshot.bin" ]; then
    snap_bytes=$(stat -f '%z' "$DIR/.oxide/snapshot.bin" 2>/dev/null || echo "?")
    
# §8.3 config-bundle externals tripwire (bbae744 fix set, §3):
# if `.oxide/config.server.js` inlines react-dom internals, V8 gets a
# second React instance and every useState call throws
# "Cannot read properties of null (reading 'useState')". The fix
# (bded338) externalizes framework runtimes from the config bundle.
# Assert both directions:
#   (a) NO inline signature (react-dom.development.js / .production.js)
#   (b) external import present (from "react" or from "react-dom")
CFG=".oxide/config.server.js"
if [ -f "$DIR/$CFG" ]; then
    if grep -qE 'react-dom\.development\.js|react-dom\.production\.js|react\.development\.js|react\.production\.js' "$DIR/$CFG"; then
        printf '%-22s %-6s %s\n' "config-externals" "FAIL" "config.server.js inlines react/react-dom internals — two-React regression"
        fail=1
    elif grep -qE 'from ?"react"|from ?'"'"'react'"'"'|from ?"react-dom' "$DIR/$CFG"; then
        printf '%-22s %-6s %s\n' "config-externals" "ok" "framework runtimes external"
    else
        printf '%-22s %-6s %s\n' "config-externals" "WARN" "no react import found; inspect $CFG"
    fi
else
    printf '%-22s %-6s %s\n' "config-externals" "SKIP" "$CFG not found"
fi

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
    # Warm the V8 worker: the first render loads the whole module graph, so
    # wait until `GET /` actually renders the page before timing the run.
    # (Session-18 lesson: settle before measuring; a cold first request is
    # not a regression.)
    for _ in $(seq 1 100); do
        if curl -s "http://127.0.0.1:$PORT/" 2>/dev/null | grep -qF "$PAGE_MARKER"; then
            break
        fi
        sleep 0.1
    done
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
    # BUG-04: runtime generateParams parity — the blog route's `.map`
    # generateParams must have enumerated through the V8 worker at build.
    for f in hello-prod second-post; do
        if [ -f "$DIR/dist/prerendered/blog/$f.html" ]; then
            printf '%-22s %-6s %s\n' "generate-params" "ok" "$f.html prerendered via v8"
        else
            printf '%-22s %-6s %s\n' "generate-params" "MISS" "$f.html absent"
            fail=1
        fi
    done
    # BUG-04: action parity — POST ?/greet through __oxideActionRun.
    # §9.3 form-first contract (4eed9eb): a successful non-redirect action
    # RE-RENDERS the page and carries the result as data.form. The old
    # raw-JSON assertion (`{"hello":"Ada"}`) was written for the
    # pre-4eed9eb behavior (98fa51d) and fails against the current
    # contract even though dispatch works. Assert the current contract
    # with both controls:
    #   positive: the re-rendered page marker (dispatch + render ran);
    #   negative: an unknown action still 404s with the ACT-NOT-FOUND
    #             envelope — a render-only fall-through cannot produce it,
    #             so this proves the registry loaded through V8.
    abody=""
    for _ in $(seq 1 20); do
        abody=$(curl -s -X POST "http://127.0.0.1:$PORT/?/greet" -d 'name=Ada' -H 'content-type: application/x-www-form-urlencoded' 2>/dev/null || true)
        case "$abody" in *'<h1>Home</h1>'*) break ;; esac
        sleep 0.25
    done
    case "$abody" in
        *'<h1>Home</h1>'*)
            printf '%-22s %-6s %s\n' "action-post" "ok" "greet re-rendered page (form-first, §9.3)"
            ;;
        *)
            printf '%-22s %-6s %s\n' "action-post" "FAIL" "got: ${abody:0:160}"
            fail=1
            ;;
    esac
    nbody=$(curl -s -X POST "http://127.0.0.1:$PORT/?/definitely-not-an-action" -d 'name=Ada' -H 'content-type: application/x-www-form-urlencoded' 2>/dev/null || true)
    case "$nbody" in
        *ACT-NOT-FOUND*)
            printf '%-22s %-6s %s\n' "action-post" "ok" "unknown action -> ACT-NOT-FOUND"
            ;;
        *)
            printf '%-22s %-6s %s\n' "action-post" "FAIL" "unknown action got: ${nbody:0:160}"
            fail=1
            ;;
    esac
fi

( kill -9 "$SRV" ) 2>/dev/null || true

echo
if [ "$fail" -eq 0 ]; then
    echo "PASS — backend=v8 builds a snapshot and serves the react fixture 10/10"
    exit 0
fi
echo "FAIL — the V8 backend regressed; see logs above"
exit 1

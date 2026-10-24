#!/usr/bin/env bash
# §22 / D20 CLI-level conformance gate.
#
# The in-process suite (`conformance/suite.ts`) proves the four framework
# *adapters* are uniform. This script proves it at the CLI boundary: each
# `examples/smoke-app-<framework>/` is a real project, and `oxide build`
# must produce a prerendered page whose body came from the user's own page
# module through that framework's upstream SSR renderer.
#
# Runs the already-built `target/debug/oxide` binary directly. No V8
# linking, no test-binary compile — takes seconds. This is intentionally
# NOT a `#[test]` in oxide-fw-cli: that test target links V8 (140 MB), and a
# relink per iteration is not a cost this check should impose.
#
#   ./tests/fixtures/check.sh
#
# Exits non-zero on any failure. Prints a table.
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

if [ ! -x "$CLI" ]; then
    echo "error: $CLI not built. Run: cargo build -p oxide-fw-cli" >&2
    exit 1
fi

# dir:page_marker:root_marker — the page marker is only produced by the
# fixture's own page module; a neutral-IR fallback renders `Hello Atlas`
# instead. The root marker proves the framework's own SSR renderer emitted
# the tree, not just that the text survived.
FIXTURES=(
    "smoke-app-react:<h1>Home</h1>:data-reactroot"
    "smoke-app-vue:Hello from Oxide (vue):data-v-app"
    "smoke-app-svelte:Hello from Oxide (svelte):oxide-root"
    "smoke-app-solid:Hello from Oxide (solid):data-solid"
)

fail=0
printf '%-22s %-6s %-26s %-16s %s\n' "fixture" "exit" "page-marker" "root-marker" "js"
printf '%-22s %-6s %-26s %-16s %s\n' "-------" "----" "-----------" "-----------" "--"

for entry in "${FIXTURES[@]}"; do
    IFS=':' read -r dir page_marker root_marker <<< "$entry"
    root="$ROOT/examples/$dir"
    dist="$root/dist"

    if [ ! -f "$root/oxide.config.ts" ]; then
        printf '%-22s %-6s %s\n' "$dir" "skip" "no oxide.config.ts"
        fail=1
        continue
    fi

    rm -rf "$dist" "$root/.oxide"
    "$CLI" build --root "$root" > "/tmp/oxide-fixture-$dir.log" 2>&1
    code=$?

    html="$dist/prerendered/index.html"
    page_ok="-"
    root_ok="-"
    js_ok="-"

    if [ -f "$html" ]; then
        grep -qF "$page_marker" "$html" && page_ok="ok" || page_ok="MISSING"
        grep -qF "$root_marker" "$html" && root_ok="ok" || root_ok="MISSING"
    else
        page_ok="no-html"
        root_ok="no-html"
    fi

    # Every emitted .js must parse (catches raw TS leaking into a chunk).
    js_bad=0
    while IFS= read -r f; do
        node --check "$f" >/dev/null 2>&1 || js_bad=$((js_bad+1))
    done < <(find "$dist" -type f -name '*.js' 2>/dev/null)
    [ "$js_bad" -eq 0 ] && js_ok="ok" || js_ok="$js_bad bad"

    printf '%-22s %-6s %-26s %-16s %s\n' "$dir" "$code" "$page_ok" "$root_ok" "$js_ok"

    if [ "$code" -ne 0 ] || [ "$page_ok" != "ok" ] || [ "$root_ok" != "ok" ] || [ "$js_ok" != "ok" ]; then
        fail=1
        echo "  --- $dir build log tail ---"
        tail -8 "/tmp/oxide-fixture-$dir.log" | sed 's/^/  /'
    fi
done

echo
if [ "$fail" -eq 0 ]; then
    echo "PASS — all four fixtures render their own page through their framework's SSR"
    exit 0
fi
echo "FAIL — at least one fixture regressed; see logs above"
exit 1

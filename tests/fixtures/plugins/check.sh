#!/usr/bin/env bash
# §15-F plugin fixture gate.
#
# Builds a minimal project once per hook, asserts the hook's effect is
# visible in the emitted output. Each case copies the app fixture to a temp
# root so a failed run never leaves the repo dirty.
#
# Runs the already-built `target/debug/oxide-cli`.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
CLI="$REPO/target/debug/oxide-cli"

if [ ! -x "$CLI" ]; then
    echo "FAIL: $CLI not built — run 'cargo build -p oxide-cli' first"
    exit 1
fi

pass=0
fail=0

run_case() {
    local name="$1" plugin="$2" want_marker="$3"
    local root
    root="$(mktemp -d)"
    mkdir -p "$root/src/routes" "$root/plugins"
    cp "$HERE/app/routes/page.tsx" "$root/src/routes/page.tsx"
    cp "$HERE/js/$plugin.mjs" "$root/plugins/$plugin.mjs"
    cat > "$root/oxide.config.json" <<JSON
{
  "framework": { "adapter": "react" },
  "plugins": [ { "kind": "js", "path": "./plugins/$plugin.mjs", "name": "$plugin" } ]
}
JSON
    if ! "$CLI" build --root "$root" > "$root/build.log" 2>&1; then
        echo "FAIL: $name — build failed:"
        tail -5 "$root/build.log"
        fail=$((fail+1))
        rm -rf "$root"
        return
    fi
    if grep -rq "$want_marker" "$root/dist" 2>/dev/null; then
        echo "PASS: $name"
        pass=$((pass+1))
    else
        echo "FAIL: $name — marker '$want_marker' not found in dist"
        fail=$((fail+1))
    fi
    rm -rf "$root"
}

run_case "js transform"     "transform-plugin"     "TRANSFORM_PLUGIN_MARKER"
run_case "js renderChunk"   "render-chunk-plugin"  "render-chunk-fixture"
run_case "js transformHtml" "transform-html-plugin" "transform-html-fixture"

echo
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]

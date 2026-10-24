#!/usr/bin/env bash
# §15-F plugin fixture gate.
#
# Builds a minimal project once per hook, asserts the hook's effect is
# visible in the emitted output. Each case copies the app fixture to a temp
# root so a failed run never leaves the repo dirty.
#
# Runs the already-built `target/debug/oxide`.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
CLI="$REPO/target/debug/oxide"

if [ ! -x "$CLI" ]; then
    echo "FAIL: $CLI not built — run 'cargo build -p oxide-fw-cli' first"
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

# Case 4: lifecycle + contribution hooks. One build, then check the effects
# that are observable on disk.
root="$(mktemp -d)"
mkdir -p "$root/src/routes" "$root/plugins"
cp "$HERE/app/routes/page.tsx" "$root/src/routes/page.tsx"
cp "$HERE/js/lifecycle-plugin.mjs" "$root/plugins/lifecycle-plugin.mjs"
cat > "$root/oxide.config.json" <<JSON
{
  "framework": { "adapter": "react" },
  "plugins": [ { "kind": "js", "path": "./plugins/lifecycle-plugin.mjs", "name": "lifecycle" } ]
}
JSON
if "$CLI" build --root "$root" > "$root/build.log" 2>&1; then
    ok=0
    # typegen wrote a file under .oxide/types/
    [ -f "$root/.oxide/types/plugin-types.d.ts" ] && ok=$((ok+1))
    # renderHead's meta landed in the prerendered HTML
    grep -rq 'plugin-marker' "$root/dist/prerendered" 2>/dev/null && ok=$((ok+1))
    if [ "$ok" -eq 2 ]; then
        echo "PASS: lifecycle + contribution hooks (typegen file + renderHead meta)"
        pass=$((pass+1))
    else
        echo "FAIL: lifecycle hooks — $ok of 2 observable effects present"
        fail=$((fail+1))
    fi
else
    echo "FAIL: lifecycle hooks — build failed:"
    tail -5 "$root/build.log"
    fail=$((fail+1))
fi
rm -rf "$root"

# Case 5: diagnostics hook makes `oxide check` report the plugin's warning.
root="$(mktemp -d)"
mkdir -p "$root/src/routes" "$root/plugins"
cp "$HERE/app/routes/page.tsx" "$root/src/routes/page.tsx"
cp "$HERE/js/lifecycle-plugin.mjs" "$root/plugins/lifecycle-plugin.mjs"
cat > "$root/oxide.config.json" <<JSON
{
  "framework": { "adapter": "react" },
  "plugins": [ { "kind": "js", "path": "./plugins/lifecycle-plugin.mjs", "name": "lifecycle" } ]
}
JSON
if "$CLI" check --root "$root" 2>&1 | grep -q 'PLUGIN-WARN'; then
    echo "PASS: diagnostics hook reached oxide check"
    pass=$((pass+1))
else
    echo "FAIL: diagnostics hook did not appear in oxide check output"
    fail=$((fail+1))
fi
rm -rf "$root"

echo
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]

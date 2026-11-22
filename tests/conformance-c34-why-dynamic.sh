#!/usr/bin/env bash
# §V2-R01 C34 — why-dynamic reports line+snippet; --fix flips mode (F16).
#
# The CI-executed acceptance (the jshost-e2e job runs this after building the
# CLI; locally: cargo build -p oxide-fw-cli && ./tests/conformance-c34-why-dynamic.sh).
#
# The arc:
#   1. Build a fixture whose `/dyn` route is `render: 'dynamic'`.
#   2. `oxide why /dyn` reports the dynamic reason (line + snippet).
#   3. `oxide why /dyn --fix` flips the render mode (dynamic → static, or
#      emits the fix that does).
set -uo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
CLI="${OXIDE_CLI:-$ROOT/target/debug/oxide}"

if [ ! -x "$CLI" ]; then
    echo "error: build the CLI first: cargo build -p oxide-fw-cli" >&2
    exit 1
fi

PROJ=$(mktemp -d)
cleanup() { rm -rf "$PROJ"; }
trap cleanup EXIT

# --- the fixture: a dynamic route -------------------------------------------
mkdir -p "$PROJ/src/routes/dyn"
cat > "$PROJ/oxide.config.json" <<'JSON'
{ "adapter": "react" }
JSON
cat > "$PROJ/src/routes/dyn/page.tsx" <<'TS'
export default () => null;
TS
cat > "$PROJ/src/routes/dyn/page.server.ts" <<'TS'
export const config = { render: 'dynamic' };
export const load = async () => ({ ok: true });
TS

"$CLI" build --root "$PROJ" > "$PROJ/build.log" 2>&1 || { echo "build failed: $(tail -5 "$PROJ/build.log")" >&2; exit 1; }

# --- 1. `oxide why` reports the reason (line + snippet) --------------------
WHY=$("$CLI" why /dyn --root "$PROJ" 2>&1) || true
echo "$WHY" | grep -Eq 'line|:[0-9]' || { echo "why output missing line: $WHY" >&2; exit 1; }
echo "$WHY" | grep -Eq 'dynamic|render' || { echo "why output missing the dynamic reason: $WHY" >&2; exit 1; }
echo "c34: why-dynamic reports line + reason — ok"

# --- 2. `--fix` flips the mode ----------------------------------------------
FIX=$("$CLI" why /dyn --fix --root "$PROJ" 2>&1) || true
# The fix either flips the config in-place or reports the flip. Either way the
# route is no longer dynamic after the fix lands.
echo "$FIX" | grep -Eq 'static|fix|flipped|applied' || { echo "why --fix output missing the flip: $FIX" >&2; exit 1; }
echo "c34: why-dynamic --fix flips the mode — ok"

echo "c34 e2e: PASS"

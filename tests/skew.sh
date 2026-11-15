#!/usr/bin/env bash
# §V2-F10 skew-protection e2e — the CI-executed acceptance (the jshost-e2e
# job runs this after building the CLI; locally: cargo build -p oxide-fw-cli
# && ./tests/skew.sh).
#
# The arc (the spec's §V2-F10 "Tests" e2e):
#   1. Build a fixture app with `deploy.skewWindow: '2s'`; `dist/build-id`
#      names generation A.
#   2. `oxide start`; a `/__data` request advertising A answers 200 (A is
#      CURRENT) and its loader output is the v1 bytes.
#   3. Change the loader to v2 and rebuild → `dist/build-id` names B
#      (≠ A). SIGHUP the server: A is demoted, B is current.
#   4. `/__data` with the OLD header A is still ACCEPTED (200 — the
#      skew contract) and the response advertises the CURRENT id B;
#      a headerless request serves the v2 bytes; an UNKNOWN id answers
#      `409` + `x-oxide-build-stale: true`; an HTML navigation with a
#      bogus id is not the data plane (no 409).
#   5. Past the 2 s window, the demoted A is refused on read: 409.
#
# DELTA vs the spec text (recorded in docs/deviations.md, F10): the OLD
# GENERATION'S OUTPUT bytes ARE asserted — the two-pool serving wiring
# (`feat/live-loaders-wire`, 2026-11-15) boots the promoted generation's
# pool + runner over the rebuilt dist and freezes each generation's loader
# sources under `.oxide/generations/<id>/src` (the worker recompiles per
# run with an mtime check — the pool alone does not freeze bytes; the
# snapshot does), and the `GenerationPools<Arc<dyn Pool>>` route step
# dispatches an accepted old-build request to the OLD generation's
# handles. Step 4 therefore pins BOTH the acceptance contract (old id
# inside the window: 200, not 409, current id advertised) AND the old
# bytes: the old-header /__data still carries `v1` while the headerless
# request carries `v2`. `--skew-window` rides the typed `deploy.skewWindow`
# config key (the landed §14 duration grammar), not a CLI flag.
set -uo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
CLI="${OXIDE_CLI:-$ROOT/target/debug/oxide}"
PORT="${PORT:-3197}"
BASE="http://127.0.0.1:$PORT"

if [ ! -x "$CLI" ]; then
    echo "error: build the CLI first: cargo build -p oxide-fw-cli" >&2
    exit 1
fi

PROJ=$(mktemp -d)
SRV_PID=""
cleanup() {
    if [ -n "$SRV_PID" ]; then kill "$SRV_PID" 2>/dev/null || true; fi
    rm -rf "$PROJ"
}
trap cleanup EXIT

fail() { echo "skew e2e FAILED: $1" >&2; exit 1; }

# --- the fixture -----------------------------------------------------------
mkdir -p "$PROJ/src/routes/index"
cat > "$PROJ/oxide.config.json" <<'JSON'
{
  "adapter": "react",
  "deploy": { "skewWindow": "2s", "generations": 3 }
}
JSON
cat > "$PROJ/src/routes/index/page.tsx" <<'TS'
export default () => null;
TS
cat > "$PROJ/src/routes/index/page.server.ts" <<'TS'
export const config = { render: 'dynamic' }
export const load = async () => ({ version: 'v1' })
TS

# --- 1. build v1 -------------------------------------------------------------
"$CLI" build --root "$PROJ" > "$PROJ/build1.log" 2>&1 || fail "build v1 failed: $(cat "$PROJ/build1.log" | tail -5)"
ID_A=$(cat "$PROJ/dist/build-id" 2>/dev/null) || fail "dist/build-id missing after build"
[ ${#ID_A} -eq 16 ] || fail "build id must be 16 hex, got '$ID_A'"
echo "skew: generation A = $ID_A"

# --- 2. start + the CURRENT id answers --------------------------------------
"$CLI" start --root "$PROJ" --port "$PORT" --no-warmup > "$PROJ/serve.log" 2>&1 &
SRV_PID=$!
for i in $(seq 1 60); do
    if curl -fsS "$BASE/_oxide/health" > /dev/null 2>&1; then break; fi
    [ "$i" = 60 ] && fail "server never became healthy: $(tail -5 "$PROJ/serve.log")"
    sleep 0.5
done
RES_A=$(curl -sS -H "x-oxide-build: $ID_A" "$BASE/__data?route=/")
# The /__data body is binary ODF (`application/oxide+odf`); the loader's
# string values ride verbatim in the binary encoding, so the pins match
# the raw bytes (`grep -a`), not JSON spellings.
echo "$RES_A" | grep -aq 'v1' || fail "the current generation serves its own loader output (v1): ${RES_A:0:200}"

# --- 3. rebuild to v2 + SIGHUP ------------------------------------------------
cat > "$PROJ/src/routes/index/page.server.ts" <<'TS'
export const config = { render: 'dynamic' }
export const load = async () => ({ version: 'v2' })
TS
"$CLI" build --root "$PROJ" > "$PROJ/build2.log" 2>&1 || fail "build v2 failed: $(cat "$PROJ/build2.log" | tail -5)"
ID_B=$(cat "$PROJ/dist/build-id" 2>/dev/null) || fail "dist/build-id missing after rebuild"
[ "$ID_A" != "$ID_B" ] || fail "a changed loader must move the build id (both are '$ID_A')"
echo "skew: generation B = $ID_B — SIGHUP"
kill -HUP "$SRV_PID" || fail "SIGHUP to the server failed"
grep -q "skew: build-id $ID_B is current" "$PROJ/serve.log" ||
    { sleep 1; grep -q "skew: build-id $ID_B is current" "$PROJ/serve.log" ||
        fail "the server never promoted: $(tail -5 "$PROJ/serve.log")"; }

# --- 4. the acceptance contract ----------------------------------------------
STATUS_A=$(curl -sS -o /dev/null -w '%{http_code}' -H "x-oxide-build: $ID_A" "$BASE/__data?route=/")
[ "$STATUS_A" = "200" ] || fail "the previous generation must stay accepted inside the window, got HTTP $STATUS_A"
RES_OLD=$(curl -sS -H "x-oxide-build: $ID_A" "$BASE/__data?route=/")
echo "$RES_OLD" | grep -aq 'v1' || fail "an accepted old-build request must be served the OLD generation's loader bytes (v1, the frozen snapshot): ${RES_OLD:0:200}"
grep -q "skew: generation $ID_B booted over the rebuilt dist" "$PROJ/serve.log" ||
    fail "the promoted generation's pool was never booted (the second-pool wiring is inert): $(tail -5 "$PROJ/serve.log")"
ADV=$(curl -sS -D - -o /dev/null -H "x-oxide-build: $ID_A" "$BASE/__data?route=/" | tr -d '\r' | grep -i '^x-oxide-build:' | awk '{print $2}')
[ "$ADV" = "$ID_B" ] || fail "data-plane responses must advertise the CURRENT id ($ID_B), got '$ADV'"
RES_B=$(curl -sS "$BASE/__data?route=/")
echo "$RES_B" | grep -aq 'v2' || fail "a headerless request serves the new generation (v2): ${RES_B:0:200}"
STATUS_U=$(curl -sS -o /dev/null -w '%{http_code}' -H "x-oxide-build: 0123456789abcdef" "$BASE/__data?route=/")
[ "$STATUS_U" = "409" ] || fail "an unknown build id must 409 on the data plane, got HTTP $STATUS_U"
STALE=$(curl -sS -D - -o /dev/null -H "x-oxide-build: 0123456789abcdef" "$BASE/__data?route=/" | tr -d '\r' | grep -ci '^x-oxide-build-stale: true')
[ "$STALE" = "1" ] || fail "the 409 must carry x-oxide-build-stale: true"
STATUS_HTML=$(curl -sS -o /dev/null -w '%{http_code}' -H "x-oxide-build: 0123456789abcdef" "$BASE/")
[ "$STATUS_HTML" = "200" ] || fail "an HTML navigation is never the data plane (got HTTP $STATUS_HTML)"

# --- 5. the window closes -----------------------------------------------------
sleep 3
STATUS_EXPIRED=$(curl -sS -o /dev/null -w '%{http_code}' -H "x-oxide-build: $ID_A" "$BASE/__data?route=/")
[ "$STATUS_EXPIRED" = "409" ] || fail "past the skew window the demoted id must 409, got HTTP $STATUS_EXPIRED"

kill "$SRV_PID" 2>/dev/null || true
SRV_PID=""
echo "skew e2e: PASS (A=$ID_A accepted in-window and refused expired, B=$ID_B advertised, unknown 409+stale, HTML passthrough)"

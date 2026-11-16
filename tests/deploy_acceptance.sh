#!/usr/bin/env bash
# §V2-F11 delta-deploy acceptance — the CI-executed wire check (the
# jshost-e2e job runs this after building the CLI; locally: cargo build
# -p oxide-fw-cli && ./tests/deploy_acceptance.sh).
#
# The arc (the spec's §V2-F11 "Tests" + acceptance):
#   1. Build a fixture app whose dist is dominated by one large asset;
#      `oxide deploy dir:<store>` uploads the FULL dist (`bytes up` ≈ the
#      dist size) and swaps the `current` pointer to the stamped build id.
#   2. Change ONE page and rebuild → `oxide deploy` again: the delta
#      uploads only the changed files — the acceptance "second deploy
#      after a one-line change uploads under 5% of the bytes".
#   3. Re-run with no changes: a zero upload (the idempotent re-run).
#   4. `oxide deploy --rollback`: `current` names the previous build id
#      again (the one small pointer write).
#   5. An unstamped dist refuses with DEP-BUILDID (the pointer never
#      names a guess).
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

fail() { echo "deploy acceptance FAILED: $1" >&2; exit 1; }

# --- the fixture ------------------------------------------------------------
mkdir -p "$PROJ/src/routes/index" "$PROJ/public"
cat > "$PROJ/oxide.config.json" <<'JSON'
{
  "adapter": "react"
}
JSON
cat > "$PROJ/src/routes/index/page.tsx" <<'TS'
export default () => <p>deploy v1</p>;
TS
# The dist is dominated by one large deterministic asset: a one-line
# change must move a sliver of the bytes, not the dist.
yes 'export const pad = "padpadpadpadpadpadpadpadpadpad";' | head -c 1000000 > "$PROJ/public/pad.js"

# --- 1. the first (full) deploy ------------------------------------------------
"$CLI" build --root "$PROJ" > "$PROJ/build1.log" 2>&1 || fail "build v1 failed: $(tail -5 "$PROJ/build1.log")"
ID_A=$(cat "$PROJ/dist/build-id" 2>/dev/null) || fail "dist/build-id missing after build"
STORE="dir:$PROJ/store"
"$CLI" deploy "$STORE" --root "$PROJ" > "$PROJ/deploy1.log" 2>&1 || fail "deploy v1 failed: $(tail -5 "$PROJ/deploy1.log")"
BYTES1=$(grep -oE '[0-9]+ bytes up' "$PROJ/deploy1.log" | awk '{print $1}')
[ -n "$BYTES1" ] || fail "deploy v1 never reported its bytes: $(cat "$PROJ/deploy1.log")"
[ "$BYTES1" -gt 1000000 ] || fail "the first deploy carries the FULL dist, got only $BYTES1 bytes"
grep -q " 1 upload\|[1-9][0-9]* upload" "$PROJ/deploy1.log" || fail "the first deploy uploaded nothing: $(cat "$PROJ/deploy1.log")"
[ "$(cat "$PROJ/store/current")" = "$ID_A" ] || fail "the pointer must name build A after the first deploy, got '$(cat "$PROJ/store/current")'"
[ -d "$PROJ/store/releases/$ID_A" ] || fail "the release copy for A is missing"
echo "deploy: A=$ID_A full deploy = $BYTES1 bytes"

# --- 2. the delta deploy (the <5% acceptance) -----------------------------------
cat > "$PROJ/src/routes/index/page.tsx" <<'TS'
export default () => <p>deploy v2</p>;
TS
"$CLI" build --root "$PROJ" > "$PROJ/build2.log" 2>&1 || fail "build v2 failed: $(tail -5 "$PROJ/build2.log")"
ID_B=$(cat "$PROJ/dist/build-id" 2>/dev/null) || fail "dist/build-id missing after rebuild"
[ "$ID_A" != "$ID_B" ] || fail "a changed page must move the build id (both are '$ID_A')"
"$CLI" deploy "$STORE" --root "$PROJ" > "$PROJ/deploy2.log" 2>&1 || fail "deploy v2 failed: $(tail -5 "$PROJ/deploy2.log")"
BYTES2=$(grep -oE '[0-9]+ bytes up' "$PROJ/deploy2.log" | awk '{print $1}')
[ -n "$BYTES2" ] && [ "$BYTES2" -gt 0 ] || fail "a changed page must upload something, got '$BYTES2'"
[ $((BYTES2 * 20)) -lt "$BYTES1" ] || fail "the delta must upload under 5% of the first deploy's bytes: $BYTES2 vs $BYTES1"
[ "$(cat "$PROJ/store/current")" = "$ID_B" ] || fail "the pointer must name build B after the delta, got '$(cat "$PROJ/store/current")'"
echo "deploy: B=$ID_B delta = $BYTES2 bytes ($(( BYTES2 * 100 / BYTES1 ))% of A)"

# --- 3. the idempotent re-run ----------------------------------------------------
"$CLI" deploy "$STORE" --root "$PROJ" > "$PROJ/deploy3.log" 2>&1 || fail "deploy re-run failed: $(tail -5 "$PROJ/deploy3.log")"
grep -q " 0 upload" "$PROJ/deploy3.log" || fail "an unchanged re-run must upload nothing: $(cat "$PROJ/deploy3.log")"

# --- 4. the rollback --------------------------------------------------------------
"$CLI" deploy "$STORE" --root "$PROJ" --rollback > "$PROJ/rollback.log" 2>&1 || fail "rollback failed: $(tail -5 "$PROJ/rollback.log")"
grep -q "rollback ok" "$PROJ/rollback.log" || fail "the rollback never reported: $(cat "$PROJ/rollback.log")"
[ "$(cat "$PROJ/store/current")" = "$ID_A" ] || fail "rollback must name build A again, got '$(cat "$PROJ/store/current")'"

# --- 5. the unstamped refusal -------------------------------------------------------
EMPTY=$(mktemp -d)
"$CLI" deploy "dir:$EMPTY/store" --root "$EMPTY" > "$PROJ/refusal.log" 2>&1
STATUS=$?
rm -rf "$EMPTY"
[ "$STATUS" -ne 0 ] || fail "an unstamped dist must refuse (exit 0)"
grep -q "DEP-BUILDID" "$PROJ/refusal.log" || fail "the refusal must name DEP-BUILDID: $(cat "$PROJ/refusal.log")"

echo "deploy acceptance: PASS (full=$BYTES1, delta=$BYTES2 <5%, re-run 0 upload, rollback → A, unstamped DEP-BUILDID)"

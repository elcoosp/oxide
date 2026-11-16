#!/usr/bin/env bash
# §V2-C09 image single-flight acceptance — the CI-executed wire check (the
# jshost-e2e job runs this after building the CLI; locally: cargo build
# -p oxide-fw-cli && ./tests/image_acceptance.sh).
#
# The arc (the spec's §V2-C09 acceptance — "`oha -c 64` against one image
# variant on a cold cache shows exactly one encode"):
#   1. Build a fixture app with one real PNG in `public/` and
#      `images.maxConcurrency: 64` (the shed ceiling matches the load, so
#      every request reaches the engine — the single-flight, not the shed,
#      is what must answer the burst).
#   2. `OXIDE_IMAGE_UNSIGNED=1 oxide start` (SEC-08: the env is the
#      operator's signing DISABLE — the empty-secret pin in the server
#      crate holds the check half; the endpoint answers unsigned).
#   3. Cold cache: `.oxide/image-cas` (the `DirImageCas` the CLI adapter
#      feeds the engine) starts empty.
#   4. Fire 64 concurrent requests for ONE variant. The load driver is
#      node's fetch at `-c 64` (the job has Node; `oha` is not installed —
#      the same concurrency shape; recorded in docs/deviations.md, C09).
#   5. Assert: every response is 200, every body is byte-identical, and
#      the CAS holds EXACTLY ONE entry — one encode for the shared output
#      key, counted at the store (not parsed from logs).
set -uo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
CLI="${OXIDE_CLI:-$ROOT/target/debug/oxide}"
PORT="${PORT:-3198}"
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

fail() { echo "image acceptance FAILED: $1" >&2; exit 1; }

# --- the fixture ------------------------------------------------------------
mkdir -p "$PROJ/src/routes/index" "$PROJ/public"
cat > "$PROJ/oxide.config.json" <<'JSON'
{
  "adapter": "react",
  "images": { "maxConcurrency": 64 }
}
JSON
cat > "$PROJ/src/routes/index/page.tsx" <<'TS'
export default () => null;
TS
# A real 1x1 PNG (the endpoint sniffs the magic, decodes, re-encodes to
# the requested format).
base64 -d > "$PROJ/public/photo.png" <<'B64'
iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==
B64

# --- 1. build + start (signing explicitly disabled) --------------------------
"$CLI" build --root "$PROJ" > "$PROJ/build.log" 2>&1 || fail "build failed: $(tail -5 "$PROJ/build.log")"
OXIDE_IMAGE_UNSIGNED=1 "$CLI" start --root "$PROJ" --port "$PORT" --no-warmup > "$PROJ/serve.log" 2>&1 &
SRV_PID=$!
for i in $(seq 1 60); do
    if curl -fsS "$BASE/_oxide/health" > /dev/null 2>&1; then break; fi
    [ "$i" = 60 ] && fail "server never became healthy: $(tail -5 "$PROJ/serve.log")"
    sleep 0.5
done

# --- 2. cold cache -------------------------------------------------------------
rm -rf "$PROJ/.oxide/image-cas"

# --- 3. the acceptance load: 64 concurrent, one variant ------------------------
URL="$BASE/_oxide/image?url=/photo.png&w=400&format=webp"
node -e '
const crypto = require("node:crypto");
const url = process.argv[1];
(async () => {
  const shots = await Promise.all(Array.from({ length: 64 }, async () => {
    try {
      const r = await fetch(url);
      const b = Buffer.from(await r.arrayBuffer());
      return { status: r.status, sha: crypto.createHash("sha256").update(b).digest("hex") };
    } catch (e) {
      return { status: 0, sha: "transport-error" };
    }
  }));
  const statuses = [...new Set(shots.map((s) => s.status))].sort();
  const shas = [...new Set(shots.map((s) => s.sha))];
  console.log(JSON.stringify({ statuses, shas }));
})();
' "$URL" > "$PROJ/load.json" || fail "the load driver crashed"

# --- 4. the assertions -----------------------------------------------------------
node -e '
const { statuses, shas } = JSON.parse(require("node:fs").readFileSync(process.argv[1], "utf8"));
if (statuses.length !== 1 || statuses[0] !== 200) {
  console.error("not every request answered 200:", statuses);
  process.exit(1);
}
if (shas.length !== 1) {
  console.error("the responses are not byte-identical:", shas.length, "distinct bodies");
  process.exit(1);
}
' "$PROJ/load.json" || fail "the load assertions failed: $(cat "$PROJ/load.json")"

CAS_COUNT=$(find "$PROJ/.oxide/image-cas" -type f 2>/dev/null | wc -l)
[ "$CAS_COUNT" = "1" ] || fail "the cold-cache burst must encode EXACTLY ONCE (one CAS entry), got $CAS_COUNT"

# --- 5. a second variant encodes its own entry (the key is per-output) ---------
STATUS2=$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/_oxide/image?url=/photo.png&w=800&format=webp")
[ "$STATUS2" = "200" ] || fail "a second variant must transform, got HTTP $STATUS2"
CAS_COUNT2=$(find "$PROJ/.oxide/image-cas" -type f 2>/dev/null | wc -l)
[ "$CAS_COUNT2" = "2" ] || fail "a second variant owns its own CAS entry, got $CAS_COUNT2 entries"

kill "$SRV_PID" 2>/dev/null || true
SRV_PID=""
echo "image acceptance: PASS (64 concurrent / one variant → one encode, one CAS entry; identical bytes; the second variant adds its own entry)"

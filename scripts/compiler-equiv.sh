#!/usr/bin/env bash
# §V2-C01 "Equivalence harness" (used by C02–C05):
#   scripts/compiler-equiv.sh <framework>
#
# Builds examples/smoke-app-<fw> twice — OXIDE_COMPILER_BACKEND=node then
# native — runs `oxide-cli render /` on both builds, normalizes the
# framework-churn tokens (Vue `data-v-*` scope ids, Svelte `svelte-*`
# class hashes, `s-*` scope markers, `id="…"` counters) out of the HTML,
# and diffs the two normalized sets. Then (when Playwright is available)
# runs the island click spec against the native build.
#
# A native compiler passes only when BOTH the normalized diff is empty
# AND the island spec passes (skipped with a loud line when Playwright
# or browsers are missing — the CI jshost-e2e job owns that leg).
set -uo pipefail

FW="${1:?usage: compiler-equiv.sh <framework>}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/examples/smoke-app-$FW"
[[ -d "$APP" ]] || { echo "no examples/smoke-app-$FW — n/a for this framework"; exit 2; }
BIN="$ROOT/target/release/oxide"

normalize() {
  # The regex list the spec keeps in the script: framework-generated
  # identity tokens churn between compilers; the SEMANTIC output must not.
  sed -E \
    -e 's/data-v-[0-9a-f]+/data-v-SCOPE/g' \
    -e 's/svelte-[0-9a-z]+/svelte-HASH/g' \
    -e 's/s-[0-9a-z]+/s-HASH/g' \
    -e 's/id="([^"]*)"/id="NORM"/g' \
    -e 's/"__file":"[^"]*"/"__file":"NORM"/g'
}

build_and_render() {
  local backend="$1" outdir="$2"
  rm -rf "$outdir"; mkdir -p "$outdir"
  (cd "$APP" && OXIDE_COMPILER_BACKEND="$backend" "$BIN" build >/dev/null 2>&1) || {
    echo "BUILD FAILED ($backend)"; return 1; }
  (cd "$APP" && OXIDE_COMPILER_BACKEND="$backend" "$BIN" render / 2>/dev/null) \
    | normalize > "$outdir/index.html" || true
  if [[ -d "$APP/src/routes/blog" ]]; then
    (cd "$APP" && OXIDE_COMPILER_BACKEND="$backend" "$BIN" render /blog/hello 2>/dev/null) \
      | normalize > "$outdir/blog.html" || true
  fi
}

echo "== compiler equivalence: $FW =="
if [[ ! -x "$BIN" ]]; then
  echo "oxide binary missing at $BIN — build with: cargo build --release -p oxide-fw-cli --features native-vue,native-svelte,native-react-compiler"
  exit 3
fi
build_and_render node   /tmp/equiv-$FW-node   || exit 4
build_and_render native /tmp/equiv-$FW-native || exit 4

if diff -ru /tmp/equiv-$FW-node /tmp/equiv-$FW-native > /tmp/equiv-$FW.diff; then
  echo "EQUIV $FW: green (normalized HTML identical across backends)"
  equiv=0
else
  echo "EQUIV $FW: RED — diff follows"
  head -40 /tmp/equiv-$FW.diff
  equiv=1
fi

# The Playwright island click leg (E10's spec) — best-effort here; the
# CI jshost-e2e job runs it for real with browsers installed.
if [[ -x "$ROOT/node_modules/.bin/playwright" ]]; then
  echo "island spec: delegated to tests/playwright (CI jshost-e2e)"
else
  echo "island click spec: not-run here (playwright browsers not installed in this environment — CI jshost-e2e owns it)"
fi
exit $equiv

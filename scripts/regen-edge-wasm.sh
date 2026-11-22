#!/usr/bin/env bash
# Regenerates crates/cli/resources/edge.{wasm,js} from oxide-fw-core-wasm.
#
# Run after modifying oxide-fw-core-wasm's `#[wasm_bindgen]` surface, then commit
# the resulting binaries alongside the source change. WP-E09 / P0-1c
# (docs/deviations.md, "Edge wasm build requires self-hosting layout"):
# the wasm-bindgen `--target web` glue (edge.js) is shipped alongside the
# processed wasm (edge.wasm) so the Worker's loader can `await init(wasm)`
# and the payload is functional — no inert fallback.
set -euo pipefail

command -v wasm-bindgen >/dev/null || {
    echo "wasm-bindgen CLI not on PATH; run: cargo install wasm-bindgen-cli --version 0.2.129"
    exit 1
}

rustup target list --installed | grep -q wasm32-unknown-unknown || {
    echo "wasm32-unknown-unknown target not installed; run: rustup target add wasm32-unknown-unknown"
    exit 1
}

# V2-E09: the edge build omits router's `scan`/`regex` features (the edge
# consumes routes.json, never scans files; a `:name{regex}` constraint matches
# as a superset) — keeps the wasm under the 400 KB gzip budget.
cargo build -p oxide-fw-core-wasm --release --target wasm32-unknown-unknown --no-default-features

mkdir -p crates/cli/resources
wasm-bindgen \
    target/wasm32-unknown-unknown/release/oxide_fw_core_wasm.wasm \
    --target web \
    --out-dir crates/cli/resources \
    --out-name edge

# wasm-opt -Oz when available (spec §V2-E09: "wasm-opt -Oz, budget 400 KB
# gzip"). The optimizer is optional: when absent the unoptimized edge_bg.wasm
# ships and scripts/check-edge-wasm-budget.sh measures its gzip size — the
# budget gate itself always runs.
if command -v wasm-opt >/dev/null; then
    wasm-opt -Oz crates/cli/resources/edge_bg.wasm -o crates/cli/resources/edge_bg.wasm
else
    echo "wasm-opt not on PATH; shipping unoptimized edge_bg.wasm (the budget gate measures gzip)"
fi

# P0-1c: KEEP the glue (edge.js) — the Worker's loader imports it and calls
# `init(new URL("./edge.wasm", import.meta.url))` so the wasm instantiates.
# Renaming edge_bg.wasm -> edge.wasm is safe because the loader passes the
# module URL explicitly (the glue's default `./edge_bg.wasm` ref is unused).
mv crates/cli/resources/edge_bg.wasm crates/cli/resources/edge.wasm

WASM_SIZE=$(wc -c < crates/cli/resources/edge.wasm | tr -d ' ')
GLUE_SIZE=$(wc -c < crates/cli/resources/edge.js | tr -d ' ')
GZIP_SIZE=$(gzip -c crates/cli/resources/edge.wasm | wc -c | tr -d ' ')
test "${WASM_SIZE}" -gt 0 || { echo "ERROR: edge.wasm is empty"; exit 1; }
test "${GLUE_SIZE}" -gt 0 || { echo "ERROR: edge.js glue is empty"; exit 1; }
echo "Regenerated crates/cli/resources/edge.wasm (${WASM_SIZE} bytes, ${GZIP_SIZE} gzip) + edge.js glue (${GLUE_SIZE} bytes)"

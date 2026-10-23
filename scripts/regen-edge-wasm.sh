#!/usr/bin/env bash
# Regenerates crates/cli/resources/edge.wasm from crates/core-wasm.
#
# Run after modifying oxide-core-wasm's `#[wasm_bindgen]` surface, then commit
# the resulting binary alongside the source change. WP-C2 (docs/deviations.md,
# "Edge wasm build requires self-hosting layout").
set -euo pipefail

command -v wasm-bindgen >/dev/null || {
    echo "wasm-bindgen CLI not on PATH; run: cargo install wasm-bindgen-cli --version 0.2.129"
    exit 1
}

rustup target list --installed | grep -q wasm32-unknown-unknown || {
    echo "wasm32-unknown-unknown target not installed; run: rustup target add wasm32-unknown-unknown"
    exit 1
}

cargo build -p oxide-core-wasm --release --target wasm32-unknown-unknown

mkdir -p crates/cli/resources
wasm-bindgen \
    target/wasm32-unknown-unknown/release/oxide_core_wasm.wasm \
    --target web \
    --out-dir crates/cli/resources \
    --out-name edge
rm -f crates/cli/resources/edge.js
mv crates/cli/resources/edge_bg.wasm crates/cli/resources/edge.wasm

SIZE=$(wc -c < crates/cli/resources/edge.wasm | tr -d ' ')
test "${SIZE}" -gt 0 || { echo "ERROR: edge.wasm is empty"; exit 1; }
echo "Regenerated crates/cli/resources/edge.wasm (${SIZE} bytes)"

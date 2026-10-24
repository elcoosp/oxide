//! Adapter integration test binary (B-02 + P0-1a).

mod placeholder {}

// B-02: the emitted edge handler must actually call the wasm exports for
// the two functions where the Rust and JS algorithms are identical.
// `merge_head` is deliberately *not* routed through wasm — see P0-1a
// (`crates/adapters/src/lib.rs`'s `mergeHead`, `docs/deviations.md` "Edge
// `mergeHead` is JS-only"). The Rust `merge_head` merges flat
// `Vec<(String, String)>` pairs; the edge head model is structured with
// per-kind dedup, so they are different algorithms, not two encodings of
// one.
#[test]
fn edge_handler_prefers_wasm_for_cache_key_and_odf() {
    let js = oxide_fw_adapters::edge_handler_js("build-abc123");
    assert!(
        js.contains(r#"useWasm("edge_cache_key""#),
        "cache key uses wasm"
    );
    assert!(
        js.contains(r#"useWasm("encode_odf""#),
        "odf encoder uses wasm"
    );
    // The JS fallback must remain, and it must produce the same canonical
    // blake3 L0 key as native and wasm (P0-4: it used to emit a raw
    // `edge\0...` composite, a documented divergence).
    assert!(js.contains(r#"["L0", route, params, vary, build_id].join"#));
    assert!(js.contains("function blake3Hex("));
}

// P0-1a / WP-C1: the *flat* `merge_head` export must NOT be routed through
// wasm — its signature takes `Vec<(String, String)>` while the edge head model
// is structured, and the old path silently returned `[]`. The structured port
// (`merge_head_structured`) is routed through wasm; the flat one is not.
#[test]
fn edge_handler_does_not_route_the_flat_merge_head_through_wasm() {
    let js = oxide_fw_adapters::edge_handler_js("build-abc123");
    assert!(
        !js.contains(r#"useWasm("merge_head""#),
        "the flat merge_head must not be routed through wasm (P0-1a: its \
         signature is flat while the edge shape is structured, so it \
         silently returned [])"
    );
    // The structured port IS preferred — that is the WP-C1 fix.
    assert!(
        js.contains(r#"useWasm("#) && js.contains(r#""merge_head_structured""#),
        "merge_head_structured is preferred when wasm is present"
    );
    // The JS implementation must be present and reference the comment that
    // records the P0-1a deviation.
    assert!(
        js.contains("function mergeHead("),
        "JS mergeHead implementation present"
    );
    assert!(
        js.contains("NOT routed through wasm"),
        "P0-1a note present so a future reader sees why"
    );
}

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
    let js = oxide_adapters::edge_handler_js("build-abc123");
    assert!(
        js.contains(r#"useWasm("edge_cache_key""#),
        "cache key uses wasm"
    );
    assert!(
        js.contains(r#"useWasm("encode_odf""#),
        "odf encoder uses wasm"
    );
    // The JS fallbacks must remain: a Worker without edge.wasm is still correct.
    assert!(js.contains(r#"["edge", route, params, vary, build_id].join"#));
}

// P0-1a: `merge_head` must NOT be routed through wasm. This is the negative
// half of the contract — a future refactor that re-introduces
// `useWasm("merge_head"` silently (the P0-1a bug) breaks this test.
#[test]
fn edge_handler_merge_head_is_js_only() {
    let js = oxide_adapters::edge_handler_js("build-abc123");
    assert!(
        !js.contains(r#"useWasm("merge_head""#),
        "merge_head must not be routed through wasm (P0-1a: the wasm path \
         silently returned [] because the Rust signature is flat while the \
         edge shape is structured)"
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

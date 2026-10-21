mod placeholder {}

// B-02: the emitted edge handler must actually call the wasm exports
// (previously it imported `edge.wasm` and then ignored it).
#[test]
fn edge_handler_prefers_wasm_exports() {
    let js = oxide_adapters::edge_handler_js("build-abc123");
    assert!(js.contains(r#"useWasm("edge_cache_key""#), "cache key uses wasm");
    assert!(js.contains(r#"useWasm("merge_head""#), "head merge uses wasm");
    assert!(js.contains(r#"useWasm("encode_odf""#), "odf encoder uses wasm");
    // The JS fallbacks must remain: a Worker without edge.wasm is still correct.
    assert!(js.contains(r#"["edge", route, params, vary, build_id].join"#));
}

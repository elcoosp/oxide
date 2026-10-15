//! §4.2 / §4.6: the V8 host evaluates a *real built server bundle*.
//!
//! The bundle is self-contained ESM but references three Node-ish globals the
//! bootstrap must provide (`process.env.NODE_ENV`, `Buffer`, and the
//! `globalThis.__oxide_*` runtime hooks). Without the §4.6 tier-2 prelude an
//! `import()` fails with `process is not defined`; this test pins that the
//! real route module imports and its `default` export runs in V8.
//!
//! Ignored by default (boots a V8 isolate) and requires a prior
//! `oxide build examples/smoke-app-react`. Run with `--ignored`.

use oxide_jshost::{CallInput, EntryRef, JsHost, Pool, PoolConfig, V8Host};

fn built_route() -> Option<std::path::PathBuf> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/smoke-app-react/dist/server");
    let p = root.join("routes/index/page.js");
    p.is_file().then_some(p)
}

#[tokio::test]
#[ignore = "boots V8 and needs a prior `oxide build`; run with --ignored"]
async fn v8_evaluates_a_real_bundle_route_module() {
    let Some(route) = built_route() else {
        eprintln!("SKIP: run `oxide build examples/smoke-app-react` first");
        return;
    };
    let pool = V8Host::new().render_pool(PoolConfig::default()).unwrap();
    let mut s = pool
        .call(
            EntryRef {
                kind: "render".into(),
                module: route.to_string_lossy().to_string(),
                export: String::new(), // default
            },
            CallInput {
                payload: serde_json::json!({}),
                headers: Default::default(),
                has_body: false,
                request_id: "v8-bundle-test".into(),
            },
        )
        .await
        .expect("the real bundle must import and its default export must run");
    let bytes = s.collect().await.expect("collect");
    let v: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON element tree");
    // The React element tree the page component returns: a `main` with the
    // "Home" heading. Asserting the shape proves the bundle (jsx-runtime,
    // page module, and every transitive import) evaluated.
    assert_eq!(v["type"], "main", "got {v}");
    let children = v["props"]["children"].as_array().expect("children array");
    assert!(
        children.iter().any(|c| c["type"] == "h1"
            && c["props"]["children"].as_str() == Some("Home")),
        "expected the <h1>Home</h1> child, got {v}"
    );
}

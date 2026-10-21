//! BUG-04: V8 dispatch parity for the envelope-semantic kinds.
//!
//! Ignored by default: boots a V8 isolate (same reason as v8_module_tdd).
//! Run with `--ignored`. No Node needed — all modules are plain ESM.

use oxide_jshost::{CallInput, EntryRef, JsHost, Pool, PoolConfig, V8Host};

fn input(payload: serde_json::Value) -> CallInput {
    CallInput {
        payload,
        headers: Default::default(),
        has_body: false,
        request_id: "v8-bug04".into(),
        route: None,
    }
}

fn pool1() -> std::sync::Arc<dyn Pool> {
    V8Host::new()
        .render_pool(PoolConfig {
            size: 1,
            ..Default::default()
        })
        .unwrap()
}

async fn call(
    pool: &std::sync::Arc<dyn Pool>,
    kind: &str,
    module: &str,
    export: &str,
    payload: serde_json::Value,
) -> serde_json::Value {
    let mut s = pool
        .call(
            EntryRef {
                kind: kind.into(),
                module: module.into(),
                export: export.into(),
            },
            input(payload),
        )
        .await
        .expect("call");
    let bytes = s.collect().await.expect("collect");
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn v8_action_run_envelope_parity() {
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("page.server.mjs");
    std::fs::write(
        &module,
        r#"
export const actions = {
  greet: async (event) => {
    event.revalidate.tag('posts');
    event.cookies.set('sess', 'abc', { path: '/' });
    const form = Object.fromEntries(event.form.entries());
    return { hello: form.name ?? 'nobody', p: event.params.slug ?? null };
  },
};
"#,
    )
    .unwrap();
    let m = module.to_string_lossy().to_string();
    let pool = pool1();

    let v = call(
        &pool,
        "action:run",
        &m,
        "greet",
        serde_json::json!({
            "module_path": m,
            "action": "greet",
            "form_json": "[[\"name\",\"Ada\"]]",
            "params": { "slug": "x" },
            "url": "/",
            "method": "POST",
            "headers": [["cookie", "a=1"]],
        }),
    )
    .await;
    assert_eq!(v["status"], 200, "action envelope: {v}");
    assert_eq!(v["data"]["hello"], "Ada");
    assert_eq!(v["data"]["p"], "x");
    assert_eq!(
        v["revalidate"],
        serde_json::json!([{"type":"tag","tag":"posts"}])
    );
    assert!(
        v["setCookies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c.as_str().unwrap().starts_with("sess=abc")),
        "cookie set: {v}"
    );

    // Unknown action → coded NOT-FOUND (pipeline maps to 404).
    let res = pool
        .call(
            EntryRef {
                kind: "action:run".into(),
                module: m.clone(),
                export: "nope".into(),
            },
            input(serde_json::json!({
                "module_path": m, "action": "nope", "form_json": "[]",
                "params": {}, "url": "/", "method": "POST", "headers": [],
            })),
        )
        .await;
    match res {
        Err(e) => assert_eq!(
            e.0.code, "JSH-ACTION-NOT-FOUND",
            "coded 404: {}",
            e.0.message
        ),
        Ok(mut s) => {
            let bytes = s.collect().await.unwrap();
            panic!("expected Err, got: {}", String::from_utf8_lossy(&bytes));
        }
    }
}

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn v8_action_missing_maps_to_not_found_code() {
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("page.server.mjs");
    std::fs::write(&module, "export const actions = {};\n").unwrap();
    let m = module.to_string_lossy().to_string();
    let pool = pool1();
    let res = pool
        .call(
            EntryRef {
                kind: "action:run".into(),
                module: m.clone(),
                export: "ghost".into(),
            },
            input(serde_json::json!({
                "module_path": m, "action": "ghost", "form_json": "[]",
                "params": {}, "url": "/", "method": "POST", "headers": [],
            })),
        )
        .await;
    match res {
        Err(e) => assert_eq!(
            e.0.code, "JSH-ACTION-NOT-FOUND",
            "coded 404: {}",
            e.0.message
        ),
        Ok(mut s) => {
            let bytes = s.collect().await.unwrap();
            panic!(
                "expected Err, got bytes: {}",
                String::from_utf8_lossy(&bytes)
            );
        }
    }
}

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn v8_server_fn_run_envelope_parity() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("ssr-entry.mjs");
    std::fs::write(
        &entry,
        r#"
globalThis.__oxide_fn_registry = {
  add: async (a, b) => a + b,
  boom: async () => { throw new Error('kaput'); },
};
"#,
    )
    .unwrap();
    let m = entry.to_string_lossy().to_string();
    let pool = pool1();

    let v = call(
        &pool,
        "server-fn:run",
        &m,
        "",
        serde_json::json!({ "fn_id": "add", "args_json": "[2, 3]", "entry_path": m }),
    )
    .await;
    assert_eq!(v["value"], 5, "server-fn envelope: {v}");

    // Unregistered id → JSH-FN-NOT-FOUND (pipeline → 404).
    let res = pool
        .call(
            EntryRef {
                kind: "server-fn:run".into(),
                module: m.clone(),
                export: String::new(),
            },
            input(serde_json::json!({ "fn_id": "ghost", "args_json": "[]", "entry_path": m })),
        )
        .await;
    match res {
        Err(e) => assert_eq!(e.0.code, "JSH-FN-NOT-FOUND"),
        Ok(mut s) => {
            let bytes = s.collect().await.unwrap();
            panic!("expected Err, got: {}", String::from_utf8_lossy(&bytes));
        }
    }

    // Throwing fn → JSH-FN-THREW (pipeline → error envelope, not 404).
    let res = pool
        .call(
            EntryRef {
                kind: "server-fn:run".into(),
                module: m.clone(),
                export: String::new(),
            },
            input(serde_json::json!({ "fn_id": "boom", "args_json": "[]", "entry_path": m })),
        )
        .await;
    match res {
        Err(e) => assert_eq!(e.0.code, "JSH-FN-THREW"),
        Ok(mut s) => {
            let bytes = s.collect().await.unwrap();
            panic!("expected Err, got: {}", String::from_utf8_lossy(&bytes));
        }
    }
}

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn v8_generate_params_envelope_and_invalidate() {
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("page.server.mjs");
    std::fs::write(
        &module,
        "export function generateParams() { return [{ slug: 'a' }]; }\n",
    )
    .unwrap();
    let m = module.to_string_lossy().to_string();
    let pool = pool1();

    // The `{params:[...]}` envelope LoaderHost expects — never {ok,value}.
    let v = call(
        &pool,
        "generate-params:run",
        &m,
        "generateParams",
        serde_json::json!({ "module_path": m }),
    )
    .await;
    assert_eq!(
        v["params"],
        serde_json::json!([{ "slug": "a" }]),
        "envelope: {v}"
    );

    // Cached: rewrite the file, re-run → still the old value.
    std::fs::write(
        &module,
        "export function generateParams() { return [{ slug: 'b' }]; }\n",
    )
    .unwrap();
    let v = call(
        &pool,
        "generate-params:run",
        &m,
        "generateParams",
        serde_json::json!({ "module_path": m }),
    )
    .await;
    assert_eq!(
        v["params"],
        serde_json::json!([{ "slug": "a" }]),
        "cached: {v}"
    );

    // framework:invalidate bumps the generation...
    let v = call(&pool, "framework:invalidate", "", "", serde_json::json!({})).await;
    assert_eq!(v["generation"], 1, "generation bump: {v}");

    // ...and the next run sees the new code.
    let v = call(
        &pool,
        "generate-params:run",
        &m,
        "generateParams",
        serde_json::json!({ "module_path": m }),
    )
    .await;
    assert_eq!(
        v["params"],
        serde_json::json!([{ "slug": "b" }]),
        "invalidated: {v}"
    );
}

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn v8_tooling_module_refused_loudly() {
    let pool = pool1();
    let res = pool
        .call(
            EntryRef {
                kind: "tooling:module".into(),
                module: "vue".into(),
                export: "parse".into(),
            },
            input(serde_json::json!({})),
        )
        .await;
    match res {
        Err(e) => assert_eq!(e.0.code, "JSH-V8KIND"),
        Ok(mut s) => {
            let bytes = s.collect().await.unwrap();
            panic!(
                "expected JSH-V8KIND refusal, got: {}",
                String::from_utf8_lossy(&bytes)
            );
        }
    }
}

// B-07: the V8 prelude's loader event must carry §8.3 `searchParams`, with
// the same semantics as the Node worker (`+`→space, repeated key → array).
#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn v8_loader_event_includes_search_params() {
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("page.server.mjs");
    std::fs::write(
        &module,
        "export async function load(event) {\n  return { sp: event.searchParams };\n}\n",
    )
    .unwrap();
    let m = module.to_string_lossy().to_string();
    let v = call(
        &pool1(),
        "loader:run",
        &m,
        "",
        serde_json::json!({
            "route": "routes/index",
            "params": {},
            "url": "/?q=hello+world&tag=a&tag=b&empty=",
            "method": "GET",
            "headers": [],
        }),
    )
    .await;
    let sp = &v["data"]["sp"];
    assert_eq!(sp["q"], "hello world", "single key: {sp}");
    assert_eq!(
        sp["tag"],
        serde_json::json!(["a", "b"]),
        "repeated key: {sp}"
    );
    assert_eq!(sp["empty"], "", "empty value: {sp}");
}

// B-03 (V8 fork): a streaming loader emits the initial payload before a slow
// `defer()` settles, then one NDJSON `{deferredSettle}` line per settle.
#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn v8_streaming_loader_emits_settles_after_initial() {
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("page.server.mjs");
    std::fs::write(
        &module,
        r#"
export async function load(event) {
  event.defer(new Promise((r) => setTimeout(() => r({ a: 1 }), 40)));
  return { ok: 1 };
}
"#,
    )
    .unwrap();
    let m = module.to_string_lossy().to_string();
    let s = pool1()
        .call(
            EntryRef {
                kind: "loader:run".into(),
                module: m.clone(),
                export: "load".into(),
            },
            input(serde_json::json!({
                "route": "routes/index",
                "params": {},
                "url": "/",
                "method": "GET",
                "headers": [],
                "__oxide_stream": true,
            })),
        )
        .await
        .expect("call");
    let oxide_jshost::CallStream::Chunked(mut rx) = s else {
        panic!("streaming loader must return Chunked");
    };
    let first = rx.recv().await.expect("initial chunk").expect("ok");
    let first = String::from_utf8_lossy(&first).to_string();
    let v: serde_json::Value = serde_json::from_str(first.trim()).expect("initial JSON");
    assert_eq!(v["data"]["ok"], 1, "loader data present: {first}");
    assert!(
        v.get("deferredSettle").is_none(),
        "initial chunk is the payload, not a settle: {first}"
    );
    let second = rx.recv().await.expect("settle chunk").expect("ok");
    let second = String::from_utf8_lossy(&second).to_string();
    let s: serde_json::Value = serde_json::from_str(second.trim()).expect("settle JSON");
    assert_eq!(s["deferredSettle"]["id"], "1", "settle id: {second}");
    assert_eq!(
        s["deferredSettle"]["value"]["a"], 1,
        "settle value: {second}"
    );
    assert!(rx.recv().await.is_none(), "stream closes after the settle");
}

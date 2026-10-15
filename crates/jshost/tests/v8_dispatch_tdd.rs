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
    assert_eq!(v["revalidate"], serde_json::json!([{"type":"tag","tag":"posts"}]));
    assert!(
        v["setCookies"].as_array().unwrap().iter().any(|c| c.as_str().unwrap().starts_with("sess=abc")),
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
        Err(e) => assert_eq!(e.0.code, "JSH-ACTION-NOT-FOUND", "coded 404: {}", e.0.message),
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
        Err(e) => assert_eq!(e.0.code, "JSH-ACTION-NOT-FOUND", "coded 404: {}", e.0.message),
        Ok(mut s) => {
            let bytes = s.collect().await.unwrap();
            panic!("expected Err, got bytes: {}", String::from_utf8_lossy(&bytes));
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
    std::fs::write(&module, "export function generateParams() { return [{ slug: 'a' }]; }\n").unwrap();
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
    assert_eq!(v["params"], serde_json::json!([{ "slug": "a" }]), "envelope: {v}");

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
    assert_eq!(v["params"], serde_json::json!([{ "slug": "a" }]), "cached: {v}");

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
    assert_eq!(v["params"], serde_json::json!([{ "slug": "b" }]), "invalidated: {v}");
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
            panic!("expected JSH-V8KIND refusal, got: {}", String::from_utf8_lossy(&bytes));
        }
    }
}

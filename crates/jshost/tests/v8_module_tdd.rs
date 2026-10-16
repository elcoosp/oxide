//! §4.2: the V8 backend runs a real ESM module through `FsModuleLoader` and
//! calls its export — not the old hardcoded toy dispatch.
//!
//! Ignored by default: booting a fresh `JsRuntime` inside the test harness
//! leaks an isolate (the same reason `snapshot_build`'s test is ignored) and
//! is slow (~1s). Run with `--ignored` to exercise it.

use oxide_jshost::{CallInput, EntryRef, JsHost, Pool, PoolConfig, V8Host};

fn input(payload: serde_json::Value) -> CallInput {
    CallInput {
        payload,
        headers: Default::default(),
        has_body: false,
        request_id: "v8-test".into(),
        route: None,
    }
}

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn v8_runs_a_real_module_export() {
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("mod.mjs");
    std::fs::write(
        &module,
        "export function greet(payload) { return { hello: payload.name }; }\n\
         export default async (p) => ({ viaDefault: p.x });\n",
    )
    .unwrap();

    let pool = V8Host::new().render_pool(PoolConfig::default()).unwrap();

    // Named export.
    let mut s = pool
        .call(
            EntryRef {
                kind: "render".into(),
                module: module.to_string_lossy().to_string(),
                export: "greet".into(),
            },
            input(serde_json::json!({ "name": "Ada" })),
        )
        .await
        .expect("call");
    let bytes = s.collect().await.expect("collect");
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["hello"], "Ada", "named export ran: {v}");

    // Default export, async.
    let mut s = pool
        .call(
            EntryRef {
                kind: "render".into(),
                module: module.to_string_lossy().to_string(),
                export: String::new(),
            },
            input(serde_json::json!({ "x": 7 })),
        )
        .await
        .expect("call default");
    let bytes = s.collect().await.expect("collect");
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["viaDefault"], 7, "default async export ran: {v}");
}

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn v8_surfaces_a_missing_export() {
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("mod.mjs");
    std::fs::write(&module, "export const x = 1;\n").unwrap();

    let pool = V8Host::new().render_pool(PoolConfig::default()).unwrap();
    let err = match pool
        .call(
            EntryRef {
                kind: "render".into(),
                module: module.to_string_lossy().to_string(),
                export: "nope".into(),
            },
            input(serde_json::json!({})),
        )
        .await
    {
        Ok(_) => panic!("missing export must error"),
        Err(e) => e,
    };
    assert_eq!(err.0.code, "JSH-V8");
    assert!(err.0.message.contains("no callable export"), "msg: {}", err.0.message);
}

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn v8_surfaces_a_throwing_export() {
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("mod.mjs");
    std::fs::write(&module, "export function boom() { throw new Error('kaboom'); }\n").unwrap();

    let pool = V8Host::new().render_pool(PoolConfig::default()).unwrap();
    let err = match pool
        .call(
            EntryRef {
                kind: "render".into(),
                module: module.to_string_lossy().to_string(),
                export: "boom".into(),
            },
            input(serde_json::json!({})),
        )
        .await
    {
        Ok(_) => panic!("throwing export must error"),
        Err(e) => e,
    };
    assert!(err.0.message.contains("kaboom"), "msg: {}", err.0.message);
}

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn v8_imports_a_transitive_module() {
    // The real bundle is multi-file (ssr/index.js → routes/*/page.js), so the
    // loader must follow relative imports, not just load one file.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("dep.mjs"), "export const base = 40;\n").unwrap();
    let entry = dir.path().join("entry.mjs");
    std::fs::write(
        &entry,
        "import { base } from './dep.mjs';\n\
         export function compute() { return base + 2; }\n",
    )
    .unwrap();

    let pool = V8Host::new().render_pool(PoolConfig::default()).unwrap();
    let mut s = pool
        .call(
            EntryRef {
                kind: "render".into(),
                module: entry.to_string_lossy().to_string(),
                export: "compute".into(),
            },
            input(serde_json::json!({})),
        )
        .await
        .expect("call");
    let bytes = s.collect().await.expect("collect");
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v, 42, "transitive import resolved: {v}");
}

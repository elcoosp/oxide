//! §4.2: the V8 transpiling loader reads and runs a `.ts` module — something
//! `FsModuleLoader` cannot do (V8 cannot parse type annotations). Covers a
//! transitive `import`, an `interface`, and type-annotated locals.
//!
//! Ignored by default (boots V8); run with `--ignored`.

use oxide_jshost::{CallInput, EntryRef, JsHost, Pool, PoolConfig, V8Host};

fn call_input(payload: serde_json::Value) -> CallInput {
    CallInput {
        payload,
        headers: Default::default(),
        has_body: false,
        request_id: "ts-loader".into(),
    }
}

#[tokio::test]
#[ignore = "boots V8; run with --ignored"]
async fn v8_loads_a_typescript_module() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("dep.ts"),
        "export const base: number = 40;\n",
    )
    .unwrap();
    let entry = dir.path().join("entry.ts");
    std::fs::write(
        &entry,
        "import { base } from './dep.ts';\n\
         interface P { n: number }\n\
         export function compute(p: P): { total: number } {\n\
           const x: number = base + p.n;\n\
           return { total: x };\n\
         }\n",
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
            call_input(serde_json::json!({ "n": 2 })),
        )
        .await
        .expect("a .ts module must load and run");
    let bytes = s.collect().await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["total"], 42, "typed TS ran: {v}");
}

#[tokio::test]
#[ignore = "boots V8; run with --ignored"]
async fn v8_reports_a_typescript_syntax_error_clearly() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("bad.ts");
    std::fs::write(&entry, "export function broken( {\n").unwrap();

    let pool = V8Host::new().render_pool(PoolConfig::default()).unwrap();
    let err = match pool
        .call(
            EntryRef {
                kind: "render".into(),
                module: entry.to_string_lossy().to_string(),
                export: "broken".into(),
            },
            call_input(serde_json::json!({})),
        )
        .await
    {
        Ok(_) => panic!("a syntax error must fail the load"),
        Err(e) => e,
    };
    // The loader's error names the file; the failure is not a silent success.
    assert!(
        err.0.message.contains("bad.ts") || err.0.message.contains("lower"),
        "unhelpful error: {}",
        err.0.message
    );
}

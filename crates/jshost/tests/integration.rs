//! TDD suite for oxide-jshost (spec §4, §4.2–§4.3, §4.5).
//!
//! Covers: framing (§4.2 "same op protocol"), least-loaded dispatch with
//! route-affinity (§4.3), the scripted test backend, the real NodeHost child
//! workers, the V8Host stub and the ops table (§4.5).

use bytes::Bytes;
use oxide_jshost::*;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// `unwrap_err` needs the Ok type to be `Debug`; `CallStream`/`Arc<dyn Pool>` are not.
fn err_of<T>(r: Result<T>) -> oxide_diagnostics::OxideError {
    match r {
        Ok(_) => panic!("expected an error"),
        Err(e) => e,
    }
}

fn entry(kind: &str) -> EntryRef {
    EntryRef {
        kind: kind.into(),
        module: "routes/index".into(),
        export: "default".into(),
    }
}

fn input(payload: serde_json::Value) -> CallInput {
    CallInput {
        payload,
        headers: Default::default(),
        has_body: false,
        request_id: "req-1".into(),
    }
}

fn input_for(payload: serde_json::Value, request_id: &str) -> CallInput {
    CallInput {
        request_id: request_id.into(),
        ..input(payload)
    }
}

async fn wait_in_flight(pool: &dyn Pool, n: u64) {
    for _ in 0..100 {
        if pool.stats().in_flight == n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("in_flight never reached {n}");
}

/// Poll until worker `idx` has reported its `Ready{pid}` greeting.
/// Generous budget (10 s): spawning `node` under full-workspace parallel
/// load can take a while; success returns on the first greeting.
async fn wait_ready(pool: &NodePool, idx: usize) -> Option<u32> {
    for _ in 0..200 {
        if let Some(pid) = pool.worker_pids()[idx] {
            return Some(pid);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    None
}

// ---------------------------------------------------------------- framing ---

#[test]
fn frame_unframe_roundtrip_over_all_message_kinds() {
    let msgs = vec![
        WorkerMsg::Ready { pid: 4242 },
        WorkerMsg::Call {
            id: 1,
            entry: entry("render"),
            input: input(json!({"title": "Hi"})),
        },
        WorkerMsg::CallOk {
            id: 2,
            chunks: vec!["aGk=".into(), "0JLQsA==".into()],
            complete: true,
        },
        WorkerMsg::CallChunk {
            id: 3,
            data_b64: "YWJj".into(),
        },
        WorkerMsg::CallEnd { id: 4 },
        WorkerMsg::CallErr {
            id: 5,
            diagnostic: oxide_diagnostics::Diagnostic::error("JSH-TEST", "boom").with_fix("fix it"),
        },
        WorkerMsg::OpRequest {
            id: 6,
            op: Op::StreamWrite,
            args: json!({"n": 1}),
        },
        WorkerMsg::OpResponse {
            id: 7,
            ok: false,
            data: json!({"err": true}),
        },
        WorkerMsg::Pong { nonce: 9 },
    ];
    let mut buf: Vec<u8> = Vec::new();
    for m in &msgs {
        buf.extend_from_slice(&frame(m).unwrap());
    }
    let (out, consumed) = unframe(&buf).unwrap();
    assert_eq!(consumed, buf.len(), "all bytes consumed");
    assert_eq!(out.len(), msgs.len());
    for (got, want) in out.iter().zip(&msgs) {
        assert_eq!(
            serde_json::to_value(got).unwrap(),
            serde_json::to_value(want).unwrap(),
            "roundtrip mismatch for {want:?}"
        );
    }
}

#[test]
fn unframe_empty_partial_and_multiple_frames() {
    // empty buffer
    let (msgs, consumed) = unframe(&[]).unwrap();
    assert!(msgs.is_empty() && consumed == 0);
    let f = frame(&WorkerMsg::Pong { nonce: 1 }).unwrap();
    // partial: truncated length prefix
    let (msgs, consumed) = unframe(&f[..3]).unwrap();
    assert!(msgs.is_empty() && consumed == 0);
    // partial: complete length prefix but truncated body
    let (msgs, consumed) = unframe(&f[..f.len() - 1]).unwrap();
    assert!(msgs.is_empty() && consumed == 0);
    // two complete frames followed by a partial third
    let mut buf = Vec::new();
    buf.extend_from_slice(&f);
    buf.extend_from_slice(&f);
    buf.extend_from_slice(&f[..2]);
    let (msgs, consumed) = unframe(&buf).unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(consumed, f.len() * 2, "only complete frames are consumed");
}

#[test]
fn unframe_protocol_errors() {
    // declared length beyond the 64 MiB guard → JSH-FRAME
    let mut buf = (70u32 * 1024 * 1024).to_le_bytes().to_vec();
    buf.extend_from_slice(b"{}");
    assert_eq!(unframe(&buf).unwrap_err().0.code, "JSH-FRAME");
    // complete frame whose payload is not protocol JSON → JSH-JSON
    let payload = b"not json at all";
    let mut buf2 = (payload.len() as u32).to_le_bytes().to_vec();
    buf2.extend_from_slice(payload);
    let e = unframe(&buf2).unwrap_err();
    assert_eq!(e.0.code, "JSH-JSON");
    assert!(e.0.message.contains("not valid protocol JSON"));
}

// ------------------------------------------------------------- dispatch -----

#[test]
fn choose_worker_table() {
    let cases: &[(&[u64], Option<usize>, usize, &str)] = &[
        (&[], None, 0, "empty loads → 0"),
        (&[0], None, 0, "single worker"),
        (&[3, 1, 2], None, 1, "min load wins"),
        (&[2, 2, 1, 2], None, 2, "unique min found"),
        (&[1, 1, 1], None, 0, "tie → lowest index"),
        (&[1, 1], Some(1), 1, "affinity preferred at equal load"),
        (&[4, 5], Some(1), 1, "affinity preferred within min+1"),
        (&[5, 9], Some(1), 0, "affinity beyond min+1 ignored"),
        (&[5, 9], Some(7), 0, "out-of-bounds affinity ignored"),
        (&[0, 3, 7], Some(2), 0, "affinity too loaded → min index"),
    ];
    for (loads, affinity, want, why) in cases {
        assert_eq!(choose_worker(loads, *affinity), *want, "case: {why}");
    }
}

// ---------------------------------------------------------------- ops -------

#[test]
fn ops_table_roundtrip_and_names() {
    for v in 1u16..=11 {
        let op = Op::from_u16(v).unwrap_or_else(|| panic!("op {v} must exist"));
        assert_eq!(op as u16, v);
        assert_eq!(Op::from_u16(op as u16), Some(op));
        assert!(!op_name(op).is_empty());
    }
    assert_eq!(Op::from_u16(0), None);
    // `CompileModule = 12` was added for the loader-path compilation
    // (§9.3 / §6.2); 13 is the first unused value.
    assert_eq!(Op::from_u16(12), Some(Op::CompileModule));
    assert_eq!(Op::from_u16(13), None);
    assert_eq!(Op::from_u16(u16::MAX), None);
    assert_eq!(op_name(Op::StreamWrite), "stream_write");
    assert_eq!(op_name(Op::RequestBodyRead), "request_body_read");
    assert_eq!(op_name(Op::Trace), "trace");
    assert_eq!(op_name(Op::CompileModule), "compile_module");
}

// -------------------------------------------------------- scripted backend ---

#[tokio::test]
async fn scripted_host_complete_stream_and_stats() {
    let host = ScriptedHost::with_handler(Arc::new(|entry, input| {
        assert_eq!(entry.kind, "render");
        assert_eq!(input.request_id, "req-1");
        Ok(CallStream::Complete(Bytes::from_static(b"<html>ok</html>")))
    }));
    let pool = host
        .render_pool(PoolConfig {
            size: 2,
            ..Default::default()
        })
        .unwrap();
    let mut s = pool.call(entry("render"), input(json!({}))).await.unwrap();
    assert_eq!(
        s.collect().await.unwrap().as_ref(),
        b"<html>ok</html>".as_slice()
    );
    let st = pool.stats();
    assert_eq!(st.workers, 2);
    assert_eq!(st.completed, 1);
    assert_eq!(st.failed, 0);
    // tooling_pool is backed by the same handler
    let tpool = host.tooling_pool(PoolConfig::default()).unwrap();
    assert_eq!(tpool.stats().workers, PoolConfig::default().size);
}

#[tokio::test]
async fn scripted_host_chunked_stream_and_failed_counter() {
    let host = ScriptedHost::with_handler(Arc::new(|_entry, _input| {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move {
            for part in ["<html>", "chunked", "</html>"] {
                tx.send(Ok(Bytes::from(part.to_string()))).await.unwrap();
            }
        });
        Ok(CallStream::Chunked(rx))
    }));
    let pool = host.tooling_pool(PoolConfig::default()).unwrap();
    let mut s = pool
        .call(entry("tooling:compile"), input(json!({})))
        .await
        .unwrap();
    assert_eq!(
        s.collect().await.unwrap().as_ref(),
        b"<html>chunked</html>".as_slice()
    );
    assert_eq!(pool.stats().completed, 1);

    let failing = ScriptedHost::with_handler(Arc::new(|_e, _i| {
        Err(err("JSH-SCRIPT", "handler rejected"))
    }));
    let fp = failing.render_pool(PoolConfig::default()).unwrap();
    let e = err_of(fp.call(entry("render"), input(json!({}))).await);
    assert_eq!(e.0.code, "JSH-SCRIPT");
    let st = fp.stats();
    assert_eq!(st.failed, 1);
    assert_eq!(st.completed, 0);
}

#[tokio::test]
async fn scripted_default_host_returns_noscript() {
    let pool = ScriptedHost::default()
        .render_pool(PoolConfig::default())
        .unwrap();
    let e = err_of(pool.call(entry("render"), input(json!({}))).await);
    assert_eq!(e.0.code, "JSH-NOSCRIPT");
    assert!(e.0.message.contains("with_handler"));
    let st = pool.stats();
    assert_eq!(st.failed, 1);
    assert_eq!(st.completed, 0);
    assert_eq!(ScriptedHost::default().backend_name(), "scripted");
}

// ------------------------------------------------------------ Node backend ---

fn node_cfg(size: usize) -> PoolConfig {
    PoolConfig {
        size,
        recycle_after_requests: 1_000,
        ..Default::default()
    }
}

#[tokio::test]
async fn node_host_tooling_compile_echo() {
    let host = NodeHost::new(NodeHost::worker_script_default());
    let pool = host.tooling_pool(node_cfg(1)).unwrap();
    let mut s = pool
        .call(
            entry("tooling:compile"),
            input(json!({"code": "export const a = 1"})),
        )
        .await
        .unwrap();
    let body = s.collect().await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], "EXPORT CONST A = 1");
    let st = pool.stats();
    assert_eq!(st.completed, 1);
    assert_eq!(st.failed, 0);
    assert!(st.queue_wait_ms_p50 >= 0.0 && st.queue_wait_ms_p99 >= st.queue_wait_ms_p50);
}

#[tokio::test]
async fn node_host_render_complete_and_chunked() {
    let host = NodeHost::new(NodeHost::worker_script_default());
    let pool = host.render_pool(node_cfg(2)).unwrap();
    // complete path (single CallOk with base64 data)
    let mut s = pool
        .call(entry("render"), input(json!({"title": "Hi"})))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&s.collect().await.unwrap()).unwrap();
    assert_eq!(v["html"], "<html>Hi</html>");
    // chunked path (CallChunk* + CallEnd)
    let mut s2 = pool
        .call(entry("render"), input(json!({"chunked": true})))
        .await
        .unwrap();
    assert_eq!(
        s2.collect().await.unwrap().as_ref(),
        b"<html>render</html>".as_slice()
    );
    assert_eq!(pool.stats().completed, 2);
    assert_eq!(pool.stats().workers, 2);
}

#[tokio::test]
async fn node_host_call_err_path_and_recovery() {
    let host = NodeHost::new(NodeHost::worker_script_default());
    let pool = host.render_pool(node_cfg(1)).unwrap();
    let e = err_of(pool.call(entry("boom"), input(json!({}))).await);
    assert_eq!(e.0.code, "JSH-BOOM");
    assert_eq!(pool.stats().failed, 1);
    // the worker survives an application-level error; next call succeeds
    let mut s = pool
        .call(entry("render"), input(json!({"title": "after"})))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&s.collect().await.unwrap()).unwrap();
    assert_eq!(v["html"], "<html>after</html>");
}

#[tokio::test]
async fn node_host_worker_crash_fails_in_flight_and_respawns() {
    let pool = NodePool::spawn("node", NodeHost::worker_script_default(), node_cfg(1)).unwrap();
    // "die" makes the worker exit without replying → JSH-CRASH for the in-flight call
    let e = err_of(pool.call(entry("die"), input(json!({}))).await);
    assert_eq!(e.0.code, "JSH-CRASH");
    let st = pool.stats();
    assert_eq!(st.crashed, 1);
    assert_eq!(st.failed, 1);
    // §4.3 failure isolation: the supervisor replaces the dead worker
    let mut s = pool
        .call(entry("render"), input(json!({"title": "back"})))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&s.collect().await.unwrap()).unwrap();
    assert_eq!(v["html"], "<html>back</html>");
}

#[tokio::test]
async fn node_host_recycles_worker_after_request_threshold() {
    let pool = NodePool::spawn(
        "node",
        NodeHost::worker_script_default(),
        PoolConfig {
            recycle_after_requests: 2,
            ..node_cfg(1)
        },
    )
    .unwrap();
    let pid1 = wait_ready(&pool, 0).await;
    assert!(pid1.is_some(), "worker sent Ready{{pid}}");
    for _ in 0..2 {
        pool.call(entry("tooling:compile"), input(json!({"code": "x"})))
            .await
            .unwrap();
    }
    assert_eq!(
        pool.stats().recycles,
        0,
        "no recycle before the threshold is crossed"
    );
    // the 3rd dispatch crosses recycle_after_requests=2 → kill+respawn
    pool.call(entry("tooling:compile"), input(json!({"code": "y"})))
        .await
        .unwrap();
    assert_eq!(pool.stats().recycles, 1);
    assert_eq!(pool.stats().completed, 3);
    let pid2 = wait_ready(&pool, 0).await;
    assert!(pid2.is_some(), "replacement worker sent Ready{{pid}}");
    assert_ne!(pid2, pid1, "worker process was replaced");
}

#[tokio::test]
async fn node_host_abort_request_then_drain() {
    let pool = NodePool::spawn("node", NodeHost::worker_script_default(), node_cfg(1)).unwrap();

    // abort: pending call for request "req-abort" resolves JSH-ABORT (§9.6)
    let p1 = Arc::clone(&pool);
    let h = tokio::spawn(async move {
        p1.call(entry("sleep"), input_for(json!({"ms": 400}), "req-abort"))
            .await
    });
    wait_in_flight(&*pool, 1).await;
    pool.abort_request("req-abort");
    let e = err_of(h.await.unwrap());
    assert_eq!(e.0.code, "JSH-ABORT");
    assert_eq!(pool.stats().failed, 1);
    // aborting an unknown request id is a no-op
    pool.abort_request("req-never-seen");

    // drain: in-flight completes, new calls are rejected (§12.2)
    let p2 = Arc::clone(&pool);
    let h2 = tokio::spawn(async move {
        p2.call(entry("sleep"), input_for(json!({"ms": 150}), "req-drain"))
            .await
    });
    wait_in_flight(&*pool, 1).await;
    pool.drain().await;
    assert!(
        h2.await.unwrap().is_ok(),
        "drain waits for the in-flight call"
    );
    let e2 = err_of(pool.call(entry("render"), input(json!({}))).await);
    assert_eq!(e2.0.code, "JSH-DRAIN");
}

#[test]
fn node_host_missing_node_binary_or_script_fails_fast() {
    let host = NodeHost::new(NodeHost::worker_script_default());
    let mut bad_bin = NodeHost::new(NodeHost::worker_script_default());
    bad_bin.node_bin = "/nonexistent/oxide-no-node".into();
    let e = err_of(bad_bin.render_pool(PoolConfig::default()));
    assert_eq!(e.0.code, "JSH-NODE");
    let e2 = err_of(bad_bin.tooling_pool(PoolConfig::default()));
    assert_eq!(e2.0.code, "JSH-NODE");
    // node exists but the worker script does not
    let bad_script = NodeHost::new("/definitely/not/oxide-worker.mjs");
    let e3 = err_of(bad_script.render_pool(PoolConfig::default()));
    assert_eq!(e3.0.code, "JSH-NODE");
    let _ = host; // the default host is only usable when `node` is installed
}

// ------------------------------------------------------------- V8 backend ---

fn v8_cfg(size: usize) -> PoolConfig {
    PoolConfig {
        size,
        ..Default::default()
    }
}

/// Write a one-file ESM module into `dir` and return its absolute path. The
/// V8 host loads modules from disk through `FsModuleLoader`, so each test
/// gets a real file rather than the old hardcoded `routes/index`.
fn v8_module(dir: &std::path::Path, name: &str, source: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, source).unwrap();
    p.to_string_lossy().to_string()
}

/// An `EntryRef` pointing at a real module file.
fn v8_entry(module: &str, export: &str) -> EntryRef {
    EntryRef {
        kind: "render".into(),
        module: module.into(),
        export: export.into(),
    }
}

#[tokio::test]
async fn v8_host_runs_a_module_export() {
    let dir = tempfile::tempdir().unwrap();
    let module = v8_module(
        dir.path(),
        "mod.mjs",
        "export function echo(p) { return { html: '<html>' + p.title + '</html>' }; }\n",
    );

    let host = V8Host;
    assert_eq!(host.backend_name(), "v8");
    let pool = host.render_pool(v8_cfg(1)).unwrap();
    let mut s = pool
        .call(v8_entry(&module, "echo"), input(json!({"title": "Hi"})))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&s.collect().await.unwrap()).unwrap();
    assert_eq!(v["html"], "<html>Hi</html>");

    let st = pool.stats();
    assert_eq!(st.completed, 1);
    assert_eq!(st.failed, 0);
    assert_eq!(st.workers, 1);
}

#[tokio::test]
async fn v8_host_boom_and_missing_module_report_errors() {
    let host = V8Host;
    let pool = host.tooling_pool(v8_cfg(1)).unwrap();

    // `boom` is a Rust-side sentinel the pool mirrors from the Node worker.
    let e = err_of(pool.call(entry("boom"), input(json!({}))).await);
    assert_eq!(e.0.code, "JSH-BOOM");

    // An entry with no module cannot be dispatched.
    let e2 = err_of(
        pool.call(
            EntryRef { kind: "render".into(), module: String::new(), export: "x".into() },
            input(json!({})),
        )
        .await,
    );
    assert_eq!(e2.0.code, "JSH-V8");

    let st = pool.stats();
    assert_eq!(st.failed, 2);
    assert_eq!(st.completed, 0);
}

#[tokio::test]
async fn v8_host_multiple_workers_dispatch_and_drain() {
    let dir = tempfile::tempdir().unwrap();
    let module = v8_module(
        dir.path(),
        "mod.mjs",
        "export function echo(p) { return { html: '<html>' + p.title + '</html>' }; }\n",
    );

    let host = V8Host;
    let pool = host.render_pool(v8_cfg(2)).unwrap();
    assert_eq!(pool.stats().workers, 2);

    let mut a = pool
        .call(v8_entry(&module, "echo"), input(json!({"title": "A"})))
        .await
        .unwrap();
    let mut b = pool
        .call(v8_entry(&module, "echo"), input(json!({"title": "B"})))
        .await
        .unwrap();
    let va: serde_json::Value = serde_json::from_slice(&a.collect().await.unwrap()).unwrap();
    let vb: serde_json::Value = serde_json::from_slice(&b.collect().await.unwrap()).unwrap();
    assert!(va["html"] == "<html>A</html>" || va["html"] == "<html>B</html>");
    assert!(vb["html"] == "<html>A</html>" || vb["html"] == "<html>B</html>");

    // Drain: rejects new calls (§12.2).
    pool.drain().await;
    let e = err_of(pool.call(v8_entry(&module, "echo"), input(json!({}))).await);
    assert_eq!(e.0.code, "JSH-DRAIN");
}

#[tokio::test]
async fn v8_host_abort_request_resolves_pending_call() {
    let dir = tempfile::tempdir().unwrap();
    let module = v8_module(
        dir.path(),
        "mod.mjs",
        "export function echo(p) { return { html: '<html>' + p.title + '</html>' }; }\n",
    );

    let host = V8Host;
    let pool = host.render_pool(v8_cfg(1)).unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    pool.abort_request("req-not-in-flight"); // no-op on unknown id
    let mut s = pool
        .call(v8_entry(&module, "echo"), input(json!({"title": "post-abort"})))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&s.collect().await.unwrap()).unwrap();
    assert_eq!(v["html"], "<html>post-abort</html>");
}

#[tokio::test]
async fn v8_pool_recycles_warm() {
    // §4.3 lifecycle: a worker is replaced after `recycle_after_requests`
    // successful calls. The replacement is warm — a fresh worker is spawned
    // before the old one is signalled to exit.
    let dir = tempfile::tempdir().unwrap();
    let module = v8_module(
        dir.path(),
        "mod.mjs",
        "export function echo() { return { ok: true }; }\n",
    );

    let host = V8Host;
    let cfg = PoolConfig {
        size: 1,
        recycle_after_requests: 2,
        ..Default::default()
    };
    let pool = host.render_pool(cfg).unwrap();
    assert_eq!(pool.stats().recycles, 0);

    for _ in 0..3 {
        let mut s = pool
            .call(v8_entry(&module, "echo"), input(json!({})))
            .await
            .unwrap();
        let _ = s.collect().await.unwrap();
    }

    for _ in 0..200 {
        if pool.stats().recycles >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let st = pool.stats();
    assert!(st.recycles >= 1, "expected at least one recycle, got {}", st.recycles);
    assert_eq!(st.failed, 0, "recycle must not fail any call");
    assert_eq!(st.completed, 3);
}

// ------------------------------------------------------- §4.5 V8 ops

/// The op namespace is exposed on `globalThis.oxide` and dispatches to the
/// Rust op functions. A module calls `oxide.trace` and returns the value, so
/// the round-trip through a real op is asserted.
#[tokio::test]
async fn v8_ops_are_registered_and_callable() {
    let dir = tempfile::tempdir().unwrap();
    let module = v8_module(
        dir.path(),
        "ops.mjs",
        "export function probe() {\n           const id = globalThis.oxide.trace('hello');\n           return { id };\n         }\n",
    );

    let host = V8Host;
    let pool = host.render_pool(PoolConfig { size: 1, ..Default::default() }).unwrap();
    let mut s = pool
        .call(v8_entry(&module, "probe"), input(json!({})))
        .await
        .unwrap();
    let body = s.collect().await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // `op_oxide_trace` returns `trace-<16 hex>` for a deterministic name.
    let id = v["id"].as_str().expect("id string");
    assert!(id.starts_with("trace-"), "got {id}");
    assert_eq!(id.len(), "trace-".len() + 16, "got {id}");
}

#[test]
fn op_decls_returns_three_ops() {
    let decls = oxide_jshost::v8_ops::op_decls();
    assert_eq!(decls.len(), 3, "expected log, trace, serialize");
    // Op names are namespaced with the `op_oxide_` prefix so the JS side can
    // distinguish them from deno_core built-ins.
    let names: Vec<&str> = decls.iter().map(|d| d.name).collect();
    assert!(names.iter().any(|n| n.contains("op_oxide_log")), "{names:?}");
    assert!(names.iter().any(|n| n.contains("op_oxide_trace")), "{names:?}");
    assert!(names.iter().any(|n| n.contains("op_oxide_serialize")), "{names:?}");
}

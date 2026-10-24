//! BUG-07: V8 prelude correctness — stream-safe TextDecoder, macrotask
//! MessagePort, robust setInterval.
//!
//! Ignored by default: boots a V8 isolate. Run with `--ignored`.
//! deno_core ships no native TextDecoder (it lives in deno_web, which the
//! oxide extension does not include), so `new TextDecoder()` answers with
//! the bootstrap shim — these tests exercise the live path, including the
//! exact chunked-decode shape `__oxideFrameworkRender` uses.

use oxide_fw_jshost::{CallInput, EntryRef, JsHost, Pool, PoolConfig, V8Host};

fn input(payload: serde_json::Value) -> CallInput {
    CallInput {
        payload,
        headers: Default::default(),
        has_body: false,
        request_id: "v8-bug07".into(),
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

async fn run(pool: &std::sync::Arc<dyn Pool>, module: &str, export: &str) -> serde_json::Value {
    let mut s = pool
        .call(
            EntryRef {
                kind: "probe".into(),
                module: module.into(),
                export: export.into(),
            },
            input(serde_json::json!({})),
        )
        .await
        .expect("call");
    let bytes = s.collect().await.expect("collect");
    serde_json::from_slice(&bytes).unwrap()
}

const PRELUDE_MOD: &str = r#"
// '€' is E2 82 AC — split across two chunks, as a streamed render does.
export function splitEuro() {
  const d = new TextDecoder();
  const a = d.decode(new Uint8Array([0xE2, 0x82]), { stream: true });
  const b = d.decode(new Uint8Array([0xAC]), { stream: true });
  const c = d.decode();
  return { a, b, c };
}
export function splitEuroNoStream() {
  // Without {stream:true} each decode stands alone: the split char is lost.
  const d = new TextDecoder();
  const a = d.decode(new Uint8Array([0xE2, 0x82]));
  const b = d.decode(new Uint8Array([0xAC]));
  return { a, b };
}
export async function messagePortRoundTrip() {
  const { port1, port2 } = new MessageChannel();
  const received = [];
  port1.onmessage = (ev) => received.push(ev.data);
  port2.postMessage('one');
  port2.postMessage('two');
  await new Promise((r) => setTimeout(r, 50));
  return { received };
}
export async function messagePortYieldsToTimer() {
  // Macrotask delivery must not starve a pending timer: both fire.
  const { port1, port2 } = new MessageChannel();
  let timerFired = false;
  let msgFired = false;
  setTimeout(() => { timerFired = true; }, 0);
  port1.onmessage = () => { msgFired = true; };
  port2.postMessage('x');
  await new Promise((r) => setTimeout(r, 50));
  return { timerFired, msgFired };
}
export async function intervalSurvivesThrow() {
  let count = 0;
  const id = setInterval(() => {
    count += 1;
    if (count === 2) throw new Error('tick-two');
  }, 5);
  await new Promise((r) => setTimeout(r, 60));
  clearInterval(id);
  const atClear = count;
  await new Promise((r) => setTimeout(r, 40));
  return { ticks: atClear, afterClear: count };
}
"#;

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn v8_text_decoder_streams_split_multibyte() {
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("prelude.mjs");
    std::fs::write(&module, PRELUDE_MOD).unwrap();
    let m = module.to_string_lossy().to_string();
    let pool = pool1();

    let v = run(&pool, &m, "splitEuro").await;
    assert_eq!(v["a"], "", "partial char held back, not mojibake: {v}");
    assert_eq!(v["b"], "€", "split € reassembled across chunks: {v}");
}

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn v8_message_port_delivers_async_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("prelude.mjs");
    std::fs::write(&module, PRELUDE_MOD).unwrap();
    let m = module.to_string_lossy().to_string();
    let pool = pool1();

    let v = run(&pool, &m, "messagePortRoundTrip").await;
    assert_eq!(v["received"], serde_json::json!(["one", "two"]));

    let v = run(&pool, &m, "messagePortYieldsToTimer").await;
    assert_eq!(v["timerFired"], true, "timer not starved: {v}");
    assert_eq!(v["msgFired"], true, "message delivered: {v}");
}

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn v8_interval_survives_throw_and_stops_on_clear() {
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("prelude.mjs");
    std::fs::write(&module, PRELUDE_MOD).unwrap();
    let m = module.to_string_lossy().to_string();
    let pool = pool1();

    let v = run(&pool, &m, "intervalSurvivesThrow").await;
    let ticks = v["ticks"].as_u64().unwrap();
    assert!(ticks >= 3, "chain survived the throw (ticks={ticks}): {v}");
    assert_eq!(
        v["afterClear"], v["ticks"],
        "no ticks after clearInterval: {v}"
    );
    // The worker is still alive (a throwing interval must not crash it).
    let v2 = run(&pool, &m, "splitEuroNoStream").await;
    assert!(
        v2.get("a").is_some(),
        "worker alive after interval test: {v2}"
    );
}

// ---------------------------------------------------------------------------
// §9.3 / P0-4: the V8 prelude must expose `__oxide_op_request` so a
// `'use cache'` function (and `cache()`) can reach the host cache tier.
// Without it, `'use cache'` under `backend=v8` silently degraded to an
// in-process Map — the entry never reached the tiered cache.
// ---------------------------------------------------------------------------

const OP_BRIDGE_MOD: &str = r#"
export function bridgePresent() {
  return {
    isFunction: typeof globalThis.__oxide_op_request === 'function',
  };
}
export async function cacheRoundTrip() {
  // No cache handler is wired in this test, so `cache_set` answers false and
  // `cache_get` answers null — the point is that the bridge EXISTS and the
  // ops are reachable (not that a handler is present).
  const setOk = await globalThis.__oxide_op_request('cache_set', {
    key: 'k1',
    status: 200,
    body_b64: 'T0RGMQAK',
    tags: ['t'],
  });
  const got = await globalThis.__oxide_op_request('cache_get', { key: 'k1' });
  return { setOk, got };
}
export async function unknownOpRejects() {
  try {
    await globalThis.__oxide_op_request('nope', {});
    return { threw: false };
  } catch (e) {
    return { threw: true, message: String(e && e.message) };
  }
}
"#;

fn write_probe_module(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("oxide-v8-probe-{}-{}", std::process::id(), name));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("mod.ts");
    let _ = std::fs::write(&p, OP_BRIDGE_MOD);
    p
}

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn v8_prelude_defines_the_op_request_bridge() {
    let pool = pool1();
    let path = write_probe_module("bridge");
    let v = run(&pool, path.to_str().unwrap(), "bridgePresent").await;
    assert_eq!(
        v["isFunction"], true,
        "globalThis.__oxide_op_request must be a function on the V8 backend: {v}"
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn v8_op_request_reaches_the_cache_ops() {
    let pool = pool1();
    let path = write_probe_module("cache");
    let v = run(&pool, path.to_str().unwrap(), "cacheRoundTrip").await;
    // No handler wired: set answers false, get answers null. Both prove the
    // bridge routed to the op (a missing op would have thrown).
    assert_eq!(v["setOk"], false, "no handler -> cache_set is false: {v}");
    assert!(v["got"].is_null(), "no handler -> cache_get is null: {v}");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[tokio::test]
#[ignore = "boots a V8 isolate; run with --ignored"]
async fn v8_op_request_rejects_an_unknown_op() {
    let pool = pool1();
    let path = write_probe_module("unknown");
    let v = run(&pool, path.to_str().unwrap(), "unknownOpRejects").await;
    assert_eq!(v["threw"], true, "an unknown op must reject: {v}");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

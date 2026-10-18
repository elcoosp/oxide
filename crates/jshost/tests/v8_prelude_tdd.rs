//! BUG-07: V8 prelude correctness — stream-safe TextDecoder, macrotask
//! MessagePort, robust setInterval.
//!
//! Ignored by default: boots a V8 isolate. Run with `--ignored`.
//! deno_core ships no native TextDecoder (it lives in deno_web, which the
//! oxide extension does not include), so `new TextDecoder()` answers with
//! the bootstrap shim — these tests exercise the live path, including the
//! exact chunked-decode shape `__oxideFrameworkRender` uses.

use oxide_jshost::{CallInput, EntryRef, JsHost, Pool, PoolConfig, V8Host};

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

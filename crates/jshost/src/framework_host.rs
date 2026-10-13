//! Framework adapter host (spec §7, §6.3).
//!
//! Loads a user's oxide.config.ts through the JS worker, extracts the
//! `FrameworkAdapter` the config exports (the value passed to
//! `defineConfig({ framework: ... })`), and dispatches `render()` calls
//! through the same worker. This is the Rust side of the JS↔Rust contract
//! documented in `packages/oxide/src/server/README.md`.
//!
//! Two ops, both routed through a `Pool` (spec §4.2):
//!
//! | op                 | payload                 | response                                  |
//! |--------------------|-------------------------|-------------------------------------------|
//! | `framework:load`   | `{ path }`              | `{ name, extensions, capabilities }`      |
//! | `framework:render` | `{ path, input }`       | `{ html }`                                |
//!
//! The worker dynamic-imports the config file (`.ts` or `.mjs`), evaluates
//! the module, and either returns a §7 descriptor (load) or calls
//! `adapter.render(input)` and drains the returned `ReadableStream<Uint8Array>`
//! into one HTML string (render). This is deliberately the "collect the
//! whole stream" path — chunked streaming through the worker is a follow-up.
//!
//! Errors surface as `JSH-FRAMEWORK` diagnostics with the worker's message
//! attached. Any adapter-side throw (a page component that panics, a missing
//! framework dep, invalid JSON in the response) becomes a `JSH-FRAMEWORK`
//! error the caller can render as an error boundary (§10.2 step 5).

use crate::{CallInput, CallStream, EntryRef, Pool, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Descriptor for a loaded FrameworkAdapter — the subset of §7 the Rust
/// pipeline needs before it dispatches a render call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FrameworkDescriptor {
    /// Adapter name (`react` / `vue` / `svelte` / `solid`).
    pub name: String,
    /// File extensions the adapter claims (`.tsx`, `.vue`, `.svelte`, …).
    pub extensions: Vec<String>,
    /// The `capabilities` sub-object from §7 (`suspense`, `islands`, `hmr`,
    /// optional `rsc?`). Kept as `Value` so new fields flow through without
    /// a schema change here.
    pub capabilities: Value,
}

/// Host wrapper around a `Pool` for the two framework-adapter ops.
pub struct FrameworkHost {
    pool: Arc<dyn Pool>,
}

impl FrameworkHost {
    pub fn new(pool: Arc<dyn Pool>) -> Self {
        Self { pool }
    }

    /// Load `config_path`, evaluate it in the worker, extract the framework
    /// adapter, and return its descriptor.
    ///
    /// # Errors
    ///
    /// - `JSH-FRAMEWORK`: the worker reported an error (bad path, missing
    ///   `framework` export, failed dynamic import, etc.).
    /// - `JSH-FRAMEWORK`: the worker's response was not valid JSON or did not
    ///   match `FrameworkDescriptor`.
    pub async fn load_config(&self, config_path: &str) -> Result<FrameworkDescriptor> {
        let input = CallInput {
            payload: serde_json::json!({ "path": config_path }),
            headers: Default::default(),
            has_body: false,
            request_id: format!("framework:load:{config_path}"),
        };
        let entry = EntryRef {
            kind: "framework:load".into(),
            module: String::new(),
            export: String::new(),
        };
        let mut stream = self.pool.call(entry, input).await?;
        let bytes = stream.collect().await?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| {
            crate::err(
                "JSH-FRAMEWORK",
                format!("framework:load returned invalid JSON: {e}"),
            )
        })?;
        if let Some(err_msg) = v.get("__oxide_error").and_then(|x| x.as_str()) {
            return Err(crate::err("JSH-FRAMEWORK", err_msg.to_string()));
        }
        let descriptor: FrameworkDescriptor = serde_json::from_value(v).map_err(|e| {
            crate::err(
                "JSH-FRAMEWORK",
                format!("framework descriptor shape unexpected: {e}"),
            )
        })?;
        Ok(descriptor)
    }

    /// Render `input` through the adapter the config at `config_path` exports.
    /// The worker drains the adapter's `ReadableStream<Uint8Array>` and returns
    /// the HTML as a UTF-8 string (chunked streaming is a follow-up).
    ///
    /// # Errors
    ///
    /// - `JSH-FRAMEWORK`: the worker reported an error, the response was not
    ///   valid JSON, or the response was missing its `html` field.
    pub async fn render(&self, config_path: &str, input: Value) -> Result<String> {
        let call_input = CallInput {
            payload: serde_json::json!({ "path": config_path, "input": input }),
            headers: Default::default(),
            has_body: false,
            request_id: format!("framework:render:{config_path}"),
        };
        let entry = EntryRef {
            kind: "framework:render".into(),
            module: String::new(),
            export: String::new(),
        };
        let mut stream = self.pool.call(entry, call_input).await?;
        let bytes = stream.collect().await?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| {
            crate::err(
                "JSH-FRAMEWORK",
                format!("framework:render returned invalid JSON: {e}"),
            )
        })?;
        if let Some(err_msg) = v.get("__oxide_error").and_then(|x| x.as_str()) {
            return Err(crate::err("JSH-FRAMEWORK", err_msg.to_string()));
        }
        v.get("html")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                crate::err("JSH-FRAMEWORK", "framework:render response missing `html`")
            })
    }

    /// Streaming variant of [`render`]: hands back the raw [`CallStream`]
    /// from the pool without collecting it. This is the low-level primitive
    /// a hyper-body writer wants — pull a chunk, write it, pull the next.
    ///
    /// # Wire shape
    ///
    /// The worker's `framework:render` op emits `call_chunk` frames whose
    /// concatenated base64 payloads decode to a JSON object `{"html": "..."}`.
    /// The `CallStream::Chunked` variant this returns has already decoded
    /// base64; iterating it yields byte chunks of that JSON envelope, not
    /// HTML directly. A caller that wants the HTML string should still call
    /// [`render`]. A caller that wants to write bytes to an HTTP response as
    /// they arrive can consume this stream and remember that the *whole*
    /// envelope must be reassembled before it parses.
    ///
    /// # Contract
    ///
    /// Callers **must** drain the stream to EOF. Leaving it un-drained keeps
    /// the channel alive, holding a worker slot in the pool.
    pub async fn render_streaming(
        &self,
        config_path: &str,
        input: Value,
    ) -> Result<CallStream> {
        let call_input = CallInput {
            payload: serde_json::json!({ "path": config_path, "input": input }),
            headers: Default::default(),
            has_body: false,
            request_id: format!("framework:render:{config_path}"),
        };
        let entry = EntryRef {
            kind: "framework:render".into(),
            module: String::new(),
            export: String::new(),
        };
        self.pool.call(entry, call_input).await
    }

    /// Bump the worker's module-generation counter (spec §13.3). Every
    /// subsequent `import()` in the worker is tagged with `?v=<gen>`, so Node
    /// re-evaluates cached modules. The dev server calls this when a watched
    /// source file changes; the next render or loader call sees the new code.
    ///
    /// Returns the new generation. Idempotent from the caller's perspective
    /// (each call bumps once); cost is one round-trip to the worker.
    pub async fn invalidate(&self) -> Result<u64> {
        let input = CallInput {
            payload: serde_json::json!({}),
            headers: Default::default(),
            has_body: false,
            request_id: "framework:invalidate".to_string(),
        };
        let entry = EntryRef {
            kind: "framework:invalidate".into(),
            module: String::new(),
            export: String::new(),
        };
        let mut stream = self.pool.call(entry, input).await?;
        let bytes = stream.collect().await?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|e| {
            crate::err(
                "JSH-FRAMEWORK",
                format!("framework:invalidate returned invalid JSON: {e}"),
            )
        })?;
        if let Some(err_msg) = v.get("__oxide_error").and_then(|x| x.as_str()) {
            return Err(crate::err("JSH-FRAMEWORK", err_msg.to_string()));
        }
        v.get("generation")
            .and_then(|x| x.as_u64())
            .ok_or_else(|| {
                crate::err(
                    "JSH-FRAMEWORK",
                    "framework:invalidate response missing `generation`",
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CallStream, JsHost, PoolConfig, ScriptedHost};
    use bytes::Bytes;
    use serde_json::json;

    /// A ScriptedHost that mimics the worker's responses for both ops.
    fn host_with(kind_responses: impl Fn(&str, &Value) -> Value + Send + Sync + 'static) -> Arc<dyn Pool> {
        let host = ScriptedHost::with_handler(Arc::new(move |entry, input| {
            let v = kind_responses(&entry.kind, &input.payload);
            let bytes = serde_json::to_vec(&v).unwrap();
            Ok(CallStream::Complete(Bytes::from(bytes)))
        }));
        host.tooling_pool(PoolConfig::default()).unwrap()
    }

    #[tokio::test]
    async fn load_config_round_trips_descriptor() {
        let pool = host_with(|kind, payload| {
            assert_eq!(kind, "framework:load");
            assert_eq!(payload["path"], "/abs/oxide.config.ts");
            json!({
                "name": "react",
                "extensions": [".tsx"],
                "capabilities": { "suspense": "native", "islands": true, "hmr": "refresh" }
            })
        });
        let h = FrameworkHost::new(pool);
        let d = h.load_config("/abs/oxide.config.ts").await.unwrap();
        assert_eq!(d.name, "react");
        assert_eq!(d.extensions, vec![".tsx"]);
        assert_eq!(d.capabilities["suspense"], "native");
        assert_eq!(d.capabilities["islands"], true);
    }

    #[tokio::test]
    async fn load_config_surfaces_worker_error() {
        let pool = host_with(|_kind, _payload| {
            json!({ "__oxide_error": "config at /x has no framework export" })
        });
        let h = FrameworkHost::new(pool);
        let err = h.load_config("/x").await.unwrap_err();
        assert_eq!(err.0.code, "JSH-FRAMEWORK");
        assert!(err.0.message.contains("no framework export"), "{}", err.0.message);
    }

    #[tokio::test]
    async fn render_returns_html() {
        let pool = host_with(|kind, payload| {
            assert_eq!(kind, "framework:render");
            assert_eq!(payload["path"], "/abs/oxide.config.ts");
            assert_eq!(payload["input"]["route"], "routes/index");
            json!({ "html": "<!doctype html><html><body>Hello</body></html>" })
        });
        let h = FrameworkHost::new(pool);
        let html = h
            .render(
                "/abs/oxide.config.ts",
                json!({ "route": "routes/index", "url": "/", "params": {}, "data": {}, "head": {} }),
            )
            .await
            .unwrap();
        assert!(html.contains("Hello"));
        assert!(html.starts_with("<!doctype html>"));
    }

    #[tokio::test]
    async fn render_surfaces_adapter_throw_as_jsh_framework() {
        let pool = host_with(|_kind, _payload| {
            json!({ "__oxide_error": "framework:render failed: Boom is not a function" })
        });
        let h = FrameworkHost::new(pool);
        let err = h.render("/x", json!({})).await.unwrap_err();
        assert_eq!(err.0.code, "JSH-FRAMEWORK");
        assert!(err.0.message.contains("Boom"), "{}", err.0.message);
    }

    /// Build a pool whose `framework:render` returns a `CallStream::Chunked`
    /// over the given byte chunks.
    ///
    /// The chunk `Vec<Vec<u8>>` is captured by move; each call to the
    /// scripted handler re-sends the same chunk sequence over a fresh
    /// channel. `try_send` with a buffer of 16 is enough for the small
    /// fixtures these tests use (3-4 chunks), so the sync handler completes
    /// without needing an async context.
    ///
    /// Note: previous drafts of this helper took `Vec<&'static [u8]>` and
    /// constructed the chunks from raw string literals — that's a footgun
    /// because `br#"..."#`'s closing delimiter is `"#`, so any literal that
    /// ends in `"` loses that character. Tests build the chunk bytes via
    /// `serde_json::to_vec` instead, which sidesteps the issue entirely.
    fn chunked_pool(chunks: Vec<Vec<u8>>) -> Arc<dyn Pool> {
        let host = ScriptedHost::with_handler(Arc::new(move |_entry, _input| {
            let (tx, rx) = tokio::sync::mpsc::channel::<crate::Result<Bytes>>(16);
            for c in &chunks {
                tx.try_send(Ok(Bytes::copy_from_slice(c)))
                    .expect("chunk fits in buffer");
            }
            drop(tx); // closes the channel; rx yields all buffered items then None
            Ok(CallStream::Chunked(rx))
        }));
        host.tooling_pool(PoolConfig::default()).unwrap()
    }

    /// Split a JSON `Value` into three chunks by byte length, use the pool,
    /// and return the chunks so tests can also assert on the exact split.
    /// Split points are chosen to fall mid-token (arbitrary), which is the
    /// interesting case for a stream chunker.
    fn split3(bytes: &[u8]) -> Vec<Vec<u8>> {
        // Split at ~1/3 and ~2/3 of the byte length; if the input is too
        // short, still return three chunks (the middle may be empty).
        let n = bytes.len();
        let a = n / 3;
        let b = (2 * n) / 3;
        vec![
            bytes[..a].to_vec(),
            bytes[a..b].to_vec(),
            bytes[b..].to_vec(),
        ]
    }

    #[tokio::test]
    async fn render_streaming_preserves_chunk_boundaries() {
        // Build the payload through serde so the byte content is exactly
        // what `serde_json::from_slice` expects; then split it into 3
        // arbitrary byte-slices and stream them.
        let payload = serde_json::to_vec(&json!({
            "html": "<h1>Chunk</h1>"
        }))
        .unwrap();
        let expected = payload.clone();
        let chunks = split3(&payload);
        let pool = chunked_pool(chunks.clone());
        let h = FrameworkHost::new(pool);
        let mut s = h
            .render_streaming("/abs/config.mjs", json!({}))
            .await
            .unwrap();

        match &mut s {
            CallStream::Chunked(rx) => {
                let c1 = rx.recv().await.unwrap().unwrap();
                let c2 = rx.recv().await.unwrap().unwrap();
                let c3 = rx.recv().await.unwrap().unwrap();
                assert_eq!(c1.as_ref(), chunks[0].as_slice());
                assert_eq!(c2.as_ref(), chunks[1].as_slice());
                assert_eq!(c3.as_ref(), chunks[2].as_slice());
                assert!(rx.recv().await.is_none(), "channel closed after 3 chunks");

                // Reassemble and prove it parses to the original JSON.
                let mut buf = Vec::new();
                buf.extend_from_slice(&c1);
                buf.extend_from_slice(&c2);
                buf.extend_from_slice(&c3);
                assert_eq!(buf, expected, "reassembled bytes match the serialization");
                let v: Value = serde_json::from_slice(&buf).unwrap();
                assert_eq!(v["html"], "<h1>Chunk</h1>");
            }
            other => panic!("expected Chunked, got {:?}", std::mem::discriminant(other)),
        }
    }

    #[tokio::test]
    async fn render_streaming_complete_variant_passes_through() {
        let pool = host_with(|_kind, _payload| {
            json!({ "html": "<h1>complete</h1>" })
        });
        let h = FrameworkHost::new(pool);
        let mut s = h
            .render_streaming("/abs/config.mjs", json!({}))
            .await
            .unwrap();
        match &mut s {
            CallStream::Complete(bytes) => {
                let v: Value = serde_json::from_slice(bytes).unwrap();
                assert_eq!(v["html"], "<h1>complete</h1>");
            }
            other => panic!("expected Complete, got {:?}", std::mem::discriminant(other)),
        }
    }

    #[tokio::test]
    async fn render_streaming_payload_error_visible_to_caller() {
        // `render_streaming` does not detect `__oxide_error`; a caller that
        // reassembles and parses gets the error payload verbatim. This test
        // pins the contract: the streaming API is raw, parsing is the
        // caller's job.
        let payload = serde_json::to_vec(&json!({
            "__oxide_error": "boom",
            "hint": "check config"
        }))
        .unwrap();
        let pool = chunked_pool(split3(&payload));
        let h = FrameworkHost::new(pool);
        let mut s = h
            .render_streaming("/x", json!({}))
            .await
            .unwrap();
        let bytes = s.collect().await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["__oxide_error"], "boom");
        assert_eq!(v["hint"], "check config");
    }

    #[tokio::test]
    async fn render_still_works_on_chunked_pool() {
        // Regression: `render` (which collects) must keep working when the
        // pool returns a Chunked stream — the L4 change made that the
        // default worker behaviour.
        let payload = serde_json::to_vec(&json!({
            "html": "<h1>full</h1>"
        }))
        .unwrap();
        let pool = chunked_pool(split3(&payload));
        let h = FrameworkHost::new(pool);
        let html = h.render("/x", json!({})).await.unwrap();
        assert_eq!(html, "<h1>full</h1>");
    }

    #[tokio::test]
    async fn invalidate_bumps_generation() {
        let pool = host_with(|kind, payload| {
            assert_eq!(kind, "framework:invalidate");
            assert!(payload.is_object(), "payload: {payload}");
            json!({ "generation": 1 })
        });
        let h = FrameworkHost::new(pool);
        let generation = h.invalidate().await.unwrap();
        assert_eq!(generation, 1);
    }

    #[tokio::test]
    async fn invalidate_surfaces_worker_error() {
        let pool = host_with(|_kind, _payload| {
            json!({ "__oxide_error": "invalidate not supported" })
        });
        let h = FrameworkHost::new(pool);
        let err = h.invalidate().await.unwrap_err();
        assert_eq!(err.0.code, "JSH-FRAMEWORK");
        assert!(err.0.message.contains("not supported"), "{}", err.0.message);
    }

    #[tokio::test]
    async fn invalidate_rejects_missing_generation() {
        let pool = host_with(|_kind, _payload| json!({}));
        let h = FrameworkHost::new(pool);
        let err = h.invalidate().await.unwrap_err();
        assert_eq!(err.0.code, "JSH-FRAMEWORK");
        assert!(err.0.message.contains("missing `generation`"));
    }

    #[tokio::test]
    async fn render_rejects_response_missing_html() {
        let pool = host_with(|_kind, _payload| json!({ "body": "oops" }));
        let h = FrameworkHost::new(pool);
        let err = h.render("/x", json!({})).await.unwrap_err();
        assert_eq!(err.0.code, "JSH-FRAMEWORK");
        assert!(err.0.message.contains("missing `html`"), "{}", err.0.message);
    }
}

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

use crate::{CallInput, EntryRef, Pool, Result};
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

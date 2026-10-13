//! Typed renderer facade over `FrameworkHost` (spec §7).
//!
//! `FrameworkHost` talks JSON to the Node worker. This module adds a typed
//! layer the pipeline can hold as a single `Arc<dyn FrameworkRenderer>`:
//!
//! - [`FrameworkRenderInput`] is the shape the pipeline fills per request.
//! - [`FrameworkRenderer`] is the trait the pipeline depends on (so tests
//!   can use a fake without Node).
//! - [`FrameworkHostRenderer`] eagerly loads the config descriptor at
//!   construction, then exposes `render()` for as long as it lives — so a
//!   long-running server pays the config-load cost once.
//!
//! This is deliberately the "collect the whole stream" path: the trait's
//! `render` returns one `String`. Streaming through the pool (chunked reads
//! so SSR HTML flushes as it's produced) is a follow-up.

use crate::framework_host::{FrameworkDescriptor, FrameworkHost};
use crate::{Pool, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// The per-request shape the pipeline hands to a renderer (spec §7 RenderInput).
///
/// Fields map 1:1 to the JS-side `RenderInput` in packages/oxide/src/types.ts,
/// minus the optional `odf` (a serialized byte blob the JS side accepts but
/// does not require for a first render). `head` and `data` stay as `Value`
/// because the pipeline doesn't own the head-model shape and loader output
/// can be any ODF-decoded tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FrameworkRenderInput {
    /// Matched route id, e.g. `routes/blog/[slug]`.
    pub route: String,
    /// Request URL, path + query.
    pub url: String,
    /// URL params (path segments) as a JSON object.
    pub params: Value,
    /// Loader data (ODF-decoded, JS-side shape).
    pub data: Value,
    /// Neutral head model (spec §16.3) as a JSON object.
    pub head: Value,
}

impl FrameworkRenderInput {
    /// Convenience for a bare input with empty params/data/head.
    pub fn new(route: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            route: route.into(),
            url: url.into(),
            params: Value::Object(Default::default()),
            data: Value::Object(Default::default()),
            head: serde_json::json!({
                "meta": [], "link": [], "style": [], "script": []
            }),
        }
    }

    /// Serialize into the JSON payload the JS-side worker expects.
    fn to_payload(&self) -> Value {
        serde_json::json!({
            "route": self.route,
            "url": self.url,
            "params": self.params,
            "data": self.data,
            "head": self.head,
        })
    }
}

/// A typed renderer the pipeline depends on. Object-safe; the pipeline
/// holds an `Arc<dyn FrameworkRenderer>` behind an `Option` so builds with
/// no framework adapter configured keep the IR path.
#[async_trait]
pub trait FrameworkRenderer: Send + Sync {
    /// Adapter name (`react` / `vue` / `svelte` / `solid`); surfaced for
    /// inspector output and log lines.
    fn name(&self) -> &str;
    /// Render one route to HTML.
    async fn render(&self, input: FrameworkRenderInput) -> Result<String>;
}

/// `FrameworkHostRenderer` wraps a `FrameworkHost` + a cached config path and
/// descriptor. Construct once per process; call `render` per request.
pub struct FrameworkHostRenderer {
    host: FrameworkHost,
    config_path: String,
    descriptor: FrameworkDescriptor,
}

impl std::fmt::Debug for FrameworkHostRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The pool is a trait object that doesn't promise Debug; we print
        // the fields that matter for diagnostics and skip the rest.
        f.debug_struct("FrameworkHostRenderer")
            .field("name", &self.descriptor.name)
            .field("extensions", &self.descriptor.extensions)
            .field("config_path", &self.config_path)
            .finish_non_exhaustive()
    }
}

impl FrameworkHostRenderer {
    /// Load the config once, cache the descriptor, and hand back a renderer
    /// that can service many requests against the same Node pool.
    pub async fn new(pool: Arc<dyn Pool>, config_path: impl Into<String>) -> Result<Self> {
        let config_path = config_path.into();
        let host = FrameworkHost::new(pool);
        let descriptor = host.load_config(&config_path).await?;
        Ok(Self { host, config_path, descriptor })
    }

    /// The descriptor loaded at construction time.
    pub fn descriptor(&self) -> &FrameworkDescriptor {
        &self.descriptor
    }

    /// Config path this renderer was built for.
    pub fn config_path(&self) -> &str {
        &self.config_path
    }
}

#[async_trait]
impl FrameworkRenderer for FrameworkHostRenderer {
    fn name(&self) -> &str {
        &self.descriptor.name
    }

    async fn render(&self, input: FrameworkRenderInput) -> Result<String> {
        self.host.render(&self.config_path, input.to_payload()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CallStream, JsHost, PoolConfig, ScriptedHost};
    use bytes::Bytes;
    use serde_json::json;

    /// A pool whose handler mimics the worker for both ops:
    /// - `framework:load` returns the descriptor the test wants.
    /// - `framework:render` returns HTML that captures the input for assertions.
    fn mock_pool(name: &'static str, capabilities: Value) -> Arc<dyn Pool> {
        let host = ScriptedHost::with_handler(Arc::new(move |entry, input| {
            let v = match entry.kind.as_str() {
                "framework:load" => json!({
                    "name": name,
                    "extensions": [".tsx"],
                    "capabilities": capabilities,
                }),
                "framework:render" => {
                    // Echo the request's route + data so tests can assert
                    // the typed input survived the trip.
                    let route = input.payload["input"]["route"].clone();
                    let data_name = input.payload["input"]["data"]["name"].clone();
                    json!({
                        "html": format!(
                            "<!doctype html><html><body><h1>route={} name={}</h1></body></html>",
                            route.as_str().unwrap_or("?"),
                            data_name.as_str().unwrap_or("?"),
                        )
                    })
                }
                other => json!({ "__oxide_error": format!("unexpected op: {other}") }),
            };
            let bytes = serde_json::to_vec(&v).unwrap();
            Ok(CallStream::Complete(Bytes::from(bytes)))
        }));
        host.tooling_pool(PoolConfig::default()).unwrap()
    }

    #[tokio::test]
    async fn new_eagerly_loads_descriptor() {
        let pool = mock_pool("react", json!({"suspense": "native", "islands": true}));
        let r = FrameworkHostRenderer::new(pool, "/abs/oxide.config.mjs")
            .await
            .expect("new");
        assert_eq!(r.name(), "react");
        assert_eq!(r.config_path(), "/abs/oxide.config.mjs");
        let d = r.descriptor();
        assert_eq!(d.name, "react");
        assert_eq!(d.extensions, vec![".tsx"]);
        assert_eq!(d.capabilities["suspense"], "native");
        assert_eq!(d.capabilities["islands"], true);
    }

    #[tokio::test]
    async fn new_surfaces_worker_error_for_missing_config() {
        let host = ScriptedHost::with_handler(Arc::new(|_e, _i| {
            let v = json!({ "__oxide_error": "framework:load failed: not found" });
            let bytes = serde_json::to_vec(&v).unwrap();
            Ok(CallStream::Complete(Bytes::from(bytes)))
        }));
        let pool = host.tooling_pool(PoolConfig::default()).unwrap();
        let err = FrameworkHostRenderer::new(pool, "/no/such/path.mjs")
            .await
            .expect_err("new must error");
        assert_eq!(err.0.code, "JSH-FRAMEWORK");
        assert!(err.0.message.contains("not found"), "{}", err.0.message);
    }

    #[tokio::test]
    async fn render_dispatch_passes_typed_input_to_worker() {
        let pool = mock_pool("react", json!({}));
        let r = FrameworkHostRenderer::new(pool, "/abs/oxide.config.mjs")
            .await
            .unwrap();
        let mut input = FrameworkRenderInput::new("routes/index", "/");
        input.data = json!({ "name": "Typed" });
        let html = r.render(input).await.unwrap();
        assert!(html.contains("route=routes/index"), "html: {html}");
        assert!(html.contains("name=Typed"), "html: {html}");
    }

    #[tokio::test]
    async fn render_errors_bubble_up_with_jsh_framework_code() {
        let host = ScriptedHost::with_handler(Arc::new(|entry, _i| {
            let v = if entry.kind == "framework:load" {
                json!({"name": "react", "extensions": [".tsx"], "capabilities": {}})
            } else {
                json!({ "__oxide_error": "framework:render failed: Boom" })
            };
            let bytes = serde_json::to_vec(&v).unwrap();
            Ok(CallStream::Complete(Bytes::from(bytes)))
        }));
        let pool = host.tooling_pool(PoolConfig::default()).unwrap();
        let r = FrameworkHostRenderer::new(pool, "/x").await.unwrap();
        let err = r
            .render(FrameworkRenderInput::new("routes/boom", "/boom"))
            .await
            .unwrap_err();
        assert_eq!(err.0.code, "JSH-FRAMEWORK");
        assert!(err.0.message.contains("Boom"), "{}", err.0.message);
    }

    #[tokio::test]
    async fn renderer_is_object_safe_and_arc_friendly() {
        let pool = mock_pool("vue", json!({}));
        let r: Arc<dyn FrameworkRenderer> = Arc::new(
            FrameworkHostRenderer::new(pool, "/x").await.unwrap(),
        );
        assert_eq!(r.name(), "vue");
        let html = r
            .render(FrameworkRenderInput::new("routes/index", "/"))
            .await
            .unwrap();
        assert!(html.contains("route=routes/index"));
    }

    #[test]
    fn input_new_defaults_shape() {
        let i = FrameworkRenderInput::new("routes/x", "/x");
        assert_eq!(i.params, json!({}));
        assert_eq!(i.data, json!({}));
        assert_eq!(i.head["meta"], json!([]));
        assert_eq!(i.head["link"], json!([]));
        // Sanity on the payload conversion.
        let p = i.to_payload();
        assert_eq!(p["route"], "routes/x");
        assert_eq!(p["url"], "/x");
    }
}

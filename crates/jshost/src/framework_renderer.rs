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
use bytes::Bytes;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

/// §6.4 / WP-B1: where the adapter resolves route components from.
///
/// Production: `entry` is the build-emitted `dist/server/ssr/index.js`, whose
/// `routes` map is imported once per module generation. Dev: `sources` maps a
/// canonical route id to an importable source file (or its precompiled JS).
/// Both are additive to the §7 wire shape; the config no longer carries a
/// component closure.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComponentSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sources: BTreeMap<String, String>,
}

impl ComponentSource {
    pub fn is_empty(&self) -> bool {
        self.entry.is_none() && self.sources.is_empty()
    }
}

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
    /// §8.2 / §16.3: layout chain, outermost → innermost. Mirrors
    /// `PageRenderInput.layouts`; the JS adapter consumes it as
    /// `input.layouts`. `#[serde(default)]` for test round-trips and any
    /// external constructor.
    #[serde(default)]
    pub layouts: Vec<String>,
    /// §10.2 step 5: nearest `error.tsx` ancestor route id, if any. The
    /// adapter renders this component when the page throws during SSR.
    /// Mirrors `PageRenderInput.error_route_id`; the JS adapter reads it
    /// as `input.errorRoute`.
    #[serde(default)]
    pub error_route_id: Option<String>,
    /// §6.4: the build-emitted component surface (or the dev source map).
    /// Filled by the renderer from the stack's shared source; absent keeps
    /// the legacy adapter behaviour (an explicit `componentFor` override).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<ComponentSource>,
    /// §16.4 / O2: the resolved locale for this render, when the project
    /// configures i18n. Threads through to the adapter so a page can call
    /// `t()` / `getLocale()` and know which language it renders in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,

    /// §7.1 / §9.2: the loader's resolved `event.defer()` values, keyed by
    /// the id it assigned. `data` carries `{ "__deferred": id }`
    /// placeholders; the adapter substitutes them before building the
    /// component tree so a component reads the value, not the placeholder.
    ///
    /// Already resolved — the pipeline collects the settle stream before
    /// calling render. Per-settle streaming is the §7.1 per-adapter path
    /// (`suspense: 'native'` vs `<Defer>`), a separate change.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub deferred: std::collections::BTreeMap<String, serde_json::Value>,

    /// §17.1 / §7.1: the request's CSP nonce. Forwarded to the adapter so
    /// the scripts it emits — notably the deferred-boundary swapper — carry
    /// it under `strict-dynamic`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
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
            layouts: Vec::new(),
            error_route_id: None,
            components: None,
            locale: None,
            deferred: std::collections::BTreeMap::new(),
            nonce: None,
        }
    }

    /// Serialize into the JSON payload the JS-side worker expects.
    fn to_payload(&self) -> Value {
        let mut payload = serde_json::json!({
            "route": self.route,
            "url": self.url,
            "params": self.params,
            "data": self.data,
            "head": self.head,
            "layouts": self.layouts,
            "errorRoute": self.error_route_id,
        });
        if let Some(cs) = &self.components {
            if !cs.is_empty() {
                payload["components"] = serde_json::to_value(cs).unwrap_or(Value::Null);
            }
        }
        if let Some(loc) = &self.locale {
            if !loc.is_empty() {
                payload["locale"] = Value::String(loc.clone());
            }
        }
        // §7.1 / §9.2: the resolved `defer()` map. The adapter substitutes
        // `{__deferred: id}` in `data` with these values before rendering.
        if !self.deferred.is_empty() {
            payload["deferred"] = serde_json::to_value(&self.deferred).unwrap_or(Value::Null);
        }
        if let Some(n) = &self.nonce {
            payload["nonce"] = Value::String(n.clone());
        }
        payload
    }
}

/// A typed renderer the pipeline depends on. Object-safe; the pipeline
/// holds an `Arc<dyn FrameworkRenderer>` behind an `Option` so builds with
/// no framework adapter configured keep the IR path.
/// Type alias for a chunked render stream: byte chunks in order, each
/// `Ok(chunk)` or a terminal `Err`. The stream is `Send + 'static` so it
/// can move across task boundaries (worker pool → HTTP response task).
pub type RenderStream = BoxStream<'static, crate::Result<Bytes>>;

#[async_trait]
pub trait FrameworkRenderer: Send + Sync {
    /// Adapter name (`react` / `vue` / `svelte` / `solid`); surfaced for
    /// inspector output and log lines.
    fn name(&self) -> &str;
    /// Render one route to HTML.
    async fn render(&self, input: FrameworkRenderInput) -> Result<String>;

    /// Streaming variant. Yields HTML bytes as chunks arrive from the
    /// upstream renderer. The default collects `render` into a single
    /// chunk, so implementations that cannot stream still work — and
    /// callers that only support the collected shape see no change.
    ///
    /// **Contract:** the concatenated chunks equal what `render` would
    /// return, modulo chunk boundaries. Callers must drain to `None` to
    /// release the underlying resource (worker slot).
    async fn render_streaming(&self, input: FrameworkRenderInput) -> Result<RenderStream> {
        let html = self.render(input).await?;
        let stream = futures::stream::once(async move { Ok(Bytes::from(html)) });
        Ok(Box::pin(stream))
    }
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
        Ok(Self {
            host,
            config_path,
            descriptor,
        })
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
        self.host
            .render(&self.config_path, input.to_payload())
            .await
    }

    /// Override: use the raw worker CallStream and forward each chunk as
    /// it arrives, instead of collecting the whole payload first. This is
    /// the version a hyper body writer wants — TTFB is the first chunk's
    /// arrival, not the whole response's.
    async fn render_streaming(&self, input: FrameworkRenderInput) -> Result<RenderStream> {
        let stream = self
            .host
            .render_streaming(&self.config_path, input.to_payload())
            .await?;
        // Adapt the CallStream into a `Stream<Item = Result<Bytes>>`.
        // CallStream::Chunked is an mpsc receiver; we turn it into a
        // Stream with `ReceiverStream`. CallStream::Complete yields one
        // Bytes item and closes.
        let out = async_stream_via_channel(stream);
        Ok(out)
    }
}

/// Convert a `CallStream` into a boxed `Stream<Item = Result<Bytes>>`.
/// Yields each chunk as it arrives, closes when the channel ends.
fn async_stream_via_channel(stream: crate::CallStream) -> RenderStream {
    match stream {
        crate::CallStream::Complete(b) => {
            let s = futures::stream::once(async move { Ok(b) });
            Box::pin(s)
        }
        crate::CallStream::Chunked(mut rx) => {
            let s = futures::stream::poll_fn(move |cx| rx.poll_recv(cx));
            Box::pin(s)
        }
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
        // Adapter errors travel as CallErr frames; the pool surfaces them
        // as Err. load_config still needs to succeed for FrameworkHostRenderer
        // to be constructed, so the handler branches on entry.kind.
        let host = ScriptedHost::with_handler(Arc::new(|entry, _i| {
            if entry.kind == "framework:load" {
                Ok(CallStream::Complete(Bytes::from_static(
                    br#"{"name":"react","extensions":[".tsx"],"capabilities":{}}"#,
                )))
            } else {
                Err(crate::err("JSH-FRAMEWORK", "framework:render failed: Boom"))
            }
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
        let r: Arc<dyn FrameworkRenderer> =
            Arc::new(FrameworkHostRenderer::new(pool, "/x").await.unwrap());
        assert_eq!(r.name(), "vue");
        let html = r
            .render(FrameworkRenderInput::new("routes/index", "/"))
            .await
            .unwrap();
        assert!(html.contains("route=routes/index"));
    }

    #[tokio::test]
    async fn render_streaming_default_collects() {
        // A renderer that only implements `render`. The default
        // `render_streaming` should collect it into one chunk.
        struct StaticRenderer;
        #[async_trait]
        impl FrameworkRenderer for StaticRenderer {
            fn name(&self) -> &str {
                "static"
            }
            async fn render(&self, _input: FrameworkRenderInput) -> Result<String> {
                Ok("<h1>default</h1>".to_string())
            }
        }
        let r = StaticRenderer;
        let mut stream = r
            .render_streaming(FrameworkRenderInput::new("routes/index", "/"))
            .await
            .unwrap();
        let mut chunks: Vec<Bytes> = Vec::new();
        use futures::StreamExt;
        while let Some(item) = stream.next().await {
            chunks.push(item.unwrap());
        }
        assert_eq!(chunks.len(), 1, "default yields one chunk");
        assert_eq!(chunks[0].as_ref(), b"<h1>default</h1>");
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

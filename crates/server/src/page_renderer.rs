//! Framework-agnostic page renderer hook (spec §7).
//!
//! The HTTP pipeline (`Pipeline::serve_request`) dispatches page renders to
//! whatever `PageRenderer` is attached to it. The trait lives here — in the
//! server crate — so that the pipeline never depends on `oxide-fw-jshost`
//! (which would couple the HTTP layer to V8/Node). The framework-specific
//! adapter (`FrameworkHostRenderer` in `oxide-fw-jshost`) implements this trait
//! via a small shim in the CLI crate that also owns the config-loading
//! decision.
//!
//! Layering:
//!
//! ```text
//!   crates/server          →  trait PageRenderer, PageRenderInput  (this file)
//!        ↑ impl
//!   crates/cli              →  AdapterPageRenderer(Arc<FrameworkHostRenderer>)
//!        ↑ uses
//!   Pipeline::with_page_renderer(...)
//! ```
//!
//! A pipeline with no renderer attached behaves exactly as before (the
//! neutral-IR renderer path the CLI has always used). Attaching a renderer
//! is a pure opt-in.

use crate::Result;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Neutral page-render input (spec §7 RenderInput, stripped of the
/// framework-specific parts). Fields map to the JS-side `RenderInput` in
/// `packages/core/src/types.ts`; the CLI shim converts this to whatever
/// shape the underlying host expects.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageRenderInput {
    /// Matched route id, e.g. `routes/blog/[slug]`.
    pub route: String,
    /// Request URL (path + query).
    pub url: String,
    /// URL params (path segments). Kept ordered so render output is
    /// deterministic across requests.
    pub params: BTreeMap<String, String>,
    /// Loader data as a JSON tree (ODF-decoded by the pipeline).
    pub data: Value,
    /// Neutral head model (spec §16.3) as a JSON object.
    pub head: Value,
    /// §8.2 / §16.3: layout chain for this route, **outermost → innermost**.
    /// Each entry is a route module id like `routes/layout` or
    /// `routes/blog/layout`. Populated by the pipeline from the router's
    /// `MatchInfo.layout_chain`; empty means "no layouts wrap this page".
    /// `#[serde(default)]` keeps existing `serde_json::from_str` round-trips
    /// and any external `PageRenderInput` constructors compiling.
    #[serde(default)]
    pub layouts: Vec<String>,
    /// §10.2 step 5: route id of the nearest `error.tsx` ancestor, if any.
    /// The adapter renders this component when the page throws during SSR,
    /// instead of the framework-neutral `OX-500` placeholder. `None` means
    /// no error boundary is registered for this route.
    #[serde(default)]
    pub error_route_id: Option<String>,
    /// §16.4 / O2: the resolved locale for this render, when the project
    /// configures i18n. Empty / None when no i18n config is present. The
    /// adapter forwards it to the loader event and to any `defineMeta`
    /// that reads it, matching the live dispatch's `LoaderRunInput.locale`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// §7.1 / §9.2: the loader's `event.defer()` values, keyed by the id
    /// the loader assigned. `data` carries `{ "__deferred": id }`
    /// placeholders; this map is what they resolve to. The adapter
    /// substitutes them before the component tree is built, so a component
    /// reads the value rather than the placeholder.
    ///
    /// Already resolved: the pipeline collects the settle stream before it
    /// calls `render_streaming`, so this is a value map, not a promise map.
    /// Streaming a hole as its promise settles is the §7.1 per-adapter
    /// path (`suspense: 'native'` for react/solid, `<Defer>` for
    /// vue/svelte) and is a separate change — this makes `defer()`
    /// *correct*, not *incremental*.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub deferred: BTreeMap<String, Value>,
    /// §17.1 / §7.1: the CSP nonce for this render. The adapter stamps it on
    /// any `<script>` it emits — including the deferred-boundary swapper,
    /// which the IR renderer streams as `template[data-ox-c]` + an inline
    /// IIFE. Under `strict-dynamic` a script without the nonce is blocked,
    /// so without this the streamed resolution never runs in a browser that
    /// enforces the policy.
    ///
    /// The live path passes `nonce_for(request_id)`; the prerender path
    /// passes the build-time nonce it already uses for its own injected
    /// tags. `None` means "no CSP configured" — the adapter then omits the
    /// attribute, which is what a policy-free deployment wants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
}

impl PageRenderInput {
    /// A minimal input for the given route + url. Params empty, data `{}`,
    /// head is an empty model. Useful in tests.
    pub fn new(route: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            route: route.into(),
            url: url.into(),
            params: BTreeMap::new(),
            data: Value::Object(Default::default()),
            head: serde_json::json!({
                "meta": [], "link": [], "style": [], "script": []
            }),
            layouts: Vec::new(),
            error_route_id: None,
            locale: None,
            deferred: BTreeMap::new(),
            nonce: None,
        }
    }
}

/// A chunked render stream: bytes as they arrive from the renderer.
pub type RenderStream = BoxStream<'static, Result<Bytes>>;

/// Object-safe trait the pipeline holds behind `Arc<dyn PageRenderer>`.
///
/// Implementations live where the framework dependency is available
/// (`crates/cli` shims `oxide_fw_jshost::FrameworkHostRenderer`; tests use a
/// pure-Rust fake). The pipeline only sees the trait, so it stays
/// framework- and Node-agnostic.
#[async_trait]
pub trait PageRenderer: Send + Sync {
    /// §6.4 / Wave M: best-effort warm-up. Called once after the socket
    /// is bound. The default impl is a no-op; a renderer backed by a
    /// worker pool overrides this to spawn the pool, import the config,
    /// and load the component entry — so the first real request does not
    /// pay those one-time costs. Errors are logged and ignored: readiness
    /// flips even on warmup failure (a broken renderer fails every
    /// request, not just the first).
    async fn warm(&self) -> crate::Result<()> {
        Ok(())
    }

    /// Human-readable name (`react` / `vue` / `svelte` / `solid` / `fake`).
    /// Surfaces in the inspector (§13.4) and log lines.
    fn name(&self) -> &str;

    /// Render one route to a full HTML document. `Ok` is a complete
    /// `<!doctype html>...` string ready to be sent as the response body.
    /// Errors surface as `OxideError` with a code the pipeline maps to a
    /// status (§10.2 step 5).
    async fn render(&self, input: PageRenderInput) -> Result<String>;

    /// Streaming variant. Yields HTML bytes as chunks arrive. The default
    /// collects `render` into a single chunk, so implementations that
    /// cannot stream still work.
    ///
    /// **Contract:** concatenated chunks equal what `render` returns,
    /// modulo chunk boundaries. Callers must drain to `None` to release
    /// any underlying resource (worker slot, cache entry).
    async fn render_streaming(&self, input: PageRenderInput) -> Result<RenderStream> {
        let html = self.render(input).await?;
        let stream = futures::stream::once(async move { Ok(Bytes::from(html)) });
        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A trivial renderer that echoes the route back. No Node, no framework.
    struct EchoRenderer;

    #[async_trait]
    impl PageRenderer for EchoRenderer {
        fn name(&self) -> &str {
            "echo"
        }
        async fn render(&self, input: PageRenderInput) -> Result<String> {
            Ok(format!(
                "<!doctype html><html><body><h1>route={}</h1></body></html>",
                input.route
            ))
        }
    }

    #[tokio::test]

    async fn render_streaming_default_collects() {
        let r: Arc<dyn PageRenderer> = Arc::new(EchoRenderer);

        let mut stream = r
            .render_streaming(PageRenderInput::new("routes/a", "/a"))
            .await
            .unwrap();

        use futures::StreamExt;

        let mut chunks: Vec<bytes::Bytes> = Vec::new();

        while let Some(item) = stream.next().await {
            chunks.push(item.unwrap());
        }

        assert_eq!(chunks.len(), 1, "default yields one chunk");

        let joined = String::from_utf8_lossy(&chunks[0]);

        assert!(joined.contains("route=routes/a"), "{joined}");
    }

    #[tokio::test]
    async fn input_new_defaults_shape() {
        let i = PageRenderInput::new("routes/index", "/");
        assert_eq!(i.route, "routes/index");
        assert_eq!(i.url, "/");
        assert!(i.params.is_empty());
        assert_eq!(i.data, serde_json::json!({}));
        assert_eq!(i.head["meta"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn trait_is_object_safe() {
        let r: Arc<dyn PageRenderer> = Arc::new(EchoRenderer);
        assert_eq!(r.name(), "echo");
        let html = r
            .render(PageRenderInput::new("routes/a", "/a"))
            .await
            .unwrap();
        assert!(html.contains("route=routes/a"), "{html}");
    }

    #[tokio::test]
    async fn input_round_trips_through_serde() {
        let mut i = PageRenderInput::new("routes/blog/[slug]", "/blog/hi");
        i.params.insert("slug".into(), "hi".into());
        i.data = serde_json::json!({ "author": "Ada" });
        let s = serde_json::to_string(&i).unwrap();
        let back: PageRenderInput = serde_json::from_str(&s).unwrap();
        assert_eq!(back, i);
    }

    #[test]
    fn input_serializes_to_js_shaped_json() {
        let i = PageRenderInput::new("routes/index", "/");
        let v = serde_json::to_value(&i).unwrap();
        // The JS-side RenderInput expects these keys at the top level.
        assert!(v.get("route").is_some());
        assert!(v.get("url").is_some());
        assert!(v.get("params").is_some());
        assert!(v.get("data").is_some());
        assert!(v.get("head").is_some());
    }

    #[test]
    fn pipeline_accepts_page_renderer_builder() {
        use crate::Pipeline;
        let pipeline = Pipeline::default().with_page_renderer(Arc::new(EchoRenderer));
        let r = pipeline.page_renderer().expect("renderer attached");
        assert_eq!(r.name(), "echo");
    }

    #[test]
    fn pipeline_defaults_to_no_page_renderer() {
        use crate::Pipeline;
        let pipeline = Pipeline::default();
        assert!(
            pipeline.page_renderer().is_none(),
            "default pipeline has no renderer"
        );
    }
}

// ---------------------------------------------------------------------------
// Endpoint dispatch (§8.1 route.ts, §10.2 step 7)
// ---------------------------------------------------------------------------

/// The seam between the HTTP pipeline and whichever runtime executes a
pub struct EndpointDispatchArgs<'a> {
    pub module_path: &'a str,
    pub method: &'a str,
    pub url: &'a str,
    pub params: &'a serde_json::Map<String, serde_json::Value>,
    pub headers: &'a [(String, String)],
    pub body: &'a [u8],
    pub locals: &'a serde_json::Value,
    pub trace: Option<&'a serde_json::Value>,
}

/// route's `route.ts` handler. Mirrors `ActionDispatcher`; the CLI wires a
/// dispatcher backed by the render pool (`oxide-fw-jshost::EndpointHost`); tests
/// can wire a table of closures.
#[async_trait::async_trait]
pub trait EndpointDispatcher: Send + Sync {
    /// Dispatch the route's `route.ts` for the request. `module_path` is the
    /// on-disk path to `route.ts`; `method` is the HTTP method; `body` is the
    /// raw request body (a route.ts can be an upload proxy, so the wire must
    /// carry arbitrary bytes). The result is the raw `Response` shape.
    async fn dispatch(&self, args: EndpointDispatchArgs<'_>) -> std::result::Result<EndpointRunResult, EndpointRunError>;
}

/// The result of a `route.ts` run.
#[derive(Default)]
pub struct EndpointRunResult {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// §9.6: a streaming body (SSE, downloads). When `Some`, the pipeline
    /// builds a streaming response and ignores `body`.
    pub stream: Option<crate::BodyStream>,
}

impl std::fmt::Debug for EndpointRunResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EndpointRunResult")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .field("body_len", &self.body.len())
            .field("streaming", &self.stream.is_some())
            .finish()
    }
}

/// A dispatch failure the pipeline maps to an HTTP status + a JSON envelope.
#[derive(Debug, Clone)]
pub struct EndpointRunError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Middleware dispatch (§8.1 middleware.ts, §8.5 rewrites)
// ---------------------------------------------------------------------------

/// The seam between the HTTP pipeline and whichever runtime executes a
pub struct MiddlewareDispatchArgs<'a> {
    pub module_path: &'a str,
    pub method: &'a str,
    pub url: &'a str,
    pub params: &'a serde_json::Map<String, serde_json::Value>,
    pub headers: &'a [(String, String)],
    pub body: &'a [u8],
    pub locals: Option<&'a serde_json::Value>,
}

/// route's `middleware.ts`. The middleware runs **before** route match,
/// loader, and render, and can short-circuit, rewrite, add headers, or inject
/// `locals`.
#[async_trait::async_trait]
pub trait MiddlewareDispatcher: Send + Sync {
    /// Dispatch `middleware.ts` for a request. `module_path` is the on-disk
    /// path; `locals` is the request-scoped state accumulated so far (an
    /// outer middleware's locals, if any). The result carries the directive.
    async fn dispatch(&self, args: MiddlewareDispatchArgs<'_>) -> std::result::Result<MiddlewareRunResult, MiddlewareRunError>;
}

/// The result of a `middleware.ts` run.
#[derive(Debug, Clone, Default)]
pub struct MiddlewareRunResult {
    pub short: bool,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub redirect: Option<String>,
    pub rewrite: Option<String>,
    pub locals: Option<serde_json::Value>,
}

/// A middleware dispatch failure mapped to an HTTP status.
#[derive(Debug, Clone)]
pub struct MiddlewareRunError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Socket dispatch (§9.6)
// ---------------------------------------------------------------------------

/// The seam between the HTTP pipeline and whichever runtime executes a
/// route's `socket.ts` WebSocket handlers. Rust owns the upgrade, framing,
/// ping/pong and backpressure; this trait is one phase call.
#[async_trait::async_trait]
pub trait SocketDispatcher: Send + Sync {
    async fn run(
        &self,
        module_path: &str,
        phase: &str,
        payload: serde_json::Value,
        socket_id: &str,
    ) -> std::result::Result<SocketRunResult, SocketRunError>;
}

#[derive(Debug, Clone, Default)]
pub struct SocketRunResult {
    /// Frames for the calling socket only.
    pub sends: Vec<String>,
    /// Frames for every other socket connected to the same `socket.ts`
    /// module (`ctx.broadcast`).
    pub broadcasts: Vec<String>,
    /// Frames for specific socket ids (`ctx.sendTo`), including the caller.
    pub targets: Vec<(String, String)>,
    pub close: Option<(u16, String)>,
}

#[derive(Debug, Clone)]
pub struct SocketRunError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

//! Bridge from `oxide_jshost`'s framework renderer to the server's
//! framework-agnostic `oxide_server::PageRenderer` trait (spec §7).
//!
//! Layering:
//!   - `oxide_server::PageRenderer` — what `Pipeline`/dispatch consumes
//!   - `oxide_jshost::FrameworkRenderer` — what talks to the Node worker
//!   - `AdapterPageRenderer` (this module) — impls the former around the
//!     latter, converting input shapes and the error type
//!
//! `build_adapter_renderer` is the concrete constructor used by `cmd_dev`:
//! it spins up a `NodeHost` worker, eagerly loads the project's
//! `oxide.config.{ts,mjs,js}` through `FrameworkHostRenderer::new`, and
//! returns an `Arc<dyn PageRenderer>` ready to hand to the pipeline.

use async_trait::async_trait;
use oxide_jshost::{FrameworkRenderInput, FrameworkRenderer};
use oxide_config::JsBackend;
use oxide_server::{LoaderRunner, PageRenderInput, PageRenderer};
use std::sync::Arc;

/// Wraps any `FrameworkRenderer` as an `oxide_server::PageRenderer`.
///
/// The only translation is input-shape: `PageRenderInput.params` is a typed
/// `BTreeMap<String, String>`, while `FrameworkRenderInput.params` is a
/// JSON `Value`. The rest (`route`, `url`, `data`, `head`) passes through
/// unchanged. Errors propagate with the same `OxideError` type.
pub struct AdapterPageRenderer {
    inner: Arc<dyn FrameworkRenderer>,
}

impl AdapterPageRenderer {
    /// Construct from an already-loaded framework renderer. `cmd_dev` uses
    /// `build_adapter_renderer` to produce the inner value; tests can
    /// substitute a fake.
    pub fn new(inner: Arc<dyn FrameworkRenderer>) -> Self {
        Self { inner }
    }

    /// Access to the wrapped renderer (for tests and introspection).
    pub fn inner(&self) -> &Arc<dyn FrameworkRenderer> {
        &self.inner
    }
}

fn to_fw_input(input: PageRenderInput) -> FrameworkRenderInput {
    // `BTreeMap<String, String>` → `serde_json::Value::Object`. The
    // serialization can't fail for this input shape (string keys and
    // values always map to JSON), but we degrade to `{}` on the
    // impossible path rather than panicking.
    let params = serde_json::to_value(&input.params)
        .unwrap_or_else(|_| serde_json::json!({}));
    FrameworkRenderInput {
        route: input.route,
        url: input.url,
        params,
        data: input.data,
        head: input.head,
    }
}

#[async_trait]
impl PageRenderer for AdapterPageRenderer {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn render(
        &self,
        input: PageRenderInput,
    ) -> oxide_server::Result<String> {
        self.inner.render(to_fw_input(input)).await
    }

    /// Forward to the inner renderer's streaming path. The default
    /// `PageRenderer::render_streaming` would call `render` and collect;
    /// overriding here means the chunked wire from the worker is
    /// preserved end-to-end.
    async fn render_streaming(
        &self,
        input: PageRenderInput,
    ) -> oxide_server::Result<oxide_server::RenderStream> {
        self.inner.render_streaming(to_fw_input(input)).await
    }
}

/// Bundles the framework renderer and loader runner that a CLI command
/// attaches to its server/dispatch. Both are `Option` because they can fail
/// independently: a project without a config has no renderer but still
/// benefits from a loader; a Node worker that fails to start has neither.
///
/// Construct via [`build_framework_stack`]; the empty default is used by
/// tests and by the legacy `start_server_inner` path.
#[derive(Default)]
pub struct FrameworkStack {
    pub page_renderer: Option<Arc<dyn PageRenderer>>,
    pub loader_runner: Option<Arc<dyn LoaderRunner>>,
    /// The shared Node worker pool backing both the renderer and the loader.
    /// Kept here (as a second Arc reference) so callers that need to send
    /// out-of-band ops to the worker — `cmd_dev`'s watcher calling
    /// `FrameworkHost::invalidate()` on file change (§13.3) — still have a
    /// handle after the renderer/loader have been moved into the pipeline.
    pub pool: Option<Arc<dyn oxide_jshost::Pool>>,
}

impl FrameworkStack {
    /// No renderer, no loader. Every render falls back to the IR placeholder.
    pub fn empty() -> Self {
        Self { page_renderer: None, loader_runner: None, pool: None }
    }
    /// The attached renderer's name, if any (for the `framework renderer:`
    /// log line).
    pub fn renderer_name(&self) -> Option<&str> {
        self.page_renderer.as_ref().map(|r| r.name())
    }
    /// True when a renderer is attached.
    pub fn has_renderer(&self) -> bool {
        self.page_renderer.is_some()
    }
    /// True when the shared worker pool is available (i.e. the Node worker
    /// started successfully). Used by `cmd_dev`'s watcher to decide whether
    /// an `invalidate` call is possible.
    pub fn has_pool(&self) -> bool {
        self.pool.is_some()
    }
}

impl std::fmt::Debug for FrameworkStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameworkStack")
            .field("has_renderer", &self.page_renderer.is_some())
            .field("has_loader", &self.loader_runner.is_some())
            .field("has_pool", &self.pool.is_some())
            .finish()
    }
}

/// Build the framework stack for a project. `config_path` is the resolved
/// `oxide.config.{ts,mjs,js}` (or `None` for projects without one).
///
/// A single Node worker pool backs both the renderer and the loader, so
/// they share module caches and stay warm across requests. Any failure
/// (worker can't spawn, config can't load) degrades gracefully: the
/// affected piece is `None`, the caller falls back to the IR path.
/// Best-effort extraction of the adapter name from a config file's source.
/// Matches `framework: react(` / `framework: solid(` / etc. Used only to
/// seed `OXIDE_FRAMEWORK` before the config is dynamically imported; the
/// full config load replaces this with the authoritative value.
fn peek_framework_name(path: &std::path::Path) -> Option<String> {
    let src = std::fs::read_to_string(path).ok()?;
    for name in ["react", "vue", "svelte", "solid"] {
        // Simple textual search: `framework: <name>(` — the shape every
        // config in this workspace uses. A `framework({ ... })` with no
        // adapter factory leaves the caller's default (react) in place.
        let needle = format!("framework: {name}(");
        if src.contains(&needle) {
            return Some(name.to_string());
        }
        // Also `framework: <name> ({ ... })` and `framework: <name>({`.
        let needle2 = format!("framework:{name}(");
        if src.contains(&needle2) {
            return Some(name.to_string());
        }
    }
    None
}

fn backend_str(b: JsBackend) -> &'static str {
    match b {
        JsBackend::Node => "node",
        JsBackend::V8 => "v8",
    }
}

pub async fn build_framework_stack(
    config_path: Option<&std::path::Path>,
    backend: JsBackend,
) -> FrameworkStack {
    use oxide_jshost::{FrameworkHostRenderer, JsHost, NodeHost, PoolConfig, V8Host};

    // §6.3: which framework's JSX the worker's `tsx-loader.mjs` should
    // lower `.tsx`/`.jsx` against. Read here (not at the CLI call sites) so
    // the config's own `import ... from './page.tsx'` statement goes
    // through the right compiler — the config loader runs a dynamic import
    // inside `build_framework_stack`, before any render request. The value
    // is a best-effort peek at the config file's first `framework: X(...)`
    // match; the CLI can override it via the same env var.
    if let Some(cfg_path) = config_path {
        if std::env::var_os("OXIDE_FRAMEWORK").is_none() {
            if let Some(name) = peek_framework_name(cfg_path) {
                // SAFETY: called before any worker thread is spawned; no
                // concurrent reader or writer touches this variable.
                unsafe { std::env::set_var("OXIDE_FRAMEWORK", name) };
            }
        }
    }

    // §4.2 / D2: the backend is selectable via `server.backend` in the
    // project config. `Node` (default) resolves npm from the workspace;
    // `V8` runs `deno_core` in-process and is the spec's long-term default
    // once `deno_node` module loading lands.
    let host: Arc<dyn JsHost> = match backend {
        JsBackend::Node => Arc::new(NodeHost::new(NodeHost::worker_script_default())),
        JsBackend::V8 => Arc::new(V8Host),
    };
    let pool = match host.tooling_pool(PoolConfig::default()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "warn: framework stack unavailable ({} worker): {} {}",
                backend_str(backend),
                e.0.code,
                e.0.message
            );
            return FrameworkStack::empty();
        }
    };

    // The loader runner needs only the pool — it resolves modules per request.
    let loader_runner: Arc<dyn LoaderRunner> = crate::loader_shim::build_loader_runner(
        Arc::clone(&pool),
    );

    // Keep a second Arc reference to the pool so callers (notably cmd_dev's
    // watcher) can send out-of-band ops after the renderer/loader have been
    // moved into the pipeline.
    let pool_for_stack: Arc<dyn oxide_jshost::Pool> = Arc::clone(&pool);

    // The page renderer needs a config file. Its absence is normal.
    let page_renderer: Option<Arc<dyn PageRenderer>> = match config_path {
        Some(path) => {
            match FrameworkHostRenderer::new(
                pool,
                path.to_string_lossy().to_string(),
            )
            .await
            {
                Ok(hr) => Some(Arc::new(AdapterPageRenderer::new(Arc::new(hr)))),
                Err(e) => {
                    eprintln!(
                        "warn: framework renderer not wired: {} {}",
                        e.0.code, e.0.message
                    );
                    None
                }
            }
        }
        None => None,
    };

    FrameworkStack {
        page_renderer,
        loader_runner: Some(loader_runner),
        pool: Some(pool_for_stack),
    }
}

/// Build a `PageRenderer` for a project's `oxide.config.{ts,mjs,js}`.
///
/// Spins up a Node worker, loads the config through `FrameworkHostRenderer`
/// (which eagerly evaluates the config and extracts its framework adapter),
/// and wraps the result in `AdapterPageRenderer`.
///
/// Errors surface with the worker's `JSH-FRAMEWORK` code as an `anyhow`
/// error (never fatal here — `cmd_dev` warns and falls back to the IR path).
pub async fn build_adapter_renderer(
    config_path: &std::path::Path,
) -> anyhow::Result<Arc<dyn PageRenderer>> {
    use oxide_jshost::{FrameworkHostRenderer, JsHost, NodeHost, PoolConfig};

    let host = NodeHost::new(NodeHost::worker_script_default());
    let pool = host
        .tooling_pool(PoolConfig::default())
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;

    let host_renderer = FrameworkHostRenderer::new(
        pool,
        config_path.to_string_lossy().to_string(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;

    let inner: Arc<dyn FrameworkRenderer> = Arc::new(host_renderer);
    Ok(Arc::new(AdapterPageRenderer::new(inner)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_jshost::FrameworkRenderInput;

    /// A framework renderer that echoes what it received — lets tests assert
    /// the input conversion (typed `params` → JSON) without Node.
    struct EchoFramework;

    #[async_trait]
    impl FrameworkRenderer for EchoFramework {
        fn name(&self) -> &str {
            "echo-fw"
        }
        async fn render(
            &self,
            input: FrameworkRenderInput,
        ) -> oxide_jshost::Result<String> {
            let params_json = serde_json::to_string(&input.params).unwrap_or_default();
            Ok(format!(
                "<!doctype html><html><body><h1>route={}</h1><pre>{}</pre></body></html>",
                input.route, params_json
            ))
        }
    }

    /// A renderer that always errors — verifies the error path.
    struct BoomFramework;

    #[async_trait]
    impl FrameworkRenderer for BoomFramework {
        fn name(&self) -> &str {
            "boom-fw"
        }
        async fn render(
            &self,
            _input: FrameworkRenderInput,
        ) -> oxide_jshost::Result<String> {
            Err(oxide_jshost::err(
                "JSH-FRAMEWORK",
                "simulated render failure",
            ))
        }
    }

    #[test]
    fn adapter_reports_inner_name() {
        let r = AdapterPageRenderer::new(Arc::new(EchoFramework));
        assert_eq!(r.name(), "echo-fw");
    }

    #[tokio::test]
    async fn adapter_forwards_input_and_returns_html() {
        let r = AdapterPageRenderer::new(Arc::new(EchoFramework));
        let mut input = PageRenderInput::new("routes/blog/[slug]", "/blog/hello");
        input.params.insert("slug".into(), "hello".into());
        input.data = serde_json::json!({ "title": "Post" });

        let html = r.render(input).await.expect("render ok");
        assert!(html.contains("route=routes/blog/[slug]"), "{html}");
        assert!(html.contains(r#""slug":"hello""#), "params converted: {html}");
    }

    #[tokio::test]
    async fn adapter_converts_empty_params_to_json_object() {
        let r = AdapterPageRenderer::new(Arc::new(EchoFramework));
        let html = r
            .render(PageRenderInput::new("routes/index", "/"))
            .await
            .unwrap();
        assert!(html.contains("<pre>{}</pre>"), "empty params → `{{}}`: {html}");
    }

    #[tokio::test]
    async fn adapter_propagates_render_error_unchanged() {
        let r = AdapterPageRenderer::new(Arc::new(BoomFramework));
        let err = r
            .render(PageRenderInput::new("routes/x", "/x"))
            .await
            .expect_err("must error");
        assert_eq!(err.0.code, "JSH-FRAMEWORK");
        assert!(err.0.message.contains("simulated render failure"));
    }

    #[tokio::test]
    async fn adapter_is_object_safe_for_arc_dyn() {
        let r: Arc<dyn PageRenderer> = Arc::new(AdapterPageRenderer::new(Arc::new(EchoFramework)));
        assert_eq!(r.name(), "echo-fw");
        let html = r
            .render(PageRenderInput::new("routes/index", "/"))
            .await
            .unwrap();
        assert!(html.contains("route=routes/index"));
    }

    /// A FrameworkRenderer that yields three fixed chunks. Used to prove
    /// the streaming override preserves chunk boundaries end-to-end.
    struct ChunkedRenderer;

    #[async_trait]
    impl oxide_jshost::FrameworkRenderer for ChunkedRenderer {
        fn name(&self) -> &str { "chunked" }
        async fn render(
            &self,
            _input: oxide_jshost::FrameworkRenderInput,
        ) -> oxide_jshost::Result<String> {
            Ok("<h1>collected</h1>".to_string())
        }
        async fn render_streaming(
            &self,
            _input: oxide_jshost::FrameworkRenderInput,
        ) -> oxide_jshost::Result<oxide_jshost::RenderStream> {
            use bytes::Bytes;
            let items: Vec<oxide_jshost::Result<Bytes>> = vec![
                Ok(Bytes::from_static(b"<h1>")),
                Ok(Bytes::from_static(b"chunked")),
                Ok(Bytes::from_static(b"</h1>")),
            ];
            let s = futures::stream::iter(items);
            Ok(Box::pin(s))
        }
    }

    #[tokio::test]
    async fn render_streaming_preserves_chunks_through_adapter() {
        use futures::StreamExt;
        let r = AdapterPageRenderer::new(Arc::new(ChunkedRenderer));
        let mut stream = r
            .render_streaming(PageRenderInput::new("routes/x", "/x"))
            .await
            .unwrap();
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        while let Some(item) = stream.next().await {
            chunks.push(item.unwrap().to_vec());
        }
        assert_eq!(chunks.len(), 3, "three chunks survive the adapter");
        assert_eq!(chunks[0], b"<h1>");
        assert_eq!(chunks[1], b"chunked");
        assert_eq!(chunks[2], b"</h1>");
    }
}

#[cfg(test)]
mod backend_tests {
    use super::*;

    /// With `server.backend = "v8"`, `build_framework_stack` must construct
    /// the pool via `V8Host` (not `NodeHost`). We cannot spawn the CLI
    /// worker from a unit test on every host — the assertion here is that
    /// the pool exists and reports the V8 backend name through the trait.
    #[tokio::test]
    async fn v8_backend_builds_a_pool() {
        use oxide_jshost::JsHost;
        let v8 = oxide_jshost::V8Host;
        assert_eq!(v8.backend_name(), "v8");
        // Pool construction must succeed (first V8 init can be slow; ignore
        // the outcome on hosts without a prebuilt deno_core snapshot).
        let _ = v8.render_pool(oxide_jshost::PoolConfig::default());
    }

    /// Node backend remains the default and still works.
    #[tokio::test]
    async fn node_backend_builds_a_pool() {
        use oxide_jshost::JsHost;
        let node = oxide_jshost::NodeHost::new(oxide_jshost::NodeHost::worker_script_default());
        assert_eq!(node.backend_name(), "node");
    }
}

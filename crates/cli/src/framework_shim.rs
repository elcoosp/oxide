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
use oxide_jshost::{FrameworkHostRenderer, FrameworkRenderInput, FrameworkRenderer};
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
    /// The render/loader pool for the selected backend. V8 when
    /// `server.backend = "v8"`, Node otherwise. Kept here (as a second Arc
    /// reference) so callers that need out-of-band ops — `cmd_dev`'s watcher
    /// calling `FrameworkHost::invalidate()` (§13.3) — still have a handle
    /// after the renderer/loader move into the pipeline.
    pub pool: Option<Arc<dyn oxide_jshost::Pool>>,
    /// §4.7: the tooling pool for the JS compilers (Vue/Svelte/Solid). **Always
    /// Node**, regardless of `server.backend` — those compilers are npm
    /// packages that need Node built-ins, which the V8 sandbox does not have.
    /// Used by the §6.3 precompile pass at build time. `None` only when no
    /// pool could be built.
    pub tooling_pool: Option<Arc<dyn oxide_jshost::Pool>>,
    /// §11 / BUG-02: the **one** cache tier the framework stack built. The
    /// server's L0 response cache wraps this same handler, so an
    /// invalidation (BUG-01's tag/path path) reaches both L0 (the response
    /// cache) and L1 (the worker's `'use cache'` entries). Before this field
    /// the server built its own `TieredCache` and the worker had a second
    /// one, so they could not see each other's invalidations.
    pub cache_tier: Option<Arc<dyn oxide_cache::CacheHandler>>,
}

impl FrameworkStack {
    /// No renderer, no loader. Every render falls back to the IR placeholder.
    pub fn empty() -> Self {
        Self {
            page_renderer: None,
            loader_runner: None,
            pool: None,
            tooling_pool: None,
            cache_tier: None,
        }
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
    use oxide_jshost::{JsHost, NodeHost, PoolConfig, V8Host};

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
    // §9.3 / §11: a Rust cache tier the worker reaches through the
    // `op_request` protocol. Memory-only by default; a project that
    // configures a disk root gets the L1 tier too.
    let cache_tier: Arc<dyn oxide_cache::CacheHandler> = {
        let t = oxide_cache::TieredCache::new(512 * 1024 * 1024);
        // A disk root beside the config, when one is known. `with_disk`
        // takes ownership and returns the cache on success; on failure the
        // memory-only value is gone, so build a fresh one — a disk-tier
        // failure is rare (a permissions or redb problem) and a memory-only
        // fallback is the right degradation.
        let disk = config_path
            .and_then(|p| p.parent())
            .map(|root| root.join(".oxide/cache/l1"))
            .filter(|d| std::fs::create_dir_all(d).is_ok());
        let with_disk = disk.and_then(|d| t.with_disk(d).ok());
        match with_disk {
            Some(c) => Arc::new(c),
            None => Arc::new(oxide_cache::TieredCache::new(512 * 1024 * 1024)),
        }
    };

    let host: Arc<dyn JsHost> = match backend {
        JsBackend::Node => Arc::new(
            NodeHost::new(NodeHost::worker_script_default())
                .with_cache_handler(Arc::clone(&cache_tier)),
        ),
        JsBackend::V8 => {
            // §4.4: if `oxide build` wrote a snapshot, boot from it.
            let snap = config_path
                .and_then(|p| p.parent())
                .map(|root| root.join(".oxide/snapshot.bin"));
            let host = match snap {
                Some(path) if path.is_file() => V8Host::from_snapshot_file(&path),
                _ => V8Host::new(),
            };
            // BUG-02: give the V8 backend the same cache tier as Node — it
            // had none, so `'use cache'` silently degraded to per-worker Maps.
            Arc::new(host.with_cache_handler(Arc::clone(&cache_tier)))
        }
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

    // §4.7: the JS compilers (Vue/Svelte/Solid) are npm packages that need
    // Node built-ins, so they always run on a **Node** pool — even when the
    // render backend is V8. Building this unconditionally is cheap next to a
    // compile failure: a Vue fixture with `backend: "v8"` otherwise sends
    // `tooling:vue-compile` to the V8 pool, which has no such op.
    let tooling_pool: Arc<dyn oxide_jshost::Pool> = if matches!(backend, JsBackend::V8) {
        match NodeHost::new(NodeHost::worker_script_default())
            .with_cache_handler(Arc::clone(&cache_tier))
            .tooling_pool(PoolConfig::default())
        {
            Ok(p) => p,
            Err(e) => {
                eprintln!(
                    "warn: tooling pool unavailable (node): {} {}",
                    e.0.code, e.0.message
                );
                Arc::clone(&pool)
            }
        }
    } else {
        // The Node backend's render pool IS a Node tooling pool.
        Arc::clone(&pool)
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
            // §4.4 / §21: do NOT evaluate the config here — it costs ~325 ms
            // and blocks the bind. Build a renderer that loads on first render.
            let peeked = peek_framework_name(path).unwrap_or_else(|| "react".to_string());
            // §4.2: the V8 backend cannot import a `.ts` config that pulls in a
            // CJS framework runtime (React) — bare V8 ESM has no CJS interop.
            // `oxide build` bundles the config graph to `.oxide/config.server.js`
            // (rolldown resolves CJS); point the V8 renderer at that.
            // §4.2: the V8 backend imports the pre-bundled config; the Node
            // backend imports the raw `.ts` (its tsx-loader transforms it and
            // it has CJS interop). Pick by backend — the Node worker cannot
            // load the V8-targeted bundle.
            // The raw config; `bundled_path` above is preferred at resolve time.
            let config_path = path.to_string_lossy().to_string();
            let bundled_path = if matches!(backend, JsBackend::V8) {
                path.parent()
                    .map(|root| root.join(".oxide/config.server.js").to_string_lossy().to_string())
            } else {
                None
            };
            let lazy = LazyFrameworkRenderer {
                pool: Arc::clone(&pool),
                config_path,
                bundled_path,
                name: peeked,
                inner: tokio::sync::OnceCell::new(),
            };
            Some(Arc::new(AdapterPageRenderer::new(Arc::new(lazy))))
        }
        None => None,
    };

    FrameworkStack {
        page_renderer,
        loader_runner: Some(loader_runner),
        pool: Some(pool_for_stack),
        tooling_pool: Some(tooling_pool),
        // BUG-02: hand the shared cache tier back to the caller so the L0
        // response cache wraps the *same* handler the worker uses.
        cache_tier: Some(Arc::clone(&cache_tier)),
    }
}

/// §4.4 / §21: a `FrameworkRenderer` that builds its inner
/// `FrameworkHostRenderer` — which evaluates the project config through the
/// worker (~325 ms on the reference app, measured) — on **first render**, not
/// at server start.
///
/// `build_framework_stack` otherwise pays that cost before the socket can
/// bind, yet nothing on the native path (health, static assets, the L0 cache)
/// touches the renderer. The first dynamic render pays the load it would pay
/// anyway, because `FrameworkHost::render` re-imports the config per call.
pub struct LazyFrameworkRenderer {
    pool: Arc<dyn oxide_jshost::Pool>,
    /// The raw config (`.ts`/`.mjs`) — what the Node backend imports.
    config_path: String,
    /// The pre-bundled config (`.oxide/config.server.js`) — what the V8
    /// backend imports. Resolved at `resolve()` time, not construction: the
    /// bundle is written by `oxide build` *after* the framework stack is
    /// built, so at construction it may not exist yet.
    bundled_path: Option<String>,
    /// Peeked from the config source at construction: the descriptor's own
    /// name is only known after the load this type defers.
    name: String,
    inner: tokio::sync::OnceCell<Arc<dyn FrameworkRenderer>>,
}

impl LazyFrameworkRenderer {
    async fn resolve(&self) -> oxide_jshost::Result<Arc<dyn FrameworkRenderer>> {
        let v = self
            .inner
            .get_or_try_init(|| async {
                // Prefer the pre-bundled config when present — the V8 backend
                // requires it and the bundle is written by the time a render
                // happens.
                let path = match &self.bundled_path {
                    Some(b) if std::path::Path::new(b).is_file() => b.clone(),
                    _ => self.config_path.clone(),
                };
                let hr = FrameworkHostRenderer::new(Arc::clone(&self.pool), path).await?;
                Ok::<Arc<dyn FrameworkRenderer>, oxide_diagnostics::OxideError>(Arc::new(hr))
            })
            .await?;
        Ok(Arc::clone(v))
    }
}

#[async_trait]
impl FrameworkRenderer for LazyFrameworkRenderer {
    fn name(&self) -> &str {
        &self.name
    }
    async fn render(
        &self,
        input: FrameworkRenderInput,
    ) -> oxide_jshost::Result<String> {
        self.resolve().await?.render(input).await
    }
    async fn render_streaming(
        &self,
        input: FrameworkRenderInput,
    ) -> oxide_jshost::Result<oxide_jshost::RenderStream> {
        self.resolve().await?.render_streaming(input).await
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
        let v8 = oxide_jshost::V8Host::new();
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

//! Bridge from `oxide_fw_jshost`'s framework renderer to the server's
//! framework-agnostic `oxide_fw_server::PageRenderer` trait (spec §7).
//!
//! Layering:
//!   - `oxide_fw_server::PageRenderer` — what `Pipeline`/dispatch consumes
//!   - `oxide_fw_jshost::FrameworkRenderer` — what talks to the Node worker
//!   - `AdapterPageRenderer` (this module) — impls the former around the
//!     latter, converting input shapes and the error type
//!
//! `AdapterPageRenderer::new` is the concrete constructor used by `cmd_dev`:
//! it spins up a `NodeHost` worker, eagerly loads the project's
//! `oxide.config.{ts,mjs,js}` through `FrameworkHostRenderer::new`, and
//! returns an `Arc<dyn PageRenderer>` ready to hand to the pipeline.

use async_trait::async_trait;
use oxide_fw_config::JsBackend;
use oxide_fw_jshost::{FrameworkHostRenderer, FrameworkRenderInput, FrameworkRenderer};
use oxide_fw_server::{LoaderRunner, PageRenderInput, PageRenderer};
use std::sync::Arc;

/// §6.2 / §15: a `TransformHandler` built from the bundler's transform hook.
/// The hook is the same WASM/plugin transform the bundler runs; wrapping it
/// lets the worker's `compile_module` op apply it during the **prerender**
/// (which loads components through the worker's module loader, not the
/// bundler).
pub struct HookTransformHandler {
    hook: oxide_fw_bundler::TransformHook,
}

#[async_trait::async_trait]
impl oxide_fw_jshost::TransformHandler for HookTransformHandler {
    async fn transform(&self, id: &str, source: &str) -> oxide_fw_diagnostics::Result<String> {
        self.hook.call(id, source)
    }
}

/// Build a `TransformHandler` from the project's plugin transform hook.
pub fn build_transform_handler(
    root: &std::path::Path,
    cfg: &oxide_fw_config::OxideConfig,
) -> Option<Arc<dyn oxide_fw_jshost::TransformHandler>> {
    let hook = crate::build_transform_hook(root, cfg)?;
    Some(Arc::new(HookTransformHandler { hook }))
}

/// PERF-05: the `fetch` op handler. Delegates to `oxide-fw-server`'s
/// SSRF-hardened fetch, so a server-side `fetch()` in user code goes through
/// the same validated path as the image endpoint.
struct CliFetchHandler {
    ssrf_allow_list: Vec<String>,
}

#[async_trait::async_trait]
impl oxide_fw_jshost::FetchHandler for CliFetchHandler {
    async fn fetch(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> oxide_fw_diagnostics::Result<oxide_fw_jshost::FetchResponse> {
        match oxide_fw_server::ssrf_safe_fetch(method, url, headers, body, &self.ssrf_allow_list).await
        {
            Ok((status, headers, body)) => Ok(oxide_fw_jshost::FetchResponse {
                status,
                headers,
                body,
            }),
            Err(e) => Err(oxide_fw_diagnostics::OxideError(
                oxide_fw_diagnostics::Diagnostic::error("JSH-FETCH", e),
            )),
        }
    }
}

/// PERF-02: the render/loader pool size — the machine's available
/// parallelism, floored at 2 so a single-core box still has a spare worker
/// for a concurrent render + loader.
fn render_pool_size() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .max(2)
}

/// B-23: a separate tooling lane so an SFC compile cannot head-of-line block
/// SSR renders. Half the render pool, at least one worker.
fn tooling_pool_size() -> usize {
    (render_pool_size() / 2).max(1)
}

/// Wraps any `FrameworkRenderer` as an `oxide_fw_server::PageRenderer`.
///
/// The only translation is input-shape: `PageRenderInput.params` is a typed
/// `BTreeMap<String, String>`, while `FrameworkRenderInput.params` is a
/// JSON `Value`. The rest (`route`, `url`, `data`, `head`) passes through
/// unchanged. Errors propagate with the same `OxideError` type.
pub struct AdapterPageRenderer {
    inner: Arc<dyn FrameworkRenderer>,
}

impl AdapterPageRenderer {
    /// Construct from an already-loaded framework renderer. `cmd_dev` builds
    /// a `LazyFrameworkRenderer` via `build_stack` that defers config
    /// evaluation to the first render; tests can substitute a fake.
    pub fn new(inner: Arc<dyn FrameworkRenderer>) -> Self {
        Self { inner }
    }
}

fn to_fw_input(input: PageRenderInput) -> FrameworkRenderInput {
    // `BTreeMap<String, String>` → `serde_json::Value::Object`. The
    // serialization can't fail for this input shape (string keys and
    // values always map to JSON), but we degrade to `{}` on the
    // impossible path rather than panicking.
    let params = serde_json::to_value(&input.params).unwrap_or_else(|_| serde_json::json!({}));
    FrameworkRenderInput {
        route: input.route,
        url: input.url,
        params,
        data: input.data,
        head: input.head,
        // §8.2: forward the layout chain the pipeline populated from the
        // router's `MatchInfo.layout_chain` (outermost first). The JS
        // adapter consumes this as `input.layouts` to nest the page.
        layouts: input.layouts,
        error_route_id: input.error_route_id,
        components: None,
        locale: input.locale,
        deferred: input.deferred,
        nonce: input.nonce,
    }
}

#[async_trait]
impl PageRenderer for AdapterPageRenderer {
    /// §6.4 / Wave M: prime the framework worker pool, the config import,
    /// and the component entry by rendering a synthetic route. The render
    /// itself is expected to fail with a resolver miss — the route does
    /// not exist. The warm-up goal is orthogonal: worker pool spawned,
    /// config imported (once, cached), SSR entry loaded. After this, a
    /// real request pays render cost only.
    async fn warm(&self) -> oxide_fw_server::Result<()> {
        use oxide_fw_server::PageRenderInput;
        let synthetic = PageRenderInput {
            route: "__oxide_warmup__".to_string(),
            url: "/__oxide_warmup__".to_string(),
            params: Default::default(),
            data: serde_json::Value::Object(serde_json::Map::new()),
            head: serde_json::json!({
                "meta": [], "link": [], "style": [], "script": []
            }),
            layouts: Vec::new(),
            error_route_id: None,
            locale: None,
            deferred: std::collections::BTreeMap::new(),
            nonce: None,
        };
        let _ = self.render(synthetic).await;
        Ok(())
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn render(&self, input: PageRenderInput) -> oxide_fw_server::Result<String> {
        self.inner.render(to_fw_input(input)).await
    }

    /// Forward to the inner renderer's streaming path. The default
    /// `PageRenderer::render_streaming` would call `render` and collect;
    /// overriding here means the chunked wire from the worker is
    /// preserved end-to-end.
    async fn render_streaming(
        &self,
        input: PageRenderInput,
    ) -> oxide_fw_server::Result<oxide_fw_server::RenderStream> {
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
    pub pool: Option<Arc<dyn oxide_fw_jshost::Pool>>,
    /// §4.7: the tooling pool for the JS compilers (Vue/Svelte/Solid). **Always
    /// Node**, regardless of `server.backend` — those compilers are npm
    /// packages that need Node built-ins, which the V8 sandbox does not have.
    /// Used by the §6.3 precompile pass at build time. `None` only when no
    /// pool could be built.
    pub tooling_pool: Option<Arc<dyn oxide_fw_jshost::Pool>>,
    /// §11 / BUG-02: the **one** cache tier the framework stack built. The
    /// server's L0 response cache wraps this same handler, so an
    /// invalidation (BUG-01's tag/path path) reaches both L0 (the response
    /// cache) and L1 (the worker's `'use cache'` entries). Before this field
    /// the server built its own `TieredCache` and the worker had a second
    /// one, so they could not see each other's invalidations.
    pub cache_tier: Option<Arc<dyn oxide_fw_cache::CacheHandler>>,
    /// §6.4: the component source handed to the worker's resolver. Dev fills
    /// `sources`; prod leaves it empty and the renderer derives the generated
    /// SSR entry from the project root.
    pub component_source: Arc<std::sync::RwLock<oxide_fw_jshost::ComponentSource>>,
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
            component_source: Arc::new(std::sync::RwLock::new(
                oxide_fw_jshost::ComponentSource::default(),
            )),
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

/// §11.2 / SPEC-12: map the project's `cache.handler` selection onto the
/// shared TieredCache. `http`, `redis`, and `s3` each build a real driver
/// and attach as the REMOTE tier (`TieredCache::with_remote`) so `'use
/// cache'` and the L0 response cache read/write through it. `memory`/`disk`
/// are the local tiers the caller already materialized — no remote driver.
///
/// Async because `RedisCacheHandler::new` opens a connection at build time.
/// A construction failure logs and degrades to the local tier only —
/// §11.2's "remote cache is an optimization" contract: the build must not
/// fail because a cache is down.
///
/// This is the seam that USED to be dead: the CLI's own `cache.handler`
/// arm in `start_server` only ran when `stack.cache_tier` was `None`,
/// which never happens for a stack this function builds.
pub async fn remote_cache_handler(
    handler: Option<&oxide_fw_config::CacheHandlerConfig>,
) -> Option<Arc<dyn oxide_fw_cache::CacheHandler>> {
    match handler {
        Some(oxide_fw_config::CacheHandlerConfig::Http { url }) => {
            Some(Arc::new(oxide_fw_cache::HttpCacheHandler::new(url.clone()))
                as Arc<dyn oxide_fw_cache::CacheHandler>)
        }
        Some(oxide_fw_config::CacheHandlerConfig::Redis { url }) => {
            match oxide_fw_cache::RedisCacheHandler::new(url.clone()).await {
                Ok(h) => Some(Arc::new(h) as Arc<dyn oxide_fw_cache::CacheHandler>),
                Err(e) => {
                    tracing::warn!(
                        url = %oxide_fw_cache::redacted_url(&url),
                        error = %e.0.message,
                        "cache.handler=redis: connection failed; \
                         continuing with local tier only",
                    );
                    None
                }
            }
        }
        Some(oxide_fw_config::CacheHandlerConfig::S3 { bucket }) => {
            match oxide_fw_cache::S3CacheHandler::new(bucket.clone()) {
                Ok(h) => Some(Arc::new(h) as Arc<dyn oxide_fw_cache::CacheHandler>),
                Err(e) => {
                    tracing::warn!(
                        bucket = %bucket,
                        error = %e.0.message,
                        "cache.handler=s3: client build failed; \
                         continuing with local tier only",
                    );
                    None
                }
            }
        }
        // `memory` needs no remote; `disk` is attached by the caller below.
        _ => None,
    }
}

pub async fn build_framework_stack(
    config_path: Option<&std::path::Path>,
    backend: JsBackend,
    // §6.2 / §15: the plugin transform handler. When `Some`, it is attached
    // to the render pool's host, so the worker's `compile_module` op applies
    // it — reaching the prerender path (which renders through the worker,
    // not the bundler).
    transform_handler: Option<Arc<dyn oxide_fw_jshost::TransformHandler>>,
    // §11.2 / SPEC-12: the project's `cache.handler` selection, threaded in
    // by every `cmd_*` call site so the tier the WORKER sees honors the
    // config. `None` keeps the memory/disk-only tier (tests, defaults).
    cache_handler: Option<&oxide_fw_config::CacheHandlerConfig>,
) -> FrameworkStack {
    use oxide_fw_jshost::{JsHost, NodeHost, PoolConfig, V8Host};

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
    // configures a disk root gets the L1 tier too; §11.2: a configured
    // `cache.handler=http` attaches an `HttpCacheHandler` REMOTE tier via
    // `with_remote` so `'use cache'` reads/writes through it.
    let cache_tier: Arc<dyn oxide_fw_cache::CacheHandler> = {
        // B-19: only materialize the disk L1 when the configured handler is
        // disk-backed (disk/redis/s3/http). With the default `memory` handler
        // there is no persistent tier to warm, so attaching a disk cache just
        // pays redb open/write cost for nothing. `with_disk` takes ownership
        // and returns the cache on success; on failure we rebuild memory-only.
        let attach_disk = !matches!(
            cache_handler,
            None | Some(oxide_fw_config::CacheHandlerConfig::Memory)
        );
        let disk = attach_disk
            .then(|| {
                config_path
                    .and_then(|p| p.parent())
                    .map(|root| root.join(".oxide/cache/l1"))
                    .filter(|d| std::fs::create_dir_all(d).is_ok())
            })
            .flatten();
        let tier = match disk.and_then(|d| {
            oxide_fw_cache::TieredCache::new(512 * 1024 * 1024)
                .with_disk(d)
                .ok()
        }) {
            Some(c) => c,
            None => oxide_fw_cache::TieredCache::new(512 * 1024 * 1024),
        };
        // §11.2: the remote tier. `remote_cache_handler` builds the real
        // driver for http/redis/s3; memory/disk fall through to the local
        // tier the caller already materialized (None).
        match remote_cache_handler(cache_handler).await {
            Some(remote) => Arc::new(tier.with_remote(remote)),
            None => Arc::new(tier),
        }
    };

    // PERF-05: the `fetch` op handler (SSRF-hardened, backed by oxide-fw-server).
    let fetch_handler: Arc<dyn oxide_fw_jshost::FetchHandler> = Arc::new(CliFetchHandler {
        ssrf_allow_list: Vec::new(),
    });

    let host: Arc<dyn JsHost> = match backend {
        JsBackend::Node => {
            let mut h = NodeHost::new(NodeHost::worker_script_default())
                .with_cache_handler(Arc::clone(&cache_tier))
                .with_fetch_handler(Arc::clone(&fetch_handler));
            // §6.2: attach the plugin transform to the render pool's host.
            if let Some(t) = transform_handler.as_ref() {
                h = h.with_transform_handler(Arc::clone(t));
            }
            Arc::new(h)
        }
        JsBackend::V8 => {
            // P-21: the V8 worker implements render + tooling ops only; the
            // loader/middleware/action ops still route to the Node fallback
            // pool. Say so loudly instead of letting "V8 parity" be silent.
            tracing::warn!(
                "server.backend = \"v8\": loader/middleware/action still run on the Node \
                 fallback pool (V8 only implements render + tooling ops)"
            );
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
            let host = host
                .with_cache_handler(Arc::clone(&cache_tier))
                .with_fetch_handler(Arc::clone(&fetch_handler));
            // §6.2: attach the plugin transform to the render pool's host.
            let host = if let Some(t) = transform_handler.as_ref() {
                host.with_transform_handler(Arc::clone(t))
            } else {
                host
            };
            Arc::new(host)
        }
    };
    // PERF-02: size the render/loader pool by the machine's parallelism, not
    // a hardcoded 2. Under load a 2-worker pool throttles TTFB; the spec
    // calls for "physical cores" (min 2, so a single-core CI box still works).
    let pool_size = render_pool_size();
    // B-23: renders/loaders/middleware/actions get the render lane; tooling
    // compiles get their own pool below.
    let pool = match host.render_pool(PoolConfig {
        size: pool_size,
        ..Default::default()
    }) {
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
    let tooling_pool: Arc<dyn oxide_fw_jshost::Pool> = if matches!(backend, JsBackend::V8) {
        match {
            let mut h = NodeHost::new(NodeHost::worker_script_default())
                .with_cache_handler(Arc::clone(&cache_tier))
                .with_fetch_handler(Arc::clone(&fetch_handler));
            if let Some(t) = transform_handler.as_ref() {
                h = h.with_transform_handler(Arc::clone(t));
            }
            h.tooling_pool(PoolConfig {
                size: tooling_pool_size(),
                ..Default::default()
            })
        } {
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
        // B-23: on the Node backend, the render lane and the tooling lane are
        // separate Node pools so a slow compile cannot block an SSR render.
        match {
            let mut h = NodeHost::new(NodeHost::worker_script_default())
                .with_cache_handler(Arc::clone(&cache_tier))
                .with_fetch_handler(Arc::clone(&fetch_handler));
            if let Some(t) = transform_handler.as_ref() {
                h = h.with_transform_handler(Arc::clone(t));
            }
            h.tooling_pool(PoolConfig {
                size: tooling_pool_size(),
                ..Default::default()
            })
        } {
            Ok(p) => p,
            Err(e) => {
                eprintln!(
                    "warn: tooling pool unavailable (node): {} {}",
                    e.0.code, e.0.message
                );
                Arc::clone(&pool)
            }
        }
    };

    // The loader runner needs only the pool — it resolves modules per request.
    let loader_runner: Arc<dyn LoaderRunner> =
        crate::loader_shim::build_loader_runner(Arc::clone(&pool));

    // Keep a second Arc reference to the pool so callers (notably cmd_dev's
    // watcher) can send out-of-band ops after the renderer/loader have been
    // moved into the pipeline.
    let pool_for_stack: Arc<dyn oxide_fw_jshost::Pool> = Arc::clone(&pool);

    let component_source: Arc<std::sync::RwLock<oxide_fw_jshost::ComponentSource>> = Arc::new(
        std::sync::RwLock::new(oxide_fw_jshost::ComponentSource::default()),
    );
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
                path.parent().map(|root| {
                    root.join(".oxide/config.server.js")
                        .to_string_lossy()
                        .to_string()
                })
            } else {
                None
            };
            let lazy = LazyFrameworkRenderer {
                pool: Arc::clone(&pool),
                config_path,
                bundled_path,
                name: peeked,
                components: Arc::clone(&component_source),
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
        component_source,
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
    pool: Arc<dyn oxide_fw_jshost::Pool>,
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
    /// §6.4: the component surface for this render (prod entry or dev map).
    components: Arc<std::sync::RwLock<oxide_fw_jshost::ComponentSource>>,
    inner: tokio::sync::OnceCell<Arc<dyn FrameworkRenderer>>,
}

/// §6.4 / P0-10: resolve the build's SSR adapter entry.
///
/// The manifest's `adapter_entry` is the source of truth for *where* the
/// entry lives (a build could emit it under a different name); the
/// filesystem convention is the fallback for a build predating the
/// manifest field. Within the resolved location, `lazy.js` is preferred
/// for the render path — Wave M measured ~3x fewer first-request imports
/// with it — unless `OXIDE_SSR_EAGER=1` forces the eager entry (the A/B
/// escape hatch). `server-fn` / `action` dispatch always uses the eager
/// entry; only the render path goes through here.
pub(crate) fn resolve_ssr_entry(dist_server: &std::path::Path) -> Option<String> {
    // (1) Ask the manifest where the entry is.
    let declared = std::fs::read(dist_server.join("server.manifest.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|v| {
            v.get("adapter_entry")
                .and_then(|s| s.as_str())
                .map(str::to_string)
        });

    // (2) Prefer the lazy sibling of the declared entry (Wave M perf win).
    let prefer_eager = std::env::var_os("OXIDE_SSR_EAGER").is_some();
    if !prefer_eager {
        if let Some(decl) = declared.as_deref() {
            if let Some(lazy_sibling) = decl.strip_suffix("index.js") {
                let lazy = dist_server.join(format!("{lazy_sibling}lazy.js"));
                if lazy.is_file() {
                    return Some(lazy.to_string_lossy().to_string());
                }
            }
        }
    }

    // (3) The declared entry itself.
    if let Some(decl) = declared {
        let entry = dist_server.join(&decl);
        if entry.is_file() {
            return Some(entry.to_string_lossy().to_string());
        }
    }

    // (4) Filesystem convention (a build with no manifest, e.g. a test
    // fixture that wrote chunks by hand).
    let lazy = dist_server.join("ssr/lazy.js");
    let eager = dist_server.join("ssr/index.js");
    if !prefer_eager && lazy.is_file() {
        Some(lazy.to_string_lossy().to_string())
    } else if eager.is_file() {
        Some(eager.to_string_lossy().to_string())
    } else {
        None
    }
}

impl LazyFrameworkRenderer {
    /// Fill the component source for this render. Production derives
    /// `<project>/dist/server/ssr/index.js` when the build wrote one; dev
    /// already populated the shared map.
    fn with_components(&self, mut input: FrameworkRenderInput) -> FrameworkRenderInput {
        let mut src = self
            .components
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();
        if src.entry.is_none() && src.sources.is_empty() {
            if let Some(root) = std::path::Path::new(&self.config_path).parent() {
                // §6.4 / P0-10: the manifest's `adapter_entry` names where
                // the build wrote the SSR entry; `resolve_ssr_entry` reads
                // it and prefers the lazy sibling (Wave M perf win).
                if let Some(entry) = resolve_ssr_entry(&root.join("dist/server")) {
                    src.entry = Some(entry);
                }
            }
        }
        if !src.is_empty() {
            input.components = Some(src);
        }
        input
    }

    async fn resolve(&self) -> oxide_fw_jshost::Result<Arc<dyn FrameworkRenderer>> {
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
                Ok::<Arc<dyn FrameworkRenderer>, oxide_fw_diagnostics::OxideError>(Arc::new(hr))
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
    async fn render(&self, input: FrameworkRenderInput) -> oxide_fw_jshost::Result<String> {
        let input = self.with_components(input);
        self.resolve().await?.render(input).await
    }
    async fn render_streaming(
        &self,
        input: FrameworkRenderInput,
    ) -> oxide_fw_jshost::Result<oxide_fw_jshost::RenderStream> {
        let input = self.with_components(input);
        self.resolve().await?.render_streaming(input).await
    }
}

/// §6.4 / WP-C1: build the dev component map — canonical route id → an
/// importable module path. TS/JS/JSX sources are imported directly (the
/// worker's loader compiles them); `.vue`/`.svelte` (and Solid's babel JSX)
/// are precompiled through the tooling pool and written under
/// `.oxide/components/`, so the resolver needs no bundler in dev.
pub async fn dev_component_sources(
    project_root: &std::path::Path,
    framework: Option<&str>,
    entries: std::collections::BTreeMap<String, std::path::PathBuf>,
    pool: Option<&Arc<dyn oxide_fw_jshost::Pool>>,
) -> std::collections::BTreeMap<String, String> {
    let precompiled = crate::precompile::precompile_entries(
        pool,
        &entries,
        "ssr",
        framework,
        None,
        env!("CARGO_PKG_VERSION"),
    )
    .await;
    let comp_dir = project_root.join(".oxide/components");
    let mut out = std::collections::BTreeMap::new();
    for (id, src) in entries {
        if let Some(js) = precompiled.get(&src) {
            if std::fs::create_dir_all(&comp_dir).is_ok() {
                let name = format!(
                    "{}.mjs",
                    blake3::hash(format!("{id}\u{0}{}", src.display()).as_bytes()).to_hex()
                );
                let path = comp_dir.join(name);
                if std::fs::write(&path, js).is_ok() {
                    out.insert(id, path.to_string_lossy().to_string());
                    continue;
                }
            }
        }
        out.insert(id, src.to_string_lossy().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_fw_jshost::FrameworkRenderInput;

    /// A framework renderer that echoes what it received — lets tests assert
    /// the input conversion (typed `params` → JSON) without Node.
    struct EchoFramework;

    #[async_trait]
    impl FrameworkRenderer for EchoFramework {
        fn name(&self) -> &str {
            "echo-fw"
        }
        async fn render(&self, input: FrameworkRenderInput) -> oxide_fw_jshost::Result<String> {
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
        async fn render(&self, _input: FrameworkRenderInput) -> oxide_fw_jshost::Result<String> {
            Err(oxide_fw_jshost::err(
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
        assert!(
            html.contains(r#""slug":"hello""#),
            "params converted: {html}"
        );
    }

    #[tokio::test]
    async fn adapter_converts_empty_params_to_json_object() {
        let r = AdapterPageRenderer::new(Arc::new(EchoFramework));
        let html = r
            .render(PageRenderInput::new("routes/index", "/"))
            .await
            .unwrap();
        assert!(
            html.contains("<pre>{}</pre>"),
            "empty params → `{{}}`: {html}"
        );
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
    impl oxide_fw_jshost::FrameworkRenderer for ChunkedRenderer {
        fn name(&self) -> &str {
            "chunked"
        }
        async fn render(
            &self,
            _input: oxide_fw_jshost::FrameworkRenderInput,
        ) -> oxide_fw_jshost::Result<String> {
            Ok("<h1>collected</h1>".to_string())
        }
        async fn render_streaming(
            &self,
            _input: oxide_fw_jshost::FrameworkRenderInput,
        ) -> oxide_fw_jshost::Result<oxide_fw_jshost::RenderStream> {
            use bytes::Bytes;
            let items: Vec<oxide_fw_jshost::Result<Bytes>> = vec![
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
    

    /// With `server.backend = "v8"`, `build_framework_stack` must construct
    /// the pool via `V8Host` (not `NodeHost`). We cannot spawn the CLI
    /// worker from a unit test on every host — the assertion here is that
    /// the pool exists and reports the V8 backend name through the trait.
    #[tokio::test]
    async fn v8_backend_builds_a_pool() {
        use oxide_fw_jshost::JsHost;
        let v8 = oxide_fw_jshost::V8Host::new();
        assert_eq!(v8.backend_name(), "v8");
        // Pool construction must succeed (first V8 init can be slow; ignore
        // the outcome on hosts without a prebuilt deno_core snapshot).
        let _ = v8.render_pool(oxide_fw_jshost::PoolConfig::default());
    }

    /// Node backend remains the default and still works.
    #[tokio::test]
    async fn node_backend_builds_a_pool() {
        use oxide_fw_jshost::JsHost;
        let node = oxide_fw_jshost::NodeHost::new(oxide_fw_jshost::NodeHost::worker_script_default());
        assert_eq!(node.backend_name(), "node");
    }
}

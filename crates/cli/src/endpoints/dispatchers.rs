//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// §12.1 step 3: load `dist/client/` into the pipeline's static asset store so
/// the shell's `<script type="module" src="/client/...">` resolves. Returns
/// `None` when the directory is missing (dev before a build, or an app with
/// no client bundle) — the pipeline then 404s those paths, which is honest.
///
/// `StaticAssets::scan` indexes every file by its URL path relative to the
/// root, so `<root>/client/counter-abc.js` is reachable at
/// `/client/counter-abc.js` when the root itself is `dist/`. Since our
/// directory is `dist/client` and the URL prefix is `/client`, we scan
/// `dist/` — then the boot chunk's `/client/...` URL matches the file path.
/// §16.1: adapt `oxide_fw_assets::ImageBackend` to the `oxide-fw-server` seam.
///
/// The two crates do not know about each other — `oxide-fw-server` names only
/// the trait, `oxide-fw-assets` names only its own request/result types — so
/// the CLI is where the two shapes are joined. The transform itself is
/// unchanged; only the field names differ.
pub(crate) struct AssetsImageBackend;

impl oxide_fw_server::ImageEndpointBackend for AssetsImageBackend {
    fn process(
        &self,
        req: oxide_fw_server::ImageEndpointRequest,
    ) -> std::result::Result<oxide_fw_server::ImageEndpointResult, String> {
        use oxide_fw_assets::{ImageBackend as _, ImageFormat, ImageRequest, RustImageBackend};

        // Detect the source format from the bytes. The `image` crate's
        // guess is what the backend already relies on for decode; this is
        // a header sniff so the request carries a format the backend can
        // assert against.
        let source_format = if req.source.starts_with(b"\x89PNG") {
            ImageFormat::Png
        } else if req.source.starts_with(b"\xFF\xD8") {
            ImageFormat::Jpeg
        } else if req.source.starts_with(b"RIFF") {
            ImageFormat::Webp
        } else {
            // Default to PNG; the backend re-sniffs on decode anyway.
            ImageFormat::Png
        };

        let format = match req.format {
            Some("image/avif") => ImageFormat::Avif,
            Some("image/webp") => ImageFormat::Webp,
            Some("image/jpeg") => ImageFormat::Jpeg,
            Some("image/png") => ImageFormat::Png,
            _ => source_format,
        };

        let ireq = ImageRequest {
            source: req.source,
            source_format,
            width: req.width,
            height: req.height,
            format,
            quality: req.quality.unwrap_or(75),
            placeholder: req.placeholder.as_deref().and_then(|p| match p {
                "blur" => Some(oxide_fw_assets::PlaceholderKind::Blur),
                "pixelate" => Some(oxide_fw_assets::PlaceholderKind::Lqip),
                _ => None,
            }),
        };

        RustImageBackend
            .process(ireq)
            .map(|r| oxide_fw_server::ImageEndpointResult {
                bytes: r.bytes,
                content_type: r.format.mime(),
                cache_key: r.cache_key,
                placeholder: r.placeholder,
            })
            .map_err(|e| format!("{}: {}", e.0.code, e.0.message))
    }
}

/// §16.1: construct the endpoint from config. Returns `None` when the
/// project declares no remote image patterns AND has no `public/` or
/// `dist/client/` directory to serve from — a project with images off
/// wants a 404, not an endpoint that always errors.
/// §16.3: the OG-image endpoint. Enabled by default — the `oxide-fw-assets`
/// backend is pure-Rust (resvg + taffy + parley), so there is no external
/// process and no reason to gate it. A project that does not want the
/// endpoint can turn it off with a config flag (post-v0.1).
pub(crate) struct SvgOgBackendImpl;

impl oxide_fw_server::OgEndpointBackend for SvgOgBackendImpl {
    fn render(
        &self,
        tree: &serde_json::Value,
        width: u32,
        height: u32,
    ) -> std::result::Result<Vec<u8>, String> {
        use oxide_fw_assets::OgBackend as _;
        oxide_fw_assets::SvgOgBackend
            .render(tree, width, height)
            .map_err(|e| format!("{} {}", e.0.code, e.0.message))
    }
}

/// §9.4: the `ServerFnDispatcher` impl backed by the render worker.
///
/// Wraps `oxide_fw_jshost::ServerFnHost`, which forwards `(id, args)` to the
/// worker's `server-fn:run` op. `entry_path` is the SSR adapter entry
/// (`dist/server/ssr/index.js`) — the worker imports it once to populate
/// the `__oxide_fn_registry` global that the compiler's `__oxide_register_fn`
/// lines fill in.
///
/// The pipeline's `NativePrefix::ServerFn` arm calls this; the JSON body
/// the function returns becomes the `{ok, data}` envelope's `data`.
pub(crate) struct ServerFnDispatcherImpl {
    host: oxide_fw_jshost::ServerFnHost,
    entry_path: String,
}

#[async_trait::async_trait]
impl oxide_fw_server::ServerFnDispatcher for ServerFnDispatcherImpl {
    async fn dispatch(
        &self,
        id: &str,
        args_json: &[u8],
    ) -> std::result::Result<Vec<u8>, oxide_fw_server::DispatchError> {
        match self.host.run_coded(id, args_json, &self.entry_path).await {
            Ok(v) => {
                // The worker op returns `{value: <fn result>}` — a stable
                // envelope it can extend later (streaming, diagnostics). The
                // HTTP envelope wraps the *value*, not the worker's
                // container, so the client sees `data` = the fn's own return
                // and not `{value: …}`.
                let value = v.get("value").cloned().unwrap_or(v);
                serde_json::to_vec(&value).map_err(|e| {
                    oxide_fw_server::DispatchError::internal(format!("serialize fn result: {e}"))
                })
            }
            Err(e) => {
                // The worker's JSH-FN-NOT-FOUND maps to a 404 envelope; a
                // function that threw maps to a 400 with the message; any
                // other failure is a 500.
                let err = match e.code.as_str() {
                    "JSH-FN-NOT-FOUND" => oxide_fw_server::DispatchError {
                        status: 404,
                        code: "FN-NOT-FOUND".into(),
                        message: e.message,
                    },
                    "JSH-FN-THREW" => oxide_fw_server::DispatchError {
                        status: 400,
                        code: "FN-THREW".into(),
                        message: e.message,
                    },
                    _ => oxide_fw_server::DispatchError::internal(e.message),
                };
                Err(err)
            }
        }
    }
}

/// Build the dispatcher when a pool and an SSR entry are both present.
/// Otherwise `None`, and the pipeline answers every `/_oxide/fn/*` request
/// with a clean `FN-NO-DISPATCHER` 404. The §V2-F08 consult rides on
/// top: the native `*.server.rs` host (the libraries mounted under
/// `.oxide/rust/`) answers its §9.4 ids before the JS pool is asked.
pub(crate) fn build_server_fn_dispatcher(
    rootp: &Path,
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
) -> Option<std::sync::Arc<dyn oxide_fw_server::ServerFnDispatcher>> {
    let native = mount_native_fn_source(rootp);
    let js = build_js_server_fn_dispatcher(rootp, pool);
    if native.is_none() && js.is_none() {
        return None;
    }
    Some(std::sync::Arc::new(
        oxide_fw_server::RustFirstDispatcher::new(native, js),
    ))
}

/// The pre-F08 dispatcher (the JS pool backed by the render isolate),
/// unchanged in shape — the consult's fallback half.
pub(crate) fn build_js_server_fn_dispatcher(
    rootp: &Path,
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
) -> Option<std::sync::Arc<dyn oxide_fw_server::ServerFnDispatcher>> {
    let pool = pool?;
    // The SSR entry is written by `oxide build`; in dev without a prior
    // build there is none, which is correct — the dev render path uses
    // the source graph, not the emitted bundle.
    let entry = rootp.join("dist/server/ssr/index.js");
    if !entry.is_file() {
        return None;
    }
    let entry_path = entry.to_string_lossy().to_string();
    Some(std::sync::Arc::new(ServerFnDispatcherImpl {
        host: oxide_fw_jshost::ServerFnHost::new(std::sync::Arc::clone(pool)),
        entry_path,
    }))
}

/// §V2-F08: the native `*.server.rs` source behind the consult — the
/// libraries the build loop loaded into `.oxide/rust/` (`lib-<n>`),
/// mounted into the [`oxide_rustfn::RustFnHost`] and keyed by the
/// §9.4 ids the `.server.rs` stub generator used. The `#[server_fn]`
/// macro recorded `file!()` per export in the library's types side
/// export, so the id triple reconstructs without a Rust AST scan.
/// `None` when nothing is mounted: the consult degrades to the pure
/// JS pool. A broken library degrades loud-but-not-fatal (the `RFN-003`
/// warn names it; the JS pool still answers).
pub(crate) fn mount_native_fn_source(
    rootp: &Path,
) -> Option<std::sync::Arc<dyn oxide_fw_server::NativeFnSource>> {
    let rust_dir = rootp.join(".oxide/rust");
    if !rust_dir.is_dir() {
        return None;
    }
    let secret = match build_secret(rootp) {
        Ok(secret) => secret,
        Err(e) => {
            tracing::warn!(
                code = "RFN-LOAD",
                error = %e,
                "the build secret is unavailable; the native server functions stay unmounted"
            );
            return None;
        }
    };
    let mut host = oxide_rustfn::RustFnHost::new();
    let mut registered = 0usize;
    let entries = match std::fs::read_dir(&rust_dir) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(
                code = oxide_rustfn::host::RFN_LOAD,
                dir = %rust_dir.display(),
                error = %e,
                "the native function library directory is unreadable"
            );
            return None;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "so" | "dll" | "dylib") {
            continue;
        }
        let lib = match oxide_rustfn::RustFnLibrary::load(&path, &rust_dir) {
            Ok(lib) => lib,
            Err(e) => {
                tracing::warn!(
                    code = oxide_rustfn::host::RFN_LOAD,
                    path = %path.display(),
                    error = %e.0.message,
                    "native function library failed to mount"
                );
                continue;
            }
        };
        let index = host.mount(lib);
        let types = host.types_json();
        if let Some(types) = types {
            match serde_json::from_str::<oxide_rustfn::types::TypesPayload>(&types) {
                Ok(payload) => {
                    for f in &payload.fns {
                        let id = oxide_fw_compiler::server_fn_id(&f.file, &f.name, &secret);
                        host.register(id, index, &f.name);
                        registered += 1;
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        code = oxide_rustfn::types::TYPES_JSON,
                        path = %path.display(),
                        error = %e,
                        "the native function types payload did not parse; its exports stay unrouted"
                    );
                }
            }
        }
    }
    if registered == 0 {
        return None;
    }
    Some(std::sync::Arc::new(NativeFnSourceImpl {
        host: std::sync::Arc::new(host),
    }))
}

/// The consult's native half: `oxide-rustfn`'s registry behind the
/// server crate's `NativeFnSource` seam (the registry error kinds map
/// onto the same envelope shapes the JS pool produces).
pub(crate) struct NativeFnSourceImpl {
    host: std::sync::Arc<oxide_rustfn::RustFnHost>,
}

#[async_trait::async_trait]
impl oxide_fw_server::NativeFnSource for NativeFnSourceImpl {
    fn has(&self, id: &str) -> bool {
        self.host.has(id)
    }
    async fn call(
        &self,
        id: &str,
        args_json: &[u8],
    ) -> std::result::Result<Vec<u8>, oxide_fw_server::DispatchError> {
        use oxide_rustfn::RustFnRegistry as _;
        self.host
            .call(id, args_json)
            .await
            .map_err(|e| oxide_fw_server::DispatchError {
                status: e.status(),
                code: e.envelope_code().into(),
                message: e.message,
            })
    }
}

/// §9.3: a `LoaderRunner`-shaped bridge to the worker's `action:run` op.
pub(crate) struct ActionDispatcherImpl {
    host: oxide_fw_jshost::ActionHost,
}

#[async_trait::async_trait]
impl oxide_fw_server::ActionDispatcher for ActionDispatcherImpl {
    async fn dispatch(
        &self,
        args: ActionDispatchArgs<'_>,
    ) -> std::result::Result<oxide_fw_server::ActionRunResult, oxide_fw_server::DispatchError> {
        let ActionDispatchArgs {
            module_path,
            action,
            form_json,
            params,
            url,
            method,
            headers,
            locals,
            trace,
        } = args;

        match self
            .host
            .run(ActionRunArgs {
                module_path,
                action,
                form_json,
                params,
                url,
                method,
                headers,
                locals,
                trace,
            })
            .await
        {
            Ok(out) => Ok(oxide_fw_server::ActionRunResult {
                status: out.status,
                location: out.location,
                data: out.data,
                set_cookies: out.set_cookies,
                headers: out.headers,
                revalidate: out
                    .revalidate
                    .into_iter()
                    .map(|ev| match ev {
                        oxide_fw_jshost::RevalidateEvent::Tag { tag } => ("tag".to_string(), tag),
                        oxide_fw_jshost::RevalidateEvent::Path { path } => {
                            ("path".to_string(), path)
                        }
                    })
                    .collect(),
            }),
            Err(e) => {
                let code = e.0.code.to_string();
                let status = if code.contains("NOT-FOUND") { 404 } else { 500 };
                Err(oxide_fw_server::DispatchError {
                    status,
                    code: code.replace("JSH-ACTION", "ACT"),
                    message: e.0.message.to_string(),
                })
            }
        }
    }
}

/// §9.3: build the action dispatcher when a pool is present.
pub(crate) fn build_action_dispatcher(
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
) -> Option<std::sync::Arc<dyn oxide_fw_server::ActionDispatcher>> {
    let pool = pool?;
    Some(std::sync::Arc::new(ActionDispatcherImpl {
        host: oxide_fw_jshost::ActionHost::new(std::sync::Arc::clone(pool)),
    }))
}

/// §8.1 route.ts / §10.2 step 7: a bridge to the worker's `endpoint:run` op.
pub(crate) struct EndpointDispatcherImpl {
    host: oxide_fw_jshost::EndpointHost,
}

#[async_trait::async_trait]
impl oxide_fw_server::EndpointDispatcher for EndpointDispatcherImpl {
    async fn dispatch(
        &self,
        args: EndpointDispatchArgs<'_>,
    ) -> std::result::Result<oxide_fw_server::EndpointRunResult, oxide_fw_server::EndpointRunError>
    {
        let EndpointDispatchArgs {
            module_path,
            method,
            url,
            params,
            headers,
            body,
            locals,
            trace,
        } = args;

        match self
            .host
            .run(LoaderRunArgs {
                module_path,
                method,
                url,
                params,
                headers,
                body,
                locals,
                trace,
            })
            .await
        {
            Ok(out) => {
                // §9.6: a streaming outcome's receiver becomes a `BodyStream`.
                let stream: Option<oxide_fw_server::BodyStream> = out.stream.map(|mut rx| {
                    // The worker's chunk receiver carries `OxideError`; the
                    // transport body stream carries `std::io::Error`. Pump
                    // through a fresh channel that maps at the boundary,
                    // preserving the diagnostic code/message.
                    let (tx, mut out_rx) = tokio::sync::mpsc::channel::<
                        std::result::Result<bytes::Bytes, std::io::Error>,
                    >(8);
                    tokio::spawn(async move {
                        while let Some(item) = rx.recv().await {
                            let mapped = item.map_err(|e| {
                                std::io::Error::other(format!("{}: {}", e.0.code, e.0.message))
                            });
                            if tx.send(mapped).await.is_err() {
                                return;
                            }
                        }
                    });
                    Box::pin(futures::stream::poll_fn(move |cx| out_rx.poll_recv(cx)))
                        as oxide_fw_server::BodyStream
                });
                Ok(oxide_fw_server::EndpointRunResult {
                    status: out.status,
                    headers: out.headers,
                    body: out.body,
                    stream,
                })
            }
            Err(e) => Err(oxide_fw_server::EndpointRunError {
                status: 500,
                code: e.0.code.to_string().replace("JSH-ENDPOINT", "ENDPOINT"),
                message: e.0.message.to_string(),
            }),
        }
    }
}

/// §9.6: the URL-pattern → `socket.ts` map for the WebSocket service. A
/// `socket.ts` at `routes/chat/[room]/socket` maps to `/chat/:room`; the
/// root `routes/socket` maps to `/`. Params become `:name`; catch-all `[...x]`
/// becomes `*`.
pub(crate) fn cfg_socket_routes(
    rootp: &std::path::Path,
    cfg: &oxide_fw_config::OxideConfig,
    tree: &oxide_fw_router::RouteTree,
) -> std::collections::BTreeMap<String, std::path::PathBuf> {
    let routes_root = rootp.join(&cfg.routes.dir);
    let mut out = std::collections::BTreeMap::new();
    for e in &tree.entries {
        if !matches!(e.kind, oxide_fw_router::RouteKind::Socket) {
            continue;
        }
        // `routes/chat/[room]/socket` → segs `chat`, `[room]` → `/chat/:room`.
        let base =
            e.id.0
                .strip_prefix("routes")
                .unwrap_or(&e.id.0)
                .trim_matches('/')
                .strip_suffix("socket")
                .unwrap_or("")
                .trim_matches('/');
        let mut path = String::from("/");
        if !base.is_empty() {
            let segments: Vec<String> = base
                .split('/')
                .map(|seg| {
                    if let Some(rest) = seg.strip_prefix("[...").and_then(|s| s.strip_suffix(']')) {
                        format!("*{rest}")
                    } else if let Some(name) =
                        seg.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
                    {
                        format!(":{name}")
                    } else if let Some(name) =
                        seg.strip_prefix("[[").and_then(|s| s.strip_suffix("]]"))
                    {
                        format!(":{name}")
                    } else {
                        seg.to_string()
                    }
                })
                .collect();
            path.push_str(&segments.join("/"));
        }
        out.insert(path, routes_root.join(&e.file));
    }
    out
}

/// §8.1 route.ts / §10.2 step 7: build the endpoint dispatcher when a pool is present.
pub(crate) fn build_endpoint_dispatcher(
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
) -> Option<std::sync::Arc<dyn oxide_fw_server::EndpointDispatcher>> {
    let pool = pool?;
    Some(std::sync::Arc::new(EndpointDispatcherImpl {
        host: oxide_fw_jshost::EndpointHost::new(std::sync::Arc::clone(pool)),
    }))
}

/// §8.1 middleware.ts / §8.5: a bridge to the worker's `middleware:run` op.
pub(crate) struct MiddlewareDispatcherImpl {
    host: oxide_fw_jshost::MiddlewareHost,
}

#[async_trait::async_trait]
impl oxide_fw_server::MiddlewareDispatcher for MiddlewareDispatcherImpl {
    async fn dispatch(
        &self,
        args: MiddlewareDispatchArgs<'_>,
    ) -> std::result::Result<
        oxide_fw_server::MiddlewareRunResult,
        oxide_fw_server::MiddlewareRunError,
    > {
        let MiddlewareDispatchArgs {
            module_path,
            method,
            url,
            params,
            headers,
            body,
            locals,
        } = args;

        match self
            .host
            .run(EndpointRunArgs {
                module_path,
                method,
                url,
                params,
                headers,
                body,
                locals,
            })
            .await
        {
            Ok(out) => Ok(oxide_fw_server::MiddlewareRunResult {
                short: out.short,
                status: out.status,
                headers: out.headers,
                body: out.body,
                redirect: out.redirect,
                rewrite: out.rewrite,
                locals: out.locals,
            }),
            Err(e) => Err(oxide_fw_server::MiddlewareRunError {
                status: 500,
                code: e.0.code.to_string().replace("JSH-MIDDLEWARE", "MIDDLEWARE"),
                message: e.0.message.to_string(),
            }),
        }
    }
}

/// §8.1 middleware.ts / §8.5: build the middleware dispatcher when a pool is present.
pub(crate) fn build_middleware_dispatcher(
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
) -> Option<std::sync::Arc<dyn oxide_fw_server::MiddlewareDispatcher>> {
    let pool = pool?;
    Some(std::sync::Arc::new(MiddlewareDispatcherImpl {
        host: oxide_fw_jshost::MiddlewareHost::new(std::sync::Arc::clone(pool)),
    }))
}

/// §9.6: a bridge to the worker's `socket:run` op.
pub(crate) struct SocketDispatcherImpl {
    host: oxide_fw_jshost::SocketHost,
}

#[async_trait::async_trait]
impl oxide_fw_server::SocketDispatcher for SocketDispatcherImpl {
    async fn run(
        &self,
        module_path: &str,
        phase: &str,
        payload: serde_json::Value,
        socket_id: &str,
    ) -> std::result::Result<oxide_fw_server::SocketRunResult, oxide_fw_server::SocketRunError>
    {
        match self.host.run(module_path, phase, payload, socket_id).await {
            Ok(out) => Ok(oxide_fw_server::SocketRunResult {
                sends: out.sends,
                broadcasts: out.broadcasts,
                targets: out.targets,
                close: out.close,
            }),
            Err(e) => Err(oxide_fw_server::SocketRunError {
                status: 500,
                code: e.0.code.to_string().replace("JSH-SOCKET", "SOCKET"),
                message: e.0.message.to_string(),
            }),
        }
    }
}

/// §9.6: build the socket dispatcher when a pool is present.
pub(crate) fn build_socket_dispatcher(
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
) -> Option<std::sync::Arc<dyn oxide_fw_server::SocketDispatcher>> {
    let pool = pool?;
    Some(std::sync::Arc::new(SocketDispatcherImpl {
        host: oxide_fw_jshost::SocketHost::new(std::sync::Arc::clone(pool)),
    }))
}

/// V2-F09 (feature `search`): construct the search endpoint's attached
/// state from the typed `search` config and the project root. The index
/// is the build's artifact (`dist/search/index`); the open is read-only
/// mmap and best-effort — a missing or corrupt index logs a warning (the
/// code and message of the engine's `SRC-101`) and returns `None`, so
/// `/_oxide/search` answers the documented not-configured 404 instead of
/// failing the serve start. Unconfigured `search.collections` is the
/// plain `None` path (nothing is constructed, nothing logged).
///
/// Returns the endpoint AND the hot-reload handle (the dev watcher's
/// swap target): the handle is `Some` exactly when the endpoint is —
/// both stand or fall with the open — and the endpoint serves THROUGH
/// the handle, so a dev-loop swap is visible to `/_oxide/search` without
/// any further wiring.
#[cfg(feature = "search")]
pub(crate) fn build_search_endpoint(
    search: &oxide_fw_config::SearchConfig,
    rootp: &std::path::Path,
) -> (
    Option<std::sync::Arc<oxide_fw_server::SearchEndpoint>>,
    Option<std::sync::Arc<oxide_fw_search::SwappableSearchIndex>>,
) {
    if search.collections.is_empty() {
        return (None, None);
    }
    let dir = rootp
        .join("dist")
        .join("search")
        .join(oxide_fw_search::SEARCH_INDEX_DIR);
    match oxide_fw_search::SwappableSearchIndex::open(
        &dir,
        &oxide_fw_search::SearchEngineOptions::from_config(search),
    ) {
        Ok(hot) => {
            let hot = std::sync::Arc::new(hot);
            let endpoint = std::sync::Arc::new(oxide_fw_server::SearchEndpoint::new(hot.clone()));
            (Some(endpoint), Some(hot))
        }
        Err(e) => {
            tracing::warn!(
                code = e.0.code,
                message = %e.0.message,
                dir = %dir.display(),
                "search index not available; /_oxide/search answers the not-configured 404"
            );
            (None, None)
        }
    }
}

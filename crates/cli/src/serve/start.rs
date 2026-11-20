//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// A handle over a started server, so tests can shut it down deterministically.
/// Optional dev-server feature flags (§13.5).
#[derive(Debug, Clone, Default)]
pub struct DevFlags {
    pub host: bool,
    pub https: bool,
    pub qr: bool,
    pub open: bool,
    pub port: Option<u16>,
    pub proxy: Vec<String>,
    /// §2.9: opt out of the native ESM dev server (bundler path).
    pub no_esm: bool,
    /// V2-F15: keep the secret header trio verbatim in the records
    /// (§V2-F15 `--include-secrets`). Read through the
    /// `OXIDE_INCLUDE_SECRETS` env by the stack builder, so the flag
    /// reaches the recorder without threading a new parameter through the
    /// stack builder's signature.
    pub include_secrets: bool,
}

impl DevFlags {
    /// Parse the `--proxy` strings into `ProxyRule`s. Format:
    /// `[!]PREFIX=TARGET`, where a leading `!` requests prefix stripping.
    pub fn proxy_rules(&self) -> Vec<oxide_fw_dev::ProxyRule> {
        self.proxy
            .iter()
            .filter_map(|s| {
                let (strip, rest) = match s.strip_prefix('!') {
                    Some(r) => (true, r),
                    None => (false, s.as_str()),
                };
                let (prefix, target) = rest.split_once('=')?;
                let rule = oxide_fw_dev::ProxyRule::new(prefix, target);
                Some(if strip { rule.with_strip(true) } else { rule })
            })
            .collect()
    }
}

pub struct StartedServer {
    pub addr: std::net::SocketAddr,
    pub shutdown: tokio::sync::watch::Sender<bool>,
    /// Metrics registry (§20) shared with the pipeline and the dispatch
    /// delegate; exposed so tests and observability tooling can read it.
    pub metrics: std::sync::Arc<oxide_fw_server::Metrics>,
    /// SEC-09: the dev inspector token, when the inspector is attached (dev
    /// only). The dev banner prints it so the developer can open
    /// `/__oxide?token=…`; `None` in production.
    pub dev_token: Option<String>,
    /// SPEC-14: the dev HMR hub, when dev. The dev loop publishes file
    /// changes to it; `/_oxide/hmr` subscribers receive them.
    pub hmr: Option<std::sync::Arc<oxide_fw_dev::HmrHub>>,
    /// §10.8: the PPR shell store, when `render.ppr` is on. The dev watcher
    /// clears it on every change so a shell captured before an edit cannot
    /// outlive the source it was rendered from.
    pub(crate) ppr_shells: Option<PprShells>,
    /// WP-B: the route tree the dispatch reads through. The dev watcher
    /// stores a fresh tree here after re-scanning; `cmd_serve` never swaps.
    pub(crate) swappable_tree: SwappableRouteTree,
    /// V2-A04/F10: the generation pool the data plane checks against.
    /// The serve loop promotes a rebuilt dist's id here on `SIGHUP` (the
    /// old build stays accepted for its skew window, the 30 s task
    /// reclaims expired slots); tests promote through
    /// `SkewState::promote_from_dist`.
    pub(crate) generations:
        Option<std::sync::Arc<std::sync::RwLock<oxide_fw_dispatch::skew::GenerationPool>>>,
    /// V2-F10 (the serving half): the per-generation pools the SIGHUP
    /// promote boots into (`ServingPools::promote_booted`); `None` in dev
    /// and when the boot snapshot failed (acceptance-only skew).
    pub(crate) serving_pools: Option<std::sync::Arc<crate::serve::generations::ServingPools>>,
    /// V2-F09 (feature `search`): the hot-reload handle for the search
    /// index the serve side opened. The dev watcher swaps a rebuilt
    /// index in after re-emitting the corpus (the zero-downtime posture
    /// `refresh_search_index` documents); `cmd_serve` never swaps —
    /// production reindexes by building and starting anew.
    #[cfg(feature = "search")]
    pub(crate) hot_search: Option<std::sync::Arc<oxide_fw_search::SwappableSearchIndex>>,
    /// V2-F12: the brownout governor the sampler drives. The e2e pins
    /// flip the level through it (the same seam the sampler writes) to
    /// pin the enforcement stage and the readiness flip on the wire;
    /// `None` when `brownout.enabled` is off (the default).
    pub(crate) brownout: Option<std::sync::Arc<crate::serve::brownout::BrownoutWiring>>,
}

/// §5.6: wrap a dispatch so a mid-flight file change aborts instead of
/// serving a result computed from pre-change source.
///
/// Behaviour:
///   - Record the stale generation at entry.
///   - `tokio::select!` races the dispatch future against
///     `hub.wait_stale_generation()`.
///   - If the dispatch completes first AND the generation is unchanged:
///     serve normally.
///   - If the dispatch completes first BUT the generation changed during
///     the window (a race the select! poll order can produce): 503 + retry.
///   - If the change notification fires first: abort the dispatch (drop
///     the future, releasing the renderer / loader resources it held) and
///     503 + retry.
///
/// The 503 response carries:
///   - `retry-after: 0` — a browser fetch with default redirect mode will
///     not auto-retry, but the dev HMR client (which reloads on every file
///     change event) will. A manual reload also gets fresh content.
///   - `x-oxide-stale-generation: <new>` — audit trail: the request was
///     refused because the world advanced past its entry generation.
///
/// Streaming responses are unaffected: a streaming dispatch completes
/// its `OxideResponse` (with a live body stream) early, so the select!
/// window is a few milliseconds — the abort almost never fires. Non-
/// streaming (SSR, `/__data`, loaders) get the full guard.
pub(crate) fn wrap_dispatch_with_stale_guard(
    dispatch: std::sync::Arc<dyn Fn(oxide_fw_server::OxideRequest) -> BoxedResponse + Send + Sync>,
    hub: std::sync::Arc<oxide_fw_dev::HmrHub>,
) -> std::sync::Arc<dyn Fn(oxide_fw_server::OxideRequest) -> BoxedResponse + Send + Sync> {
    std::sync::Arc::new(move |req: oxide_fw_server::OxideRequest| {
        let hub = std::sync::Arc::clone(&hub);
        let inner = std::sync::Arc::clone(&dispatch);
        Box::pin(async move {
            let start = hub.stale_generation();
            let fut = inner(req);
            let result = tokio::select! {
                r = fut => r,
                _ = hub.wait_stale_generation() => {
                    return stale_503(start, hub.stale_generation());
                }
            };
            // Exit check: the select! poll order can let the dispatch
            // future complete on the first tick without polling the
            // notification future — a bump between entry and completion
            // would then be missed. This catches that narrow race.
            let now = hub.stale_generation();
            if now != start {
                return stale_503(start, now);
            }
            result
        })
    })
}

/// §5.6: the retryable 503 a mid-flight file change produces.
pub(crate) fn stale_503(start: u64, now: u64) -> oxide_fw_server::OxideResponse {
    let body = format!(
        "<!doctype html><html><head><title>stale</title></head><body>\
         <p>file changed during dispatch (generation {} to {}); retry</p>\
         </body></html>",
        start, now
    );
    let mut resp = oxide_fw_server::OxideResponse::html(503, body);
    resp.headers
        .push(("retry-after".to_string(), "0".to_string()));
    resp.headers
        .push(("x-oxide-stale-generation".to_string(), now.to_string()));
    resp
}

pub(crate) async fn start_server_inner(
    root: &str,
    port: Option<u16>,
    with_inspector: bool,
) -> anyhow::Result<StartedServer> {
    let bind = port.map(|p| (std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), p));
    start_server_inner_full(
        root,
        bind,
        with_inspector,
        None,
        Vec::new(),
        crate::framework_shim::FrameworkStack::default(),
        None,
    )
    .await
}

pub(crate) fn resolve_bind_host(host: &str) -> std::net::IpAddr {
    let h = host.trim();
    if h.is_empty() || h.eq_ignore_ascii_case("localhost") {
        return std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
    }
    if let Ok(ip) = h.parse::<std::net::IpAddr>() {
        return ip;
    }
    tracing::warn!(
        host = h,
        "server.host is not an IP literal; binding loopback (use an IP like 0.0.0.0 to expose)"
    );
    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
}

pub(crate) async fn start_server_inner_full(
    root: &str,
    bind: Option<(std::net::IpAddr, u16)>,
    with_inspector: bool,
    tls: Option<std::sync::Arc<tokio_rustls::rustls::ServerConfig>>,
    proxy_rules: Vec<oxide_fw_dev::ProxyRule>,
    stack: crate::framework_shim::FrameworkStack,
    hmr: Option<std::sync::Arc<oxide_fw_dev::HmrHub>>,
) -> anyhow::Result<StartedServer> {
    // Canonicalize so every path derived here (route modules, the SSR entry,
    // the client manifest) is absolute — the worker resolves a module path
    // against its own cwd, so a relative root breaks `action:run` / `loader:run`.
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    // §16.5 / WP-r9-1: OXIDE_ROOT is written PRE-RUNTIME by
    // `apply_startup_env` (P1 readiness env-race fix); nothing to do here.
    // §16.6 / WP-r10-1: the config env must match the server mode.
    // Before this fix, dev loaded "production" and so ignored
    // `security.sessionTtlSecs` set in the dev config (a common
    // testing knob — set a 60s TTL in dev, 1800s in prod). This is the
    // only place the TTL is published, so fixing the env here is the
    // whole fix.
    let cfg_env = if with_inspector {
        "development"
    } else {
        "production"
    };
    let cfg = load_config_or_default(&rootp, cfg_env);
    // §2.9: `--no-esm` is threaded via an env var because cmd_dev's
    // override on its own cfg never reaches this reload.
    let cfg = {
        let mut cfg = cfg;
        if std::env::var_os("OXIDE_DEV_NO_ESM").is_some() {
            cfg.dev.esm = false;
        }
        cfg
    };
    set_session_ttl(cfg.security.session_ttl_secs);
    // §10.1: refine inferred render modes from the AST, exactly as `oxide build`
    // does — otherwise the live server sees the scanner's line-oriented guess
    // (and `None` for a route with no `page.server.ts`), and the §11 L0 layer
    // cannot tell a static route from a dynamic one.
    let tree = std::sync::Arc::new(refine_render_modes_from_ast(
        &rootp,
        &cfg,
        scan_routes(&rootp, &cfg, stack.tooling_pool.as_ref()).await?,
    ));

    // Native ESM dev server (Phase 1): build the shared state the dispatch
    // intercept uses to serve `/src/*` and `/@*` as transformed ESM. Opt-in
    // via `dev.esm = true` in `oxide.config.ts`; only in dev
    // (`with_inspector`), so prod never builds it.
    let dev_esm = with_inspector && cfg.dev.esm;
    let esm_state: Option<std::sync::Arc<crate::esm_dev::EsmDevState>> = if dev_esm {
        // The configured framework decides (a) which `@oxide-fw/*` package
        // the boot entry wires as `clientAdapter`, (b) which framework
        // runtime packages must be prebundled for the client, and (c) which
        // `/src/*` files compile through the Node tooling pool.
        let esm_framework = cfg
            .framework
            .adapter
            .clone()
            .or_else(|| cfg.framework.name.clone())
            .unwrap_or_else(|| "react".to_string());
        let mut components: std::collections::BTreeMap<String, String> =
            std::collections::BTreeMap::new();
        for entry in &tree.entries {
            let id_str: &str = entry.id.0.as_str();
            let rel = id_str.strip_prefix("routes/").unwrap_or(id_str);
            let mut candidates: Vec<String> = Vec::new();
            // `.vue`/`.svelte` pages are first-class here: the ESM dev server
            // serves them pool-compiled (see `needs_pool_compile`), and
            // without them in the manifest a Vue/Svelte app had NO component
            // URL at all — `componentFor` returned undefined and the client
            // could never render or HMR the route.
            for ext in [".tsx", ".ts", ".jsx", ".js", ".vue", ".svelte"] {
                candidates.push(format!("src/routes/{rel}{ext}"));
                candidates.push(format!("src/routes/{rel}/page{ext}"));
            }
            if rel == "index" || rel.is_empty() {
                for ext in [".tsx", ".ts", ".jsx", ".js", ".vue", ".svelte"] {
                    candidates.push(format!("src/routes/page{ext}"));
                    candidates.push(format!("src/routes/index{ext}"));
                }
            }
            for cand in &candidates {
                if !rootp.join(cand).is_file() {
                    continue;
                }
                // REJECT server-only route files. These exist on disk under
                // `src/routes/` but must NEVER reach the browser: an API
                // route (route.ts), websocket (socket.ts), or route-level
                // middleware (middleware.ts) is server-executed and imports
                // server-only modules (e.g. `md.server.ts`) the browser
                // cannot fetch (our handler returns 403). Shipping one of
                // these into the boot entry broke the ENTIRE module graph:
                // the browser fetches it, hits a 403 on a transitive
                // `.server.` import, and never reaches the boot script that
                // opens the HMR WebSocket. Symptom: edit a file, nothing
                // happens, no console error, no HMR message.
                let file_name = cand.rsplit('/').next().unwrap_or(cand);
                let server_only = file_name.ends_with(".server.ts")
                    || file_name.ends_with(".server.tsx")
                    || file_name.ends_with(".server.js")
                    || file_name.ends_with(".server.jsx")
                    || file_name == "route.ts"
                    || file_name == "route.js"
                    || file_name == "socket.ts"
                    || file_name == "socket.js"
                    || file_name == "middleware.ts"
                    || file_name == "middleware.js";
                if server_only {
                    continue;
                }
                components.insert(id_str.to_string(), format!("/{cand}"));
                break;
            }
        }
        // Framework runtime packages must be in the prebundle + rewrite map
        // or the client graph 404s: the pool-compiled Vue SFC imports
        // `vue`, the Svelte output imports `svelte/internal/client`, the
        // Solid output imports `@solidjs/web`, and the `@oxide-fw/*`
        // adapter dist files import their framework runtime by bare name.
        // The default list is React-shaped; extend it per framework without
        // disturbing an explicit project override (already-present entries
        // stay where the config put them).
        let mut prebundle_list = cfg.dev.prebundle.clone();
        for extra in framework_prebundle_extras(&esm_framework) {
            if !prebundle_list.contains(&extra) {
                prebundle_list.push(extra);
            }
        }
        let _ = crate::esm_dev::prebundle_cjs_deps(&rootp, &prebundle_list);
        if let Err(e) = crate::esm_dev::build_esm_manifest(&rootp, &components) {
            tracing::warn!(error = %e, "esm dev: manifest write failed");
        }
        // `cfg.framework.islands` is the typed island registry
        // (component name → source path relative to project root).
        // Filter to entries that actually exist on disk so a stale config
        // doesn't produce a boot entry with a missing import. The boot
        // entry's import specifier is a ROOT-ABSOLUTE URL (a bare
        // relative path would resolve against the current page URL), so
        // the source rides in its URL spelling.
        let mut islands: std::collections::BTreeMap<String, oxide_fw_config::IslandSpec> =
            std::collections::BTreeMap::new();
        for (name, spec) in &cfg.framework.islands {
            let rel_path = spec.src();
            let full_path = rootp.join(rel_path);
            if full_path.is_file() {
                islands.insert(
                    name.clone(),
                    oxide_fw_config::IslandSpec::Plain(format!("/{rel_path}")),
                );
            }
        }
        if let Err(e) =
            crate::esm_dev::generate_esm_boot_entry(&rootp, &components, &islands, &esm_framework)
        {
            tracing::warn!(error = %e, "esm dev: boot entry write failed");
        }
        // §6.4 / WP-C1 (ESM path): `ensure_dev_client_bundle` is skipped in
        // ESM mode, so no `dist/server/ssr/index.js` is produced by the
        // dev server. Populate the worker's component-source map here
        // instead, so the render path can resolve routes without a build
        // entry. Guarded by emptiness because `cmd_dev` already populated
        // this map on the CLI path (and we'd rather not precompile twice).
        let already_populated = stack
            .component_source
            .read()
            .map(|s| !s.sources.is_empty())
            .unwrap_or(false);
        if !already_populated {
            populate_dev_component_source(&rootp, &cfg, &tree, &stack).await;
        }
        let rewrite_map = crate::esm_dev::build_rewrite_map(&rootp, &prebundle_list);
        tracing::info!(
            routes_with_component = components.len(),
            "esm dev: state ready"
        );
        Some(std::sync::Arc::new(crate::esm_dev::EsmDevState {
            root: rootp.clone(),
            jsx_import_source: jsx_import_source_for(&esm_framework),
            rewrite_map: std::sync::Arc::new(rewrite_map),
            framework: esm_framework,
            // The F02 dev warn: the boot-time island manifest (read-only,
            // `None` with the auto-islands gate off) and its per-session
            // warned-site dedup.
            islands: crate::build::dev_island_manifest(&rootp, &cfg).map(std::sync::Arc::new),
            warned_pure_on_props: std::sync::Arc::new(std::sync::Mutex::new(
                std::collections::BTreeSet::new(),
            )),
        }))
    } else {
        None
    };
    let (bind_ip, requested_port) = match bind {
        // A caller-supplied port is honoured verbatim — including 0, which
        // means "ephemeral, OS chooses" (the test and CI convention).
        Some((ip, p)) => (ip, p),
        None => (resolve_bind_host(&cfg.server.host), cfg.server.port),
    };
    let port = resolve_port(requested_port).await;
    let rules = oxide_fw_router::RulesEngine::compile(config_rules_to_router(&cfg.rules))
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    // §20 metrics: ONE registry shared by the pipeline (native branches) and
    // the dispatch delegate (per-route labels). Cardinality is bounded by the
    // route pattern (e.g. `/blog/[slug]`), never the resolved URL.
    let metrics = std::sync::Arc::new(oxide_fw_server::Metrics::new());
    // §V2-B03 item 4: freeze the route labels BEFORE the server starts — a
    // registered label's `observe` answers through the frozen map with no
    // lock. The same entry filter and `default_pattern_of` spelling the
    // dispatch context table builds with (dispatch/entry.rs), so the frozen
    // labels match what the matcher stage observes.
    metrics.register_routes(
        tree.entries
            .iter()
            .filter(|e| {
                !matches!(
                    e.kind,
                    oxide_fw_router::RouteKind::Layout
                        | oxide_fw_router::RouteKind::Middleware
                        | oxide_fw_router::RouteKind::Error
                        | oxide_fw_router::RouteKind::Loading
                )
            })
            .map(|e| std::sync::Arc::from(oxide_fw_router::default_pattern_of(e))),
    );

    let assets = load_client_assets(&rootp);
    let image_endpoint = if cfg.images_endpoint.enabled {
        build_image_endpoint(&rootp, &cfg)
    } else {
        None
    };
    // S-09: the OG rasterization endpoint is opt-in (`og.enabled: true`) — it
    // is CPU-bound and, when reachable, unauthenticated.
    let og_endpoint = if cfg.og.enabled {
        build_og_endpoint()
    } else {
        None
    };
    let server_fns = build_server_fn_dispatcher(&rootp, stack.pool.as_ref());
    let actions = build_action_dispatcher(stack.pool.as_ref());
    let endpoint_dispatcher = build_endpoint_dispatcher(stack.pool.as_ref());
    let middleware_dispatcher = build_middleware_dispatcher(stack.pool.as_ref());
    // §9.6: the WebSocket route service. Built only when the project has
    // any `socket.ts` routes AND a dispatcher (a pool). URL segments →
    // `socket.ts` path, so the hot path is a map probe.
    let socket_service: Option<std::sync::Arc<oxide_fw_server::SocketService>> = match (
        build_socket_dispatcher(stack.pool.as_ref()),
        cfg_socket_routes(&rootp, &cfg, &tree),
    ) {
        (Some(dispatcher), routes) if !routes.is_empty() => {
            Some(std::sync::Arc::new(oxide_fw_server::SocketService {
                dispatcher,
                routes: std::sync::Arc::new(routes),
                registry: std::sync::Arc::new(std::sync::Mutex::new(
                    std::collections::HashMap::new(),
                )),
            }))
        }
        _ => None,
    };
    // SEC-09: mint the dev inspector token when the inspector is attached
    // (dev only). `serve` (production) never sets an inspector, so this is
    // `None` there.
    let dev_token: Option<String> = with_inspector.then(oxide_fw_server::dev_token);
    let client_manifest_json: Option<serde_json::Value> = {
        let client_manifest = rootp.join("dist/client/client.manifest.json");
        std::fs::read_to_string(&client_manifest)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
    };
    let boot_script_url: Option<String> = if esm_state.is_some() {
        // ESM mode has no `client.manifest.json` — the boot entry is a
        // generated module that imports `@oxide-fw/core/client/hot` first
        // (installs the HMR runtime) then each route component.
        Some(crate::esm_dev::BOOT_SCRIPT_URL.to_string())
    } else {
        client_manifest_json.as_ref().and_then(|v| {
            v.get("routes")
                .and_then(|r| r.get("__boot__"))
                .and_then(|r| r.get("entry_chunk"))
                .and_then(|e| e.as_str())
                .map(|f| format!("client/{f}"))
        })
    };
    // V2-E04: the boot chunk's manifest digest for the live SSR boot tag
    // (the prerender path reads its own copy at build time). Gated on
    // `security.sri`; a pre-E04 manifest or the dev `/@boot/` URL yields
    // `None` and the tag stays byte-identical.
    let boot_script_integrity: Option<String> = if cfg.security.sri {
        boot_script_url.as_deref().and_then(|url| {
            manifest_integrity_for(
                client_manifest_json
                    .as_ref()
                    .and_then(|v| v.get("integrity")),
                url,
            )
        })
    } else {
        None
    };
    // §11 L0: a response cache for static/isr routes. The build id scopes
    // entries to this deployment, so a rebuild invalidates them. The read
    // order: the client manifest's field first (the bundler's identity —
    // the L0 seed keys were computed with it at build time), then the
    // canonical `dist/build-id` stamp (A04 — a dist whose manifest predates
    // the field still serves with its stamped id), then `"dev"` (a dev
    // server with no dist serves uncached).
    let build_id = client_manifest_json
        .as_ref()
        .and_then(|v| v.get("build_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| oxide_fw_cache::read_build_id(&rootp.join("dist")))
        .unwrap_or_else(|| "dev".to_string());

    // §20: the per-request waterfall recorder the inspector renders. Created
    // here so the dispatch closure (which records) and the inspector (which
    // reads) share one instance. `None` for a non-inspector server — the
    // dispatch then skips recording (a branch, not an allocation).
    let waterfall_recorder: Option<oxide_fw_dev::WaterfallRecorder> =
        with_inspector.then(oxide_fw_dev::WaterfallRecorder::new);
    // V2-F21: the dev-plane trace feed's sink. Created here so the pipeline
    // (which records one `RequestTrace` per completed request — the §20
    // consumption of the parsed W3C trace context) and the
    // `GET /__oxide/traces` endpoint (which serves the window) share one
    // instance — the same one-instance-two-readers posture the waterfall
    // recorder rides. `None` for a non-inspector server: the record point
    // is a None check and the endpoint falls through (production
    // byte-identical).
    //
    // V2-F22: the export arm. When `OXIDE_TRACE_LOG` opts in, the
    // pipeline's slot becomes a `FanoutTraceSink` over the ring and the
    // `TracingTraceSink` — every completed request's row ALSO leaves as
    // one `tracing::info!` event (the subscriber `init_tracing` installed
    // at boot renders it: console, `OXIDE_LOG=json`, and with
    // `OTEL_EXPORTER_OTLP_ENDPOINT` the OTLP stream). The endpoint keeps
    // serving the ring, so the wire contract is unchanged. Default off:
    // the console output stays byte-identical.
    let trace_ring: Option<std::sync::Arc<oxide_fw_server::RingTraceSink>> =
        with_inspector.then(|| {
            std::sync::Arc::new(oxide_fw_server::RingTraceSink::new(
                oxide_fw_server::TRACE_FEED_CAPACITY,
            ))
        });
    let trace_export: Option<std::sync::Arc<oxide_fw_server::TracingTraceSink>> = with_inspector
        .then(crate::observability::trace_log_enabled)
        .filter(|enabled| *enabled)
        .map(|_| std::sync::Arc::new(oxide_fw_server::TracingTraceSink::new()));
    // V2-F25: the file arm. When `OXIDE_TRACE_FILE` opts in, the dev boot
    // opens the path (create + append) and the pipeline's slot becomes a
    // `FanoutTraceSink` over the ring, the `TracingTraceSink` and the
    // `FileTraceSink` — every completed request's row ALSO appends as one
    // JSONL line (the same camelCase row shape the endpoint serves; the
    // line is flushed on record, so a crash cannot lose the already-
    // recorded rows). The attach is loud: a bad path aborts the boot
    // rather than silently losing the audit trail. Default off: no file
    // is written.
    let trace_file: Option<std::sync::Arc<oxide_fw_server::FileTraceSink>> = match with_inspector
        .then(crate::observability::trace_file_path)
        .flatten()
        .map(|path| oxide_fw_server::FileTraceSink::create(&path))
    {
        Some(Ok(sink)) => Some(std::sync::Arc::new(sink)),
        // The attach is loud: a bad path aborts the boot rather than
        // silently losing the audit trail (the session-keys posture).
        Some(Err(e)) => {
            return Err(anyhow::anyhow!(
                "OXIDE_TRACE_FILE export cannot be opened: {e}"
            ));
        }
        None => None,
    };
    let trace_sink: Option<std::sync::Arc<dyn oxide_fw_server::TraceSink>> =
        match trace_ring.clone() {
            Some(ring) => {
                // The window child stays first (the fanout's snapshot
                // contract: each child's rows in construction order, the
                // exporters contributing none — the endpoint renders the
                // ring's JSON either way).
                let mut children: Vec<std::sync::Arc<dyn oxide_fw_server::TraceSink>> =
                    vec![ring.clone() as std::sync::Arc<dyn oxide_fw_server::TraceSink>];
                if let Some(export) = trace_export {
                    children.push(export as std::sync::Arc<dyn oxide_fw_server::TraceSink>);
                }
                if let Some(file) = trace_file {
                    children.push(file as std::sync::Arc<dyn oxide_fw_server::TraceSink>);
                }
                if children.len() == 1 {
                    Some(ring as std::sync::Arc<dyn oxide_fw_server::TraceSink>)
                } else {
                    Some(std::sync::Arc::new(oxide_fw_server::FanoutTraceSink::new(
                        children,
                    )))
                }
            }
            None => None,
        };
    // §16.6: the session codec from `security.sessionKeys`. A malformed key
    // (too short) is a hard config error — silently falling back to an
    // unsigned cookie would be a security downgrade.
    let session_codec: Option<std::sync::Arc<oxide_fw_server::SessionCodec>> = {
        let keys: Vec<Vec<u8>> = cfg
            .security
            .session_keys
            .iter()
            .map(|k| k.as_bytes().to_vec())
            .collect();
        if keys.is_empty() {
            None
        } else {
            match oxide_fw_server::SessionCodec::new(&keys) {
                Ok(c) => Some(std::sync::Arc::new(c)),
                Err(e) => {
                    eprintln!(
                        "error: security.sessionKeys is invalid: {} {}",
                        e.0.code, e.0.message
                    );
                    return Err(anyhow::anyhow!("invalid session keys"));
                }
            }
        }
    };

    // §6.4 / Wave M: capture the renderer handle for warmup BEFORE it
    // moves into the pipeline. Arc clone is cheap; the renderer itself
    // lives in the pipeline but we need a second reference for the task.
    let renderer_to_warm = stack.page_renderer.clone();
    let render_ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
        renderer_to_warm.is_none(),
    ));
    // V2-F13: the learned-prefetch nav graph (`prefetch.learned`). The
    // hourly snapshot is merged in at start (a restart keeps the graph it
    // persisted), the flusher snapshots immediately and then every
    // `NavGraphSettings::flush_secs`. `learned: false` (the default)
    // constructs nothing — the request path reads nothing new and
    // `GET /_oxide/nav` 404s (the image-endpoint contract).
    let nav_graph: Option<std::sync::Arc<oxide_fw_server::NavGraph>> = if cfg.prefetch.learned {
        let g = std::sync::Arc::new(oxide_fw_server::NavGraph::default());
        g.merge_from(&rootp.join(".oxide").join("nav-graph.json"));
        // The handle lives for the server's lifetime; aborting it is the
        // process teardown's business, so the value is intentionally dropped.
        let _flusher = g.spawn_flusher(rootp.join(".oxide").join("nav-graph.json"));
        Some(g)
    } else {
        None
    };
    // V2-F09: the built-in search endpoint (`search.collections`). The
    // index is the build's artifact (`dist/search/index`, the mmap-able
    // read-only open the engine owns). Opening it is best-effort: a
    // missing or corrupt index logs a warning and `/_oxide/search`
    // answers the documented not-configured 404 — never a serve-start
    // failure. Unconfigured (or a default build without the `search`
    // feature) constructs nothing, byte-identical to the pre-F09 shape.
    #[cfg(feature = "search")]
    let (search_endpoint, hot_search) = build_search_endpoint(&cfg.search, &rootp);
    // Whether a search endpoint actually opened — read BEFORE
    // `search_endpoint` moves into the pipeline struct (the render stage's
    // `search_flag` at the dispatch build reads the fact, not the endpoint).
    #[cfg(feature = "search")]
    let search_configured = search_endpoint.is_some();
    // V2-F15: the recorder capture (`record:` config key) and the
    // in-flight map. The observer rides the pipeline (the one
    // `finish_response` funnel); the closure receives the same pair.
    let recordings = std::sync::Arc::new(crate::record_wire::Recordings::new());
    let record_capture = crate::record_wire::RecordCapture::from_config(
        &cfg.record,
        &rootp,
        &build_id,
        stack.pool.clone(),
    );
    // V2-F12: the brownout wiring — built only when `brownout.enabled`
    // is on (the inert-defaults contract: with the mode off the request
    // path gains nothing). The sampler task (the spec's 500 ms tick) and
    // the loop-lag probe (100 ms) run for the process's lifetime; their
    // handles drop at boot scope (detach), the same posture as the skew
    // expiry task.
    let brownout_wiring = crate::serve::brownout::BrownoutWiring::from_config(
        &cfg.brownout,
        stack.pool.clone(),
        std::sync::Arc::clone(&metrics),
    )
    .map(std::sync::Arc::new);
    if let Some(bw) = brownout_wiring.as_ref() {
        let _sampler = crate::serve::brownout::BrownoutWiring::spawn_sampler(bw, &cfg.brownout);
        let _probe = crate::serve::brownout::BrownoutWiring::spawn_loop_lag_probe(bw);
    }
    // V2-F49: the posture probe's wire — the F48 handoff's recorded tail
    // (the posture pull's second production surface). The opt-in resolves
    // ONCE here (the `trace_file_path` posture: a boot-time read, not a
    // per-request poll) and the provider attaches only when BOTH halves
    // hold — the resolved opt-in AND the typed tier handle (the F40
    // doctrine on a surface: a boot that never opted in keeps the
    // pre-F49 shape, the probe path 404s, nothing leaks). The `/-/health`
    // path rides the register-always shape (the metrics endpoint's): an
    // UNCONFIGURED probe 404s "not configured", the dev-tooling contract
    // the 404-leaks-nothing arm established.
    let cache_health_opt_in =
        crate::observability::cache_plane_health_with_config(cfg.cache.health_metrics);
    // V2-F50: the probe's path resolves ONCE through the layered
    // resolver (the `cache.postureProbePath` baseline beside the F50
    // flag and the `OXIDE_CACHE_POSTURE_PATH` env spelling) and the gate
    // grows its third half — the surface exists only where the resolved
    // path is non-empty AND the boot opted in AND a tier handle exists.
    let cache_posture_path = crate::observability::cache_posture_path_with_config(
        cfg.cache.posture_probe_path.as_deref(),
    );
    let cache_posture = crate::framework_shim::posture_probe_provider(
        cache_health_opt_in,
        &cache_posture_path,
        stack.cache_tier_typed.as_ref(),
    );
    // V2-F59: the F57 mount's boot wire resolves ONCE through the layered
    // resolver (the `cache.planeMount` baseline beside the F59 flag and
    // the `OXIDE_CACHE_PLANE_MOUNT` env spelling) and the gate grows its
    // own two halves — the plane mounts only where the boot opted in AND
    // a tier handle exists (the posture gate's shape). The composition is
    // `cache_plane_mount_provider`'s: the service over the boot's own
    // tier, the tier doubling as the F58 health provider, so the mounted
    // plane answers `/entry/`, `/tag/`, `/invalidate/` AND `GET /health`
    // on this boot's socket. `None` keeps the boot byte-identical (the
    // F40 doctrine).
    //
    // V2-F61: the boot's §20 exporter rides the same wire — the tier it
    // roots in already fans every cache event onto the exporter (the
    // F36/F37 construction), so the provider composes both §20 halves and
    // the mounted plane answers `GET /metrics` too. A boot without an
    // exporter (the test-stack shape) mounts the plane without the
    // metrics arm — the mount's provider check keeps `/metrics` out of
    // its path families (the F40 doctrine over the app's own route).
    let cache_plane = crate::framework_shim::cache_plane_mount_provider(
        crate::observability::cache_plane_mount_with_config(cfg.cache.plane_mount),
        stack.cache_tier_typed.as_ref(),
        stack.cache_exporter.as_ref(),
    );
    if let Some(service) = &cache_plane {
        tracing::info!(
            handler = %service.backing_kind(),
            "cache plane mounted on this node's socket (§11.2): \
             /entry/, /tag/, /invalidate/, GET /health and GET /metrics answer here"
        );
    }
    let pipeline = std::sync::Arc::new(oxide_fw_server::Pipeline {
        // A3: OG endpoint rate limit. The rasterization is CPU-bound and the
        // endpoint is unauthenticated; 10 renders per peer per minute bounds
        // the worst case without blocking a legitimate OG card.
        og_rate_limiter: oxide_fw_server::RateLimiter::default(),
        // V2-E04: the csp-report sink's own bucket (20/min/IP), never
        // shared with the OG limiter (an OG-heavy client must not eat
        // the report budget or vice versa).
        csp_report_rate_limiter: oxide_fw_server::RateLimiter::default(),
        // V2-F13: the learned-prefetch graph — `Some` only when the typed
        // config's `prefetch.learned` is on; `None` keeps the data plane
        // and `/_oxide/nav` at the pre-F13 shapes.
        nav: nav_graph,
        // V2-F09: the search endpoint's state — `Some` only when
        // `search.collections` is configured AND the build's index
        // opened (feature `search`; `None` keeps the pre-F09 shapes).
        #[cfg(feature = "search")]
        search: search_endpoint,
        // V2-F09: the search endpoint's own token bucket (20 per 10 s
        // per peer), never shared with the OG/csp-report limiters.
        #[cfg(feature = "search")]
        search_rate_limiter: oxide_fw_server::RateLimiter::default(),
        og_rate_max: cfg.og.rate_limit.max,
        og_rate_window_secs: cfg.og.rate_limit.window_secs,

        rules,
        assets,
        image_endpoint,
        og_endpoint,
        server_fns,
        actions: actions.clone(),
        limits: oxide_fw_server::Limits::default(),
        // P1 hardening: the metrics bearer token. `OXIDE_METRICS_TOKEN` is the
        // env seam (a config field would belong in the schema — post-v0.1);
        // unset + non-dev → the endpoint answers 403 instead of leaking.
        metrics_auth_token: std::env::var("OXIDE_METRICS_TOKEN").ok(),
        // SPEC-10: locale policy for the pipeline (None when unconfigured).
        i18n: {
            let c = &cfg.i18n;
            (!c.locales.is_empty()).then(|| oxide_fw_server::I18nPolicy {
                locales: c.locales.clone(),
                default: c.default_locale.clone(),
                strategy: c.strategy.clone(),
            })
        },
        security: oxide_fw_server::SecuritySettings {
            csp: cfg.security.csp.clone(),
            // V2-E04: the Trusted Types mode rides the same typed config —
            // the header matrix (`csp_headers_for`) keys off it.
            trusted_types: cfg.security.trusted_types.clone(),
            csrf: cfg.security.csrf.clone(),
            // SEC-05: populate the allow-lists from config — they were never
            // read, so a configured Host/SSRF list silently did nothing.
            host_allow_list: cfg.security.host_allow_list.clone(),
            ssrf_allow_list: cfg.security.ssrf_allow_list.clone(),
            cookie_keys: {
                // SEC-10: an empty `security.cookieKeys` must not leave
                // cookies signed with the empty key. Derive from the project
                // secret (`.oxide/secret`, BUG-05), with a warn so a multi-node
                // deployment knows to set explicit keys.
                let configured: Vec<Vec<u8>> = cfg
                    .security
                    .cookie_keys
                    .iter()
                    .map(|k| k.as_bytes().to_vec())
                    .collect();
                if configured.is_empty() {
                    match build_secret(&rootp) {
                        Ok(s) => {
                            eprintln!(
                                "warn: security.cookieKeys is empty; signing cookies with \
                                 the project secret. Set security.cookieKeys for multi-node."
                            );
                            vec![s.into_bytes()]
                        }
                        Err(_) => Vec::new(),
                    }
                } else {
                    configured
                }
            },
            // §16.6: session-encryption keys. Empty -> the pipeline's
            // session helpers fall back to signed-only cookies (documented).
            session_keys: cfg
                .security
                .session_keys
                .iter()
                .map(|k| k.as_bytes().to_vec())
                .collect(),
            // S-10: trust X-Forwarded-For only behind these proxies.
            trusted_proxies: cfg.security.trusted_proxies.clone(),
            // S-21.3: whether the `x-oxide-fw-cache` status header is emitted.
            expose_cache_header: cfg.security.expose_cache_header,
        },
        rate_limiter: std::sync::Arc::new(oxide_fw_server::RateLimiter::default()),
        brownout: brownout_wiring
            .as_ref()
            .map(|bw| std::sync::Arc::clone(&bw.governor)),
        health_path: "/_oxide/health".to_string(),
        ready_path: "/_oxide/ready".to_string(),
        render_ready: std::sync::Arc::clone(&render_ready),
        metrics_path: "/_oxide/metrics".to_string(),
        metrics: Some(std::sync::Arc::clone(&metrics)),
        // V2-F49/V2-F50: the posture probe's surface. The path is the
        // resolved probe location (the F50 layered resolver — the
        // recorded `/-/health` default); the provider rides the wire
        // above — the boot's opt-in over the typed tier on a non-empty
        // path, `None` keeps the path 404ing "not configured" (the F40
        // doctrine: the default shape leaks nothing).
        posture_path: cache_posture_path,
        cache_posture,
        // V2-F57/V2-F59: the cache plane's mount. The boot wire is LIVE:
        // `cache_plane_mount_provider` composes the service over the
        // boot's own tier (the tier doubling as the F58 health provider)
        // when the layered `cache.planeMount` resolution opts the boot
        // in; `None` keeps the boot's shape byte-identical (the F40
        // doctrine) — the plane answers only where a host attaches
        // `HttpCacheService`.
        cache_plane,
        inspector: if with_inspector {
            // §20: with a recorder wired, the inspector's waterfall table
            // reflects the live per-request timings instead of staying empty.
            // §V2-E08 item 2: the boot-time importers-graph harvest feeds the
            // module-graph search — one `(module, importers)` row per module,
            // the `BTreeMap` walk's sorted order. Boot-frozen: the per-tick
            // rebuild is the dev loop's (cmd_dev), the same posture the F02
            // islands scan took (recorded in docs/deviations.md).
            let module_graph = crate::dev::build_hmr_graph(&rootp, &rootp.join(&cfg.routes.dir))
                .importers
                .into_iter()
                .map(|(module, importers)| {
                    (
                        module.to_string_lossy().into_owned(),
                        importers
                            .iter()
                            .map(|p| p.to_string_lossy().into_owned())
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>();
            let rt = match waterfall_recorder.clone() {
                Some(rec) => oxide_fw_dev::RouteTableInspector::from_router_table_with_recorder(
                    tree.table(),
                    rec,
                ),
                None => oxide_fw_dev::RouteTableInspector::from_router_table(tree.table()),
            }
            .with_module_graph(module_graph);
            Some(std::sync::Arc::new(CliInspector { state: rt.state() }))
        } else {
            None
        },
        // V2-E08/F17 (the dev-plane attach): the two endpoints ride the
        // inspector's dev posture — `with_inspector` gates both, production
        // never attaches them and stays byte-identical. The open endpoint
        // spawns through the production `CommandOpener` (the confinement
        // and the coded refusals are the endpoint module's). The cost
        // endpoint reads the build's PERSISTED records file
        // (`client.costs.json`, written by the build phase and the dev
        // bundle path) through the stat-pair-checked `FileCostsSource` —
        // the no-build shape answers the documented 404s until the first
        // rebuild lands, a rebuild's rewrite is picked up by the next
        // query with no re-attach.
        open_editor: with_inspector.then(|| {
            std::sync::Arc::new(oxide_fw_server::OpenEditorEndpoint::new(
                rootp.clone(),
                std::sync::Arc::new(oxide_fw_server::CommandOpener),
            ))
        }),
        cost_endpoint: with_inspector.then(|| {
            std::sync::Arc::new(oxide_fw_server::CostEndpoint::new(std::sync::Arc::new(
                oxide_fw_server::FileCostsSource::from_path(
                    rootp.join("dist").join("client").join("client.costs.json"),
                ),
            )
                as std::sync::Arc<dyn oxide_fw_server::CostSource>))
        }),
        // V2-F21 (the dev-plane attach): the trace feed rides the same
        // `with_inspector` gate — the pipeline records every completed
        // request into the ring, the `GET /__oxide/traces` endpoint serves
        // the same instance. Production never attaches either (the gate is
        // the dev plane's), so the default pipeline stays byte-identical.
        // V2-F22: the pipeline's slot may be the fanout (ring + tracing,
        // `OXIDE_TRACE_LOG`); the endpoint stays ring-backed either way —
        // the wire contract is unchanged.
        trace_sink: trace_sink.clone(),
        trace_feed: trace_ring.map(|ring| {
            std::sync::Arc::new(oxide_fw_server::TracesEndpoint::new(
                ring as std::sync::Arc<dyn oxide_fw_server::TraceSink>,
            ))
        }),
        // V2-F24 (the dev-plane attach): the log feed rides the same
        // `with_inspector` gate. `init_tracing` created the ring when the
        // command was the dev boot and installed the capture layer over
        // the subscriber it builds — the §20 events the pipeline and the
        // engine emit land in the window; the endpoint serves the same
        // instance (the one-instance-two-readers posture the trace feed
        // rides). `None` for a non-dev boot: no capture layer was
        // installed, the endpoint stays absent, production is
        // byte-identical.
        log_feed: with_inspector
            .then(crate::observability::log_feed_ring)
            .flatten()
            .map(|ring| {
                std::sync::Arc::new(oxide_fw_server::LogsEndpoint::new(
                    ring as std::sync::Arc<dyn oxide_fw_server::LogSink>,
                ))
            }),
        page_renderer: None,
        // SPEC-14: the HMR hub's broadcast sender, dev only.
        hmr: hmr.as_ref().map(|h| h.sender()),
        // §13.3: advertise the client manifest's build id so a stale tab can
        // detect a rebuild and reload.
        hmr_build_id: build_id.clone(),
        // SEC-02: HSTS only on TLS (§17.1) — set where the listener is built.
        tls: tls.is_some(),
        // SEC-09: the dev inspector token (see below).
        dev_token: dev_token.clone(),
        // S-03/S-04/S-21.1: dev servers get the DNS-rebinding Host gate on
        // the HMR/socket upgrade arms. Production never sets it.
        dev_bind_ip: with_inspector.then_some(bind_ip),
        // §9.6: WebSocket route dispatch (None when the project has no
        // `socket.ts` routes or no pool).
        sockets: socket_service,
        // §12.1: advertise HTTP/3 (Alt-Svc) only when it is actually enabled.
        // The h3 listener shares the TCP port over UDP. `port` is resolved
        // above (a probe picks a free port for a requested 0); the h3 bind
        // uses the same value.
        http3_advertise: cfg.server.h3_advertise,
        http3_port: if cfg.server.http3 && tls.is_some() {
            port
        } else {
            0
        },
        response_observer: record_capture.as_ref().map(|c| {
            std::sync::Arc::new(crate::record_wire::RecorderObserver::new(
                std::sync::Arc::clone(c),
                std::sync::Arc::clone(&recordings),
            )) as std::sync::Arc<dyn oxide_fw_server::ResponseObserver>
        }),
    });
    let proxy_rules: Vec<oxide_fw_dev::ProxyRule> = proxy_rules.clone();
    let routes_root = rootp.join(&cfg.routes.dir);
    // §10.3: the same boot URL that the build-time prerender injects also
    // has to be spliced into every live request's shell — otherwise dev /
    // serve would ship pages whose `<ox-island>` markup is inert.

    // BUG-02: wrap the framework stack's cache tier, not a fresh one. The
    // worker's `'use cache'` entries and the L0 response cache now share one
    // handler, so BUG-01's invalidation reaches both. A stack built without a
    // cache (tests, `FrameworkStack::empty()`) gets a private tier.
    // BUG-11: honour `cache.l0.maxSize` (parsed: "512mb" | "64kb" | bytes).
    // A malformed value falls back to 512 MiB with a warning rather than
    // refusing to start.
    // §14.1: `cache.l0.maxSize`.
    let l0_bytes = oxide_fw_config::parse_size(&cfg.cache.l0.max_size).unwrap_or_else(|_| {
        tracing::warn!(
            value = %cfg.cache.l0.max_size,
            "cache.l0.maxSize is malformed; using 512mb"
        );
        512 * 1024 * 1024
    });
    // §11.2: `cache.handler` selects the L2 remote. The LIVE wiring is in
    // `build_framework_stack` (every `cmd_*` site threads `cfg.cache.handler`
    // in, so the worker's tier honors the config); this arm only fires for a
    // stack built WITHOUT a tier (tests, `FrameworkStack::empty()`) — it
    // keeps the same handler mapping so a test stack still exercises the
    // config path. Memory/disk are the local tiers every handler gets.
    // F36/F37: the typed half + the §20 exporter handle travel beside the
    // erased handler (the stack carries both; this arm builds its own).
    // F40: the hot-key ledger rides the same shape.
    let (l0_tier, l0_tiered, cache_exporter, cache_hot_keys) = match stack.cache_tier.clone() {
        Some(tier) => (
            tier,
            stack.cache_tier_typed.clone(),
            stack.cache_exporter.clone(),
            stack.cache_hot_keys.clone(),
        ),
        None => {
            let mut tier = oxide_fw_cache::TieredCache::new(l0_bytes);
            match &cfg.cache.handler {
                oxide_fw_config::CacheHandlerConfig::Memory => {}
                oxide_fw_config::CacheHandlerConfig::Disk => {
                    // §11.2 L1: a durable local disk tier so a restart keeps
                    // its cache. Failure (permissions) degrades to memory-only
                    // with a warning rather than failing the boot.
                    let dir = rootp.join(".oxide").join("cache").join("l1");
                    // `with_disk` consumes `tier`; on failure it owns it. Use
                    // a placeholder so both arms leave a usable value.
                    match tier.with_disk(dir) {
                        Ok(t) => tier = t,
                        Err(e) => {
                            tracing::warn!(
                                error = %e.0.message,
                                "cache.handler=disk: opening the disk tier failed; \
                                 continuing memory-only",
                            );
                            tier = oxide_fw_cache::TieredCache::new(l0_bytes);
                        }
                    }
                }
                oxide_fw_config::CacheHandlerConfig::Http { url } => {
                    tier = tier.with_remote(std::sync::Arc::new(
                        oxide_fw_cache::HttpCacheHandler::new(url.clone()),
                    ));
                }
                oxide_fw_config::CacheHandlerConfig::Redis { url } => {
                    // §11.2: a real Redis L2 tier (oxide-fw-cache/redis). A
                    // connection failure logs and stays memory-only — the
                    // build must not fail because a cache is down.
                    match oxide_fw_cache::RedisCacheHandler::new(url.clone()).await {
                        Ok(h) => {
                            tier = tier.with_remote(std::sync::Arc::new(h));
                            tracing::info!(
                                url = %oxide_fw_cache::redacted_url(url),
                                "cache.handler=redis: attached remote tier",
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                url = %oxide_fw_cache::redacted_url(url),
                                error = %e.0.message,
                                "cache.handler=redis: connection failed; \
                                 continuing with local tier only",
                            );
                        }
                    }
                }
                oxide_fw_config::CacheHandlerConfig::S3 { bucket } => {
                    // §11.2: a real S3 L2 tier (oxide-fw-cache/s3). Same
                    // failure-tolerance as Redis: a bad bucket logs and stays
                    // memory-only. Credentials/region come from AWS_* env vars.
                    match oxide_fw_cache::S3CacheHandler::new(bucket.clone()) {
                        Ok(h) => {
                            tier = tier.with_remote(std::sync::Arc::new(h));
                            tracing::info!(
                                bucket = %bucket,
                                "cache.handler=s3: attached remote tier",
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                bucket = %bucket,
                                error = %e.0.message,
                                "cache.handler=s3: client build failed; \
                                 continuing with local tier only",
                            );
                        }
                    }
                }
            }
            // F36/F37: the test-stack arm attaches the §20 exporter over
            // its own tier and keeps the typed half — the same wire the
            // real stack carries. F40: the hot-key ledger joins behind
            // the F34 fan (the same construction the real boot rides).
            let cache_exporter = std::sync::Arc::new(oxide_fw_cache::MetricsCacheObserver::new());
            let cache_hot_keys = std::sync::Arc::new(oxide_fw_cache::HotKeyObserver::bounded(
                crate::framework_shim::CACHE_HOT_KEY_CAPACITY,
            ));
            let cache_observer = oxide_fw_cache::FanoutCacheObserver::shared(vec![
                std::sync::Arc::clone(&cache_exporter)
                    as std::sync::Arc<dyn oxide_fw_cache::CacheObserver>,
                std::sync::Arc::clone(&cache_hot_keys)
                    as std::sync::Arc<dyn oxide_fw_cache::CacheObserver>,
            ]);
            let typed = std::sync::Arc::new(tier.with_observer(cache_observer));
            (
                std::sync::Arc::clone(&typed) as std::sync::Arc<dyn oxide_fw_cache::CacheHandler>,
                Some(std::sync::Arc::clone(&typed)),
                Some(cache_exporter),
                Some(cache_hot_keys),
            )
        }
    };
    // §13 / HMR: `dev` must always re-render — a stale L0 entry would shadow
    // every file edit (the watcher bumps the worker generation but L0 short-
    // circuits the render path entirely). `hmr: Some(_)` is the "dev mode"
    // signal in this function (production call sites pass `None`).
    let is_dev = hmr.is_some();
    // F36: the recording half rides `l0`'s Some-ness — dev boots have no L0
    // plane and no flights, so there is nothing to record and nothing to
    // carry.
    let l0_tiered = if is_dev { None } else { l0_tiered };
    // F47: the health arm's tier handle — the posture pull needs the TYPED
    // tier (`TieredCache::health`), captured beside the exporter/hot-key
    // handles BEFORE the `is_dev` null-out above (that null-out is about
    // the L0 RESPONSE plane, not about the tier the stack attached: the
    // posture answers on the dev scrape too).
    let cache_health_tier = stack.cache_tier_typed.clone();
    // F37: the §20 cache-plane exposition — the boot's exporter renders the
    // `oxide_cache_plane_*` families beside the request-plane rows (the
    // server crate stays cache-agnostic: the slot is an `Fn() -> String`).
    // F40: the hot-key ledger's labeled family rides the same scrape —
    // one run per pull renders both planes (the composition the F37 slot
    // was shaped for: the CLI, which depends on both crates, composes).
    // F41: the hot-key top window is operator-tunable — the boot resolves
    // the `OXIDE_CACHE_PLANE_HOT_KEY_TOP` spelling ONCE here (the
    // `trace_file_path` posture: a boot-time read, not a per-scrape env
    // poll) and the closure renders the boot's window.
    // F43: the window's project baseline joins the precedence — the
    // layered resolver takes the config's `cache.hotKeyTop` (flag wins,
    // then the env spelling, then this baseline, then the F40 default).
    // F46: the coverage opt-in joins the same precedence — the layered
    // resolver takes the config's `cache.coverageMetrics` (flag wins,
    // then the env spelling, then this baseline, then default-off) and
    // the closure composes the ledger's F44 coverage exposition when the
    // boot opted in (the default shape stays byte-identical).
    // F47: the health opt-in joins the same precedence — the
    // `OXIDE_CACHE_PLANE_HEALTH` env spelling resolves ONCE here (the
    // `trace_file_path` posture) and the closure composes the posture
    // pull's F32 gauge exposition when the boot opted in (the F32
    // pull's zero-production-consumer gap closes; the default shape
    // stays byte-identical — the tier handle rides the closure only so
    // the arm CAN compose, the flag is what composes it).
    // F48: the health arm's project baseline joins the precedence — the
    // layered resolver takes the config's `cache.healthMetrics` (flag
    // wins, then the env spelling, then this baseline, then default-off)
    // and the closure composes the posture exposition when any layer
    // opted in (the default shape stays byte-identical).
    if let Some(exporter) = cache_exporter.as_ref() {
        let exporter = std::sync::Arc::clone(exporter);
        let hot_keys = cache_hot_keys.clone();
        let health_tier = cache_health_tier.clone();
        let hot_key_top =
            crate::observability::cache_hot_key_top_with_config(cfg.cache.hot_key_top);
        let coverage =
            crate::observability::cache_plane_coverage_with_config(cfg.cache.coverage_metrics);
        // V2-F49: the opt-in resolves ONCE per boot — the probe's wire
        // (above the pipeline literal) hoisted the resolution to
        // `cache_health_opt_in`, so the scrape arm and the probe surface
        // read the SAME fact (the `trace_file_path` posture: one
        // boot-time read, two consumers).
        let health = cache_health_opt_in;
        metrics.set_cache_plane(move || {
            crate::framework_shim::cache_plane_scrape(
                &exporter,
                hot_keys.as_deref(),
                hot_key_top,
                coverage,
                health_tier.as_deref(),
                health,
            )
        });
    }
    let l0 = if is_dev {
        None
    } else {
        // P-05: configurable freshness window (`cache.l0.ttlSecs`, default
        // 300) instead of the old hard-coded 60 s.
        Some(std::sync::Arc::new(l0_cache::L0Cache::new(
            l0_tier,
            build_id.clone(),
            std::time::Duration::from_secs(cfg.cache.l0.ttl_secs.unwrap_or(300)),
        )))
    };
    // PERF-04: seed L0 from the build manifest so a prerendered route is an
    // L0 hit on the very first request (no warm-up miss). A missing manifest
    // (dev, or a build with no prerendered pages) is a no-op.
    if let Some(cache) = l0.as_ref() {
        let seed_path = rootp.join("dist/server/l0-seed.json");
        // A missing seed is normal (a build with no prerendered pages);
        // a PRESENT-but-unreadable manifest is not — warn loudly (the
        // CACHE-CODEC posture) instead of silently skipping the boot's
        // warm-up.
        // (`unwrap_or_default`: a read error rides the same empty-Vec arm
        // the match spelled — the clippy-gate rewrite, behavior identical;
        // the missing-seed silence and the present-but-corrupt warn below
        // are unchanged.)
        let seed_bytes = std::fs::read(&seed_path).unwrap_or_default();
        let seed_json = if seed_bytes.is_empty() {
            None
        } else {
            match serde_json::from_slice::<serde_json::Value>(&seed_bytes) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(
                        path = %seed_path.display(),
                        error = %e,
                        "CACHE-SEED: the l0-seed.json manifest is unreadable; L0 seeding skipped",
                    );
                    None
                }
            }
        };
        if let Some(v) = seed_json {
            // Only seed a manifest from THIS build.
            let same_build = v.get("build_id").and_then(|b| b.as_str()) == Some(build_id.as_str());
            if same_build && let Some(entries) = v.get("entries").and_then(|e| e.as_array()) {
                for e in entries {
                    let key = e.get("key").and_then(|k| k.as_str()).unwrap_or("");
                    let body = e.get("body").and_then(|b| b.as_str()).unwrap_or("");
                    let status = e.get("status").and_then(|s| s.as_u64()).unwrap_or(200) as u16;
                    if !key.is_empty() && !body.is_empty() {
                        cache
                            .seed_entry(key, body.as_bytes().to_vec(), status)
                            .await;
                    }
                }
            }
        }
    }
    // PERF-07: prefer the mmap'd rkyv route archive when the build wrote one.
    // A missing/stale archive falls back to the in-memory tree (dev, or a
    // build with no archive).
    // WP-B: in dev the route table is swappable, so a stale on-disk archive
    // (written by a previous `oxide build`) must NOT shadow the live tree —
    // otherwise a route added after that build 404s until the archive is
    // rebuilt. Only the production (`serve`) path uses the mmap'd archive.
    let archived = if is_dev {
        None
    } else {
        rootp
            .join(".oxide/routes.manifest")
            .exists()
            .then(|| {
                oxide_fw_router::ArchivedTree::load_archived(&rootp.join(".oxide/routes.manifest"))
                    .ok()
                    .map(std::sync::Arc::new)
            })
            .flatten()
    };
    // §10.8: PPR shells exist only when `render.ppr` is enabled. Keep a
    // handle for the dev watcher (cleared on every edit) and for tests.
    let ppr_shells = cfg
        .render
        .ppr
        .then(|| std::sync::Arc::new(std::sync::Mutex::new(PprShellStore::default())));
    let swappable_tree = SwappableRouteTree::new(std::sync::Arc::clone(&tree));
    // V2-A04 + F10: the skew pool the data plane checks against — the
    // boot's build id accepted as CURRENT, the demotion window from the
    // typed `deploy.skewWindow` and the depth from `deploy.generations`
    // (both load-validated `CFG-DEPLOY`; an unparsable literal falls back
    // to the spec's 15 m default rather than refusing a serve). The 30 s
    // expiry task reclaims the expired slots; the serve loop's `SIGHUP`
    // handler promotes a rebuilt dist (`SkewState::promote_from_dist`).
    let skew_state = crate::serve::generations::SkewState::from_boot(
        &build_id,
        &cfg.deploy.skew_window,
        cfg.deploy.generations,
    );
    let generations: Option<
        std::sync::Arc<std::sync::RwLock<oxide_fw_dispatch::skew::GenerationPool>>,
    > = Some(skew_state.handle());
    let _generations_expiry = crate::serve::generations::spawn_expiry_task(skew_state.handle());
    // V2-F10 (the serving half, steps 1-2): freeze the boot generation's
    // loader sources and hold the boot handles as the CURRENT generation.
    // The dispatch routes every request through these pools; the SIGHUP
    // promote boots the next generation's stack and demotes these. Production
    // serve only (dev's reload path re-scans sources by design; the
    // id-level pool still owns the acceptance verdict in both).
    let serving_pools: Option<std::sync::Arc<crate::serve::generations::ServingPools>> = if is_dev {
        None
    } else {
        match stack.pool.clone() {
            Some(boot_pool) => {
                match crate::serve::generations::snapshot_generation_sources(&rootp, &build_id) {
                    Ok(snapshot_root) => Some(crate::serve::generations::ServingPools::from_boot(
                        &build_id,
                        std::sync::Arc::new(crate::serve::generations::GenServing {
                            pool: boot_pool,
                            runner: stack.loader_runner.clone(),
                            routes_root: snapshot_root,
                        }),
                        skew_state.window(),
                        cfg.deploy.generations,
                    )),
                    Err(e) => {
                        tracing::warn!(
                            code = "SKW-SNAPSHOT",
                            msg = %e,
                            "skew: the boot generation's loader sources could not be                                  frozen; the two-pool route step stays inert and the boot                                  handles serve every generation"
                        );
                        None
                    }
                }
            }
            None => None,
        }
    };
    // V2-F05: the live hub + rerun channel — built only for the `sse`
    // transport (`poll` subscribers re-fetch /__data themselves; there is
    // no endpoint and no hub to feed).
    let live_state = if matches!(cfg.live.transport, oxide_fw_config::LiveTransport::Sse) {
        Some(crate::serve::live::LiveState::new(
            cfg.live.max_subscribers,
            cfg.live.per_ip_max,
            cfg.live.keepalive_seconds,
        ))
    } else {
        None
    };
    // V2-F07: the resumable-stream wiring — the store over the typed
    // `stream.resume` bounds (`serve::streams::StreamWiring::from_config`)
    // and the §V2-F07 housekeeping task (a sweep every 30 s reclaims the
    // buffers whose TTL passed; the handle is dropped like the skew
    // expiry task's, detaching the task for the process's lifetime).
    let stream_wiring = std::sync::Arc::new(crate::serve::streams::StreamWiring::from_config(
        &cfg.stream,
    ));
    let _streams_sweep = oxide_fw_server::stream_store::spawn_sweep(
        std::sync::Arc::clone(&stream_wiring.store),
        std::time::Duration::from_secs(crate::serve::streams::SWEEP_CADENCE_SECS),
    );
    let dispatch = build_dispatch_swappable(DispatchDeps {
        swappable: swappable_tree.clone(),
        metrics: std::sync::Arc::clone(&metrics),
        proxy_rules: proxy_rules.clone(),
        page_renderer: stack.page_renderer,
        loader_runner: stack.loader_runner,
        routes_root,
        boot_script_url,
        boot_script_integrity,
        l0,
        // F36: the boot wires the typed recording half beside the erased
        // one — the render stage's flight owner records the publish count
        // through it (the F33 emitter's live call site). `None` in dev
        // (the `is_dev` arm above); the un-wired boot stays silent.
        l0_tiered,
        actions,
        archived,
        pool: stack.pool.clone(),
        i18n_cfg: {
            let c = &cfg.i18n;
            (!c.locales.is_empty()).then(|| oxide_fw_assets::i18n::I18nConfig {
                locales: c.locales.clone(),
                default: c.default_locale.clone(),
                strategy: c.strategy.clone(),
            })
        },
        endpoint_dispatcher,
        middleware_dispatcher,
        vary_headers: cfg.cache.l0.vary.clone(),
        waterfall: waterfall_recorder.clone(),
        session_codec,
        ppr_shells: ppr_shells.clone(),
        esm_state: esm_state.clone(),
        record: record_capture.clone(),
        recordings: std::sync::Arc::clone(&recordings),
        generations: generations.clone(),
        live: live_state.clone(),
        serving_pools: serving_pools.clone(),
        flight_map: std::sync::Arc::new(dashmap::DashMap::new()),
        self_dispatch: std::sync::Arc::new(std::sync::OnceLock::new()),
        stream_counter: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        streams: Some(std::sync::Arc::clone(&stream_wiring)),
        brownout: brownout_wiring.as_ref().map(std::sync::Arc::clone),
        // V2-F04: the configured experiments, converted to the engine's
        // shapes at boot (the field-by-field wiring mapping).
        experiments: crate::dispatch::stages::experiments_of_config(&cfg.experiments),
        // V2-F09: the boot's search mode — the flag rides the render
        // stage's hydration injection when the engine serves; unconfigured
        // (or a build without the `search` feature) emits nothing, so the
        // client matcher keeps its static-export fallback.
        #[cfg(feature = "search")]
        search_flag: search_configured.then_some("server" as &'static str),
        #[cfg(not(feature = "search"))]
        search_flag: None,
    });
    // §5.6: dev-mode stale guard. When a HMR hub is wired, wrap the
    // dispatch so a mid-flight file change aborts the render (no serving
    // a result computed from pre-change source). Prod (no hub) is
    // untouched — zero overhead.
    let dispatch = match hmr.as_ref() {
        Some(hub) => wrap_dispatch_with_stale_guard(dispatch, std::sync::Arc::clone(hub)),
        None => dispatch,
    };
    // V2-F05 step 5: the rerun worker. The injected closure dispatches the
    // SAME data-route path as `/__data` (the full pipeline — middleware,
    // locals, layout loads, gating; no second implementation). `None` from
    // a rerun (a redirect, an error, an empty body) clears the subscriber's
    // snapshot: the stale-terminated body answers `event: stale` and the
    // client reloads once.
    if let Some(live) = live_state.as_ref() {
        let spawned = live.spawn_runner(
            std::time::Duration::from_millis(cfg.live.debounce_ms),
            cfg.live.max_reruns_per_second,
            crate::serve::live::rerun_closure(std::sync::Arc::clone(&dispatch)),
        );
        debug_assert!(spawned, "the live runner spawns exactly once per boot");
    }
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    // Startup banner: the URL must be obvious on first run (Vite parity).
    eprintln!();
    eprintln!("  oxide dev");
    eprintln!();
    eprintln!("  Local:  http://localhost:{}/", port);
    eprintln!("  Ready:  http://localhost:{}/_oxide/ready", port);
    eprintln!();
    let (tx, rx) = tokio::sync::watch::channel(false);
    // §12.1: HTTP/3 rides QUIC over UDP on the same port. Enabled by
    // `server.http3` + TLS (QUIC has no plaintext mode). A second listener
    // task drives it; the HTTP/1+2 server is untouched.
    let http3_cfg: Option<oxide_fw_server::H3ServerConfig> = if cfg.server.http3 {
        match tls.as_ref() {
            Some(tls_cfg) => match oxide_fw_server::h3_config_from_tls(tls_cfg.as_ref()) {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::warn!(
                        code = %e.0.code,
                        msg = %e.0.message,
                        "http3: could not build the QUIC config; continuing HTTP/1+2 only",
                    );
                    None
                }
            },
            None => {
                tracing::warn!("http3: requires TLS; continuing HTTP/1+2 only");
                None
            }
        }
    } else {
        None
    };
    let pipeline_h3 = std::sync::Arc::clone(&pipeline);
    let dispatch_h3 = std::sync::Arc::clone(&dispatch);
    let rx_h3 = rx.clone();
    let h3_bind = std::net::SocketAddr::new(bind_ip, port);
    let rx_main = rx;
    // §6.4 / Wave M: warm the renderer on a background task. Socket is
    // already bound (health → 200); ready → 503 until the synthetic render
    // completes. Single-node traffic continues unaffected while this runs.
    // PERF-23: `--no-warmup` skips the synthetic render. `/_oxide/ready` is
    // gated on `render_ready`, so flip it immediately — the server is honest
    // about being cold (ready ⇒ warm would be a lie).
    if std::env::var_os("OXIDE_NO_WARMUP").is_some() {
        render_ready.store(true, std::sync::atomic::Ordering::SeqCst);
        tracing::info!("--no-warmup: skipping renderer warm-up");
    } else if let Some(renderer) = renderer_to_warm {
        let ready = std::sync::Arc::clone(&render_ready);
        tokio::spawn(async move {
            match renderer.warm().await {
                Ok(()) => tracing::info!("renderer warm; /_oxide/ready → 200"),
                Err(e) => tracing::warn!(
                    code = %e.0.code,
                    "warmup failed (readiness flips anyway): {}",
                    e.0.message
                ),
            }
            ready.store(true, std::sync::atomic::Ordering::SeqCst);
        });
    }
    tokio::spawn(async move {
        let bind_addr = std::net::SocketAddr::new(bind_ip, port);
        if let Some(h3_cfg) = http3_cfg {
            let h3_task = tokio::spawn(async move {
                if let Err(e) =
                    oxide_fw_server::serve_http3(h3_bind, pipeline_h3, dispatch_h3, rx_h3, h3_cfg)
                        .await
                {
                    tracing::warn!(code = %e.0.code, "http3 listener stopped: {}", e.0.message);
                }
            });
            let _h3_task = h3_task;
        }
        if let Err(e) =
            oxide_fw_server::Server::serve_with_tls(bind_addr, pipeline, dispatch, rx_main, tls)
                .await
        {
            tracing::error!(code = %e.0.code, "server stopped with error");
        }
    });
    Ok(StartedServer {
        addr,
        shutdown: tx,
        metrics,
        dev_token,
        hmr,
        ppr_shells,
        swappable_tree,
        generations: generations.clone(),
        serving_pools,
        brownout: brownout_wiring,
        #[cfg(feature = "search")]
        hot_search,
    })
}

/// The server binds inside `Server::serve`, so a requested port of `0` is resolved
/// by probing a free port first (test convenience; real runs use the config port).
pub(crate) async fn resolve_port(requested: u16) -> u16 {
    if requested != 0 {
        return requested;
    }
    if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await
        && let Ok(addr) = listener.local_addr()
    {
        drop(listener);
        return addr.port();
    }
    0
}

pub(crate) async fn cmd_serve(
    root: &str,
    port: Option<u16>,
    label: &str,
    w: Writer<'_>,
) -> anyhow::Result<i32> {
    // §7 / §9.1: build the framework stack (renderer + loader) for the
    // production server. Same wiring as `cmd_dev`.
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| std::path::PathBuf::from(root));
    let mut cfg = load_config_or_default(&rootp, "production");
    let config_path_opt = find_config_file(&rootp).ok();
    let transform_handler = framework_shim::build_transform_handler(&rootp, &cfg);
    // §15.2: `cacheHandler` — a plugin may select the cache backend. Dev and
    // build already consult it; `start` must too, or the same project uses a
    // different tier in production than it built against.
    if let Ok(Some(chosen)) = resolve_cache_handler_via_plugin(&rootp, &cfg, w).await {
        cfg.cache.handler = chosen;
    }
    // §11.2: `cache.handler` is honored inside the stack (see cmd_dev).
    // The stack-building inputs are captured for the SIGHUP promote too —
    // the next generation's pool + runner boot from the SAME recipe
    // (§V2-F10 step 1: the second pool boots over the rebuilt dist).
    let promote_config_path = config_path_opt.clone();
    let promote_backend = cfg.server.backend;
    let promote_cache_handler = cfg.cache.handler.clone();
    let stack = framework_shim::build_framework_stack(
        config_path_opt.as_deref(),
        cfg.server.backend,
        transform_handler,
        Some(&cfg.cache.handler),
    )
    .await;
    if let Some(name) = stack.renderer_name() {
        outln!(w, "framework renderer: {}", name);
    }

    let started = start_server(root, port, stack).await?;
    outln!(
        w,
        "{label} — listening on http://{} (health: /_oxide/health, Ctrl-C to stop, SIGHUP to promote a rebuilt dist)",
        started.addr
    );
    // V2-F12: the ops surface the brownout wiring serves. `None` means
    // `brownout.enabled` is off (the default posture); when wired, the
    // sampler drives the governor's level and the ops endpoints report it
    // as `x-oxide-brownout`.
    if started.brownout.is_some() {
        outln!(
            w,
            "brownout: governor wired — the sampler drives the level, the ops endpoints report it as x-oxide-brownout"
        );
    }
    // §12.2 graceful shutdown on Ctrl-C; §V2-F10 step 1 on SIGHUP: a
    // rebuilt `dist/` is promoted — the new build id becomes CURRENT and
    // the previous one stays accepted for `deploy.skewWindow` (the skew
    // pool holds it; the 30 s expiry task reclaims the slot at window
    // end). An unchanged or unreadable dist promotes nothing (an
    // idempotent re-deploy never churns the pool).
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut hup) = signal(SignalKind::hangup()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = hup.recv() => {
                    if let Some(gens) = &started.generations {
                        let dist = rootp.join("dist");
                        match crate::serve::generations::promote_handle_from_dist(gens, &dist) {
                            Some(new_id) => {
                                outln!(
                                    w,
                                    "skew: build-id {new_id} is current — the previous generation stays accepted for its skew window"
                                );
                                // V2-F10 step 1 (the second pool): freeze the NEW
                                // generation's loader sources, boot its stack over
                                // the rebuilt dist (a fresh pool + runner), and
                                // demote the boot handles together — accepted
                                // old-build data-plane requests run the OLD
                                // generation's handles (its loader bytes are the
                                // frozen snapshot, not the edited sources).
                                if let Some(sp) = &started.serving_pools {
                                    match crate::serve::generations::snapshot_generation_sources(&rootp, &new_id) {
                                        Ok(snapshot_root) => {
                                            let next = crate::framework_shim::build_framework_stack(
                                                promote_config_path.as_deref(),
                                                promote_backend,
                                                None,
                                                Some(&promote_cache_handler),
                                            )
                                            .await;
                                            match next.pool.clone() {
                                                Some(pool) => {
                                                    let next_gen = std::sync::Arc::new(
                                                        crate::serve::generations::GenServing {
                                                            pool,
                                                            runner: next.loader_runner.clone(),
                                                            routes_root: snapshot_root,
                                                        },
                                                    );
                                                    if sp.promote_booted(&new_id, next_gen) {
                                                        outln!(
                                                            w,
                                                            "skew: generation {new_id} booted over the rebuilt dist — its pool serves; the previous pool drains at window end"
                                                        );
                                                    }
                                                }
                                                None => tracing::warn!(
                                                    code = "SKW-POOL",
                                                    msg = "the promoted generation has no worker pool; the boot pool keeps serving every generation",
                                                ),
                                            }
                                        }
                                        Err(e) => tracing::warn!(
                                            code = "SKW-SNAPSHOT",
                                            msg = %e,
                                            "skew: the promoted generation's loader sources could not be frozen; the boot handles keep serving",
                                        ),
                                    }
                                }
                            }
                            None => outln!(w, "skew: dist unchanged — no generation promoted"),
                        }
                    }
                }
            }
        } else {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    let _ = started.shutdown.send(true);
    Ok(0)
}

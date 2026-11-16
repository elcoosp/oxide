//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// WP-B (deviation 3): a swappable slot for the route tree. The dispatch
/// closure reads through this on every request, so a dev-watcher rescan can
/// publish a fresh `RouteTree` without restarting the server. `build_dispatch`
/// (used by every test) wraps a one-shot swappable that is never swapped; dev
/// passes the live one to `build_dispatch_swappable` directly.
#[derive(Clone)]
pub(crate) struct SwappableRouteTree {
    inner: std::sync::Arc<std::sync::RwLock<std::sync::Arc<oxide_fw_router::RouteTree>>>,
}

impl SwappableRouteTree {
    pub fn new(tree: std::sync::Arc<oxide_fw_router::RouteTree>) -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::RwLock::new(tree)),
        }
    }

    pub fn load(&self) -> std::sync::Arc<oxide_fw_router::RouteTree> {
        std::sync::RwLock::read(&self.inner)
            .expect("route tree lock poisoned")
            .clone()
    }

    pub fn store(&self, tree: std::sync::Arc<oxide_fw_router::RouteTree>) {
        *std::sync::RwLock::write(&self.inner).expect("route tree lock poisoned") = tree;
    }
}

/// Optional dispatch knobs for [`build_dispatch`]. Fields default to
/// `None` / empty / false; callers override only what the test needs.
///
/// Bundled to keep `build_dispatch` at three positional arguments (the two
/// required construction handles plus the options) — a `too_many_arguments`
/// lint fires at 8, and the previous 19-argument form forced every test
/// call site to pass `None, None, false, Vec::new(), None, None` eight
/// times, hiding the two or three values the test actually varied.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct DispatchOpts {
    pub(crate) proxy_rules: Vec<oxide_fw_dev::ProxyRule>,
    pub(crate) page_renderer: Option<std::sync::Arc<dyn oxide_fw_server::PageRenderer>>,
    pub(crate) loader_runner: Option<std::sync::Arc<dyn oxide_fw_server::LoaderRunner>>,
    pub(crate) routes_root: std::path::PathBuf,
    pub(crate) boot_script_url: Option<String>,
    /// V2-E04: the manifest's integrity digest for the boot chunk.
    pub(crate) boot_script_integrity: Option<String>,
    pub(crate) l0: Option<std::sync::Arc<l0_cache::L0Cache>>,
    pub(crate) actions: Option<std::sync::Arc<dyn oxide_fw_server::ActionDispatcher>>,
    pub(crate) archived: Option<std::sync::Arc<oxide_fw_router::ArchivedTree>>,
    pub(crate) pool: Option<std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
    pub(crate) i18n_cfg: Option<oxide_fw_assets::i18n::I18nConfig>,
    pub(crate) endpoint_dispatcher: Option<std::sync::Arc<dyn oxide_fw_server::EndpointDispatcher>>,
    pub(crate) middleware_dispatcher:
        Option<std::sync::Arc<dyn oxide_fw_server::MiddlewareDispatcher>>,
    pub(crate) vary_headers: Vec<String>,
    pub(crate) waterfall: Option<oxide_fw_dev::WaterfallRecorder>,
    pub(crate) session_codec: Option<std::sync::Arc<oxide_fw_server::SessionCodec>>,
    pub(crate) ppr_shells: Option<PprShells>,
    /// V2-F07: the resumable-stream wiring (the replay store + the
    /// reattach limiter). `None` keeps the pre-F07 shapes (no reattach
    /// arm, no tee); the stream pins construct one.
    pub(crate) streams: Option<std::sync::Arc<crate::serve::streams::StreamWiring>>,
    /// V2-F12: the brownout wiring (the governor the sampler drives).
    /// `None` keeps the pre-F12 shapes (no enforcement stage); the
    /// brownout pins construct one.
    pub(crate) brownout: Option<std::sync::Arc<crate::serve::brownout::BrownoutWiring>>,
}

#[cfg(test)]
pub(crate) fn build_dispatch(
    tree: std::sync::Arc<oxide_fw_router::RouteTree>,
    metrics: std::sync::Arc<oxide_fw_server::Metrics>,
    opts: DispatchOpts,
) -> std::sync::Arc<dyn Fn(oxide_fw_server::OxideRequest) -> BoxedResponse + Send + Sync> {
    build_dispatch_swappable(DispatchArgs {
        swappable: SwappableRouteTree::new(tree),
        metrics,
        proxy_rules: opts.proxy_rules,
        page_renderer: opts.page_renderer,
        loader_runner: opts.loader_runner,
        routes_root: opts.routes_root,
        boot_script_url: opts.boot_script_url,
        boot_script_integrity: opts.boot_script_integrity,
        l0: opts.l0,
        actions: opts.actions,
        archived: opts.archived,
        pool: opts.pool,
        i18n_cfg: opts.i18n_cfg,
        endpoint_dispatcher: opts.endpoint_dispatcher,
        middleware_dispatcher: opts.middleware_dispatcher,
        vary_headers: opts.vary_headers,
        waterfall: opts.waterfall,
        session_codec: opts.session_codec,
        ppr_shells: opts.ppr_shells,
        streams: opts.streams,
        brownout: opts.brownout,
        esm_state: None,
        record: None,
        recordings: std::sync::Arc::new(crate::record_wire::Recordings::new()),
        // The test-pipeline default: no generations handle — the wrapper's
        // `None` arm keeps the pre-A04 byte shapes, and the pins that need
        // the stage construct their own set.
        generations: None,
        // No live hub either — the endpoint arm is inert and the envelope
        // never advertises `live` (the pins that exercise it boot the real
        // server, which builds the state from the config).
        live: None,
        // No serving pools — the route step is inert (the pre-pool byte
        // shapes); the skew e2e boots the real server, which builds them.
        serving_pools: None,
    })
}

/// §9.3 / B6: copy an action response's Set-Cookie headers into the
/// middleware-headers list, which the render path applies to every response
/// it produces. Before this helper existed, the 200 re-render path dropped
/// the action response entirely (extracting only the body into action_form),
/// so a cookie-setting action that re-rendered lost its cookie.
///
/// Kept as a named helper so a unit test can exercise the pattern without
/// building a full dispatch fixture (an action fires only for a POST with
/// `?/<action>` on a route that has a server module — a shape that takes a
/// lot of scaffolding to reach).
pub(crate) fn forward_action_set_cookies(
    resp_headers: &[(String, String)],
    middleware_headers: &mut Vec<(String, String)>,
) {
    for (k, v) in resp_headers {
        if k.eq_ignore_ascii_case("set-cookie") {
            middleware_headers.push((k.clone(), v.clone()));
        }
    }
}

/// P-07b: the JSON shape of matched route params, shared by the middleware
/// and endpoint dispatchers. Built lazily at the call sites so page requests
/// that reach neither dispatcher never pay the per-param allocations.
pub(crate) fn build_params_json(
    params: &std::collections::BTreeMap<String, String>,
) -> serde_json::Map<String, serde_json::Value> {
    params
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect()
}

pub(crate) struct DispatchArgs {
    pub swappable: SwappableRouteTree,
    pub metrics: std::sync::Arc<oxide_fw_server::Metrics>,
    pub proxy_rules: Vec<oxide_fw_dev::ProxyRule>,
    pub page_renderer: Option<std::sync::Arc<dyn oxide_fw_server::PageRenderer>>,
    pub loader_runner: Option<std::sync::Arc<dyn oxide_fw_server::LoaderRunner>>,
    pub routes_root: std::path::PathBuf,
    pub boot_script_url: Option<String>,
    /// V2-E04: the manifest's integrity digest for the boot chunk (None when
    /// `security.sri` is off or the manifest predates the field).
    pub boot_script_integrity: Option<String>,
    pub l0: Option<std::sync::Arc<l0_cache::L0Cache>>,
    pub actions: Option<std::sync::Arc<dyn oxide_fw_server::ActionDispatcher>>,
    pub archived: Option<std::sync::Arc<oxide_fw_router::ArchivedTree>>,
    pub pool: Option<std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
    pub i18n_cfg: Option<oxide_fw_assets::i18n::I18nConfig>,
    pub endpoint_dispatcher: Option<std::sync::Arc<dyn oxide_fw_server::EndpointDispatcher>>,
    pub middleware_dispatcher: Option<std::sync::Arc<dyn oxide_fw_server::MiddlewareDispatcher>>,
    pub vary_headers: Vec<String>,
    pub waterfall: Option<oxide_fw_dev::WaterfallRecorder>,
    pub session_codec: Option<std::sync::Arc<oxide_fw_server::SessionCodec>>,
    pub ppr_shells: Option<PprShells>,
    pub esm_state: Option<std::sync::Arc<crate::esm_dev::EsmDevState>>,
    /// V2-F15: the recorder capture (`record:` config key) — `None` when
    /// off, and the pipeline is byte-identical.
    pub record: Option<std::sync::Arc<crate::record_wire::RecordCapture>>,
    /// V2-F15: the in-flight captures, keyed by request id.
    pub recordings: std::sync::Arc<crate::record_wire::Recordings>,
    /// V2-A04 + F10: the generations the data plane answers for — the
    /// bounded skew pool (the boot build id plus up to
    /// `deploy.generations` demoted builds, each accepted for
    /// `deploy.skewWindow` from its own demotion). `Some` on every real
    /// server boot (the stage runs); `None` keeps the pre-A04 byte shapes
    /// (the test helper's default). The stage install and the advertise
    /// stamp live in [`build_dispatch_swappable`]'s wrapper, driven
    /// through the same `AcceptedGenerations` seam the 1-deep set uses.
    pub generations:
        Option<std::sync::Arc<std::sync::RwLock<oxide_fw_dispatch::skew::GenerationPool>>>,
    /// V2-F05: the live hub + rerun channel (`serve::live::LiveState`) —
    /// `Some` on every real server boot with `live.transport = "sse"` (the
    /// endpoint arm runs, the revalidation hooks fan out); `None` for the
    /// `poll` transport and the test helper (the arm is inert).
    pub live: Option<std::sync::Arc<crate::serve::live::LiveState>>,
    /// V2-F10 (the serving half): a pool + loader runner + frozen loader
    /// source root per generation. `Some` on every production serve boot
    /// with a worker pool; `None` in dev and the test helper (the route
    /// step is inert and the boot handles serve every generation, the
    /// pre-pool byte shapes).
    pub serving_pools: Option<std::sync::Arc<crate::serve::generations::ServingPools>>,
    /// V2-F07: the resumable-stream wiring (`serve::streams::StreamWiring`
    /// — the replay store + the per-IP reattach limiter). `Some` on every
    /// real server boot (the reattach arm runs and resumable endpoint
    /// responses tee); `None` for the test helper and any arm without a
    /// store (no stream was ever teed, so the arm would answer every id
    /// `Gone` — the pre-F07 fall-through is the honest shape there).
    pub streams: Option<std::sync::Arc<crate::serve::streams::StreamWiring>>,
    /// V2-F12: the brownout wiring (`serve::brownout::BrownoutWiring` —
    /// the governor the sampler drives). `Some` on every real server boot
    /// with `brownout.enabled` (the enforcement stage runs, the ops
    /// endpoints report the level); `None` for the test helper and every
    /// `enabled: false` boot (the stage is inert, the pre-F12 shapes).
    pub brownout: Option<std::sync::Arc<crate::serve::brownout::BrownoutWiring>>,
}

/// The boot handles a demoted generation swaps in per request (the B-04
/// tuple): the loader runner, the page renderer, the JS pool and the routes
/// root the OLD generation serves from. `None` handles keep the boot
/// generation's handles (the dev shape — byte-identical to the pre-pool
/// behavior).
type RoutedGenerationHandles = (
    Option<std::sync::Arc<dyn oxide_fw_server::LoaderRunner>>,
    Option<std::sync::Arc<dyn oxide_fw_server::PageRenderer>>,
    Option<std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
    std::path::PathBuf,
);

pub(crate) fn build_dispatch_swappable(
    args: DispatchArgs,
) -> std::sync::Arc<dyn Fn(oxide_fw_server::OxideRequest) -> BoxedResponse + Send + Sync> {
    let DispatchArgs {
        swappable,
        metrics,
        proxy_rules,
        page_renderer,
        loader_runner,
        routes_root,
        boot_script_url,
        boot_script_integrity,
        l0,
        actions,
        archived,
        pool,
        i18n_cfg,
        endpoint_dispatcher,
        middleware_dispatcher,
        vary_headers,
        waterfall,
        session_codec,
        ppr_shells,
        esm_state,
        record,
        recordings,
        generations,
        live,
        serving_pools,
        streams,
        brownout,
    } = args;

    let boot_script_url_for_render = boot_script_url.clone();
    let boot_script_integrity_for_render = boot_script_integrity.clone();
    let ppr_for_render = ppr_shells.clone();
    let actions_for_dispatch = actions.clone();
    let endpoint_for_dispatch = endpoint_dispatcher.clone();
    let middleware_for_dispatch = middleware_dispatcher.clone();
    // PERF-04: single-flight coalescing. Concurrent misses for the same L0
    // key run one render; the rest await the owner's entry. One map per
    // process, shared across every dispatch call.
    let flight_map: std::sync::Arc<
        dashmap::DashMap<
            String,
            std::sync::Arc<tokio::sync::broadcast::Sender<std::sync::Arc<oxide_fw_cache::Entry>>>,
        >,
    > = std::sync::Arc::new(dashmap::DashMap::new());
    // §8.5: a middleware `{ rewrite }` re-routes the request to another path.
    // The cleanest way to honour that (without duplicating the 280-line route
    // precompute) is a bounded self-redispatch: after middleware returns a
    // rewrite, this closure re-invokes itself with the rewritten URL and a
    // hop header, so the new URL goes through the normal match → loader →
    // render path. The hop header prevents an infinite rewrite loop (a
    // middleware that always rewrites to itself would otherwise spin).
    type SelfDispatch = std::sync::Arc<
        std::sync::OnceLock<
            std::sync::Arc<dyn Fn(oxide_fw_server::OxideRequest) -> BoxedResponse + Send + Sync>,
        >,
    >;
    let self_dispatch: SelfDispatch = std::sync::Arc::new(std::sync::OnceLock::new());
    let self_dispatch_for_closure = std::sync::Arc::clone(&self_dispatch);
    // V2-A04: the render arm bakes the CURRENT build id into the streamed
    // shell's hydration bootstrap (the same value the stage checks), so the
    // client's `withBuild` echo is always accepted. The handle is captured
    // (shared-borrow use only — the closure is `Fn`); the per-request read
    // happens inside the closure body, the guard never enters an async
    // block.
    let generations_for_render = generations.clone();
    // V2-F07: the resumable-stream wiring the tee (endpoint responses) and
    // the reattach route (`GET /__oxide/stream/<id>`) share, plus the
    // per-process id counter the tee mints stream ids with
    // (`stream_id(request_id, counter)` — blake3 over the request id and
    // the counter, the A04 id convention).
    let streams_for_dispatch = streams.clone();
    let stream_counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    // V2-F12: the brownout wiring the enforcement stage reads (one
    // relaxed atomic per request at `Normal`); the sampler/probe tasks
    // are spawned at boot.
    let brownout_for_dispatch = brownout.clone();
    // The framework dispatch itself. `inner_dispatch` because the V2-A04
    // stage install (below) wraps it — the wrapper owns the data-plane
    // build check and the advertise stamp, so the ~2000-line body keeps
    // its own return points untouched.
    let inner_dispatch: std::sync::Arc<
        dyn Fn(oxide_fw_server::OxideRequest) -> BoxedResponse + Send + Sync,
    > = std::sync::Arc::new(move |req: oxide_fw_server::OxideRequest| {
        let tree = swappable.load();
        let started = std::time::Instant::now();
        // V2-A04: the build id the render arm bakes into the streamed
        // shell's hydration bootstrap — read once per request, under the
        // read guard, in this sync section (the guard never enters the
        // async blocks; a `None` handle reads as `None` and the bootstrap
        // keeps its legacy three-field shape).
        let boot_build_id = generations_for_render
            .as_ref()
            .and_then(|g| g.read().ok())
            .map(|g| g.current().to_string());
        // Rewrite-hop guard (§8.5 / S-20): the hop count is tracked on the
        // internal `rewrite_hops` field, never a client-supplied header — a
        // client cannot pre-set it to exhaust the rewrite budget. Inbound
        // `x-oxide-rewrite-hop` headers are stripped at `from_hyper_request`.
        let rewrite_hop: u32 = req.rewrite_hops as u32;
        let _is_rewritten = rewrite_hop > 0;
        if rewrite_hop > 8 {
            return Box::pin(async move {
                oxide_fw_server::OxideResponse::html(
                    508,
                    "middleware rewrite loop detected (too many hops)",
                )
            });
        }
        // Native ESM dev server (Phase 1): the dispatch intercept owns
        // `/src/*` (transformed ESM) and `/@*` (prebundle / boot / error
        // shim). The pipeline's StaticAssets pass already skips those
        // prefixes in dev; this is the actual handler.
        //
        // `/src/*` files whose client transform lives in the Node tooling
        // pool (Vue/Svelte SFCs, Solid tsx) go through the async
        // `serve_pool_compiled_module` — the pool compiles the CURRENT file
        // bytes per request, which is what lets a watcher-published
        // update entries serve fresh code with zero server-side cache
        // management.
        if let Some(esm) = esm_state.as_ref() {
            match crate::esm_dev::handle_esm_request(esm, &req) {
                Some(crate::esm_dev::EsmAction::Ready(resp)) => {
                    return Box::pin(async move { resp });
                }
                Some(crate::esm_dev::EsmAction::PoolModule(rel)) => {
                    let esm = std::sync::Arc::clone(esm);
                    let pool = pool.clone();
                    return Box::pin(async move {
                        crate::esm_dev::serve_pool_compiled_module(&esm, &rel, pool.as_ref()).await
                    });
                }
                None => {}
            }
        }
        // PERF-02/PERF-19 (readiness): surface the pool's live gauges into the
        // metrics registry — but NOT per request. `stats()` copies and sorts
        // up to 1000 wait samples under a mutex on every call, which the
        // request path paid for nothing. A gauge needs second-scale freshness:
        // refresh at most once per second (the atomic gate is process-global;
        // a lost race just skips one refresh).
        if let Some(p) = pool.as_ref() {
            const POOL_GAUGE_INTERVAL_MS: u64 = 1000;
            static POOL_GAUGE_LAST_MS: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let last = POOL_GAUGE_LAST_MS.load(std::sync::atomic::Ordering::Relaxed);
            if now_ms.saturating_sub(last) >= POOL_GAUGE_INTERVAL_MS
                && POOL_GAUGE_LAST_MS
                    .compare_exchange(
                        last,
                        now_ms,
                        std::sync::atomic::Ordering::Relaxed,
                        std::sync::atomic::Ordering::Relaxed,
                    )
                    .is_ok()
            {
                let s = p.stats();
                metrics.observe_pool(s.workers, s.in_flight, s.queue_wait_ms_p99);
            }
        }
        let raw = req.url.split(['?', '#']).next().unwrap_or("/").to_string();

        // §13.5 proxy: if the request matches a rule, forward upstream and
        // skip routing entirely. Response is passed through unchanged.
        if !proxy_rules.is_empty() {
            let query = req.url.split_once('?').map(|(_, q)| q.to_string());
            if let Some(m) = oxide_fw_dev::apply_proxy(&proxy_rules, &raw, query.as_deref()) {
                let url = m.url.clone();
                let m_rule = m.rule.prefix.clone();
                let metrics_proxy = std::sync::Arc::clone(&metrics);
                let req_owned = req.clone();
                return Box::pin(async move {
                    match oxide_fw_server::forward_proxy(&url, req_owned).await {
                        Ok(r) => {
                            metrics_proxy.observe("_proxy", started.elapsed(), r.status);
                            r
                        }
                        Err(e) => {
                            tracing::warn!(rule = %m_rule, error = %e, "proxy forward failed");
                            let r = oxide_fw_server::OxideResponse::new(
                                502,
                                format!("proxy upstream error: {}", e.0.message),
                            );
                            metrics_proxy.observe("_proxy", started.elapsed(), 502);
                            r
                        }
                    }
                });
            }
        }
        // V2-F05 step 2: `GET /__oxide/live` — the SSE endpoint. Data plane
        // (the wrapper's stage already checked the build id: EventSource
        // cannot set headers, so the A04 stage reads the `build` query
        // parameter on this path). The arm composes the substrate's pure
        // pieces: the query parse, the same-origin gate, the live-route
        // lookup, the subscribe-or-503 response, and the connect-time first
        // snapshot (the runner re-runs the data route; the watch channel
        // means a late receiver still sees it as its initial frame).
        if raw == "/__oxide/live"
            && let Some(live) = live.as_ref()
        {
            let not_accepted = |status: u16, msg: &'static str| -> BoxedResponse {
                Box::pin(async move { oxide_fw_server::OxideResponse::html(status, msg) })
            };
            if !req.method.eq_ignore_ascii_case("GET") {
                return not_accepted(405, "live: GET only");
            }
            if !oxide_fw_server::live::same_origin(
                req.header("origin"),
                req.header("sec-fetch-site"),
                req.header("host"),
            ) {
                // §V2-F05 security: Origin check mandatory; missing evidence
                // denies (`same_origin`'s contract — curl/tests included).
                return not_accepted(403, "live: cross-origin connections are refused");
            }
            let query = req.url.split_once('?').map(|(_, q)| q).unwrap_or("");
            let Some(parsed) = oxide_fw_server::live::parse_live_query(query) else {
                return not_accepted(400, "live: missing route parameter");
            };
            // The route must exist AND be opted in (`live: true` in its
            // `page.server.ts` config) — an unknown or non-live route is a
            // plain 404, the same face an unmatched URL has.
            let route_path = {
                let stripped = parsed.route.trim_end_matches('/');
                if stripped.is_empty() {
                    "/".to_string()
                } else {
                    stripped.to_string()
                }
            };
            let candidates: Vec<String> = if route_path == "/" {
                vec!["/".to_string(), "/index".to_string()]
            } else {
                vec![route_path.clone()]
            };
            let matched = candidates.iter().find_map(|p| match &archived {
                Some(a) => a.match_path(p),
                None => tree.match_path(p),
            });
            if !matched.as_ref().is_some_and(|m| m.route.live.is_some()) {
                return not_accepted(404, "live: no live route at that path");
            }
            // The header snapshot the rerun replays (cookies included; the
            // class is log-guarded by `is_secret_key`). Tags seed with the
            // route's own path tag — the fan-out surfaces both `tag` and
            // `path` revalidate events (see `serve::live`).
            let sub = std::sync::Arc::new(oxide_fw_server::live::Sub::new(
                route_path,
                parsed.url,
                (*req.headers).clone(),
                std::iter::once(oxide_fw_cache::tiers::path_tag(&parsed.route)),
                req.peer,
            ));
            // The connect-time first snapshot: the runner re-runs the data
            // route and pushes it (fresh connection ⇒ full snapshot — the
            // subscriber has no base yet, so the F06 policy ships the full).
            live.enqueue(std::sync::Arc::clone(&sub));
            let resp = live.endpoint_response(sub, boot_build_id.as_deref());
            return Box::pin(async move { resp });
        }
        // V2-F07 step 2: `GET /__oxide/stream/<id>` — the reattach route.
        // Data plane (the wrapper's stage already checked the build id:
        // the reattach replays response bytes, so it enforces the same
        // contract). The arm composes the substrate's pure pieces — the
        // id parse (`stream_resume_id`), the `Range: bytes=<n>-` offset
        // (`parse_range_offset`, absent header = 0), the store lookup and
        // the two responses (`stream_resume_response` / the `410`
        // `stream_gone_response`). The per-IP rate limit rides the
        // sharded limiter the wiring boots (a hot loop over one id is
        // pure waste; the client's own backoff ladder tops out at five).
        // Without a store (`None`: the test helper, a serve that predates
        // the config) the URL falls through to route matching and 404s —
        // no stream was ever teed, so nothing could reattach.
        if let Some(sw) = streams_for_dispatch.as_ref()
            && let Some(id) = oxide_fw_dispatch::build::stream_resume_id(&raw)
        {
            if !req.method.eq_ignore_ascii_case("GET") {
                return Box::pin(async move {
                    oxide_fw_server::OxideResponse::html(405, "stream reattach: GET only")
                });
            }
            // S-10's unspoofable-key rule: the peer socket address (set by
            // the accept loop), never a client header. A synthetic request
            // (in-process pins) has no peer and shares the limiter's
            // catch-all bucket — the cap is 60/min, far above anything a
            // pin or a well-behaved client needs.
            let peer_key = req
                .peer
                .map(|p| p.to_string())
                .unwrap_or_else(|| "stream-synthetic".to_string());
            if !sw.limiter.allow(
                &format!("stream-reattach:{peer_key}"),
                crate::serve::streams::REATTACH_RATE_MAX,
                crate::serve::streams::REATTACH_RATE_WINDOW_SECS,
            ) {
                return Box::pin(async move {
                    oxide_fw_server::OxideResponse::new(429, "stream reattach rate limit exceeded")
                        .with_header(
                            "retry-after",
                            crate::serve::streams::REATTACH_RATE_WINDOW_SECS.to_string(),
                        )
                });
            }
            let offset = oxide_fw_server::stream_store::parse_range_offset(&req.headers);
            let reattach = sw.store.get(id);
            return Box::pin(async move {
                match reattach {
                    oxide_fw_server::stream_store::Reattach::Ok(buf) => {
                        oxide_fw_server::stream_store::stream_resume_response(
                            &buf,
                            offset.unwrap_or(0),
                        )
                    }
                    oxide_fw_server::stream_store::Reattach::Gone => {
                        oxide_fw_server::stream_store::stream_gone_response()
                    }
                }
            });
        }
        // Route conventions: the root page module is `index`, so `/` maps to
        // the `/index` pattern (spec §8.1); trailing slashes are folded too.
        let normalized = if raw.len() > 1 && raw.ends_with('/') {
            raw.trim_end_matches('/').to_string()
        } else {
            raw
        };
        // §11.3 / BUG-01: remember the request path. The L0 entry for
        // this request is tagged `path:<req_path>` so a `revalidate.path`
        // event (which reaches the dispatch via `x-oxide-revalidate`)
        // actually invalidates it. Without this, entries are tagged with
        // the *route id* only, and path events match nothing.
        let req_path = normalized.clone();
        // SPEC-10 / §16.4: resolve the request locale when the project
        // configures i18n — path prefix first, then `Accept-Language`,
        // then the default. Empty when no i18n is configured.
        // `i18n_cfg` is captured by the dispatch closure (built from config
        // before any request); `req` supplies the header.
        let resolved_locale: String = match i18n_cfg.as_ref() {
            Some(a) => {
                let stripped = oxide_fw_assets::i18n::strip_locale_prefix(a, &req_path);
                let from_path = stripped.0 != a.default && stripped.1 != req_path.as_str();
                if from_path {
                    stripped.0
                } else {
                    oxide_fw_assets::i18n::negotiate(a, req.header("accept-language").unwrap_or(""))
                }
            }
            None => String::new(),
        };
        // PERF-01 / §8.4: a `/__data` GET names its target route via
        // `?route=<path>`. The endpoint re-matches THAT path against the
        // route tree (below), so the whole prelude — route id, params,
        // layout chain, nearest middleware, error boundary — describes
        // the route whose data the client router is fetching. That is
        // what makes client-side navigation data match the SSR render
        // (§8.4 parity); previously /__data matched nothing, ran no
        // middleware, no layout loads, and no defer resolution.
        let data_route_q: String =
            if req.method.eq_ignore_ascii_case("GET") && req_path == "/__data" {
                let raw_q = req.url.split_once('?').map(|(_, q)| q).unwrap_or("");
                raw_q
                    .split('&')
                    .find_map(|kv| {
                        kv.strip_prefix("route=")
                            .map(oxide_fw_server::percent_decode)
                    })
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "/".to_string())
            } else {
                String::new()
            };
        let is_data_request = !data_route_q.is_empty();
        // §8.4: for a data request, match the TARGET route (trailing
        // slash folded like a real URL path); otherwise the request path.
        let match_path = if is_data_request {
            let stripped = data_route_q.trim_end_matches('/');
            if stripped.is_empty() {
                "/".to_string()
            } else {
                stripped.to_string()
            }
        } else {
            normalized.clone()
        };
        let candidates = if match_path == "/" {
            vec!["/".to_string(), "/index".to_string()]
        } else {
            vec![match_path]
        };
        let matched: Option<oxide_fw_router::MatchResult> = match &archived {
            Some(a) => candidates.iter().find_map(|p| a.match_path(p)),
            None => candidates.iter().find_map(|p| tree.match_path(p)),
        };
        let (status, label) = match &matched {
            Some(m) => (200u16, pattern_of(&m.route.segments)),
            None => (404u16, "_unmatched".to_string()),
        };

        // V2-F10 step 2 (the serving half, §V2-F10 "Routing by build"):
        // resolve the generation this request's build id routes to, then
        // shadow the boot handles — every loader/render/ESM execution below
        // runs the ROUTED generation's pool, runner and frozen loader
        // source root. Data-plane requests with an accepted demoted id run
        // the OLD generation's handles (its loader bytes are the snapshot
        // taken when that generation became current — the source tree
        // cannot leak into an accepted old-build response); headerless
        // requests and HTML navigations ride CURRENT; unknown ids never
        // get here (the wrapper's stage answered 409). A `None` pools
        // handle (dev, the test helper) keeps the boot handles — the
        // pre-pool behavior, byte-identical.
        let (routed_runner, routed_renderer, routed_pool, routed_routes_root): RoutedGenerationHandles =
            match serving_pools.as_ref() {
            Some(sp) => {
                let on_data_plane = oxide_fw_dispatch::build::is_data_plane_path(&normalized);
                let id = if on_data_plane {
                    req.header("x-oxide-build")
                } else {
                    // HTML navigations always go current (§V2-F10 step 2).
                    None
                };
                match sp.route_for(id) {
                    Some((generation, _)) => (
                        generation.runner.clone(),
                        page_renderer.clone(),
                        Some(std::sync::Arc::clone(&generation.pool)),
                        generation.routes_root.clone(),
                    ),
                    // The stage accepted an id the pools cannot route (a
                    // poisoned lock): the boot handles serve rather than
                    // refuse — the acceptance contract stays the stage's.
                    None => (
                        loader_runner.clone(),
                        page_renderer.clone(),
                        pool.clone(),
                        routes_root.clone(),
                    ),
                }
            }
            None => (
                loader_runner.clone(),
                page_renderer.clone(),
                pool.clone(),
                routes_root.clone(),
            ),
        };
        let loader_runner = routed_runner;
        let page_renderer = routed_renderer;
        // PERF/serve: the generation-routing step produced a per-generation
        // pool handle before the serving-pools adoption moved the routing
        // into `serving_pools` — the binding is kept (drop at scope end)
        // but nothing downstream reads it.
        let _pool = routed_pool;
        let routes_root = routed_routes_root;

        // Pre-compute the loader inputs synchronously so the async block
        // owns plain data (no borrow of `matched`).
        let (module_path, route_id, params, has_server_module, render_mode, needs_parent, is_live): LoaderInputs = match &matched {
            Some(m) => {
                let has_srv = m.route.has_server_module;
                // BUG: only produce a module_path when the route actually
                // declares a server module. Setting it unconditionally made
                // `route_data_response` (and any consumer that keyed on
                // `module_path.is_some()` rather than `has_server_module`)
                // fire the loader for routes whose sibling `page.server.ts`
                // does not exist — a 500 with `ENOENT` from `/__data`.
                let mp = if has_srv {
                    Some(routes_root.join(m.route.file.with_file_name("page.server.ts")))
                } else {
                    None
                };
                (
                    mp,
                    Some(m.route.id.0.clone()),
                    m.params.clone(),
                    has_srv,
                    m.route.render_mode.clone(),
                    m.route.needs_parent,
                    m.route.live.is_some(),
                )
            }
            None => (
                None,
                None,
                std::collections::BTreeMap::new(),
                false,
                None,
                false,
                false,
            ),
        };

        // §8.1 route.ts / §10.2 step 7: pre-compute whether this is an
        // Endpoint route and, if so, the on-disk path to its `route.ts`.
        // Endpoints bypass the loader/render phases entirely: a `route.ts`
        // returns a Web `Response` directly.
        let endpoint_module_path: Option<std::path::PathBuf> = match &matched {
            Some(m) if matches!(m.route.kind, oxide_fw_router::RouteKind::Endpoint) => {
                Some(routes_root.join(&m.route.file))
            }
            _ => None,
        };

        // §8.1 middleware.ts / §8.5: resolve the full ancestor middleware
        // chain for the matched route, OUTERMOST first. Every middleware
        // whose directory is an ancestor of the route's directory runs, in
        // outer→inner order — a `routes/middleware` wraps
        // `routes/blog/middleware`, which wraps the page. (Previously only
        // the nearest ancestor ran, which dropped outer auth/security
        // middleware; §8.5 requires the chain.)
        //
        // Ordering key: ancestor-directory length. `routes` (len 6) sorts
        // before `routes/blog` (len 11), i.e. outer before inner. Ties
        // cannot happen (each directory yields at most one middleware
        // file). An unmatched URL still gets the root middleware, matching
        // the previous behaviour.
        // P-06: served from the tree's precomputed prefix index — no full
        // entry scan, no per-candidate `format!`, no sort per request.
        let middleware_module_paths: Vec<std::path::PathBuf> = tree
            .middleware_files_for(route_id.as_deref().unwrap_or(""))
            .into_iter()
            .map(|f| routes_root.join(f))
            .collect();
        // P-07b: built on first use. The JSON param map is consumed only by
        // the middleware loop and the §10.2 endpoint dispatcher; a page
        // request with no middleware file and no route.ts skips the
        // per-param allocations entirely (`params` itself stays alive for
        // the loader / L0-key paths below).
        let mut endpoint_params_json: Option<serde_json::Map<String, serde_json::Value>> = None;
        // P-07: header/method/url are borrowed from `req_owned` at the
        // dispatcher call sites instead of cloned per request.
        let endpoint_body: bytes::Bytes = req.body.clone();
        // PERF-03: one nonce per request. The blake3 hash of the request id
        // was recomputed at up to 5 injection points (L0 hit materialize,
        // 304-materialize, loader input, streaming head, deferred settles);
        // every site stamps the SAME value (§17.1 strict-dynamic), so hash
        // once here and reuse it everywhere.
        let request_nonce: String = oxide_fw_server::nonce_for(&req.request_id);

        // §10.2 step 5 / §8.3: an unmatched URL gets a 404 status (the
        // `status` label above). To render a user-visible 404 page the
        // request must resolve to the project's `not-found.tsx` route id
        // (with its layout chain), not to a placeholder component that
        // `componentFor` cannot find. Precompute both here, before
        // `matched` is consumed, so the async render block below can use
        // them.
        // P-06: not-found route + layout chain, precomputed by the tree index.
        let (unmatched_route_id, unmatched_layout_ids): (Option<String>, Vec<String>) =
            match tree.not_found_route() {
                Some((id, chain)) => (Some(id), chain.into_iter().map(|r| r.0).collect()),
                None => (None, Vec::new()),
            };

        // §10.2 step 5: resolve the NEAREST ancestor `error.tsx` for the
        // matched route (innermost boundary wins — a `routes/blog/error`
        // shadows `routes/error`). Precomputed here so the async render
        // block can pass it as `input.error_route_id`. The adapter
        // renders this component when the page throws.
        // P-06: nearest error boundary from the precomputed index.
        let error_route_id: Option<String> =
            tree.error_route_for(route_id.as_deref().unwrap_or(""));
        // The URL pattern (`pattern_of(segments)`) is for metrics labels;
        // the ROUTE ID is what the framework adapter's `componentFor`
        // expects. Two conventions to reconcile here:
        //
        //   • Router ids include the file role: `routes/index/page`,
        //     `routes/blog/[slug]/page`. That's the shape the scanner
        //     produces (see crates/router/src/scan.rs) and what
        //     `RouteEntry.id.0` carries.
        //   • Adapter keys (`PAGES` in the IR renderer, `componentFor`
        //     in a user's config) are keyed on the route directory:
        //     `routes/index`, `routes/blog/[slug]`. Trailing role
        //     suffix (`/page`, `/layout`) is not part of the key.
        //
        // Normalize by stripping a trailing `/page` (and only `/page`)
        // before handing the id to the renderer. Layout-only ids, slot
        // ids (`routes/@modal/page`), and group-prefixed ids all pass
        // through untouched.
        let route_id_for_render: Option<String> = route_id
            .as_deref()
            .map(|s| s.strip_suffix("/page").unwrap_or(s).to_string());

        // §8.2 / §16.3: precompute the layout chain (outermost first)
        // before `matched` is dropped. Ids carry the role suffix the
        // scanner emits (`routes/layout`), which is also the key
        // adapters use for `componentFor` (same convention as the page
        // id after `/page` is stripped). An unmatched URL gets an empty
        // chain.
        let layout_ids: Vec<String> = match &matched {
            Some(m) => m.layout_chain.iter().map(|id| id.0.clone()).collect(),
            None => Vec::new(),
        };
        // §9.2: layout module paths for the parallel layout-loads phase.
        // Each layout id (`routes/blog/layout`) maps to its
        // `layout.server.ts` on disk, in outermost → innermost order.
        let layout_module_paths: Vec<(String, std::path::PathBuf)> = layout_ids
            .iter()
            .filter_map(|lid| {
                tree.entries
                    .iter()
                    .find(|e| {
                        e.id.0 == *lid && matches!(e.kind, oxide_fw_router::RouteKind::Layout)
                    })
                    .map(|e| {
                        (
                            lid.clone(),
                            routes_root.join(e.file.with_file_name("layout.server.ts")),
                        )
                    })
            })
            .filter(|(_, p)| p.is_file())
            .collect();

        // §9.3: a form POST to `?/<name>` is an action invocation. Only a
        // POST with a named action on a route that has a server module is
        // eligible; everything else falls through to the loader/render path.
        let action_name: Option<String> = if req.method.eq_ignore_ascii_case("POST") {
            action_from_query(&req.url)
        } else {
            None
        };
        let action_dispatcher = actions_for_dispatch.clone();
        let action_params: serde_json::Map<String, serde_json::Value> = params
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        let action_route = route_id_for_render.clone().unwrap_or_default();

        let page_renderer = page_renderer.clone();
        let metrics = std::sync::Arc::clone(&metrics);
        // P-07: `req` is this invocation's own parameter, so move it into
        // the async block instead of cloning the whole request per call.
        let req_owned = req;
        // V2-F15: the capture starts HERE, where the request line-set and
        // the router's verdict are both known. The record keeps the raw
        // match (`route_id`) — the id the replay drives back through the
        // pipeline is the same one dispatch matched.
        let req_recording = record
            .as_ref()
            .and_then(|c| crate::record_wire::RequestRecording::start(c, &recordings, &req_owned));
        if let Some(rec) = req_recording.as_ref() {
            rec.set_route(route_id.as_deref().unwrap_or(""), &params);
        }
        // V2-F15: this request's loads run through the recording decorator
        // when the capture is on, so every load the §9.2 graph drives
        // lands in the record (the runner and the graph stay agnostic).
        let loader_runner = match req_recording.as_ref() {
            Some(rec) => rec.decorated(loader_runner.clone()),
            None => loader_runner.clone(),
        };
        // Shadow captured values that the async block consumes (an `Fn`
        // closure cannot surrender a captured variable — E0507).
        let boot_script_url_for_render = boot_script_url_for_render.clone();
        let boot_script_integrity_for_render = boot_script_integrity_for_render.clone();
        // §8.1 route.ts / §10.2 step 7: same pattern for the endpoint
        // dispatcher — the async block below moves this per-call clone.
        let endpoint_for_dispatch = endpoint_for_dispatch.clone();
        // §8.1 middleware.ts / §8.5: same shadow for middleware.
        let middleware_for_dispatch = middleware_for_dispatch.clone();
        // §8.5: same shadow for the self-dispatch handle used by a
        // middleware rewrite (the async block moves it per-call).
        let self_dispatch_for_closure = self_dispatch_for_closure.clone();
        // §20: the waterfall recorder (Arc inside, cheap to clone) is moved
        // by the async block; shadow it per call like the dispatchers above.
        let waterfall = waterfall.clone();
        // §16.6: same shadow for the session codec.
        let session_codec = session_codec.clone();
        // §11 L0: only a GET on a cacheable-mode route is a candidate.
        let l0 = l0.clone();
        // V2-F07: same shadow for the resumable-stream wiring (the tee in
        // the endpoint arm) and the stream-id counter the tee mints ids
        // with.
        let streams_for_dispatch = streams_for_dispatch.clone();
        let stream_counter = stream_counter.clone();
        // V2-F12: the brownout enforcement stage (§V2-F12, "stage after
        // prelude"): the route's overload priority (`page.server.ts`
        // `config.priority`, scanned into the route tree; the `rules`
        // `NativeRule.priority` mirror is the config-side declaration)
        // × the sampler's published level → the pure `serve_decision`.
        // A `Shed` verdict answers 503 + `retry-after: 5` +
        // `x-oxide-brownout` BEFORE any loader, cache or render work; a
        // `ServeStale` verdict arms the L0 lookup with the
        // `staleCeiling` bound and stamps the `warning: 110` header on
        // the served hit. At `Normal` the whole stage costs one relaxed
        // atomic read.
        let mut brownout_ceiling: Option<u64> = None;
        if let Some(bw) = brownout_for_dispatch.as_ref() {
            let level = bw.governor.level();
            if level != oxide_fw_server::governor::Level::Normal {
                let priority = crate::serve::brownout::route_priority(
                    matched.as_ref().and_then(|m| m.route.priority.as_deref()),
                );
                match oxide_fw_server::governor::serve_decision(level, priority) {
                    oxide_fw_server::governor::Decision::Serve => {}
                    oxide_fw_server::governor::Decision::ServeStale => {
                        brownout_ceiling = Some(bw.stale_ceiling_pct);
                    }
                    oxide_fw_server::governor::Decision::Shed => {
                        return Box::pin(
                            async move { oxide_fw_server::governor::shed_response(level) },
                        );
                    }
                }
            }
        }
        // §11.1: L0 caches the FULL response for cacheable modes. The
        // layer table puts no server-module restriction on L0 ("L0 | Full
        // response (HTML, headers, tags) | Rust, no JS on hit" — spec
        // §11.1), and loader routes invalidate through the same
        // revalidate.tag/path channel as every other entry (BUG-01), so
        // the previous `!has_server_module` exclusion is gone. The
        // render mode is the only gate: dynamic routes opt out via
        // `mode_is_cacheable` (static | isr — §10.1). Caveat noted for
        // reviewers: the Vary fingerprint today covers Accept-Language
        // only (§11.2 also lists declared cookies — a follow-up).
        // A /__data request is NEVER L0-eligible: it must re-run the
        // loaders, not serve a route's cached HTML.
        // §11.1 / §10.1: normalize `render_mode` the same way the persisted
        // server manifest does (cmd_build line ~3681): `None` + no server
        // module → static, `None` + server module → dynamic. Without this,
        // a page route whose scanner left `render_mode = None` (no
        // page.server.ts AND no explicit config.render) reaches dispatch
        // with `None`; `mode_is_cacheable(None)` returns false → the L0
        // branch is skipped entirely and every request answers
        // `x-oxide-fw-cache: bypass` (the manifest, meanwhile, shows `static`).
        // Making dispatch agree with the manifest is the fix.
        let effective_mode: &str = match render_mode.as_deref() {
            Some(m) => m,
            None => {
                if has_server_module {
                    "dynamic"
                } else {
                    "static"
                }
            }
        };
        let l0_eligible = req_owned.method.eq_ignore_ascii_case("GET")
            && l0_cache::mode_is_cacheable(Some(effective_mode))
            && route_id.is_some()
            && !is_data_request;
        let l0_route = route_id_for_render.clone().unwrap_or_default();
        let l0_params = {
            let mut ks: Vec<(&String, &String)> = params.iter().collect();
            ks.sort();
            ks.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&")
        };
        // §11.2 / §14.1: fingerprint exactly the headers `cache.l0.vary` names.
        let l0_vary = l0_cache::vary_of(&req_owned.headers, &vary_headers);
        // BUG-01: the path tag the L0 put site stamps alongside the route
        // tag, so `revalidate.path` can invalidate it.
        let l0_path = req_path;
        // PERF-04: the single-flight key (matches the L0 key shape) and the
        // shared map handle.
        let flight_key = match l0.as_ref() {
            Some(c) => l0_cache::L0Cache::key_for(&l0_route, &l0_params, &l0_vary, c.build_id()),
            None => String::new(),
        };
        let flight_map = std::sync::Arc::clone(&flight_map);
        let ppr_for_request = ppr_for_render.clone();
        // V2-F05: the revalidation hooks inside the block fan out through a
        // clone — the block is `move`, and moving the captured handle out
        // would demote the `Fn` dispatch closure.
        let live_for_request = live.clone();

        Box::pin(async move {
            // §8.1 middleware.ts / §8.5: run the route's FULL ancestor
            // middleware chain, OUTERMOST first, BEFORE the loader/render
            // phase — and before the /__data branch (§8.4 parity: client
            // nav data must see the same middleware locals/headers/
            // short-circuits as SSR). Each middleware can:
            //   * short-circuit with a `Response` or `{ redirect }`;
            //   * inject `locals` (merged into the accumulated map,
            //     delivered to the next middleware AND the loader via
            //     `LoaderRunInput.locals`);
            //   * add response headers (accumulated).
            // A `{ rewrite }` re-dispatches through the normal path. An
            // error surfaces as a 500 with a `MIDDLEWARE`-coded envelope.
            let stage_middleware_start = std::time::Instant::now();
            let mut middleware_locals: serde_json::Value = serde_json::json!({});
            // §16.6: decode the encrypted session cookie into
            // `locals.session` so a loader/action reads it as an object. A
            // missing or tampered cookie is an empty session, never an error.
            // B-05: the sealed token rides the `Cookie` header, never a
            // header named `oxide_session`; read it through the cookie parser.
            if let Some(codec) = session_codec.as_deref()
                && let Some(token) = req_owned
                    .header("cookie")
                    .and_then(|h| oxide_fw_server::cookie_value(h, "oxide_session"))
                && let Ok(sess) = oxide_fw_server::Session::decode(codec, token)
            {
                middleware_locals["session"] = sess.to_value();
            }
            // B-05: baseline for the session write-back. Middleware is the
            // only phase that can refine `locals.session` (the loader gets a
            // deserialized copy), so diffing around it captures every change.
            let session_before = middleware_locals.get("session").cloned();
            let mut middleware_headers: Vec<(String, String)> = Vec::new();
            // B-22: resume after the layer that issued the rewrite.
            let resume_after = req_owned.mw_resume_after.clone();
            let mut pending_resume = resume_after.as_ref().and_then(|_| {
                // Only honour the marker when the target layer is present in
                // this route's chain; otherwise run the chain normally.
                let present = middleware_module_paths
                    .iter()
                    .any(|p| p.to_string_lossy() == resume_after.as_deref().unwrap_or(""));
                present.then(|| resume_after.clone().unwrap_or_default())
            });
            if let Some(dispatcher) = middleware_for_dispatch.as_deref() {
                for mp in &middleware_module_paths {
                    let module_path_s = mp.to_string_lossy().to_string();
                    if let Some(target) = pending_resume.as_ref() {
                        if &module_path_s == target {
                            // The rewriting layer itself must not run twice.
                            pending_resume = None;
                        }
                        continue;
                    }
                    // Pass the full request (headers + body) so an auth /
                    // upload middleware sees what it needs, plus the
                    // locals accumulated by outer middlewares (None when
                    // nothing has accumulated — see the trait doc).
                    let locals_for_call: Option<&serde_json::Value> =
                        if middleware_locals.as_object().is_some_and(|o| !o.is_empty()) {
                            Some(&middleware_locals)
                        } else {
                            None
                        };
                    let mw_res = dispatcher
                        .dispatch(MiddlewareDispatchArgs {
                            module_path: &module_path_s,
                            method: &req_owned.method,
                            url: &req_owned.url,
                            params: endpoint_params_json
                                .get_or_insert_with(|| build_params_json(&params)),
                            headers: &req_owned.headers,
                            body: endpoint_body.as_ref(),
                            locals: locals_for_call,
                        })
                        .await;
                    match mw_res {
                        Ok(out) => {
                            if out.short {
                                metrics.observe(&label, started.elapsed(), out.status);
                                let mut resp = oxide_fw_server::OxideResponse::new(
                                    out.status,
                                    bytes::Bytes::from(out.body),
                                );
                                for (k, v) in &out.headers {
                                    resp = resp.with_header(k, v.clone());
                                }
                                // §8.5: the worker returns a middleware
                                // redirect as a sibling `redirect` field
                                // (`{ short: true, status, redirect }`), not
                                // as a `location` header. Without this the
                                // browser got a 302 with no destination.
                                if let Some(loc) = out.redirect.as_deref()
                                    && !resp
                                        .headers
                                        .iter()
                                        .any(|(k, _)| k.eq_ignore_ascii_case("location"))
                                {
                                    resp = resp.with_header("location", loc);
                                }
                                // §16.6 / WP-r10-1 / B-05: a middleware that
                                // mutates `locals.session` AND short-circuits
                                // must persist the mutation. The post-loop
                                // write-back (below) never runs for a
                                // short-circuit, so this return path is the
                                // only place the cookie can ride.
                                if let Some(codec) = session_codec.as_deref() {
                                    let after =
                                        out.locals.as_ref().and_then(|l| l.get("session")).cloned();
                                    if after.is_some() && after != session_before {
                                        let mut sess = oxide_fw_server::Session::new();
                                        if let Some(obj) =
                                            after.as_ref().and_then(|v| v.as_object())
                                        {
                                            for (k, v) in obj {
                                                sess.insert(k.clone(), v.clone());
                                            }
                                        }
                                        if let Ok(token) = sess.encode(codec) {
                                            resp = resp.with_header(
                                                "set-cookie",
                                                format!(
                                                    "oxide_session={token}; Path=/; \
                                                     HttpOnly; SameSite=Lax; Max-Age={}",
                                                    session_ttl_secs()
                                                )
                                                .replace(
                                                    "\
", "",
                                                )
                                                .replace("  ", " "),
                                            );
                                        }
                                    }
                                }
                                return resp;
                            }
                            // B-06: last-writer-wins per header name — an
                            // inner middleware refines what an outer one
                            // set, matching the locals merge below. Appending
                            // both produced duplicate header lines.
                            for (k, v) in &out.headers {
                                let lower = k.to_ascii_lowercase();
                                match middleware_headers
                                    .iter_mut()
                                    .find(|(ek, _)| ek.eq_ignore_ascii_case(&lower))
                                {
                                    Some((_, ev)) => *ev = v.clone(),
                                    None => middleware_headers.push((k.clone(), v.clone())),
                                }
                            }
                            // P-08: consume the locals by value — the merge
                            // used to deep-clone every key/value per hop.
                            if let Some(serde_json::Value::Object(src)) = out.locals {
                                let dst = middleware_locals
                                    .as_object_mut()
                                    .expect("middleware_locals is always an object");
                                for (k, v) in src {
                                    dst.insert(k, v);
                                }
                            }
                            if let Some(rw) = out.rewrite.as_deref() {
                                // §8.5: honour the rewrite by re-dispatching
                                // the request to the new URL through the
                                // normal match → loader → render path. The
                                // rewrite-hop header prevents an infinite
                                // loop when a middleware always rewrites.
                                if let Some(re_dispatch) = self_dispatch_for_closure.get() {
                                    let mut new_req = req_owned.clone();
                                    new_req.url = rw.to_string();
                                    // S-20: bump the internal counter, not a
                                    // header, so it cannot be forged.
                                    new_req.rewrite_hops = (rewrite_hop + 1) as u8;
                                    new_req.mw_resume_after = Some(module_path_s.clone());
                                    return re_dispatch(new_req).await;
                                }
                                tracing::warn!(
                                    route = %label,
                                    rewrite = %rw,
                                    "middleware rewrite: self-dispatch unavailable",
                                );
                            }
                        }
                        Err(e) => {
                            metrics.observe(&label, started.elapsed(), e.status);
                            return oxide_fw_server::OxideResponse::json(
                                e.status,
                                serde_json::json!({"error": {
                                    "code": e.code,
                                    "message": e.message,
                                    "route": label,
                                }}),
                            );
                        }
                    }
                }
            }
            // B-05: write-back. If a middleware layer changed
            // `locals.session`, seal it and ride a `Set-Cookie` on every
            // response path via `middleware_headers`.
            if let Some(codec) = session_codec.as_deref() {
                let after = middleware_locals.get("session").cloned();
                if after.is_some() && after != session_before {
                    let mut sess = oxide_fw_server::Session::new();
                    if let Some(obj) = after.as_ref().and_then(|v| v.as_object()) {
                        for (k, v) in obj {
                            sess.insert(k.clone(), v.clone());
                        }
                    }
                    match sess.encode(codec) {
                        Ok(token) => middleware_headers.push((
                            "set-cookie".to_string(),
                            format!(
                                "oxide_session={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
                                session_ttl_secs()
                            ),
                        )),
                        Err(e) => tracing::warn!(
                            error = %e.0.message,
                            "session encode failed; cookie not set",
                        ),
                    }
                }
            }
            // PERF-06: the middleware phase is done — record its duration
            // so `Server-Timing` reports the real cost (§12.1 step 8).
            let middleware_ms = stage_middleware_start.elapsed().as_secs_f64() * 1000.0;
            // V2-F15: the middleware locals are final — the record captures
            // them (the loader's threaded copy rides the loader inputs).
            if let Some(rec) = req_recording.as_ref() {
                rec.set_locals(&middleware_locals);
            }

            // PERF-01 / §8.4: `/__data` is the client router's route-data
            // endpoint. The target route was re-matched in the prelude, so
            // this branch runs with the SAME middleware locals/headers the
            // SSR render saw, the layout-load parent chain, the page
            // loader's data + resolved defers, and the resolved locale —
            // no render, just loaders + ODF encode.
            if is_data_request {
                return route_data_response(RouteDataArgs {
                    req: &req_owned,
                    loader_runner: loader_runner.as_ref(),
                    module_path: module_path.as_deref(),
                    route_id: route_id.as_deref(),
                    params: &params,
                    metrics: &metrics,
                    started,
                    middleware_locals: &middleware_locals,
                    middleware_headers: &middleware_headers,
                    layout_module_paths: &layout_module_paths,
                    locale: &resolved_locale,
                    session_codec: session_codec.as_deref(),
                    middleware_ms,
                    waterfall: waterfall.as_ref(),
                    live: is_live,
                })
                .await;
            }

            // §9.3: a form action POST runs the route's `actions[name]`.
            // A successful action does NOT return its JSON to the browser —
            // the spec's form-first contract is "runs, and re-renders the
            // page with `form` result data". The result is carried into the
            // render phase through this variable; a redirect or error status
            // still returns directly (POST/redirect/GET, and a failed action
            // must not render a success page).
            let mut action_form: Option<serde_json::Value> = None;
            if let (Some(action), Some(module_path)) =
                (action_name.as_deref(), module_path.as_deref())
                && let Some(dispatcher) = action_dispatcher.as_deref()
            {
                let form_json = {
                    let entries = parse_form_urlencoded(&req_owned.body);
                    serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string())
                };
                let action_trace = req_owned.trace.as_ref().and_then(|t| {
                    t.to_traceparent(&t.parent_id).map(
                        |tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}),
                    )
                });
                let resp = oxide_fw_server::run_action(RunActionArgs {
                    dispatcher: Some(dispatcher),
                    module_path: &module_path.to_string_lossy(),
                    route: &action_route,
                    action,
                    form_json: &form_json,
                    params: &action_params,
                    url: &req_owned.url,
                    method: &req_owned.method,
                    headers: &req_owned.headers,
                    locals: &middleware_locals,
                    trace: action_trace.as_ref(),
                })
                .await;
                // B-17: actions get their own waterfall row so the
                // inspector shows form-POST latency next to renders.
                if let Some(rec) = waterfall.as_ref() {
                    let total = started.elapsed().as_secs_f64() * 1000.0;
                    rec.record_with_trace(
                        &req_owned.request_id,
                        vec![
                            ("middleware".to_string(), middleware_ms),
                            ("action".to_string(), (total - middleware_ms).max(0.0)),
                        ],
                        req_owned.trace.as_ref().map(|t| t.trace_id.clone()),
                    );
                }
                // §11.3: apply any tag/path invalidations the action
                // asked for, then return the response.
                if let Some(rev) = resp.header("x-oxide-revalidate") {
                    let events: Vec<(String, String)> = rev
                        .split(',')
                        .filter_map(|e| {
                            e.split_once(':')
                                .map(|(k, v)| (k.to_string(), v.to_string()))
                        })
                        .collect();
                    if let Some(cache) = l0.as_ref() {
                        // BUG-01: before this fix, `tag` events were
                        // dropped and `path` events were passed to
                        // `invalidate_tag` raw (no `path:` prefix), so
                        // neither ever matched a cached entry.
                        for (kind, value) in &events {
                            match kind.as_str() {
                                "tag" => {
                                    cache.invalidate_tag(value, true).await;
                                    // V2-F05 step 6: the revalidation hook —
                                    // every live connection that depends on
                                    // the tag re-runs its loaders and pushes
                                    // the new snapshot to its tab.
                                    if let Some(live) = live_for_request.as_ref() {
                                        live.notify_tag(value);
                                    }
                                }
                                "path" => {
                                    cache.invalidate_path(value, true).await;
                                    // The `path` spelling fans out under the
                                    // `path:<url>` tag (`serve::live`).
                                    if let Some(live) = live_for_request.as_ref() {
                                        live.notify_path(value);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    // PPR shells carry the same tags; drop them with the
                    // L0 entries so a revalidate action cannot leave a
                    // stale shell serving the old document.
                    if let Some(shells) = ppr_for_request.as_ref() {
                        for (kind, value) in &events {
                            if let Ok(mut store) = shells.lock() {
                                match kind.as_str() {
                                    "tag" => {
                                        store.invalidate_tag(value);
                                    }
                                    "path" => {
                                        store.invalidate_path(value);
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                // §8.5: headers the middleware phase added ride the
                // action response (they were accumulated but never
                // applied — the previous drop made a CSP/security
                // header set in middleware vanish on POSTs).
                let mut resp = resp;
                for (k, v) in &middleware_headers {
                    resp = resp.with_header(k, v.clone());
                }
                // §9.3: 200 → carry the result into the render and fall
                // through. Anything else (a redirect, a validation
                // failure) is the action's own answer and returns.
                if resp.status == 200 {
                    // B6 fix: copy the action's Set-Cookie headers into
                    // middleware_headers before the response is dropped —
                    // the render path applies middleware_headers to every
                    // response it produces, so the cookie rides the
                    // re-rendered response. Non-200 responses return `resp`
                    // directly (they already carry the headers).
                    forward_action_set_cookies(&resp.headers, &mut middleware_headers);
                    action_form = serde_json::from_slice::<serde_json::Value>(&resp.body).ok();
                } else {
                    metrics.observe(&label, started.elapsed(), resp.status);
                    return resp;
                }
            }

            // §11.1 L0: a cache hit serves the stored response with the
            // live request's CSP nonce substituted in, no JS on the path.
            // V2-F12: at a `ServeStale` brownout decision the lookup runs
            // with the `staleCeiling` bound — entries past their SWR
            // window still serve (flagged stale, so the existing
            // background regeneration flow proceeds) and the response
            // carries the spec's `warning: 110` header.
            if l0_eligible
                && let Some(cache) = l0.as_ref()
                && let Some(hit) = match brownout_ceiling {
                    Some(pct) => {
                        cache
                            .get_brownout(&l0_route, &l0_params, &l0_vary, pct)
                            .await
                    }
                    None => cache.get(&l0_route, &l0_params, &l0_vary).await,
                }
            {
                // PERF-04: a conditional request whose validator
                // matches the stored ETag gets a 304 (no body). The
                // ETag is weak-formatted so a cache can still serve
                // it after a nonce-only body change.
                let etag = format!("W/\"{}\"", hit.etag());
                let inm = req_owned.header("if-none-match").map(str::to_string);
                if inm.as_deref().is_some_and(|v| {
                    v.split(',')
                        .map(str::trim)
                        .any(|t| t == etag || t == hit.etag() || t == "*")
                }) {
                    let mut resp = oxide_fw_server::OxideResponse::new(304, "");
                    resp = resp.with_header("etag", etag.clone());
                    resp = resp.with_header("x-oxide-fw-cache", "L0-hit");
                    // P0-5: middleware runs per request and its
                    // headers must reflect THIS request, not the
                    // request that stored the entry. Apply before
                    // returning so an L0 hit carries them too.
                    for (k, v) in &middleware_headers {
                        resp = resp.with_header(k, v.clone());
                    }
                    metrics.observe(&label, started.elapsed(), 304);
                    if let Some(rec) = waterfall.as_ref() {
                        rec.record(
                            &req_owned.request_id,
                            vec![(
                                "cache-hit".to_string(),
                                started.elapsed().as_secs_f64() * 1000.0,
                            )],
                        );
                    }
                    return resp;
                }
                // P-05: stale-while-revalidate. A stale hit is served
                // immediately, and the entry is dropped in the
                // background so the *next* request misses, re-renders
                // and re-stores (single-flight coalesces concurrent
                // misses). Previously the entry served stale forever
                // within the SWR window with nobody refreshing it.
                if hit.stale {
                    let bg = cache.clone();
                    let bg_path = l0_path.clone();
                    if let Some(shells) = ppr_for_request.as_ref() {
                        let bg_shells = std::sync::Arc::clone(shells);
                        let bg_shell_path = bg_path.clone();
                        tokio::spawn(async move {
                            if let Ok(mut store) = bg_shells.lock() {
                                store.invalidate_path(&bg_shell_path);
                            }
                        });
                    }
                    tokio::spawn(async move {
                        let _ = bg.invalidate_path(&bg_path, false).await;
                    });
                }
                let nonce = request_nonce.as_str();
                // PERF-L0 (readiness): serve the COMPRESSED representation
                // straight from the stored segment frames when the client
                // accepts one. The stored frames are reused byte-for-byte;
                // only the nonce is compressed fresh. The response carries
                // Content-Encoding so the pipeline's finish step skips its
                // own spawn_blocking compression (the old path compressed
                // every L0 hit — the React fixture's p99 missed < 1 ms).
                let negotiated = req_owned
                    .header("accept-encoding")
                    .and_then(|ae| oxide_fw_server::negotiate_encoding(ae, &["zstd", "gzip"]));
                let (body, encoding) = match negotiated
                    .as_deref()
                    .and_then(|enc| hit.materialize_compressed(nonce, enc))
                {
                    Some(b) => (b, negotiated),
                    None => (hit.materialize(nonce), None),
                };
                let mut resp = oxide_fw_server::OxideResponse::new(hit.status(), body);
                for (k, v) in &hit.headers() {
                    resp = resp.with_header(k, v.clone());
                }
                if let Some(enc) = &encoding {
                    resp = resp
                        .with_header("content-encoding", enc.clone())
                        .with_header("vary", "Accept-Encoding");
                }
                resp = resp.with_header("etag", etag);
                // §20: an explicit cache-outcome header makes the L0
                // layer observable and testable. A stale serve
                // (within the SWR window) is labelled so the caller
                // can see it; background regeneration is scoped.
                resp = resp.with_header(
                    "x-oxide-fw-cache",
                    if hit.stale { "L0-stale" } else { "L0-hit" },
                );
                // V2-F12: a hit served under the brownout's staleCeiling
                // grant (past the SWR window) carries the spec's stale
                // warning so caches between the server and the client
                // know the response is deliberately old.
                if brownout_ceiling.is_some() {
                    resp = resp.with_header(
                        oxide_fw_server::governor::STALE_WARNING_HEADER,
                        oxide_fw_server::governor::STALE_WARNING_VALUE,
                    );
                }
                // P0-5: middleware headers reflect this request; a
                // cached body must not suppress an auth/security
                // header the middleware just set.
                for (k, v) in &middleware_headers {
                    resp = resp.with_header(k, v.clone());
                }
                metrics.observe(&label, started.elapsed(), hit.status());
                // B-17: hit rows sit next to miss rows in the
                // inspector so cache-hit rate is visible.
                if let Some(rec) = waterfall.as_ref() {
                    rec.record_with_trace(
                        &req_owned.request_id,
                        vec![(
                            if hit.stale {
                                "cache-stale"
                            } else {
                                "cache-hit"
                            }
                            .to_string(),
                            started.elapsed().as_secs_f64() * 1000.0,
                        )],
                        req_owned.trace.as_ref().map(|t| t.trace_id.clone()),
                    );
                }
                return resp;
            }

            // PERF-04: single-flight. A concurrent miss for the same L0
            // key awaits the in-flight owner instead of rendering again.
            // S-15: a guard (not a bare sender) so *every* owner exit path —
            // loader error/redirect, cookie short-circuit, render error —
            // clears the in-flight map; the guard's Drop does it.
            let mut flight_owner: Option<oxide_fw_cache::FlightOwnerGuard> = None;
            if l0_eligible && l0.is_some() {
                use oxide_fw_cache::FlightSlot;
                match oxide_fw_cache::SingleFlight::new()
                    .acquire(&flight_map, &flight_key)
                    .await
                {
                    FlightSlot::Follower(mut rx) => {
                        if let Ok(Ok(entry)) =
                            tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
                                .await
                        {
                            let nonce = request_nonce.as_str();
                            // PERF-L0: follower path decodes plain OR envelope
                            // bodies through the shared materializer.
                            let body = l0_cache::materialize_entry_body(&entry, nonce);
                            let mut resp = oxide_fw_server::OxideResponse::new(entry.status, body);
                            for (k, v) in &entry.headers {
                                resp = resp.with_header(k, v.clone());
                            }
                            resp = resp.with_header("x-oxide-fw-cache", "L0-coalesced");
                            // P0-5: a coalesced follower gets the same
                            // per-request middleware headers as a direct hit.
                            for (k, v) in &middleware_headers {
                                resp = resp.with_header(k, v.clone());
                            }
                            metrics.observe(&label, started.elapsed(), entry.status);
                            return resp;
                        }
                    }
                    FlightSlot::Owner(tx) => {
                        flight_owner = Some(oxide_fw_cache::FlightOwnerGuard::new(
                            std::sync::Arc::clone(&flight_map),
                            flight_key.clone(),
                            tx,
                        ))
                    }
                }
            }

            // §9.1: loader phase. When the matched route declares a server
            // module and a LoaderRunner is attached, run it. A non-200
            // output (error(...) or redirect(...)) short-circuits the
            // response; a 200 output threads `data` into the render phase.
            let mut data = serde_json::Value::Object(Default::default());
            // B2: the page load's per-settle stream, when the runner streams.
            // `Some` only when the loader declared at least one `defer()`;
            // the render arm then holds `</body>` until the settles land.
            let mut page_settles: Option<tokio::sync::mpsc::Receiver<serde_json::Value>> = None;
            // 16-G: the route's `meta = defineMeta(fn)` contribution, if its
            // server module exports one. Merged into the render input's head.
            let mut route_meta: Option<serde_json::Value> = None;
            // §16.6 / §9.2: cookies and headers a load set on the success
            // path. Collected here and applied to whichever response the
            // render phase produces (stream or error).
            let mut loader_set_cookies: Vec<String> = Vec::new();
            let mut loader_headers: Vec<(String, String)> = Vec::new();
            // §7.1 / §9.2: resolved defer() values from the loader.
            let mut deferred_values: std::collections::BTreeMap<String, serde_json::Value> =
                std::collections::BTreeMap::new();
            // PERF-06: per-stage timing marks. `started` is the request
            // entry; these bracket load / render so `Server-Timing` can
            // report real per-stage durations instead of all-zeros.
            let stage_load_start = std::time::Instant::now();

            // §9.2: layout loads run in chain order (outer → inner); each
            // layout gets the pre-resolved data of its own outer layout as
            // `parent_data` (which a load reads via `await parent()`). The
            // innermost layout's data feeds the page's `parent_data`.
            //
            // **Parallelism (v0.1):** when `RouteEntry.needs_parent` is
            // `false` for the matched page (the scanner detected no
            // `parent` reference in `page.server.ts`), the LAST layout load
            // and the page load run concurrently via `tokio::join!` — the
            // page does not read layout data, so the ordering has no
            // observable effect. Earlier layouts still resolve outer-first.
            // Pages that call `parent()` (needs_parent = true) keep the
            // full sequential chain. Layout-then-page precedence on
            // short-circuit matches the sequential order.
            let mut layout_data_chain: Vec<serde_json::Value> = Vec::new();
            let mut layout_side_effects: LayoutSideEffects = Vec::new();
            // §9.2 parallel path (v0.1): when the page does not call
            // `parent()` (`RouteEntry.needs_parent == false`), the last
            // layout load and the page load overlap via `tokio::join!`.
            // Earlier layouts stay sequential (each needs its own parent's
            // data); only the (last-layout, page) pair overlaps.
            let run_parallel = !needs_parent
                && has_server_module
                && loader_runner.is_some()
                && module_path.is_some()
                && route_id.is_some()
                && !layout_module_paths.is_empty();
            let layouts_to_run: &[(String, std::path::PathBuf)] = if run_parallel {
                &layout_module_paths[..layout_module_paths.len() - 1]
            } else {
                &layout_module_paths[..]
            };
            if let (Some(runner), true) = (loader_runner.as_ref(), !layouts_to_run.is_empty()) {
                for (lid, lpath) in layouts_to_run {
                    let mut linput = oxide_fw_server::LoaderRunInput::new(
                        lpath.to_string_lossy().to_string(),
                        lid.clone(),
                        req_owned.url.clone(),
                    );
                    linput.method = req_owned.method.clone();
                    linput.headers = req_owned.headers.as_ref().clone();
                    linput.params = params.clone();
                    linput.locals = middleware_locals.clone();
                    linput.locale = resolved_locale.clone();
                    // §20 / G2: thread the inbound W3C trace so the loader's
                    // server-side fetch calls join the request trace.
                    linput.trace = req_owned.trace.as_ref().and_then(|t| {
                        t.to_traceparent(&t.parent_id).map(
                            |tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}),
                        )
                    });
                    linput.parent_data = layout_data_chain
                        .last()
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    match runner.run(linput).await {
                        Ok(out) if out.short_circuits() => {
                            let body = out
                                .data
                                .get("message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let mut resp = oxide_fw_server::OxideResponse::new(out.status, body);
                            if let Some(loc) = out.location {
                                resp = resp.with_header("location", loc);
                            }
                            for c in &out.set_cookies {
                                resp = resp.with_header("set-cookie", c.clone());
                            }
                            for (k, v) in &out.headers {
                                resp = resp.with_header(k, v.clone());
                            }
                            // §16.6 / WP-D1 / WP-r10-1: a short-circuit that
                            // also mutated `event.session` must persist it.
                            // `session_before` is the inbound (post-middleware)
                            // session snapshot, so an untouched loader does
                            // not re-issue a cookie.
                            if let Some(cookie) = encode_session_cookie(
                                out.session.as_ref(),
                                session_before.as_ref(),
                                session_codec.as_deref(),
                            ) {
                                resp = resp.with_header("set-cookie", cookie);
                            }
                            metrics.observe(&label, started.elapsed(), out.status);
                            return resp;
                        }
                        Ok(out) => {
                            layout_data_chain.push(out.data);
                            // §16.6 / WP-r10-1 / D1: a layout loader that
                            // mutates `event.session` must persist it. The
                            // short-circuit arm (above) already does; this
                            // Ok arm used to drop `out.session` entirely,
                            // so a layout mutation was lost on the common
                            // non-short-circuit path. Encode and append to
                            // the same cookies vec that flattens below.
                            let mut cookies = out.set_cookies;
                            if let Some(cookie) = encode_session_cookie(
                                out.session.as_ref(),
                                session_before.as_ref(),
                                session_codec.as_deref(),
                            ) {
                                cookies.push(cookie);
                            }
                            layout_side_effects.push((cookies, out.headers));
                        }
                        Err(e) => {
                            tracing::error!(code = %e.0.code, error = %e.0.message, "layout loader failed");
                            metrics.observe(&label, started.elapsed(), 500);
                            return oxide_fw_server::OxideResponse::html(
                                500,
                                format!(
                                    "layout loader error: {}",
                                    oxide_fw_render::escape_text(&e.0.message)
                                ),
                            );
                        }
                    }
                }
                // Apply outer-layout side effects to the eventual response
                // (cookies/headers accumulate outward-in).
                for (cookies, headers) in &layout_side_effects {
                    for c in cookies {
                        loader_set_cookies.push(c.clone());
                    }
                    for (k, v) in headers {
                        loader_headers.push((k.clone(), v.clone()));
                    }
                }
            }
            if run_parallel {
                // §9.2 parallel: last layout + page run concurrently.
                // Layout takes precedence (matches sequential order): if
                // either short-circuits we return its response; prior
                // layouts' side effects live in loader_set_cookies/headers
                // and are only applied on the eventual render response.
                let runner = loader_runner.as_ref().unwrap();
                let (lid, lpath) = &layout_module_paths[layout_module_paths.len() - 1];
                let mut linput = oxide_fw_server::LoaderRunInput::new(
                    lpath.to_string_lossy().to_string(),
                    lid.clone(),
                    req_owned.url.clone(),
                );
                linput.method = req_owned.method.clone();
                linput.headers = req_owned.headers.as_ref().clone();
                linput.params = params.clone();
                linput.locals = middleware_locals.clone();
                linput.locale = resolved_locale.clone();
                // §20 / G2: thread the inbound W3C trace so the loader's
                // server-side fetch calls join the request trace.
                linput.trace = req_owned.trace.as_ref().and_then(|t| {
                    t.to_traceparent(&t.parent_id).map(
                        |tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}),
                    )
                });
                linput.parent_data = layout_data_chain
                    .last()
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);

                let mp = module_path.as_ref().unwrap();
                let rid = route_id.as_ref().unwrap();
                let route_for_loader_page = rid.strip_suffix("/page").unwrap_or(rid).to_string();
                let mut pinput = oxide_fw_server::LoaderRunInput::new(
                    mp.to_string_lossy().to_string(),
                    route_for_loader_page,
                    req_owned.url.clone(),
                );
                pinput.method = req_owned.method.clone();
                pinput.headers = req_owned.headers.as_ref().clone();
                pinput.params = params.clone();
                pinput.locals = middleware_locals.clone();
                pinput.locale = resolved_locale.clone();
                // Page does not call `parent()`, so this value is unused.
                pinput.parent_data = serde_json::Value::Null;
                // §20 / G2: thread the inbound W3C trace so the loader's
                // server-side fetch calls join the request trace.
                pinput.trace = req_owned.trace.as_ref().and_then(|t| {
                    t.to_traceparent(&t.parent_id).map(
                        |tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}),
                    )
                });

                // §20 / O3c: time each side of the join. The wall-clock
                // saving vs a sequential layout→page chain is
                // `min(layout_ms, page_ms)`; recording it on the request's
                // waterfall lets a user tell whether the route's schema
                // actually exercised the parallel path or whether `needs_
                // parent` silently forced a sequential chain.
                let (layout_res, layout_ms, page_res, page_ms) = {
                    let layout_start = std::time::Instant::now();
                    let (lr, pr) = tokio::join!(
                        async {
                            let r = runner.run(linput).await;
                            (r, layout_start.elapsed().as_secs_f64() * 1000.0)
                        },
                        async {
                            let start = std::time::Instant::now();
                            // B2 / P0-2: use the streaming entry so a page
                            // that does not call parent() still flushes its
                            // defer() settles per-settle instead of
                            // buffering the whole document in this arm.
                            let r = runner.run_streaming(pinput).await;
                            (r, start.elapsed().as_secs_f64() * 1000.0)
                        }
                    );
                    let (layout_res, layout_ms) = lr;
                    let (page_res, page_ms) = pr;
                    (layout_res, layout_ms, page_res, page_ms)
                };
                if let Some(rec) = waterfall.as_ref() {
                    let saved = parallel_load_savings_ms(layout_ms, page_ms);
                    rec.record_staged(
                        &req_owned.request_id,
                        vec![(LOAD_OVERLAP_STAGE.to_string(), saved)],
                        req_owned.trace.as_ref().map(|t| t.trace_id.clone()),
                    );
                }

                // Layout result first (precedence: matches sequential).
                match layout_res {
                    Ok(out) if out.short_circuits() => {
                        let body = out
                            .data
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let mut resp = oxide_fw_server::OxideResponse::new(out.status, body);
                        if let Some(loc) = out.location {
                            resp = resp.with_header("location", loc);
                        }
                        for c in &out.set_cookies {
                            resp = resp.with_header("set-cookie", c.clone());
                        }
                        for (k, v) in &out.headers {
                            resp = resp.with_header(k, v.clone());
                        }
                        // §16.6 / WP-D1 / WP-r10-1: a layout short-circuit
                        // (parallel arm) must persist any session mutation.
                        if let Some(cookie) = encode_session_cookie(
                            out.session.as_ref(),
                            session_before.as_ref(),
                            session_codec.as_deref(),
                        ) {
                            resp = resp.with_header("set-cookie", cookie);
                        }
                        metrics.observe(&label, started.elapsed(), out.status);
                        return resp;
                    }
                    Ok(out) => {
                        layout_data_chain.push(out.data);
                        for c in &out.set_cookies {
                            loader_set_cookies.push(c.clone());
                        }
                        for (k, v) in &out.headers {
                            loader_headers.push((k.clone(), v.clone()));
                        }
                        // §16.6 / WP-r10-1 / D1: the parallel layout Ok arm
                        // also dropped `out.session`. Same fix as the
                        // sequential arm — encode and push through the
                        // cookies stream that lands on the response.
                        if let Some(cookie) = encode_session_cookie(
                            out.session.as_ref(),
                            session_before.as_ref(),
                            session_codec.as_deref(),
                        ) {
                            loader_set_cookies.push(cookie);
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            code = %e.0.code,
                            error = %e.0.message,
                            "layout loader failed",
                        );
                        metrics.observe(&label, started.elapsed(), 500);
                        return oxide_fw_server::OxideResponse::html(
                            500,
                            format!(
                                "layout loader error: {}",
                                oxide_fw_render::escape_text(&e.0.message)
                            ),
                        );
                    }
                }

                match page_res {
                    Ok((out, _settles)) if out.short_circuits() => {
                        let body = out
                            .data
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let mut resp = oxide_fw_server::OxideResponse::new(out.status, body);
                        if let Some(loc) = out.location {
                            resp = resp.with_header("location", loc);
                        }
                        for c in &out.set_cookies {
                            resp = resp.with_header("set-cookie", c.clone());
                        }
                        for (k, v) in &out.headers {
                            resp = resp.with_header(k, v.clone());
                        }
                        // §16.6 / WP-D1 / WP-r10-1: a page short-circuit
                        // (parallel arm) must persist any session mutation.
                        if let Some(cookie) = encode_session_cookie(
                            out.session.as_ref(),
                            session_before.as_ref(),
                            session_codec.as_deref(),
                        ) {
                            resp = resp.with_header("set-cookie", cookie);
                        }
                        metrics.observe(&label, started.elapsed(), out.status);
                        return resp;
                    }
                    Ok((out, settles)) => {
                        data = out.data;
                        page_settles = Some(settles);
                        // 16-G: the route's `meta = defineMeta(fn)` export,
                        // evaluated by the worker against this load's data.
                        route_meta = out.meta;
                        // NOTE (preexisting, preserved): overwrite, not
                        // extend — matches the sequential-path behavior.
                        loader_set_cookies = out.set_cookies;
                        loader_headers = out.headers;
                        deferred_values = out.deferred;
                        // §16.6 / WP-D1: the parallel arm (page + last
                        // layout run under tokio::join!) had no session
                        // encode. Any page that does not call parent() —
                        // the default — dropped its session mutation. This
                        // is the common case, so this is the highest-impact
                        // of the four D1 gaps.
                        if let Some(cookie) = encode_session_cookie(
                            out.session.as_ref(),
                            session_before.as_ref(),
                            session_codec.as_deref(),
                        ) {
                            loader_set_cookies.push(cookie);
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            code = %e.0.code,
                            error = %e.0.message,
                            route = %label,
                            "loader failed",
                        );
                        metrics.observe(&label, started.elapsed(), 500);
                        return oxide_fw_server::OxideResponse::html(
                            500,
                            format!(
                                "loader error: {}",
                                oxide_fw_render::escape_text(&e.0.message)
                            ),
                        );
                    }
                }
            } else if has_server_module
                && let (Some(runner), Some(module_path), Some(route_id)) =
                    (loader_runner.as_ref(), module_path, route_id)
            {
                // Pass the normalized id (no `/page` suffix) so user
                // `load(event)` implementations see the route
                // directory they expect.
                let route_for_loader = route_id
                    .strip_suffix("/page")
                    .unwrap_or(&route_id)
                    .to_string();
                let mut input = oxide_fw_server::LoaderRunInput::new(
                    module_path.to_string_lossy().to_string(),
                    route_for_loader,
                    req_owned.url.clone(),
                );
                input.method = req_owned.method.clone();
                input.headers = req_owned.headers.as_ref().clone();
                input.params = params.clone();
                // §8.1 middleware.ts / §9.1: request-scoped locals
                // injected by the middleware that just ran. `{}` when
                // no middleware executed.
                input.locals = middleware_locals.clone();
                // SPEC-10: expose the resolved locale to `load()`.
                input.locale = resolved_locale.clone();
                // §20 / G2: thread the inbound W3C trace so the loader's
                // server-side fetch calls join the request trace.
                input.trace = req_owned.trace.as_ref().and_then(|t| {
                    t.to_traceparent(&t.parent_id).map(
                        |tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}),
                    )
                });
                // §9.2: the innermost layout's data — what `parent()`
                // reads in a page load.
                input.parent_data = layout_data_chain
                    .last()
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);

                match runner.run_streaming(input.clone()).await {
                    Ok((out, settles)) => {
                        if out.short_circuits() {
                            // fall through to the short-circuit arm below
                            // by re-dispatching on the same shape.
                            let body = out
                                .data
                                .get("message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let mut resp = oxide_fw_server::OxideResponse::new(out.status, body);
                            if let Some(loc) = out.location {
                                resp = resp.with_header("location", loc);
                            }
                            for c in &out.set_cookies {
                                resp = resp.with_header("set-cookie", c.clone());
                            }
                            for (k, v) in &out.headers {
                                resp = resp.with_header(k, v.clone());
                            }
                            // §16.6 / WP-D1 / WP-r10-1: a page
                            // short-circuit (sequential arm, streaming
                            // path) must persist any session mutation.
                            if let Some(cookie) = encode_session_cookie(
                                out.session.as_ref(),
                                session_before.as_ref(),
                                session_codec.as_deref(),
                            ) {
                                resp = resp.with_header("set-cookie", cookie);
                            }
                            metrics.observe(&label, started.elapsed(), out.status);
                            return resp;
                        }
                        data = out.data;
                        loader_set_cookies = out.set_cookies;
                        loader_headers = out.headers;
                        deferred_values = out.deferred;
                        page_settles = Some(settles);
                        // §16.6 / WP-D1: a loader that mutated
                        // `event.session` has its change persisted. The
                        // helper compares against the inbound session so
                        // an untouched one does not re-issue a cookie.
                        if let Some(cookie) = encode_session_cookie(
                            out.session.as_ref(),
                            session_before.as_ref(),
                            session_codec.as_deref(),
                        ) {
                            loader_set_cookies.push(cookie);
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            code = %e.0.code,
                            error = %e.0.message,
                            route = %label,
                            "loader failed",
                        );
                        metrics.observe(&label, started.elapsed(), 500);
                        return oxide_fw_server::OxideResponse::html(
                            500,
                            format!(
                                "loader error: {}",
                                oxide_fw_render::escape_text(&e.0.message)
                            ),
                        );
                    }
                }
            }

            // §8.1 route.ts / §10.2 step 7: an Endpoint route returns a
            // Web `Response` directly, bypassing the loader + render
            // phases. The dispatcher is a worker op (`endpoint:run`),
            // async but bounded; the response bytes are the raw body
            // from the `Response` (a `route.ts` can serve a download).
            let endpoint_for_dispatch_inner = endpoint_for_dispatch.clone();
            if let (Some(dispatcher), Some(mp)) = (
                endpoint_for_dispatch_inner.as_deref(),
                endpoint_module_path.as_ref(),
            ) {
                let module_path_s = mp.to_string_lossy().to_string();
                let endpoint_trace = req_owned.trace.as_ref().and_then(|t| {
                    t.to_traceparent(&t.parent_id).map(
                        |tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}),
                    )
                });
                match dispatcher
                    .dispatch(EndpointDispatchArgs {
                        module_path: &module_path_s,
                        method: &req_owned.method,
                        url: &req_owned.url,
                        params: endpoint_params_json
                            .get_or_insert_with(|| build_params_json(&params)),
                        headers: &req_owned.headers,
                        body: endpoint_body.as_ref(),
                        locals: &middleware_locals,
                        trace: endpoint_trace.as_ref(),
                    })
                    .await
                {
                    Ok(out) => {
                        metrics.observe(&label, started.elapsed(), out.status);
                        // §9.6: a streaming route.ts (SSE, downloads)
                        // drives the response body from `out.stream`;
                        // a buffered one uses `out.body`.
                        let mut resp = match out.stream {
                            Some(stream) => {
                                oxide_fw_server::OxideResponse::stream(out.status, stream)
                            }
                            None => oxide_fw_server::OxideResponse::new(
                                out.status,
                                bytes::Bytes::from(out.body),
                            ),
                        };
                        for (k, v) in &out.headers {
                            resp = resp.with_header(k, v.clone());
                        }
                        // §8.5: headers the middleware phase added ride the
                        // endpoint response (previously accumulated and
                        // dropped).
                        for (k, v) in &middleware_headers {
                            resp = resp.with_header(k, v.clone());
                        }
                        // V2-F07 wiring step 1: a streamed endpoint response
                        // the handler opted in through the `oxide/server`
                        // `resumable` helper (`x-oxide-resumable: 1`) is
                        // teed into the stream store — every byte the
                        // consumer sees is buffered under a minted
                        // `stream_id(request_id, counter)`, the head gains
                        // `x-oxide-stream: <id>` (the client reattaches at
                        // `GET /__oxide/stream/<id>` with
                        // `Range: bytes=<n>-`), and the tee marks the
                        // buffer done at the natural end or a producer
                        // error. A buffered response, a response without
                        // the marker, and a `None` store (the test helper)
                        // all keep the pre-F07 byte shapes.
                        if let Some(sw) = streams_for_dispatch.as_ref()
                            && resp.stream.is_some()
                            && out.headers.iter().any(|(k, v)| {
                                k.eq_ignore_ascii_case(
                                    oxide_fw_server::stream_store::RESUMABLE_HEADER,
                                ) && v == oxide_fw_server::stream_store::RESUMABLE_VALUE
                            })
                            && let Some(inner) = resp.stream.take()
                        {
                            let counter =
                                stream_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let id = oxide_fw_server::stream_store::stream_id(
                                &req_owned.request_id,
                                counter,
                            );
                            let buf = sw.store.begin(id.clone());
                            resp =
                                resp.with_header(oxide_fw_server::stream_store::STREAM_HEADER, id);
                            resp.stream = Some(oxide_fw_server::stream_store::tee_stream(
                                &sw.store, &buf, inner,
                            ));
                        }
                        // V2-F15: a streamed endpoint response records
                        // through the tee (the observer sees the headers;
                        // the tee supplies the body when the stream ends).
                        // Composition order: the F07 stream tee sits
                        // INNERMOST (closest to the producer) so the store
                        // and the recorder observe the same bytes in the
                        // same order.
                        if let Some(rec) = req_recording.as_ref()
                            && let Some(stream) = resp.stream.take()
                        {
                            resp.stream = Some(rec.tee(stream, resp.status));
                        }
                        return resp;
                    }
                    Err(e) => {
                        metrics.observe(&label, started.elapsed(), e.status);
                        return oxide_fw_server::OxideResponse::json(
                            e.status,
                            serde_json::json!({"error": {
                                "code": e.code,
                                "message": e.message,
                                "route": label,
                            }}),
                        );
                    }
                }
            }

            // §7: render phase. Attached renderer wins; otherwise IR placeholder.
            // PERF-06: the loader phase is done; record its duration.
            let load_ms: f64 = stage_load_start.elapsed().as_secs_f64() * 1000.0;
            let stage_render_start = std::time::Instant::now();

            if let Some(renderer) = page_renderer.as_ref() {
                // §10.2 step 5: an unmatched URL is a 404 with the
                // project's `not-found.tsx` as the body, when one exists.
                // Matched routes resolve to their own id + layout chain;
                // only the no-match branch substitutes the not-found id.
                let (route_for_render, effective_layouts) = match &route_id_for_render {
                    Some(id) => (id.clone(), layout_ids.clone()),
                    None => match &unmatched_route_id {
                        Some(nf) => (nf.clone(), unmatched_layout_ids.clone()),
                        None => ("_unmatched".to_string(), Vec::new()),
                    },
                };
                // PERF-01 / §8.4 / P-11: the ODF hydration payload needs the
                // same route/url/params/data the render input carries. Encode
                // it directly from borrowed fields *before* they move into
                // `PageRenderInput`, so `data` is traversed once instead of
                // being deep-cloned and then traversed again.
                let route_for_odf = route_for_render.clone();
                let odf_url = req_owned.url.clone();
                let odf_bytes = {
                    use oxide_fw_odf::Value as OdfValue;
                    let mut obj = std::collections::BTreeMap::new();
                    obj.insert("route".to_string(), OdfValue::String(route_for_odf.clone()));
                    obj.insert("url".to_string(), OdfValue::String(odf_url.clone()));
                    obj.insert(
                        "params".to_string(),
                        OdfValue::Object(
                            params
                                .iter()
                                .map(|(k, v)| (k.clone(), OdfValue::String(v.clone())))
                                .collect(),
                        ),
                    );
                    obj.insert("data".to_string(), OdfValue::from_json(&data));
                    obj.insert(
                        "layouts".to_string(),
                        OdfValue::Array(
                            effective_layouts
                                .iter()
                                .map(|l| OdfValue::String(l.clone()))
                                .collect(),
                        ),
                    );
                    oxide_fw_odf::encode_to_bytes(&OdfValue::Object(obj)).unwrap_or_default()
                };
                let mut input =
                    oxide_fw_server::PageRenderInput::new(route_for_render, req_owned.url.clone());
                input.params = params;
                // §9.3: a form POST re-renders the page with the action's
                // result in `data.form`. Injected here rather than seeded
                // before the loader because the loader assigns `data`
                // wholesale; this runs after it, so both survive.
                if let Some(form) = action_form.take()
                    && let Some(obj) = data.as_object_mut()
                {
                    obj.insert("form".to_string(), form);
                }
                input.data = data;
                // §8.2: hand the layout chain to the adapter so it can
                // nest the page. The adapter renders
                // `L1( L2( ...( page ) ) )` using `componentFor(layoutId)`.
                input.layouts = effective_layouts.clone();
                // §10.2 step 5: nearest ancestor `error.tsx` — the adapter
                // renders it on an SSR throw instead of the generic 500.
                input.error_route_id = error_route_id.clone();
                // §16.4 / O2: thread the resolved request locale into the
                // live render, exactly as prerender does. Without it the
                // adapter cannot emit `<html lang>` and the client catalog
                // loader falls back to `en`.
                input.locale = Some(resolved_locale.clone());
                // §7.1 / §9.2: hand the loader's resolved `defer()` values to
                // the adapter so it can replace `{__deferred: id}` in `data`
                // before the component tree is built. Without this a
                // component reads the placeholder object, never the value.
                input.deferred = deferred_values.clone();
                // §17.1 / §7.1: the request's CSP nonce. The adapter stamps it
                // on any script it emits — including the deferred-boundary
                // swapper, which is an inline IIFE and would be blocked under
                // `strict-dynamic` without it. Same value the pipeline uses
                // for its own injected tags.
                input.nonce = Some(request_nonce.clone());
                // 16-G: the route's own `meta` joins the neutral head model.
                // The live path has no plugin `renderHead` contribution to
                // merge with here (that is a build-time hook); the route's
                // meta is the whole contribution.
                if let Some(meta) = route_meta.as_ref() {
                    input.head = merge_neutral_head(input.head, normalize_route_meta(meta.clone()));
                }
                match renderer.render_streaming(input).await {
                    Ok(render_stream) => {
                        // §10.2 step 5 (error-status contract): peek the
                        // FIRST chunk and scan it for an error-status
                        // marker BEFORE the response status is fixed. The
                        // adapter renders the error route with a 200 on
                        // the wire today (no status channel in the worker
                        // protocol); the contract is that an error
                        // document announces itself in its first chunk —
                        // `<!--oxide-status:500-->` (canonical) or the
                        // generic OX-500 boundary attribute. HTTP cannot
                        // change the status after the first byte is sent,
                        // so a marker in a LATER chunk is unobservable
                        // here (documented; the adapter must emit the
                        // marker before the shell).
                        let (error_status, render_stream) =
                            probe_render_error_status(render_stream).await;
                        let final_status = if error_status == 500 { 500 } else { status };
                        metrics.observe(&label, started.elapsed(), final_status);
                        // Bridge the renderer's Send-only stream to the
                        // transport's Send + Sync body stream. If the
                        // project registered islands, splice the boot
                        // script tag right before `</body>` — the sliding
                        // window in `oxide_fw_server::inject_before_close` buffers
                        // at most `len("</body>") - 1` bytes so a tag
                        // split across chunks still lands in the right
                        // place and every other chunk flushes untouched.
                        // SPEC-11: pass boundary + nonce so a mid-stream
                        // error emits the recovery boundary instead of
                        // truncating the response.
                        let bridged = render_stream_with_recovery(
                            render_stream,
                            Some(label.clone()),
                            Some(request_nonce.clone()),
                            label.clone(),
                        );

                        // B2: when the load declared at least one
                        // `defer()`, hold the document's closing tags and
                        // emit a resolution script per settle as it arrives,
                        // instead of one batched tag after the slowest.
                        let nonce_for_settles = request_nonce.clone();
                        let bridged = match page_settles.take() {
                            Some(rx) => {
                                // §20 / O3b: pass the recorder + request id
                                // so the settle drain records a
                                // `deferred-settles` stage once the last
                                // one lands.
                                strip_close_and_append_settles(
                                    bridged,
                                    rx,
                                    nonce_for_settles,
                                    waterfall.clone(),
                                    req_owned.request_id.clone(),
                                    req_owned.trace.as_ref().map(|t| t.trace_id.clone()),
                                )
                            }
                            None => bridged,
                        };
                        // §17.1: a script without the matching nonce is
                        // blocked by the response's `strict-dynamic` CSP.
                        // The pipeline derives a per-request nonce in
                        // `finish_response`; we compute the same value here
                        // from the request id. Both the deferred script and
                        // (when present) the boot module share it.
                        let nonce = oxide_fw_server::nonce_for(&req_owned.request_id);
                        // PERF-01 / §8.4: the SAME `window.__oxide` ODF
                        // payload prerendered pages embed, injected into
                        // the live streamed shell (the sliding-window
                        // injector above). Previously only write_prerender
                        // set it, so a dynamic page's boot module found no
                        // payload and skipped hydration + the router
                        // entirely (`if (root && payload)` in boot.ts).
                        let odf_tag = {
                            use base64::Engine as _;
                            let odf_b64 =
                                base64::engine::general_purpose::STANDARD.encode(&odf_bytes);
                            // V2-A04: the bootstrap advertises the CURRENT
                            // build id — the same value the stage checks and
                            // every data-plane response advertises — so the
                            // client's `withBuild` echo is accepted. The id
                            // was read once per request at the closure top
                            // (`boot_build_id`, the guard never enters the
                            // async blocks); the clone keeps nested moves
                            // legal. An absent handle (the test helper)
                            // keeps the legacy three-field shape.
                            match boot_build_id.clone() {
                                Some(id) if !id.is_empty() => {
                                    oxide_fw_render::hydration_bootstrap_with_build(
                                        &odf_b64,
                                        &route_for_odf,
                                        &nonce,
                                        &id,
                                    )
                                }
                                _ => oxide_fw_render::hydration_bootstrap(
                                    &odf_b64,
                                    &route_for_odf,
                                    &nonce,
                                ),
                            }
                        };
                        // §7.1 / §9.2: resolved defer() values ride a JSON
                        // script element so the boot module can dispatch
                        // `ox:swap` for each. Serialized through serde_json
                        // (no untrusted string concatenation) and `<` escaped
                        // so the content cannot break out of the tag.
                        let deferred_tag = if deferred_values.is_empty() {
                            String::new()
                        } else {
                            let json = serde_json::to_string(&deferred_values)
                                .unwrap_or_else(|_| "{}".into());
                            // `</script>` and `<!--` inside a JSON string
                            // would terminate the tag; `<` escaping is the
                            // OWASP-recommended fix and produces valid JS
                            // (`\u003c`).
                            let safe = json.replace('<', "\\u003c");
                            format!(
                                r#"<script type="application/oxide-deferred" nonce="{nonce}">{safe}</script>"#,
                            )
                        };
                        // §13.3: a self-contained dev HMR client. It
                        // connects to `/_oxide/hmr`, swaps the matching
                        // <link> on a CSS update, and reloads the page on
                        // a module/server update. Inlined (not a bundled
                        // module) so it works in dev without a build step.
                        let hmr_tag = String::new(); // HMR is handled by bootWithRouter via connectHmr();
                        let body = match boot_script_url_for_render.as_deref() {
                            Some(url) => {
                                // The tag is built by the same helper the
                                // prerender path uses — one definition site
                                // for the attribute order. V2-E04: when the
                                // manifest carries the boot chunk's digest
                                // and `security.sri` is on, the tag gains
                                // `integrity` + `crossorigin="anonymous"`;
                                // the dev `/@boot/` URL has no digest and
                                // stays byte-identical.
                                let tag = format!(
                                    "{odf_tag}{deferred_tag}{hmr_tag}{}",
                                    boot_script_tag(
                                        url,
                                        &nonce,
                                        boot_script_integrity_for_render.as_deref(),
                                    ),
                                );
                                oxide_fw_server::inject_before_close(
                                    bridged,
                                    b"</body>",
                                    tag.as_bytes(),
                                )
                            }
                            None => {
                                let extra = format!("{odf_tag}{deferred_tag}{hmr_tag}");
                                if extra.is_empty() {
                                    bridged
                                } else {
                                    oxide_fw_server::inject_before_close(
                                        bridged,
                                        b"</body>",
                                        extra.as_bytes(),
                                    )
                                }
                            }
                        };
                        // §10.8 / B-27: serve a previously captured static
                        // shell at once and stream only the dynamic boundary
                        // fragments into it; the first request taps the full
                        // render and stores its shell on completion.
                        // §10.8 / P0-4 / r8-P0.3: a PPR shell is a *shared*
                        // asset (the static frame every visitor reuses), so
                        // it must never be captured from a response that
                        // varies per user. The concrete case the audit found:
                        // a login page (or any loader that sets a session
                        // cookie) freezes its user-specific body into a
                        // shell served to every subsequent visitor. The L0
                        // store 30 lines below correctly refuses
                        // cookie-setting responses; this is the same guard.
                        // An error-status render is likewise never a shell.
                        // §10.8 / WP-r9-4: the tap guard must also refuse
                        // a middleware Set-Cookie. `middleware_headers` is
                        // applied at :8029 (after this decision) — without
                        // this check a middleware-mutating route freezes its
                        // shell exactly as a loader-mutating route did
                        // before the r8-P0.3 guard. Both PPR and the L0
                        // store shared this blind spot.
                        let middleware_sets_cookie = middleware_headers
                            .iter()
                            .any(|(k, _)| k.eq_ignore_ascii_case("set-cookie"));
                        let body = if error_status == 0
                            && loader_set_cookies.is_empty()
                            && !middleware_sets_cookie
                        {
                            match ppr_for_request.as_ref() {
                                Some(shells) => {
                                    // §10.8 / WP-r9: the shell key must
                                    // match the L0 key's identity, not the
                                    // raw URL. Two bugs the raw-URL form
                                    // had:
                                    //   1. No build id — a rebuild served
                                    //      stale shells until the next
                                    //      clear-all.
                                    //   2. Query-string explosion —
                                    //      ?utm=twitter and ?utm=facebook
                                    //      were distinct shells on a site
                                    //      with search, so 256 distinct
                                    //      queries cleared the map.
                                    // The shell key is the canonical L0 key
                                    // itself, not a hand-rolled join: same
                                    // build id, route id, sorted params and
                                    // normalized Vary, hashed by
                                    // `CacheKey::canonical_parts`. Dev has no
                                    // L0 object, so use the stable `"dev"`
                                    // namespace; the watcher clears shells on
                                    // every edit.
                                    let ppr_build_id =
                                        l0.as_ref().map(|c| c.build_id()).unwrap_or("dev");
                                    let pkey = l0_cache::L0Cache::key_for(
                                        &l0_route,
                                        &l0_params,
                                        &l0_vary,
                                        ppr_build_id,
                                    );
                                    let ppr_tags =
                                        vec![l0_route.clone(), format!("path:{l0_path}")];
                                    let cached =
                                        shells.lock().ok().and_then(|s| s.get(&pkey).cloned());
                                    match cached {
                                        Some(shell) => ppr_filter_stream(
                                            shell.replace(l0_cache::NONCE_SENTINEL, &nonce),
                                            body,
                                            nonce.clone(),
                                        ),
                                        None => ppr_tap_stream(
                                            body,
                                            std::sync::Arc::clone(shells),
                                            pkey,
                                            nonce.clone(),
                                            ppr_tags,
                                        ),
                                    }
                                }
                                None => body,
                            }
                        } else {
                            // No PPR on a per-user / error response: the
                            // body is served live to this caller, never
                            // tapped for a shell, never filtered from one.
                            body
                        };
                        // §11 L0: when this response is cacheable, collect
                        // the streamed body once so it can be stored, then
                        // serve it from memory. The body was rendered with
                        // the live request's nonce; replace it with the
                        // sentinel before storing so any request can reuse
                        // it. A response that set a cookie is NOT cached
                        // (it varies per user).
                        // PERF-04: streaming-preserving fill. Tee the
                        // rendered body: each chunk flushes to the client
                        // immediately AND accumulates. On stream end the
                        // sender is dropped FIRST (so the client sees EOF
                        // at once, not after the store), then the body is
                        // stored (live nonce → sentinel) and published for
                        // single-flight followers. The next request is an
                        // L0 hit.
                        if l0_eligible
                                && loader_set_cookies.is_empty()
                                // §10.2 step 5: an error-status render is never
                                // cached — a 500 is per-request state, not a
                                // shareable response.
                                && error_status == 0
                                // §10.8 / WP-r10-3: a middleware Set-Cookie
                                // response also varies per user — the L0
                                // store shared PPR's old blind spot. Refuse
                                // both. `middleware_sets_cookie` is defined
                                // above for the PPR guard.
                                && !middleware_sets_cookie
                            && let Some(cache) = l0.as_ref()
                        {
                            let nonce = oxide_fw_server::nonce_for(&req_owned.request_id);
                            let cache = std::sync::Arc::clone(cache);
                            let l0_route = l0_route.clone();
                            let l0_path = l0_path.clone();
                            let headers_for_store = loader_headers.clone();
                            // P-04: reuse the key already computed for the
                            // flight instead of re-deriving it at store time.
                            let store_key = flight_key.clone();
                            let flight_owner = flight_owner.take();
                            let render_ms = stage_render_start.elapsed().as_secs_f64() * 1000.0;
                            let (tx, mut rx) = tokio::sync::mpsc::channel::<
                                std::result::Result<bytes::Bytes, std::io::Error>,
                            >(8);
                            tokio::spawn(async move {
                                use futures::StreamExt as _;
                                let mut src = body;
                                let mut collected: Vec<u8> = Vec::new();
                                while let Some(item) = src.next().await {
                                    match item {
                                        Ok(chunk) => {
                                            collected.extend_from_slice(&chunk);
                                            if tx.send(Ok(chunk)).await.is_err() {
                                                // Client gone: no store
                                                // (the render did not
                                                // finish).
                                                return;
                                            }
                                        }
                                        Err(e) => {
                                            let _ = tx.send(Err(e)).await;
                                            return;
                                        }
                                    }
                                }
                                // Close the client stream NOW — the
                                // store below must not delay EOF.
                                drop(tx);
                                let sentineled = String::from_utf8_lossy(&collected)
                                    .replace(&nonce, l0_cache::NONCE_SENTINEL)
                                    .into_bytes();
                                let mut store_headers = vec![(
                                    "content-type".to_string(),
                                    "text/html; charset=utf-8".to_string(),
                                )];
                                for (k, v) in &headers_for_store {
                                    store_headers.push((k.clone(), v.clone()));
                                }
                                let entry = cache.make_entry(
                                    200,
                                    store_headers,
                                    sentineled,
                                    vec![l0_route.clone(), format!("path:{}", l0_path)],
                                );
                                cache.store_entry(&store_key, entry.clone()).await;
                                if let Some(guard) = flight_owner {
                                    guard.publish(std::sync::Arc::new(entry));
                                }
                            });
                            let stream: oxide_fw_server::BodyStream =
                                Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)));
                            let t = oxide_fw_render::RequestTimings {
                                middleware_ms,
                                cache_ms: 0.0,
                                load_ms,
                                render_ms,
                                inject_ms: 0.0,
                            };
                            if let Some(rec) = waterfall.as_ref() {
                                // §20 / O3: attach the inbound W3C
                                // trace id so this row correlates with
                                // the request's spans downstream.
                                rec.record_with_trace(
                                    &req_owned.request_id,
                                    vec![
                                        ("middleware".to_string(), middleware_ms),
                                        ("load".to_string(), load_ms),
                                        ("render".to_string(), render_ms),
                                    ],
                                    req_owned.trace.as_ref().map(|t| t.trace_id.clone()),
                                );
                                // SPEC-19.3 / §19-D: also persist the
                                // snapshot to disk so the MCP server (a
                                // separate process) can read the live
                                // waterfalls via `oxide_state`. The
                                // dispatch closure doesn't have `rootp`
                                // in scope (it's captured at server
                                // construction), so we capture the path
                                // via the existing `routes_root` prefix
                                // resolution — but that's per-dispatch.
                                // Simpler: use the CWD, which is the
                                // project root for the dev server.
                                let waterfall_path =
                                    std::path::PathBuf::from(".oxide/last-waterfalls.json");
                                rec.persist_to(&waterfall_path);
                            }
                            let mut resp =
                                oxide_fw_server::OxideResponse::stream(final_status, stream)
                                    .with_header("content-type", "text/html; charset=utf-8")
                                    .with_header("x-oxide-fw-cache", "L0-miss")
                                    .with_header("Server-Timing", t.server_timing());
                            for (k, v) in &loader_headers {
                                resp = resp.with_header(k, v.clone());
                            }
                            // §8.5: middleware headers ride the teed
                            // response too.
                            for (k, v) in &middleware_headers {
                                resp = resp.with_header(k, v.clone());
                            }
                            // V2-F15: a streamed render records through the
                            // tee (the observer signals the stamped meta;
                            // the tee supplies the body at stream end).
                            if let Some(rec) = req_recording.as_ref()
                                && let Some(stream) = resp.stream.take()
                            {
                                resp.stream = Some(rec.tee(stream, resp.status));
                            }
                            return resp;
                        }
                        // §10.2 step 5 / §8.3: the framework render path
                        // always used to emit a hardcoded 200 regardless of
                        // the computed `status`. That made an unmatched URL
                        // return `HTTP 200` with a page body, breaking both
                        // the status code and any client HTTP check
                        // (curl, monitoring, browser dev tools). Use the
                        // computed status: 200 for a matched route, 404 for
                        // `_unmatched` — overridden to 500 when the error
                        // document announced itself in the first chunk
                        // (§10.2 error-status contract).
                        let mut resp = oxide_fw_server::OxideResponse::stream(final_status, body)
                            .with_header("content-type", "text/html; charset=utf-8")
                            .with_header("x-oxide-fw-cache", "bypass");
                        // PERF-06: real per-stage Server-Timing.
                        let render_ms = stage_render_start.elapsed().as_secs_f64() * 1000.0;
                        let t = oxide_fw_render::RequestTimings {
                            middleware_ms,
                            cache_ms: 0.0,
                            load_ms,
                            render_ms,
                            inject_ms: 0.0,
                        };
                        resp = resp.with_header("Server-Timing", t.server_timing());
                        // §16.6 / WP-D1: middleware side effects apply
                        // BEFORE the loader's. The loader runs after
                        // middleware and its session mutation is the later
                        // intent; on the wire, HTTP applies same-name
                        // Set-Cookies in arrival order, so loader-after-
                        // middleware means the loader's value survives.
                        for (k, v) in &middleware_headers {
                            resp = resp.with_header(k, v.clone());
                        }
                        for c in &loader_set_cookies {
                            resp = resp.with_header("set-cookie", c.clone());
                        }
                        for (k, v) in &loader_headers {
                            resp = resp.with_header(k, v.clone());
                        }
                        // V2-F15: a streamed render records through the tee
                        // (the observer signals the stamped meta; the tee
                        // supplies the body at stream end).
                        if let Some(rec) = req_recording.as_ref()
                            && let Some(stream) = resp.stream.take()
                        {
                            resp.stream = Some(rec.tee(stream, resp.status));
                        }
                        resp
                    }
                    Err(e) => {
                        tracing::error!(
                            code = %e.0.code,
                            error = %e.0.message,
                            route = %label,
                            "framework render failed",
                        );
                        metrics.observe(&label, started.elapsed(), 500);
                        oxide_fw_server::OxideResponse::html(
                            500,
                            format!(
                                "render error: {}",
                                oxide_fw_render::escape_text(&e.0.message)
                            ),
                        )
                    }
                }
            } else {
                metrics.observe(&label, started.elapsed(), status);
                let mut resp = oxide_fw_server::OxideResponse::html(
                    status,
                    format!(
                        "<!doctype html><html><head><title>Oxide</title></head>\
                             <body><main>Oxide serving {label}</main></body></html>"
                    ),
                );
                // D1: the loader's side effects belong on the response
                // whichever render path produced the body.
                // §16.6 / WP-D1 precedence: middleware applies first, then
                // the loader — see the render path above for the reasoning.
                for (k, v) in &middleware_headers {
                    resp = resp.with_header(k, v.clone());
                }
                for c in &loader_set_cookies {
                    resp = resp.with_header("set-cookie", c.clone());
                }
                for (k, v) in &loader_headers {
                    resp = resp.with_header(k, v.clone());
                }
                resp
            }
        })
    });
    // V2-A04 (the wiring): the data-plane stage install. One wrapper around
    // `inner_dispatch` instead of touching its many return points:
    //
    // - a data-plane request advertising an unaccepted `x-oxide-build`
    //   answers `409` + `x-oxide-build-stale: true` BEFORE any route
    //   matching or L0 lookup runs (the stage's contract — the response
    //   carries no build advertisement: it is a rejection, not a serving);
    // - every CONTINUED data-plane response carries
    //   `x-oxide-build: <current>` (the advertise step — idempotent, so a
    //   stamp the inner path already laid wins);
    // - non-data-plane requests and a `None` generations handle (the test
    //   helper's default) pass through byte-identical to the pre-A04 shapes.
    //
    // The read guard lives only in this sync section — no guard crosses an
    // await (the dispatch future must stay `Send`). A poisoned lock takes
    // the fail-open arm (the stage's headerless behavior), never the data
    // plane down. Middleware `{ rewrite }` re-enters through
    // `self_dispatch`, i.e. through THIS wrapper — the stage is idempotent
    // per request (the hop-carried headers are the same header set).
    let dispatch_fn: std::sync::Arc<
        dyn Fn(oxide_fw_server::OxideRequest) -> BoxedResponse + Send + Sync,
    > = std::sync::Arc::new(move |req: oxide_fw_server::OxideRequest| {
        let Some(gens) = generations.as_ref() else {
            return inner_dispatch(req);
        };
        let raw_path = req.url.split(['?', '#']).next().unwrap_or("/").to_string();
        let headers = std::sync::Arc::clone(&req.headers);
        let rejected: Option<oxide_fw_server::OxideResponse> = match gens.read() {
            Ok(g) => {
                match oxide_fw_dispatch::skew::stage_check(
                    &raw_path,
                    headers.as_slice(),
                    &*g,
                    std::time::Instant::now(),
                ) {
                    oxide_fw_dispatch::flow::Flow::Respond(resp) => Some(resp),
                    oxide_fw_dispatch::flow::Flow::Continue => None,
                }
            }
            Err(_) => None,
        };
        if let Some(resp) = rejected {
            return Box::pin(async move { resp });
        }
        if !oxide_fw_dispatch::build::is_data_plane_path(&raw_path) {
            return inner_dispatch(req);
        }
        let current = match gens.read() {
            Ok(g) => g.current().to_string(),
            Err(_) => return inner_dispatch(req),
        };
        let fut = inner_dispatch(req);
        Box::pin(async move {
            let mut resp = fut.await;
            oxide_fw_dispatch::build::advertise_build(&mut resp, &current);
            resp
        })
    });
    // §8.5: publish the dispatch closure so middleware `{ rewrite }` can
    // re-enter it with a rewritten URL. The OnceLock is set once; subsequent
    // calls are no-ops (a running dispatch is already published).
    let _ = self_dispatch.set(std::sync::Arc::clone(&dispatch_fn));
    dispatch_fn
}

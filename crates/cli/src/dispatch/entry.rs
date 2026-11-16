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
    build_dispatch_swappable(DispatchDeps {
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
        flight_map: std::sync::Arc::new(dashmap::DashMap::new()),
        self_dispatch: std::sync::Arc::new(std::sync::OnceLock::new()),
        stream_counter: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
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

// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
/// The framework dispatch itself: the stage runner over one shared
/// `Arc<DispatchDeps>` (the A04 wrapper below wraps it). The old
/// ~30-capture closure became the stage table — the bodies moved verbatim
/// into `stages/` (see scripts/build_stage_split.py + split_selfcheck --stages).
pub(crate) fn build_dispatch_swappable(
    args: DispatchDeps,
) -> std::sync::Arc<dyn Fn(oxide_fw_server::OxideRequest) -> BoxedResponse + Send + Sync> {
    let deps = std::sync::Arc::new(args);
    let inner_dispatch: std::sync::Arc<
        dyn Fn(oxide_fw_server::OxideRequest) -> BoxedResponse + Send + Sync,
    > = {
        let deps = std::sync::Arc::clone(&deps);
        std::sync::Arc::new(move |req: oxide_fw_server::OxideRequest| {
            let deps = std::sync::Arc::clone(&deps);
            Box::pin(async move {
                let mut cx = RequestCtx::new(req, deps.swappable.load());
                stages::run_stages(&deps, &mut cx).await
            })
        })
    };
    // The A04 closure moves the deps Arc; the publish below goes through
    // this clone (the same OnceLock).
    let deps_for_publish = std::sync::Arc::clone(&deps);
    // <<< stage-wrapper
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
        let Some(gens) = deps.generations.as_ref() else {
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
    let _ = deps_for_publish
        .self_dispatch
        .set(std::sync::Arc::clone(&dispatch_fn));
    dispatch_fn
}

//! Moved verbatim from dispatch/entry.rs by scripts/build_stage_split.py
//! (V2-A01 phase 3).
//!
//! stage_route — per-route input precompute + the F10 generation
//! /// routing + the F12 brownout decision + the §11 L0 keying (the hot-path
//! /// hoists B03 targets): loader inputs, endpoint module path, middleware
//! /// chain paths, request nonce, not-found/error boundaries, layout chains,
//! /// action eligibility, the recording capture and the single-flight key.

use crate::*;
use oxide_fw_dispatch::flow::Flow;

// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
pub(crate) async fn stage_route(d: &DispatchDeps, cx: &mut RequestCtx) -> Flow {
    let req = &cx.req;
    let tree = &cx.tree;
    let matched = &cx.matched;
    let serving_pools = &d.serving_pools;
    let loader_runner = d.loader_runner.clone();
    let page_renderer = d.page_renderer.clone();
    let pool = d.pool.clone();
    let routes_root = d.routes_root.clone();
    let boot_script_url_for_render = d.boot_script_url.clone();
    let boot_script_integrity_for_render = d.boot_script_integrity.clone();
    let endpoint_for_dispatch = d.endpoint_dispatcher.clone();
    let waterfall = d.waterfall.clone();
    let record = &d.record;
    let recordings = &d.recordings;
    let brownout_for_dispatch = &d.brownout;
    let l0 = d.l0.clone();
    let actions_for_dispatch = d.actions.clone();
    let vary_headers = &d.vary_headers;
    let is_data_request = cx.is_data_request;
    let normalized = &cx.normalized;
    let req_path = cx.req_path.clone();
    // <<< stage-wrapper
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
                let on_data_plane = oxide_fw_dispatch::build::is_data_plane_path(normalized);
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

    // V2-B01: the matched (or unmatched not-found) route's precomputed
    // context — set by stage_match from the generation's table; every
    // chain answer below is one hit into it.
    let route_ctx = cx.route_ctx.clone();

    // Pre-compute the loader inputs synchronously so the async block
    // owns plain data (no borrow of `matched`).
    let (module_path, route_id, params, has_server_module, render_mode, needs_parent, is_live): LoaderInputs = match matched {
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
    let endpoint_module_path: Option<std::path::PathBuf> = match matched {
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
    // P-06 → V2-B01: served from the precomputed route context (one table
    // hit per request — the chain, outer→inner, is a boot-time answer); the
    // tree's P-06 index stays as the degenerate fallback (a matched route
    // is always a table hit within the prelude's matched pair).
    let middleware_module_paths: Vec<std::path::PathBuf> = match &route_ctx {
        Some(rc) => rc
            .middleware_chain
            .iter()
            .map(|f| routes_root.join(f))
            .collect(),
        None => tree
            .middleware_files_for(route_id.as_deref().unwrap_or(""))
            .into_iter()
            .map(|f| routes_root.join(f))
            .collect(),
    };
    // P-07b: built on first use. The JSON param map is consumed only by
    // the middleware loop and the §10.2 endpoint dispatcher; a page
    // request with no middleware file and no route.ts skips the
    // per-param allocations entirely (`params` itself stays alive for
    // the loader / L0-key paths below).
    let endpoint_params_json: Option<serde_json::Map<String, serde_json::Value>> = None;
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
    // P-06 → V2-B01: not-found route + layout chain from the table (the
    // first `NotFound` entry's precomputed context, same source the
    // dispatch answers use); the tree's index stays as the fallback.
    let (unmatched_route_id, unmatched_layout_ids): (Option<String>, Vec<String>) = cx
        .ctx_table
        .as_ref()
        .and_then(|t| t.unmatched())
        .map(|c| {
            (
                Some(c.route_id.0.clone()),
                c.layout_chain.iter().map(|i| i.0.clone()).collect(),
            )
        })
        .or_else(|| {
            tree.not_found_route()
                .map(|(id, chain)| (Some(id), chain.into_iter().map(|r| r.0).collect()))
        })
        .unwrap_or_default();

    // §10.2 step 5: resolve the NEAREST ancestor `error.tsx` for the
    // matched route (innermost boundary wins — a `routes/blog/error`
    // shadows `routes/error`). Precomputed here so the async render
    // block can pass it as `input.error_route_id`. The adapter
    // renders this component when the page throws.
    // P-06 → V2-B01: nearest error boundary from the precomputed context
    // (the innermost `error.tsx`, one table hit).
    let error_route_id: Option<String> = match &route_ctx {
        Some(rc) => rc.error_route.as_ref().map(|i| i.0.clone()),
        None => tree.error_route_for(route_id.as_deref().unwrap_or("")),
    };
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
    // §8.2 / §16.3 → V2-B01: the layout chain (outermost first) from the
    // precomputed context — the oracle-equal chain without the per-match
    // recompute.
    let layout_ids: Vec<String> = match &route_ctx {
        Some(rc) => rc.layout_chain.iter().map(|i| i.0.clone()).collect(),
        None => Vec::new(),
    };
    // §9.2 → V2-B01: layout load specs from the precomputed context — the
    // `layout.server.ts` paths are flag-driven (`has_server_module`), so
    // the per-layout `is_file()` stat the walk paid per request is gone,
    // and the A06 `parent()` verdict rides the same row
    // (`LayoutLoadSpec`). Unmatched requests have no layout chain (the
    // specs are empty, as the walk produced).
    let layout_module_paths: Vec<LayoutLoadSpec> = match &route_ctx {
        Some(rc) => rc
            .layout_loads
            .iter()
            .map(|l| LayoutLoadSpec {
                id: l.id.0.clone(),
                path: routes_root.join(&l.server_file),
                needs_parent: l.needs_parent,
            })
            .collect(),
        None => Vec::new(),
    };

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
    // P-07: `req` is this invocation's own parameter, so move it into
    // the async block instead of cloning the whole request per call.
    let req_owned = req;
    // V2-F15: the capture starts HERE, where the request line-set and
    // the router's verdict are both known. The record keeps the raw
    // match (`route_id`) — the id the replay drives back through the
    // pipeline is the same one dispatch matched.
    let req_recording = record
        .as_ref()
        .and_then(|c| crate::record_wire::RequestRecording::start(c, recordings, req_owned));
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
    // §8.1 route.ts / §10.2 step 7: same pattern for the endpoint
    // dispatcher — the async block below moves this per-call clone.
    // §8.1 middleware.ts / §8.5: same shadow for middleware.
    // §8.5: same shadow for the self-dispatch handle used by a
    // middleware rewrite (the async block moves it per-call).
    // §20: the waterfall recorder (Arc inside, cheap to clone) is moved
    // by the async block; shadow it per call like the dispatchers above.
    // §16.6: same shadow for the session codec.
    // §11 L0: only a GET on a cacheable-mode route is a candidate.
    let l0 = l0.clone();
    // V2-F07: same shadow for the resumable-stream wiring (the tee in
    // the endpoint arm) and the stream-id counter the tee mints ids
    // with.
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
                    return Flow::Respond(oxide_fw_server::governor::shed_response(level));
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
    let l0_vary = l0_cache::vary_of(&req_owned.headers, vary_headers);
    // BUG-01: the path tag the L0 put site stamps alongside the route
    // tag, so `revalidate.path` can invalidate it.
    let l0_path = req_path;
    // PERF-04: the single-flight key (matches the L0 key shape) and the
    // shared map handle.
    let flight_key = match l0.as_ref() {
        Some(c) => l0_cache::L0Cache::key_for(&l0_route, &l0_params, &l0_vary, c.build_id()),
        None => String::new(),
    };
    // V2-F05: the revalidation hooks inside the block fan out through a
    // clone — the block is `move`, and moving the captured handle out
    // would demote the `Fn` dispatch closure.
    // >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
    cx.loader_runner = loader_runner;
    cx.page_renderer = page_renderer;
    cx.module_path = module_path;
    cx.route_id = route_id;
    cx.params = Some(params);
    cx.has_server_module = has_server_module;
    cx.render_mode = render_mode;
    cx.needs_parent = needs_parent;
    cx.is_live = is_live;
    cx.endpoint_module_path = endpoint_module_path;
    cx.middleware_module_paths = middleware_module_paths;
    cx.endpoint_params_json = endpoint_params_json;
    cx.endpoint_body = endpoint_body;
    cx.request_nonce = request_nonce;
    cx.unmatched_route_id = unmatched_route_id;
    cx.unmatched_layout_ids = unmatched_layout_ids;
    cx.error_route_id = error_route_id;
    cx.route_id_for_render = route_id_for_render;
    cx.layout_ids = layout_ids;
    cx.layout_module_paths = layout_module_paths;
    cx.action_name = action_name;
    cx.action_dispatcher = action_dispatcher;
    cx.action_params = action_params;
    cx.action_route = action_route;
    cx.req_recording = req_recording;
    cx.endpoint_for_dispatch = endpoint_for_dispatch;
    cx.boot_script_url_for_render = boot_script_url_for_render;
    cx.boot_script_integrity_for_render = boot_script_integrity_for_render;
    cx.waterfall = waterfall;
    cx.brownout_ceiling = brownout_ceiling;
    cx.l0_eligible = l0_eligible;
    cx.l0_route = l0_route;
    cx.l0_params = l0_params;
    cx.l0_vary = l0_vary;
    cx.l0_path = l0_path;
    cx.flight_key = flight_key;
    Flow::Continue
    // <<< stage-wrapper
}

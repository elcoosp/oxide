//! Moved verbatim from dispatch/entry.rs by scripts/build_stage_split.py
//! (V2-A01 phase 3).
//!
//! stage_data_route — the §8.4 `/__data` endpoint (the sketch's
//! `stage_data_route`): the loader-only route-data response with the same
//! middleware locals/headers the SSR render saw. Since the V2-A06
//! follow-up adoption the load phase runs the §9.2 parallel load graph
//! (see `crate::data_route`); this stage only threads the args through,
//! plus the page's own `parent()` verdict the graph serializes the page
//! node on.

use crate::*;
use oxide_fw_dispatch::flow::Flow;

// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
pub(crate) async fn stage_data_route(d: &DispatchDeps, cx: &mut RequestCtx) -> Flow {
    let req_owned = &cx.req;
    let loader_runner = &cx.loader_runner;
    let module_path = &cx.module_path;
    let route_id = &cx.route_id;
    let params = cx.params.as_ref().expect("the stage order sets params");
    let metrics = std::sync::Arc::clone(&d.metrics);
    let middleware_locals = &cx.middleware_locals;
    let middleware_headers = &cx.middleware_headers;
    let layout_module_paths = &cx.layout_module_paths;
    let resolved_locale = &cx.resolved_locale;
    let session_codec = d.session_codec.clone();
    let waterfall = cx.waterfall.clone();
    let is_live = cx.is_live;
    let started = cx.started;
    let middleware_ms = cx.middleware_ms;
    let is_data_request = cx.is_data_request;
    // V2-A06 follow-up: the PAGE's own `parent()` verdict rides the args
    // so the data route's load graph serializes the page behind the
    // innermost layout exactly when the scanner says it may call
    // `parent()` (the same field `stage_loaders` reads).
    let needs_parent = cx.needs_parent;
    // <<< stage-wrapper
    // PERF-01 / §8.4: `/__data` is the client router's route-data
    // endpoint. The target route was re-matched in the prelude, so
    // this branch runs with the SAME middleware locals/headers the
    // SSR render saw, the layout-load parent chain, the page
    // loader's data + resolved defers, and the resolved locale —
    // no render, just loaders + ODF encode.
    if is_data_request {
        return Flow::Respond(
            route_data_response(RouteDataArgs {
                req: req_owned,
                loader_runner: loader_runner.as_ref(),
                module_path: module_path.as_deref(),
                route_id: route_id.as_deref(),
                params,
                metrics: &metrics,
                started,
                middleware_locals,
                middleware_headers,
                layout_module_paths,
                page_needs_parent: needs_parent,
                locale: resolved_locale,
                session_codec: session_codec.as_deref(),
                middleware_ms,
                waterfall: waterfall.as_ref(),
                live: is_live,
            })
            .await,
        );
    }
    // >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
    Flow::Continue
    // <<< stage-wrapper
}

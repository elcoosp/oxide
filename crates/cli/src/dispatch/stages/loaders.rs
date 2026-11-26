//! Moved verbatim from dispatch/entry.rs by scripts/build_stage_split.py
//! (V2-A01 phase 3).
//!
//! stage_loaders — the §9.1/§9.2 loader phase (the sketch's
//! `stage_loaders`): the V2-A06 parallel load graph over the layout chain
//! and page (the sequential loop and the two-load pair it was adopted
//! from), streaming settles, session persistence and
//! the data/deferred/meta threading into the render phase.
//!
//! V2-A06 adoption (this file deliberately diverges from the verbatim
//! split; recorded in `scripts/split_selfcheck.py` STAGE_ADOPTED and
//! `docs/deviations.md`): the node list is every layout with a server
//! module (its own scanner verdict decides whether it awaits its
//! predecessor's data) plus the page; `run_load_graph_outcomes` runs
//! independent loads concurrently, threads `parent_data` through
//! `needs_parent` chains, and returns outputs in INPUT order — so the
//! outer-to-inner side-effect accumulation and the layout-then-page
//! short-circuit precedence are exactly the sequential shapes (pinned
//! substrate-side by `loads_tdd.rs`, pipeline-side by the
//! `layout_and_page_overlap_*` pins in `tests.rs`).

use crate::*;
use oxide_fw_dispatch::flow::Flow;
use oxide_fw_dispatch::loads::{LoadNode, LoadResult, run_load_graph_outcomes};

// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
pub(crate) async fn stage_loaders(d: &DispatchDeps, cx: &mut RequestCtx) -> Flow {
    let req_owned = &cx.req;
    let loader_runner = &cx.loader_runner;
    let module_path = &cx.module_path;
    let route_id = &cx.route_id;
    let params = cx.params.as_ref().expect("the stage order sets params");
    let has_server_module = cx.has_server_module;
    let needs_parent = cx.needs_parent;
    let label = cx.label.clone();
    let metrics = std::sync::Arc::clone(&d.metrics);
    let middleware_locals = cx.middleware_locals.clone();
    let session_codec = d.session_codec.clone();
    // V2-A05 step 4: the locale rides the pipeline-owned slot.
    let resolved_locale = cx.locals.locale.clone();
    let layout_module_paths = cx.layout_module_paths.clone();
    let waterfall = cx.waterfall.clone();
    let started = cx.started;
    let session_before = cx.session_before.clone();
    // <<< stage-wrapper
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

    // §9.2 (V2-A06): the load graph. The page node exists when the route
    // has a server module; it runs through the streaming entry (B2) and
    // its settle receiver comes back through the outcomes.
    let page_loads =
        has_server_module && loader_runner.is_some() && module_path.is_some() && route_id.is_some();
    if let Some(runner) = loader_runner.as_ref()
        && (!layout_module_paths.is_empty() || page_loads)
    {
        let mut nodes: Vec<LoadNode> = Vec::new();
        for spec in &layout_module_paths {
            let mut linput = oxide_fw_server::LoaderRunInput::new(
                spec.path.to_string_lossy().to_string(),
                spec.id.clone(),
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
                t.to_traceparent(&t.parent_id)
                    .map(|tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}))
            });
            // `parent_data` is threaded by the graph from the predecessor
            // slot when this layout's own verdict says it may call
            // `parent()`; an independent layout reads Null (§9.2: loads
            // that do not call parent() run concurrently).
            nodes.push(LoadNode {
                input: linput,
                needs_parent: spec.needs_parent,
            });
        }
        let mut streaming: Vec<usize> = Vec::new();
        if page_loads {
            // B2 / P0-2: the page load streams its defer() settles per
            // settle instead of buffering the whole document — the graph
            // runs the page node through `run_streaming` and hands the
            // receiver back via the outcomes.
            streaming.push(nodes.len());
            let mp = module_path.as_ref().unwrap();
            let rid = route_id.as_ref().unwrap();
            // Pass the normalized id (no `/page` suffix) so user
            // `load(event)` implementations see the route
            // directory they expect.
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
            // The graph threads the innermost layout's data here when the
            // page's own verdict says it may call `parent()`; otherwise
            // the value stays Null (the page ran concurrently).
            pinput.parent_data = serde_json::Value::Null;
            // §20 / G2: thread the inbound W3C trace so the loader's
            // server-side fetch calls join the request trace.
            pinput.trace = req_owned.trace.as_ref().and_then(|t| {
                t.to_traceparent(&t.parent_id)
                    .map(|tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}))
            });
            nodes.push(LoadNode {
                input: pinput,
                needs_parent,
            });
        }
        let wall_start = std::time::Instant::now();
        let (result, mut outcomes) =
            run_load_graph_outcomes(std::sync::Arc::clone(runner), nodes, &streaming).await;
        // §20 / O3c: the waterfall's `overlap-saved` mark — the
        // counterfactual wall-clock the concurrent nodes would have cost
        // had they run sequentially (the sum of their own run times)
        // minus the actual wall. For the pre-A06 (layout, page) join this
        // equals `min(layout_ms, page_ms)` exactly; the mark stays a
        // saving so the inspector's total reconciles. Recorded before the
        // result is consumed — the pre-A06 join recorded it even when a
        // load short-circuited.
        if let Some(rec) = waterfall.as_ref()
            && outcomes.elapsed_ms.len() > 1
        {
            let saved = (outcomes.elapsed_ms.iter().sum::<f64>()
                - wall_start.elapsed().as_secs_f64() * 1000.0)
                .max(0.0);
            rec.record_staged(
                &req_owned.request_id,
                vec![(LOAD_OVERLAP_STAGE.to_string(), saved)],
                req_owned.trace.as_ref().map(|t| t.trace_id.clone()),
            );
        }
        match result {
            // §9.2: the first short-circuit in input order wins — an
            // outer layout's redirect/error answers before the page's,
            // exactly like the sequential walk (substrate-pinned).
            LoadResult::ShortCircuit(out) => {
                let body = out
                    .data
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mut resp = oxide_fw_server::OxideResponse::new(out.status, body);
                if let Some(loc) = out.location.as_deref() {
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
                return Flow::Respond(resp);
            }
            // A runner crash: the input-order index distinguishes the
            // layout diagnostics from the page one (the sequential arms
            // named them differently); the runner's diagnostic code and
            // message are verbatim (the message is the response body).
            LoadResult::Failed {
                index,
                code,
                message,
            } => {
                tracing::error!(
                    code = %code,
                    error = %message,
                    // WP B03 item 2: `label` is an `Arc<str>` (zero-alloc clone);
                    // deref to `&str` for the tracing field's `Display`.
                    route = %&*label,
                    "loader failed",
                );
                metrics.observe(&label, started.elapsed(), 500);
                let which = if index < layout_module_paths.len() {
                    "layout loader error"
                } else {
                    "loader error"
                };
                return Flow::Respond(oxide_fw_server::OxideResponse::html(
                    500,
                    format!("{which}: {}", oxide_fw_render::escape_text(&message)),
                ));
            }
            LoadResult::Data(outs) => {
                // Side effects accumulate outer-to-inner (layouts in
                // input order); the page — the last node, when it ran —
                // OVERWRITES the accumulated cookies/headers. That is
                // the preexisting overwrite semantics both the
                // sequential and the (last-layout, page) join arms
                // shipped (the NOTE below it is preserved verbatim).
                let layout_count = layout_module_paths.len();
                for (i, out) in outs.iter().enumerate() {
                    if i < layout_count {
                        // §16.6 / WP-r10-1 / D1: a layout loader that
                        // mutates `event.session` must persist it — the
                        // short-circuit arm above does; this Ok arm
                        // encodes through the same cookies stream the
                        // render response applies.
                        let mut cookies = out.set_cookies.clone();
                        if let Some(cookie) = encode_session_cookie(
                            out.session.as_ref(),
                            session_before.as_ref(),
                            session_codec.as_deref(),
                        ) {
                            cookies.push(cookie);
                        }
                        loader_set_cookies.extend(cookies);
                        loader_headers.extend(out.headers.iter().cloned());
                    } else {
                        data = out.data.clone();
                        // 16-G: the route's `meta = defineMeta(fn)`
                        // export, evaluated by the worker against this
                        // load's data.
                        route_meta = out.meta.clone();
                        deferred_values = out.deferred.clone();
                        page_settles = outcomes.settles[i].take();
                        // NOTE (preexisting, preserved): overwrite, not
                        // extend — matches the sequential-path behavior.
                        loader_set_cookies = out.set_cookies.clone();
                        loader_headers = out.headers.clone();
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
                }
            }
        }
    }
    // >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
    cx.data = Some(data);
    cx.page_settles = page_settles;
    cx.route_meta = route_meta;
    cx.loader_set_cookies = Some(loader_set_cookies);
    cx.loader_headers = Some(loader_headers);
    cx.deferred_values = deferred_values;
    cx.stage_load_start = stage_load_start;
    Flow::Continue
    // <<< stage-wrapper
}

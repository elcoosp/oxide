//! Moved verbatim from dispatch/entry.rs by scripts/build_stage_split.py
//! (V2-A01 phase 3).
//!
//! stage_loaders — the §9.1/§9.2 loader phase (the sketch's
//! /// `stage_loaders`): the sequential layout chain, the parallel
//! /// last-layout+page join, streaming settles, session persistence and
//! /// the data/deferred/meta threading into the render phase.

use crate::*;
use oxide_fw_dispatch::flow::Flow;

// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
pub(crate) async fn stage_loaders(d: &DispatchDeps, cx: &mut RequestCtx) -> Flow {
    let req_owned = &cx.req;
    let loader_runner = &cx.loader_runner;
    let module_path = &cx.module_path;
    let route_id = &cx.route_id;
    let params = cx.params.as_ref().expect("the stage order sets params");
    let has_server_module = cx.has_server_module;
    let needs_parent = cx.needs_parent;
    let label = &cx.label;
    let metrics = std::sync::Arc::clone(&d.metrics);
    let middleware_locals = &cx.middleware_locals;
    let session_codec = d.session_codec.clone();
    let resolved_locale = &cx.resolved_locale;
    let layout_module_paths = &cx.layout_module_paths;
    let waterfall = cx.waterfall.clone();
    let started = cx.started;
    let session_before = &cx.session_before;
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
                t.to_traceparent(&t.parent_id)
                    .map(|tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}))
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
                    metrics.observe(label, started.elapsed(), out.status);
                    return Flow::Respond(resp);
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
                    metrics.observe(label, started.elapsed(), 500);
                    return Flow::Respond(oxide_fw_server::OxideResponse::html(
                        500,
                        format!(
                            "layout loader error: {}",
                            oxide_fw_render::escape_text(&e.0.message)
                        ),
                    ));
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
            t.to_traceparent(&t.parent_id)
                .map(|tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}))
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
            t.to_traceparent(&t.parent_id)
                .map(|tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}))
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
                metrics.observe(label, started.elapsed(), out.status);
                return Flow::Respond(resp);
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
                metrics.observe(label, started.elapsed(), 500);
                return Flow::Respond(oxide_fw_server::OxideResponse::html(
                    500,
                    format!(
                        "layout loader error: {}",
                        oxide_fw_render::escape_text(&e.0.message)
                    ),
                ));
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
                metrics.observe(label, started.elapsed(), out.status);
                return Flow::Respond(resp);
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
                metrics.observe(label, started.elapsed(), 500);
                return Flow::Respond(oxide_fw_server::OxideResponse::html(
                    500,
                    format!(
                        "loader error: {}",
                        oxide_fw_render::escape_text(&e.0.message)
                    ),
                ));
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
            .unwrap_or(route_id)
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
            t.to_traceparent(&t.parent_id)
                .map(|tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}))
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
                    metrics.observe(label, started.elapsed(), out.status);
                    return Flow::Respond(resp);
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
                metrics.observe(label, started.elapsed(), 500);
                return Flow::Respond(oxide_fw_server::OxideResponse::html(
                    500,
                    format!(
                        "loader error: {}",
                        oxide_fw_render::escape_text(&e.0.message)
                    ),
                ));
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

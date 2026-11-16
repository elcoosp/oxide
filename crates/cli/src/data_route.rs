//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

pub struct RouteDataArgs<'a> {
    /// V2-F05: the matched route is live-opted — the envelope advertises
    /// `live: true` (the client's connectLive gate after a navigation).
    pub live: bool,
    pub req: &'a oxide_fw_server::OxideRequest,
    pub loader_runner: Option<&'a std::sync::Arc<dyn oxide_fw_server::LoaderRunner>>,
    pub module_path: Option<&'a std::path::Path>,
    pub route_id: Option<&'a str>,
    pub params: &'a std::collections::BTreeMap<String, String>,
    pub metrics: &'a std::sync::Arc<oxide_fw_server::Metrics>,
    pub started: std::time::Instant,
    pub middleware_locals: &'a serde_json::Value,
    pub middleware_headers: &'a [(String, String)],
    pub layout_module_paths: &'a [LayoutLoadSpec],
    pub locale: &'a str,
    pub session_codec: Option<&'a oxide_fw_server::SessionCodec>,
    pub middleware_ms: f64,
    pub waterfall: Option<&'a oxide_fw_dev::WaterfallRecorder>,
}

/// PERF-01 / §8.4: answer a `/__data?route=<path>` request. Returns the
/// ODF-encoded `{route, url, params, data, deferred, locale}` envelope the
/// client router's `normalizeInput` expects (extra keys are ignored by
/// today's client; the resolved-deferred map + locale are what make
/// client-side navigation data MATCH the SSR render — §8.4 parity).
///
/// Parity contract: the dispatch re-matches `?route=` against the route
/// tree BEFORE this runs, so the middleware phase (nearest-ancestor
/// resolution + locals + headers) and the layout-load chain
/// (`parent_data`) that ran for a full SSR render run here too. What is
/// deliberately skipped is the RENDER — no adapter, no HTML, just loaders
/// + ODF encode. Success is 200 with `application/oxide+odf`; a loader
///
/// short-circuit (`error()`/`redirect()`) propagates its status/location.
pub(crate) async fn route_data_response(args: RouteDataArgs<'_>) -> oxide_fw_server::OxideResponse {
    let RouteDataArgs {
        live,
        req,
        loader_runner,
        module_path,
        route_id,
        params,
        metrics,
        started,
        middleware_locals,
        middleware_headers,
        layout_module_paths,
        locale,
        session_codec,
        middleware_ms,
        waterfall,
    } = args;

    let request_id = req.request_id.clone();
    let resp = route_data_response_inner(RouteDataInnerArgs {
        live,
        req,
        loader_runner,
        module_path,
        route_id,
        params,
        metrics,
        started,
        middleware_locals,
        middleware_headers,
        layout_module_paths,
        locale,
        session_codec,
    })
    .await;
    if let Some(rec) = waterfall {
        let total = started.elapsed().as_secs_f64() * 1000.0;
        // §20 / WP-r9 nit: /__data rows carried `trace_id: null` because
        // this site used the trace-less `record` shim while SSR/action
        // rows use `record_with_trace`. `req.trace` is in scope here.
        rec.record_with_trace(
            &request_id,
            vec![
                ("middleware".to_string(), middleware_ms),
                ("load".to_string(), (total - middleware_ms).max(0.0)),
            ],
            req.trace.as_ref().map(|t| t.trace_id.clone()),
        );
    }
    resp
}

pub struct RouteDataInnerArgs<'a> {
    /// V2-F05: the matched route is live-opted (see [`RouteDataArgs`]).
    pub live: bool,
    pub req: &'a oxide_fw_server::OxideRequest,
    pub loader_runner: Option<&'a std::sync::Arc<dyn oxide_fw_server::LoaderRunner>>,
    pub module_path: Option<&'a std::path::Path>,
    pub route_id: Option<&'a str>,
    pub params: &'a std::collections::BTreeMap<String, String>,
    pub metrics: &'a std::sync::Arc<oxide_fw_server::Metrics>,
    pub started: std::time::Instant,
    pub middleware_locals: &'a serde_json::Value,
    pub middleware_headers: &'a [(String, String)],
    pub layout_module_paths: &'a [LayoutLoadSpec],
    pub locale: &'a str,
    pub session_codec: Option<&'a oxide_fw_server::SessionCodec>,
}

pub(crate) async fn route_data_response_inner(
    args: RouteDataInnerArgs<'_>,
) -> oxide_fw_server::OxideResponse {
    let RouteDataInnerArgs {
        live,
        req,
        loader_runner,
        module_path,
        route_id,
        params,
        metrics,
        started,
        middleware_locals,
        middleware_headers,
        layout_module_paths,
        locale,
        session_codec,
    } = args;

    // `?route=<percent-encoded path>` (default `/`).
    let route_q = req
        .url
        .split_once('?')
        .map(|(_, q)| q)
        .and_then(|q| {
            q.split('&').find_map(|kv| {
                kv.strip_prefix("route=")
                    .map(oxide_fw_server::percent_decode)
            })
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/".to_string());

    // §9.2: layout loads first (outer → inner), threading `parent_data`
    // exactly like the render path. A layout short-circuit (redirect /
    // error) short-circuits the data response too — client navigation must
    // see what a full-page load would have seen.
    let mut parent_data: serde_json::Value = serde_json::Value::Null;
    // §16.6 / WP-D1: layout loaders share the middleware locals, so a layout
    // that mutates `event.session` must both persist the cookie and make the
    // new state visible to the page loader that runs next. Keep a local
    // copy rather than mutating the borrowed `middleware_locals`.
    let mut locals_for_load = middleware_locals.clone();
    let mut layout_session_cookie: Option<String> = None;
    if let Some(runner) = loader_runner {
        for spec in layout_module_paths {
            let mut linput = oxide_fw_server::LoaderRunInput::new(
                spec.path.to_string_lossy().to_string(),
                spec.id.clone(),
                &route_q,
            );
            linput.method = req.method.clone();
            // §20 / G2: thread the inbound W3C trace into the loader so its
            // server-side fetch calls join the same trace. The worker's
            // `traceparentFromPayload` reads `payload.trace.traceparent` —
            // a W3C wire-form string — so build the dict from the parsed
            // context rather than serializing the internal struct.
            linput.trace = req.trace.as_ref().and_then(|t| {
                // Re-emit the *parent* span as this loader's inbound
                // traceparent — the worker treats it as the upstream hop.
                // The `tracestate` rides along unchanged (W3C 3.3: a
                // participant that does not mutate it must forward it).
                t.to_traceparent(&t.parent_id).map(|tp| {
                    serde_json::json!({
                        "traceparent": tp,
                        "tracestate": t.tracestate,
                    })
                })
            });
            linput.headers = req.headers.as_ref().clone();
            linput.params = params.clone();
            linput.locals = locals_for_load.clone();
            linput.locale = locale.to_string();
            linput.parent_data = parent_data.clone();
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
                    // §16.6 / WP-D1: a short-circuit (redirect / error)
                    // that also mutated `event.session` must persist it.
                    // §16.6 / WP-r10-1: the baseline must be the inbound
                    // session (post-middleware), not `None` — `None`
                    // re-issues a cookie on every untouched-session
                    // short-circuit. This is the same class of bug
                    // 119b9ec fixed in the /__data page arms; it missed
                    // the layout arm.
                    if let Some(cookie) = encode_session_cookie(
                        out.session.as_ref(),
                        locals_for_load.get("session"),
                        session_codec,
                    ) {
                        resp = resp.with_header("set-cookie", cookie);
                    }
                    metrics.observe("_data", started.elapsed(), out.status);
                    return resp;
                }
                Ok(out) => {
                    parent_data = out.data;
                    // §16.6 / WP-D1: persist a layout mutation on the
                    // non-short-circuit path too, mirroring SSR. The page
                    // loader then starts from the mutated session.
                    if let Some(cookie) = encode_session_cookie(
                        out.session.as_ref(),
                        locals_for_load.get("session"),
                        session_codec,
                    ) {
                        layout_session_cookie = Some(cookie);
                    }
                    if let Some(sess) = out.session {
                        locals_for_load["session"] = sess;
                    }
                }
                Err(e) => {
                    metrics.observe("_data", started.elapsed(), 500);
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
    }

    // §16.6 / WP-D1: session cookie for the data path
    // (attached to the envelope after the loads complete).
    let mut data_path_session_cookie: Option<String> = layout_session_cookie.clone();
    let mut data = serde_json::Value::Object(Default::default());
    // §7.1 / §9.2: resolved defer() values — the /__data envelope carries
    // them so the client router can resolve defer slots after a client-side
    // navigation (previously dropped: nav data diverged from SSR data).
    let mut deferred: std::collections::BTreeMap<String, serde_json::Value> =
        std::collections::BTreeMap::new();
    if let (Some(runner), Some(mp), Some(rid)) = (loader_runner, module_path, route_id) {
        let route_for_loader = rid.strip_suffix("/page").unwrap_or(rid).to_string();
        let mut input = oxide_fw_server::LoaderRunInput::new(
            mp.to_string_lossy().to_string(),
            route_for_loader,
            &route_q,
        );
        input.method = req.method.clone();
        input.headers = req.headers.as_ref().clone();
        input.params = params.clone();
        input.locals = locals_for_load.clone();
        input.locale = locale.to_string();
        input.parent_data = parent_data;
        // §20 / G2: thread the inbound W3C trace so the loader's
        // server-side fetch calls join the request trace.
        input.trace = req.trace.as_ref().and_then(|t| {
            t.to_traceparent(&t.parent_id)
                .map(|tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}))
        });
        match runner.run(input).await {
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
                if let Some(cookie) = layout_session_cookie.clone() {
                    resp = resp.with_header("set-cookie", cookie);
                }
                // §16.6 / WP-D1: a short-circuit (redirect / error)
                // that also mutated `event.session` must persist it.
                if let Some(cookie) = encode_session_cookie(
                    out.session.as_ref(),
                    // §16.6 / WP-r9-3: pass the request's decoded session
                    // as `before`. `middleware_locals["session"]` is set at
                    // :6735 from the inbound cookie and refined by any
                    // middleware mutation, so it is the exact state the
                    // loader started with — an untouched loader matches it
                    // and no cookie is emitted. Passing `None` here meant
                    // every /__data request from a client holding a session
                    // re-issued the cookie.
                    locals_for_load.get("session"),
                    session_codec,
                ) {
                    resp = resp.with_header("set-cookie", cookie);
                }
                metrics.observe("_data", started.elapsed(), out.status);
                return resp;
            }
            Ok(out) => {
                data = out.data;
                deferred = out.deferred;
                // §16.6 / WP-D1: persist a session mutation on the
                // data path. The response is built below; the cookie
                // rides the envelope.
                if let Some(cookie) = encode_session_cookie(
                    out.session.as_ref(),
                    // §16.6 / WP-r9-3: pass the request's decoded session
                    // as `before`. `middleware_locals["session"]` is set at
                    // :6735 from the inbound cookie and refined by any
                    // middleware mutation, so it is the exact state the
                    // loader started with — an untouched loader matches it
                    // and no cookie is emitted. Passing `None` here meant
                    // every /__data request from a client holding a session
                    // re-issued the cookie.
                    locals_for_load.get("session"),
                    session_codec,
                ) {
                    data_path_session_cookie = Some(cookie);
                }
            }
            Err(e) => {
                metrics.observe("_data", started.elapsed(), 500);
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

    let route_label = route_id
        .map(|r| r.strip_suffix("/page").unwrap_or(r).to_string())
        .unwrap_or_else(|| route_q.clone());
    // §8.2: emit the layout-id chain so the client re-render can nest the
    // new page inside the same layout components the SSR rendered it in.
    // Without this, a client nav hydrates a fresh bare `<Page>` against the
    // SSR `<RootLayout><Page/></RootLayout>` tree — the DOM shows the OLD
    // page because `realRoot.update` re-rendered the ORIGINAL Component
    // with new props instead of swapping in the new route.
    let layout_ids: Vec<String> = layout_module_paths
        .iter()
        .map(|spec| spec.id.clone())
        .collect();
    let mut payload = serde_json::json!({
        "route": route_label,
        "url": route_q,
        "params": params,
        "data": data,
        "deferred": deferred,
        "locale": locale,
        "layouts": layout_ids,
    });
    // V2-F05: the live advertisement rides the envelope (the client's
    // router opens/closes the SSE connection with the route it just
    // navigated to). Absent for non-live routes — their envelopes stay
    // byte-identical to the pre-F05 shapes.
    if live {
        payload["live"] = serde_json::Value::Bool(true);
    }
    // §9.5 / D11: encode as ODF, the wire format the client decodes.
    let body = oxide_fw_odf::encode_to_bytes(&oxide_fw_odf::Value::from_json(&payload))
        .unwrap_or_default();
    metrics.observe("_data", started.elapsed(), 200);
    let mut resp = oxide_fw_server::OxideResponse::new(200, body)
        .with_header("content-type", "application/oxide+odf");
    // §8.5 / §16.6 / WP-r9-3: middleware headers FIRST, then the loader's
    // session cookie — the loader is the later mutation and wins on the
    // wire (HTTP applies same-name Set-Cookies in arrival order). This
    // matches the render path's order (WP-D1's 663201a fix); the /__data
    // envelope was inverted, so a /__data request from a client with a
    // middleware-mutated session saw the OLD value reach the browser.
    for (k, v) in middleware_headers {
        resp = resp.with_header(k, v.clone());
    }
    if let Some(cookie) = data_path_session_cookie {
        resp = resp.with_header("set-cookie", cookie);
    }
    resp
}

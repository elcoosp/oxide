//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).
//!
//! V2-A06 follow-up adoption (deliberately diverges from the verbatim
//! move; recorded in `scripts/split_selfcheck.py` STAGE_RULES +
//! `docs/deviations.md`): the §9.2 layout-load chain runs through the SAME
//! parallel load graph `stage_loaders` runs for SSR
//! (`run_load_graph_outcomes` — a `LoadNode` per layout with its own
//! scanner verdict plus the page node), replacing the sequential loop the
//! verbatim move carried. The 2026-11-16 SSR adoption deferred exactly
//! this endpoint ("the /__data path keeps its sequential chain —
//! parallelizing it is a follow-up"); a client-side navigation's data
//! envelope now stops paying the sequential waterfall the SSR render
//! stopped paying, and both legs share the substrate's semantics:
//! independent loads run concurrently, `parent()`-calling loads serialize
//! behind their predecessor with the predecessor's data, the FIRST
//! short-circuit in input order wins, and a runner failure names its
//! node. The substrate-authoritative deltas mirror the SSR adoption and
//! are recorded in the entry below: `parent_data` threads only along
//! `needs_parent` chains (an independent layout reads Null, not its
//! predecessor's data) and every node starts from the middleware locals
//! (concurrent loads cannot thread layout session mutations through
//! locals, and SSR does not either).

use crate::*;
use oxide_fw_dispatch::loads::{LoadNode, LoadResult, run_load_graph_outcomes};

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
    /// The PAGE's own §9.2 `parent()` verdict (the matched entry's
    /// token-exact scanner scan, threaded through `RequestCtx::needs_parent`)
    /// — decides whether the page node serializes behind the innermost
    /// layout and receives its data.
    pub page_needs_parent: bool,
    pub locale: &'a str,
    pub session_codec: Option<&'a oxide_fw_server::SessionCodec>,
    pub middleware_ms: f64,
    pub waterfall: Option<&'a oxide_fw_dev::WaterfallRecorder>,
    /// P0-2 (the response-envelope side): the largest encoded envelope this
    /// route answers — the boot threads `Limits::max_data_envelope` (the
    /// request `max_body` twin) through `DispatchDeps`. An envelope past
    /// the bound (or one the ODF encoder refuses) answers the minted
    /// `DSP-117` coded 500 instead of riding the wire.
    pub max_envelope: usize,
}

/// PERF-01 / §8.4: answer a `/__data?route=<path>` request. Returns the
/// ODF-encoded `{route, url, params, data, deferred, locale}` envelope the
/// client router's `normalizeInput` expects (extra keys are ignored by
/// today's client; the resolved-deferred map + locale are what make
/// client-side navigation data MATCH the SSR render — §8.4 parity).
///
/// Parity contract: the dispatch re-matches `?route=` against the route
/// tree BEFORE this runs, so the middleware phase (nearest-ancestor
/// resolution + locals + headers) and the §9.2 load graph (the same
/// substrate and node shapes the SSR render runs) run here too. What is
/// deliberately skipped is the RENDER — no adapter, no HTML, just loaders
/// + ODF encode. Success is 200 with `application/oxide+odf`; a loader
///
/// short-circuit (`error()`/`redirect()`) propagates its status/location.
///
/// The seam is public (the F55 pattern: the bench and the walkthrough
/// example drive the pipeline's own surface — `benches/data_envelope.rs`,
/// `examples/data_envelope.rs`).
pub async fn route_data_response(args: RouteDataArgs<'_>) -> oxide_fw_server::OxideResponse {
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
        page_needs_parent,
        locale,
        session_codec,
        middleware_ms,
        waterfall,
        max_envelope,
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
        page_needs_parent,
        locale,
        session_codec,
        waterfall,
        max_envelope,
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
    /// The page's own `parent()` verdict (see [`RouteDataArgs`]).
    pub page_needs_parent: bool,
    pub locale: &'a str,
    pub session_codec: Option<&'a oxide_fw_server::SessionCodec>,
    /// The §20 waterfall recorder — the inner body records the
    /// graph's `overlap-saved` staged row through it (the same mark
    /// `stage_loaders` records for SSR).
    pub waterfall: Option<&'a oxide_fw_dev::WaterfallRecorder>,
    /// P0-2: the encoded-envelope bound the inner body refuses past
    /// (the `DSP-117` coded 500 — see [`RouteDataArgs::max_envelope`]).
    pub max_envelope: usize,
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
        page_needs_parent,
        locale,
        session_codec,
        waterfall,
        max_envelope,
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

    // §9.2 (V2-A06 follow-up): the load graph — the SAME substrate and
    // node shapes `stage_loaders` runs for SSR. A `LoadNode` per layout
    // with its OWN scanner verdict (the RouteContextTable's token-exact
    // `parent()` scan, carried by `LayoutLoadSpec`) plus the page node
    // when the route has a server module. Independent loads run
    // concurrently, a `parent()`-calling load serializes behind its
    // predecessor with the predecessor's data, and the result is decided
    // in INPUT order — the outermost short-circuit wins exactly as the
    // sequential walk did. The substrate-authoritative deltas mirror the
    // SSR adoption: `parent_data` threads only along `needs_parent`
    // chains (an independent layout reads Null) and every node starts
    // from the middleware locals — layout session mutations no longer
    // thread through the locals (concurrent loads cannot, and SSR does
    // not either), so a loader cookie's `before` baseline is the inbound
    // post-middleware session for every node.
    let mut nodes: Vec<LoadNode> = Vec::with_capacity(layout_module_paths.len() + 1);
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
        linput.locals = middleware_locals.clone();
        linput.locale = locale.to_string();
        // `parent_data` is threaded by the graph from the predecessor
        // slot when this layout's own verdict says it may call
        // `parent()`; an independent layout reads Null (§9.2: loads
        // that do not call parent() run concurrently).
        nodes.push(LoadNode {
            input: linput,
            needs_parent: spec.needs_parent,
        });
    }
    if let (Some(_runner), Some(mp), Some(rid)) = (loader_runner, module_path, route_id) {
        // The page node. The normalized id (no `/page` suffix) is what
        // user `load(event)` implementations expect to see.
        let route_for_loader = rid.strip_suffix("/page").unwrap_or(rid).to_string();
        let mut input = oxide_fw_server::LoaderRunInput::new(
            mp.to_string_lossy().to_string(),
            route_for_loader,
            &route_q,
        );
        input.method = req.method.clone();
        // §20 / G2: thread the inbound W3C trace so the loader's
        // server-side fetch calls join the request trace.
        input.trace = req.trace.as_ref().and_then(|t| {
            t.to_traceparent(&t.parent_id)
                .map(|tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}))
        });
        input.headers = req.headers.as_ref().clone();
        input.params = params.clone();
        input.locals = middleware_locals.clone();
        input.locale = locale.to_string();
        // The graph threads the innermost layout's data here when the
        // page's own verdict says it may call `parent()`; otherwise the
        // value stays Null (the page ran concurrently).
        nodes.push(LoadNode {
            input,
            needs_parent: page_needs_parent,
        });
    }
    let layout_count = layout_module_paths.len();
    // §16.6 / WP-D1: session cookie state. `layout_session_cookie` is the
    // LAST layout's mutation (the sequential chain overwrote it per
    // layout); `data_path_session_cookie` starts as that and the page —
    // the later mutation — overwrites it when it issues one.
    let mut layout_session_cookie: Option<String> = None;
    let mut data = serde_json::Value::Object(Default::default());
    // §7.1 / §9.2: resolved defer() values — the /__data envelope carries
    // them so the client router can resolve defer slots after a client-side
    // navigation (previously dropped: nav data diverged from SSR data).
    let mut deferred: std::collections::BTreeMap<String, serde_json::Value> =
        std::collections::BTreeMap::new();
    if let Some(runner) = loader_runner {
        // §20 / O3c: the waterfall's `overlap-saved` mark — the
        // counterfactual wall-clock the concurrent nodes would have cost
        // had they run sequentially (the sum of their own run times)
        // minus the actual wall. The same staged row `stage_loaders`
        // records for SSR, so the inspector reads one saving convention
        // across both legs. Recorded before the result is consumed —
        // the mark records even when a load short-circuited.
        let wall_start = std::time::Instant::now();
        let (result, outcomes) =
            run_load_graph_outcomes(std::sync::Arc::clone(runner), nodes, &[]).await;
        if let Some(rec) = waterfall
            && outcomes.elapsed_ms.len() > 1
        {
            let saved = (outcomes.elapsed_ms.iter().sum::<f64>()
                - wall_start.elapsed().as_secs_f64() * 1000.0)
                .max(0.0);
            rec.record_staged(
                &req.request_id,
                vec![(LOAD_OVERLAP_STAGE.to_string(), saved)],
                req.trace.as_ref().map(|t| t.trace_id.clone()),
            );
        }
        match result {
            // §9.2: the first short-circuit in input order wins — an
            // outer layout's redirect/error answers before the page's,
            // exactly like the sequential walk (substrate-pinned). The
            // cookie baseline is the inbound post-middleware session
            // (the substrate-authoritative shape; layout outputs before
            // the winner are not carried by the terminal result, the
            // same shape SSR's short-circuit arm consumes).
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
                // §16.6 / WP-D1 / WP-r10-1: a short-circuit that
                // also mutated `event.session` must persist it.
                // `middleware_locals["session"]` is the inbound
                // (post-middleware) session snapshot, so an untouched
                // loader does not re-issue a cookie.
                if let Some(cookie) = encode_session_cookie(
                    out.session.as_ref(),
                    middleware_locals.get("session"),
                    session_codec,
                ) {
                    resp = resp.with_header("set-cookie", cookie);
                }
                metrics.observe("_data", started.elapsed(), out.status);
                return resp;
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
                    route = %route_q,
                    "loader failed",
                );
                metrics.observe("_data", started.elapsed(), 500);
                let which = if index < layout_count {
                    "layout loader error"
                } else {
                    "loader error"
                };
                return oxide_fw_server::OxideResponse::html(
                    500,
                    format!("{which}: {}", oxide_fw_render::escape_text(&message)),
                );
            }
            LoadResult::Data(outs) => {
                // Layout session mutations persist outer-to-inner in
                // input order (§16.6 / WP-D1); the LAST layout's cookie
                // wins — the sequential chain's overwrite shape. The
                // `before` baseline is the inbound post-middleware
                // session for every layout (the graph runs them off the
                // middleware locals; SSR does the same).
                for out in outs.iter().take(layout_count) {
                    if let Some(cookie) = encode_session_cookie(
                        out.session.as_ref(),
                        middleware_locals.get("session"),
                        session_codec,
                    ) {
                        layout_session_cookie = Some(cookie);
                    }
                }
                // The page — the last node when it ran — carries the
                // envelope's data/deferred and OVERWRITES the cookie
                // state (the sequential arm's page-wins shape).
                if let Some(page_out) = outs.get(layout_count) {
                    data = page_out.data.clone();
                    deferred = page_out.deferred.clone();
                    if let Some(cookie) = encode_session_cookie(
                        page_out.session.as_ref(),
                        // §16.6 / WP-r9-3: the inbound post-middleware
                        // session as `before` — `middleware_locals["session"]`
                        // is set from the inbound cookie and refined by any
                        // middleware mutation, so it is the exact state the
                        // loader started with — an untouched loader matches
                        // it and no cookie is emitted.
                        middleware_locals.get("session"),
                        session_codec,
                    ) {
                        layout_session_cookie = Some(cookie);
                    }
                }
            }
        }
    }
    let data_path_session_cookie = layout_session_cookie;

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
    // P0-2 (the response-envelope side): the envelope is BOUNDED — the
    // twin of the request path's `max_body`/SRV-413 bound. An envelope
    // past `max_envelope` refuses with the minted `DSP-117` (a coded 500
    // naming the sizes) instead of riding the wire, and an encode refusal
    // (the §17.1 hardening: `ODF-SIZE`, `ODF-KEY`, `ODF-DEPTH`, …) is the
    // same coded 500 with the encoder's own code named — never the silent
    // empty-200 the `unwrap_or_default()` fall-through answered, which a
    // client decoder could only see as a corrupted navigation.
    let encoded = oxide_fw_odf::encode_to_bytes(&oxide_fw_odf::Value::from_json(&payload));
    let body = match encoded {
        Ok(bytes) if bytes.len() <= max_envelope => bytes,
        Ok(bytes) => {
            let actual = bytes.len();
            tracing::error!(
                code = "DSP-117",
                actual,
                limit = max_envelope,
                route = %route_q,
                "the route-data envelope exceeds the response bound",
            );
            metrics.observe("_data", started.elapsed(), 500);
            return oxide_fw_server::OxideResponse::html(
                500,
                format!(
                    "DSP-117: the route-data envelope encoded to {actual} bytes, past the \
                     {max_envelope} byte response bound — shrink the route's loader data or \
                     raise the server's max_data_envelope limit"
                ),
            );
        }
        Err(e) => {
            tracing::error!(
                code = "DSP-117",
                error = %e,
                route = %route_q,
                "the route-data envelope could not be encoded",
            );
            metrics.observe("_data", started.elapsed(), 500);
            return oxide_fw_server::OxideResponse::html(
                500,
                format!(
                    "DSP-117: the route data could not be encoded ({e}) — the ODF §17.1 \
                     hardening refused the payload; fix the loader's data shape"
                ),
            );
        }
    };
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

//! Moved verbatim from dispatch/entry.rs by scripts/build_stage_split.py
//! (V2-A01 phase 3).
//!
//! stage_prelude — the sync request prelude (spec WP-A01 phase 3,
//! /// the sketch's `stage_prelude` + its dev intercepts): per-request tree
//! /// load, boot-id read, rewrite-hop guard, the ESM dev intercept, the pool
//! /// gauge refresh, the §13.5 proxy arm, the F05 live endpoint, the F07
//! /// stream-reattach route. Every arm short-circuits through `Flow`.

use crate::*;
use oxide_fw_dispatch::flow::Flow;

// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
pub(crate) async fn stage_prelude(d: &DispatchDeps, cx: &mut RequestCtx) -> Flow {
    let req = &cx.req;
    let swappable = &d.swappable;
    // V2-B01: the tree and its context table for ONE generation (a single
    // read guard) — the stage tree consumes them as a matched pair.
    let (tree, ctx_table) = swappable.load_pair();
    let generations_for_render = &d.generations;
    let esm_state = &d.esm_state;
    let pool = d.pool.clone();
    let metrics = std::sync::Arc::clone(&d.metrics);
    let proxy_rules = &d.proxy_rules;
    let live = &d.live;
    let archived = &d.archived;
    let streams_for_dispatch = &d.streams;
    // <<< stage-wrapper
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
        return Flow::Respond(oxide_fw_server::OxideResponse::html(
            508,
            "middleware rewrite loop detected (too many hops)",
        ));
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
        match crate::esm_dev::handle_esm_request(esm, req) {
            Some(crate::esm_dev::EsmAction::Ready(resp)) => {
                return Flow::Respond(resp);
            }
            Some(crate::esm_dev::EsmAction::PoolModule(rel)) => {
                let esm = std::sync::Arc::clone(esm);
                let pool = pool.clone();
                let resp =
                    crate::esm_dev::serve_pool_compiled_module(&esm, &rel, pool.as_ref()).await;
                return Flow::Respond(resp);
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
        if let Some(m) = oxide_fw_dev::apply_proxy(proxy_rules, &raw, query.as_deref()) {
            let url = m.url.clone();
            let m_rule = m.rule.prefix.clone();
            let metrics_proxy = std::sync::Arc::clone(&metrics);
            let req_owned = req.clone();
            let resp = match oxide_fw_server::forward_proxy(&url, req_owned).await {
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
            };
            return Flow::Respond(resp);
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
        let not_accepted = |status: u16, msg: &'static str| -> oxide_fw_server::OxideResponse {
            oxide_fw_server::OxideResponse::html(status, msg)
        };
        if !req.method.eq_ignore_ascii_case("GET") {
            return Flow::Respond(not_accepted(405, "live: GET only"));
        }
        if !oxide_fw_server::live::same_origin(
            req.header("origin"),
            req.header("sec-fetch-site"),
            req.header("host"),
        ) {
            // §V2-F05 security: Origin check mandatory; missing evidence
            // denies (`same_origin`'s contract — curl/tests included).
            return Flow::Respond(not_accepted(
                403,
                "live: cross-origin connections are refused",
            ));
        }
        let query = req.url.split_once('?').map(|(_, q)| q).unwrap_or("");
        let Some(parsed) = oxide_fw_server::live::parse_live_query(query) else {
            return Flow::Respond(not_accepted(400, "live: missing route parameter"));
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
        // V2-B02: the gate reads the match through the unified view —
        // `is_live_route` reads both trees in place (the mmap entry for the
        // archive, the borrowed entry for the live tree), with no owned
        // `MatchResult` materialization on a path that only answers
        // "is this route opted in?".
        let matched = candidates.iter().find_map(|p| match archived {
            Some(a) => oxide_fw_router::MatchedView::archived(a, p),
            None => oxide_fw_router::MatchedView::live(&tree, p),
        });
        if !matched.as_ref().is_some_and(|m| m.is_live_route()) {
            return Flow::Respond(not_accepted(404, "live: no live route at that path"));
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
        return Flow::Respond(resp);
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
            return Flow::Respond(oxide_fw_server::OxideResponse::html(
                405,
                "stream reattach: GET only",
            ));
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
            return Flow::Respond(
                oxide_fw_server::OxideResponse::new(429, "stream reattach rate limit exceeded")
                    .with_header(
                        "retry-after",
                        crate::serve::streams::REATTACH_RATE_WINDOW_SECS.to_string(),
                    ),
            );
        }
        let offset = oxide_fw_server::stream_store::parse_range_offset(&req.headers);
        // §13.4: the reattach presents ITS OWN session identity (the same
        // wiring source hashes this request's cookie); the store maps a
        // mismatch — foreign or missing cookie — to the same `Gone` an
        // unknown id gets, so the 410 never confirms the id exists.
        let presented = sw.binding_of(&req.headers);
        let reattach = sw.store.get_bound(id, presented.as_ref());
        return Flow::Respond(match reattach {
            oxide_fw_server::stream_store::Reattach::Ok(buf) => {
                oxide_fw_server::stream_store::stream_resume_response(&buf, offset.unwrap_or(0))
            }
            oxide_fw_server::stream_store::Reattach::Gone => {
                oxide_fw_server::stream_store::stream_gone_response()
            }
        });
    }
    // >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
    cx.tree = tree;
    cx.ctx_table = Some(ctx_table);
    cx.boot_build_id = boot_build_id;
    cx.rewrite_hop = rewrite_hop;
    cx.raw = raw;
    Flow::Continue
    // <<< stage-wrapper
}

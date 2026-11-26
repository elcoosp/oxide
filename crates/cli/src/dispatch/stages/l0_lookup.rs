//! Moved verbatim from dispatch/entry.rs by scripts/build_stage_split.py
//! (V2-A01 phase 3).
//!
//! stage_l0_lookup — the §11.1 L0 cache-hit arm + PERF-04 single
//! /// flight (the sketch's `stage_l0_lookup`): the F12 stale-ceiling
//! /// lookup, 304 handling, P-05 stale-while-revalidate, the compressed
//! /// materialization and the follower coalescing.

use crate::*;
use oxide_fw_dispatch::flow::Flow;

// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
pub(crate) async fn stage_l0_lookup(d: &DispatchDeps, cx: &mut RequestCtx) -> Flow {
    let req_owned = &cx.req;
    let l0_eligible = cx.l0_eligible;
    let l0 = d.l0.clone();
    let brownout_ceiling = cx.brownout_ceiling;
    let l0_route = &cx.l0_route;
    let l0_params = &cx.l0_params;
    let l0_vary = &cx.l0_vary;
    let l0_path = cx.l0_path.clone();
    let flight_key = &cx.flight_key;
    let flight_map = d.flight_map.clone();
    let ppr_for_request = d.ppr_shells.clone();
    let middleware_headers = cx.middleware_headers.clone();
    let metrics = std::sync::Arc::clone(&d.metrics);
    // WP B03 item 2: borrow the label as `&str` (Deref coercion from the
    // `Arc<str>` field) — the metrics `observe(label, …)` takes `&str`.
    let label: &str = &cx.label;
    let started = cx.started;
    let request_nonce = &cx.request_nonce;
    let waterfall = cx.waterfall.clone();
    // <<< stage-wrapper
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
            Some(pct) => cache.get_brownout(l0_route, l0_params, l0_vary, pct).await,
            None => cache.get(l0_route, l0_params, l0_vary).await,
        }
    {
        // PERF-04 (the V2-F55 wire): the conditional decision rides the
        // cache crate's canonical seam — `serve_conditional` composes the
        // RFC 9110 §13 comparison (weak `If-None-Match` lists and `*`
        // first, `If-Modified-Since` against the stored `Last-Modified`
        // second — the arm the hand-rolled match never had) and the
        // §15.4.5 validator set, so the L0 plane no longer re-rolls the
        // semantics the F54 serving half's doc names as its evidence.
        let cond_headers: Vec<(String, String)> = ["if-none-match", "if-modified-since"]
            .into_iter()
            .filter_map(|name| {
                req_owned
                    .header(name)
                    .map(|value| (name.to_string(), value.to_string()))
            })
            .collect();
        if let oxide_fw_cache::ServeOutcome::NotModified { headers } =
            hit.serve_conditional(&cond_headers, l0_cache::now_secs())
        {
            let mut resp = oxide_fw_server::OxideResponse::new(304, "");
            // The canonical validator set, composed by the seam (one
            // header per name): the echoed ETag, the policy's
            // Cache-Control, the Date, and the Last-Modified the
            // decision compared against (RFC 9110 §15.4.5: a 304
            // carries what the 200 would have).
            for (k, v) in &headers {
                resp = resp.with_header(k, v.clone());
            }
            resp = resp.with_header("x-oxide-fw-cache", "L0-hit");
            // P0-5: middleware runs per request and its
            // headers must reflect THIS request, not the
            // request that stored the entry. Apply before
            // returning so an L0 hit carries them too.
            for (k, v) in &middleware_headers {
                resp = resp.with_header(k, v.clone());
            }
            metrics.observe(label, started.elapsed(), 304);
            if let Some(rec) = waterfall.as_ref() {
                rec.record(
                    &req_owned.request_id,
                    vec![(
                        "cache-hit".to_string(),
                        started.elapsed().as_secs_f64() * 1000.0,
                    )],
                );
            }
            return Flow::Respond(resp);
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
        // B09 budget repair: iterate the STORED headers directly — the old
        // `hit.headers()` cloned the whole Vec (and every pair) into an
        // intermediate that the `with_header` clone immediately discarded,
        // paying every pair twice on the serving hot path. The committed
        // `l0_alloc` dhat harness measures the hit's blocks before and
        // after; the entry is the shared `Arc<Entry>` the P-03 seam
        // serves, so the stored pair is the only source and the
        // response's own clone is the one it keeps.
        for (k, v) in hit.entry.headers.iter() {
            resp = resp.with_header(k, v.clone());
        }
        if let Some(enc) = &encoding {
            resp = resp
                .with_header("content-encoding", enc.clone())
                .with_header("vary", "Accept-Encoding");
        }
        // V2-F55: the stored etag IS the served header value (the
        // RFC-shaped weak form `make_entry` stores), so the 200 and the
        // 304 speak the same validator a client echoes back.
        resp = resp.with_header("etag", hit.etag());
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
        metrics.observe(label, started.elapsed(), hit.status());
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
        return Flow::Respond(resp);
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
            .acquire(&flight_map, flight_key)
            .await
        {
            FlightSlot::Follower(mut rx) => {
                if let Ok(Ok(entry)) =
                    tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await
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
                    metrics.observe(label, started.elapsed(), entry.status);
                    return Flow::Respond(resp);
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
    // >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
    cx.flight_owner = flight_owner;
    Flow::Continue
    // <<< stage-wrapper
}

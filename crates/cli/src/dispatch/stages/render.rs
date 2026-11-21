//! Moved verbatim from dispatch/entry.rs by scripts/build_stage_split.py
//! (V2-A01 phase 3).
//!
//! stage_render — the §7 render + finish pipeline (the sketch's
//! /// `stage_render` + `stage_finish` pair, merged because the finish work
//! /// (Server-Timing, header application, the F15 tee) is interleaved with
//! /// the response construction): the streamed render with the error-status
//! /// probe, ODF hydration payload, PPR shells, the L0 tee-and-store and
//! /// the placeholder arm. The LAST stage: returns `OxideResponse` directly
//! /// instead of `Flow` (there is nothing left to continue to).

use crate::*;

// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
pub(crate) async fn stage_render(
    d: &DispatchDeps,
    cx: &mut RequestCtx,
) -> oxide_fw_server::OxideResponse {
    let req_owned = &cx.req;
    let page_renderer = &cx.page_renderer;
    let route_id_for_render = &cx.route_id_for_render;
    let layout_ids = &cx.layout_ids;
    let unmatched_route_id = &cx.unmatched_route_id;
    let unmatched_layout_ids = &cx.unmatched_layout_ids;
    let params = cx.params.take().expect("the stage order sets params");
    let mut data = cx.data.take().expect("the stage order sets data");
    let mut action_form = cx.action_form.take();
    let mut page_settles = cx.page_settles.take();
    let route_meta = &cx.route_meta;
    let loader_set_cookies = cx
        .loader_set_cookies
        .take()
        .expect("the stage order sets loader_set_cookies");
    let loader_headers = cx
        .loader_headers
        .take()
        .expect("the stage order sets loader_headers");
    let deferred_values = &cx.deferred_values;
    let error_route_id = &cx.error_route_id;
    let resolved_locale = &cx.resolved_locale;
    let request_nonce = &cx.request_nonce;
    let middleware_headers = cx.middleware_headers.clone();
    let waterfall = cx.waterfall.clone();
    let boot_script_url_for_render = &cx.boot_script_url_for_render;
    let boot_script_integrity_for_render = &cx.boot_script_integrity_for_render;
    let ppr_for_request = d.ppr_shells.clone();
    let l0 = d.l0.clone();
    let l0_tiered = d.l0_tiered.clone();
    let l0_eligible = cx.l0_eligible;
    let l0_route = &cx.l0_route;
    let l0_params = &cx.l0_params;
    let l0_vary = &cx.l0_vary;
    let l0_path = &cx.l0_path;
    let flight_key = &cx.flight_key;
    let mut flight_owner = std::mem::take(&mut cx.flight_owner);
    let metrics = std::sync::Arc::clone(&d.metrics);
    // WP B03 item 2: borrow the label as `&str` (Deref coercion from the
    // `Arc<str>` field) so the tracing `%label` and the `{label}` body stamp
    // pay no clone and no per-request `String`.
    let label: &str = &cx.label;
    let status = cx.status;
    let stage_load_start = cx.stage_load_start;
    let boot_build_id = cx.boot_build_id.clone();
    let started = cx.started;
    let middleware_ms = cx.middleware_ms;
    let req_recording = &cx.req_recording;
    // <<< stage-wrapper
    // §7: render phase. Attached renderer wins; otherwise IR placeholder.
    // PERF-06: the loader phase is done; record its duration.
    let load_ms: f64 = stage_load_start.elapsed().as_secs_f64() * 1000.0;
    let stage_render_start = std::time::Instant::now();

    if let Some(renderer) = page_renderer.as_ref() {
        // §10.2 step 5: an unmatched URL is a 404 with the
        // project's `not-found.tsx` as the body, when one exists.
        // Matched routes resolve to their own id + layout chain;
        // only the no-match branch substitutes the not-found id.
        let (route_for_render, effective_layouts) = match route_id_for_render {
            Some(id) => (id.clone(), layout_ids.clone()),
            None => match unmatched_route_id {
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
            // Micro-debt (V2-B01 session): the old `.unwrap_or_default()`
            // silently answered an EMPTY envelope when the §17.1 encoder
            // refused the value (depth/size limits) — the client parsed a
            // corrupt-but-200 payload with no trace of the failure. The
            // empty fallback stays (the render must still answer); the
            // failure is loud now.
            match oxide_fw_odf::encode_to_bytes(&OdfValue::Object(obj)) {
                Ok(bytes) => bytes,
                Err(e) => {
                    tracing::warn!(
                        route = %route_for_render,
                        error = %e,
                        "loader-data envelope encode failed; answering an empty envelope"
                    );
                    bytes::Bytes::new()
                }
            }
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
                let (error_status, render_stream) = probe_render_error_status(render_stream).await;
                let final_status = if error_status == 500 { 500 } else { status };
                metrics.observe(label, started.elapsed(), final_status);
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
                    // WP B03 item 2: `label` is now `&str` (borrowed from the
                    // `Arc<str>` field); the recovery boundary + route_id the
                    // stream API takes as owned `String` get a `to_string`
                    // (same one allocation the prior `&String`'s `.clone()`
                    // paid — this is the rare mid-stream-error path).
                    Some(label.to_string()),
                    Some(request_nonce.clone()),
                    label.to_string(),
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
                // §V2-B03 item 3 (PERF-03): the route stage hashed the nonce once
                // per request into `cx.request_nonce`; re-deriving it re-hashed the
                // same request id — reuse the carried one.
                let nonce = request_nonce.clone();
                // PERF-01 / §8.4: the SAME `window.__oxide` ODF
                // payload prerendered pages embed, injected into
                // the live streamed shell (the sliding-window
                // injector above). Previously only write_prerender
                // set it, so a dynamic page's boot module found no
                // payload and skipped hydration + the router
                // entirely (`if (root && payload)` in boot.ts).
                let odf_tag = {
                    use base64::Engine as _;
                    let odf_b64 = base64::engine::general_purpose::STANDARD.encode(&odf_bytes);
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
                        _ => oxide_fw_render::hydration_bootstrap(&odf_b64, &route_for_odf, &nonce),
                    }
                };
                // §V2-F09: the boot's search-mode flag — the client
                // matcher (`packages/core/src/client/searchEngine.ts`)
                // queries `/_oxide/search` when the engine serves. The
                // script rides the same `</body>` injection as the
                // bootstrap; unconfigured (`None`, the inert default)
                // emits nothing and every byte stays pre-F09 identical.
                let search_flag_tag = match d.search_flag {
                    Some(mode) => oxide_fw_render::boot_search_flag_script(mode, &nonce),
                    None => String::new(),
                };
                // §7.1 / §9.2: resolved defer() values ride a JSON
                // script element so the boot module can dispatch
                // `ox:swap` for each. Serialized through serde_json
                // (no untrusted string concatenation) and `<` escaped
                // so the content cannot break out of the tag.
                let deferred_tag = if deferred_values.is_empty() {
                    String::new()
                } else {
                    let json =
                        serde_json::to_string(deferred_values).unwrap_or_else(|_| "{}".into());
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
                            "{odf_tag}{deferred_tag}{hmr_tag}{search_flag_tag}{}",
                            boot_script_tag(
                                url,
                                &nonce,
                                boot_script_integrity_for_render.as_deref(),
                            ),
                        );
                        oxide_fw_server::inject_before_close(bridged, b"</body>", tag.as_bytes())
                    }
                    None => {
                        let extra = format!("{odf_tag}{deferred_tag}{hmr_tag}{search_flag_tag}");
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
                            let ppr_build_id = l0.as_ref().map(|c| c.build_id()).unwrap_or("dev");
                            let pkey = l0_cache::L0Cache::key_for(
                                l0_route,
                                l0_params,
                                l0_vary,
                                ppr_build_id,
                            );
                            let ppr_tags = vec![l0_route.clone(), format!("path:{l0_path}")];
                            // Poison recovery (micro-debt, V2-F47): the
                            // guard's critical section is one plain map
                            // read — no invariant can break inside it —
                            // so a poisoned lock hands the guard back
                            // and the shell-cache hit still lands. The
                            // old `.ok()` shape silently missed the hit
                            // on a poisoned lock.
                            let cached = shells
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .get(&pkey)
                                .cloned();
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
                    // §V2-B03 item 3 (PERF-03): the route stage hashed the nonce once
                    // per request into `cx.request_nonce`; re-deriving it re-hashed the
                    // same request id — reuse the carried one.
                    let nonce = request_nonce.clone();
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
                            // F36: the publish count IS the coalesce row's
                            // join number — the owner records it through
                            // the typed half (the F33 emitter's live call
                            // site; the count this site used to name as
                            // dropped).
                            let joins = guard.publish(std::sync::Arc::new(entry));
                            if let Some(tier) = l0_tiered.as_ref() {
                                tier.record_coalesced(
                                    &store_key,
                                    u64::try_from(joins).unwrap_or(u64::MAX),
                                );
                            }
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
                    let mut resp = oxide_fw_server::OxideResponse::stream(final_status, stream)
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
                metrics.observe(label, started.elapsed(), 500);
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
        metrics.observe(label, started.elapsed(), status);
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
}

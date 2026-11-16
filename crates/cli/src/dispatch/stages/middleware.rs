//! Moved verbatim from dispatch/entry.rs by scripts/build_stage_split.py
//! (V2-A01 phase 3).
//!
//! stage_middleware — the §8.5 ancestor middleware chain (the sketch's
//! `stage_middleware`): session decode, B-22 resume, the V2-A05
//! `oxide_fw_dispatch::chain::run_chain` walk (the substrate the inline
//! layer loop was adopted from), rewrite re-dispatch, B-05 session
//! write-back, PERF-06 timing.
//!
//! V2-A05 adoption (this file deliberately diverges from the verbatim
//! split; recorded in `scripts/split_selfcheck.py` STAGE_ADOPTED and
//! `docs/deviations.md`): the ~200-line inline dispatcher walk became the
//! tested V8-free substrate call, keeping the request-shaped prelude
//! (session decode) and the response-shaped postlude (location header,
//! session write-back, re-dispatch) — exactly what the substrate leaves to
//! the caller.

use crate::*;
use oxide_fw_dispatch::chain::{ChainOutcome, ChainRequest, run_chain};
use oxide_fw_dispatch::flow::Flow;

// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
pub(crate) async fn stage_middleware(d: &DispatchDeps, cx: &mut RequestCtx) -> Flow {
    let req_owned = &cx.req;
    let metrics = std::sync::Arc::clone(&d.metrics);
    let label = cx.label.clone();
    let params = cx.params.as_ref().expect("the stage order sets params");
    let middleware_module_paths = cx.middleware_module_paths.clone();
    let endpoint_body = cx.endpoint_body.clone();
    let self_dispatch_for_closure = std::sync::Arc::clone(&d.self_dispatch);
    let session_codec = d.session_codec.clone();
    let middleware_for_dispatch = d.middleware_dispatcher.clone();
    let rewrite_hop = cx.rewrite_hop;
    let started = cx.started;
    // <<< stage-wrapper
    // §8.1 middleware.ts / §8.5: run the route's FULL ancestor
    // middleware chain, OUTERMOST first, BEFORE the loader/render
    // phase — and before the /__data branch (§8.4 parity: client
    // nav data must see the same middleware locals/headers/
    // short-circuits as SSR). Each middleware can:
    //   * short-circuit with a `Response` or `{ redirect }`;
    //   * inject `locals` (merged later-wins into the accumulated
    //     map, delivered to the next middleware AND the loader via
    //     `LoaderRunInput.locals`);
    //   * add response headers (accumulated, last-writer-wins per
    //     case-insensitive name — the CLI's B-06 semantics, the
    //     substrate's `merge_headers`).
    // A `{ rewrite }` re-dispatches through the normal path. An
    // error surfaces as a 500 with a `MIDDLEWARE`-coded envelope.
    //
    // V2-A05: the walk itself is `oxide_fw_dispatch::chain::run_chain`
    // (pinned by `crates/dispatch/tests/chain_tdd.rs` — outer-to-inner
    // order, later-wins locals, last-writer-wins headers,
    // short-circuit / redirect / rewrite stop the walk, errors
    // propagate verbatim, and a B-22 marker naming a layer absent
    // from this route's chain runs the chain from the top).
    let stage_middleware_start = std::time::Instant::now();
    // §16.6: decode the encrypted session cookie into
    // `locals.session` so a loader/action reads it as an object. A
    // missing or tampered cookie is an empty session, never an error.
    // B-05: the sealed token rides the `Cookie` header, never a
    // header named `oxide_session`; read it through the cookie parser.
    let mut middleware_locals: serde_json::Value = serde_json::json!({});
    if let Some(codec) = session_codec.as_deref()
        && let Some(token) = req_owned
            .header("cookie")
            .and_then(|h| oxide_fw_server::cookie_value(h, "oxide_session"))
        && let Ok(sess) = oxide_fw_server::Session::decode(codec, token)
    {
        middleware_locals["session"] = sess.to_value();
    }
    // B-05: baseline for the session write-back. Middleware is the
    // only phase that can refine `locals.session` (the loader gets a
    // deserialized copy), so diffing around it captures every change.
    let session_before = middleware_locals.get("session").cloned();
    // B-22: resume after the layer that issued the rewrite.
    let resume_after = req_owned.mw_resume_after.clone();
    let Some(dispatcher) = middleware_for_dispatch.as_deref() else {
        // No middleware dispatcher attached (the test-pipeline
        // default): the decoded session still flows downstream and
        // the write-back runs (a no-op without a codec).
        return finish_middleware_stage(
            cx,
            middleware_locals,
            Vec::new(),
            stage_middleware_start,
            &session_codec,
            session_before.as_ref(),
        );
    };
    // P-07b: the JSON param map is built on first middleware/endpoint
    // use — a page request reaching no dispatcher skips the per-param
    // allocations entirely. `run_chain` threads the same map to every
    // layer (the loop reused one map before).
    let params_json = cx
        .endpoint_params_json
        .get_or_insert_with(|| build_params_json(params));
    let outcome = run_chain(
        dispatcher,
        &middleware_module_paths,
        ChainRequest {
            method: &req_owned.method,
            url: &req_owned.url,
            params: params_json,
            headers: req_owned.headers.as_slice(),
            body: endpoint_body.as_ref(),
        },
        middleware_locals,
        resume_after.as_deref(),
    )
    .await;
    match outcome {
        Err(e) => {
            metrics.observe(&label, started.elapsed(), e.status);
            return Flow::Respond(oxide_fw_server::OxideResponse::json(
                e.status,
                serde_json::json!({"error": {
                    "code": e.code,
                    "message": e.message,
                    "route": label,
                }}),
            ));
        }
        // A middleware answered (short-circuit, or §8.5 redirect —
        // the substrate stops the walk on `{ short: true, status,
        // redirect }` per spec, so a redirect-only answer is honored
        // here exactly like the page one).
        Ok(ChainOutcome::Respond(out)) => {
            metrics.observe(&label, started.elapsed(), out.status);
            let mut resp =
                oxide_fw_server::OxideResponse::new(out.status, bytes::Bytes::from(out.body));
            for (k, v) in &out.headers {
                resp = resp.with_header(k, v.clone());
            }
            // §8.5: the worker returns a middleware redirect as a
            // sibling `redirect` field (`{ short: true, status,
            // redirect }`), not as a `location` header. Without this
            // the browser got a 302 with no destination.
            if let Some(loc) = out.redirect.as_deref()
                && !resp
                    .headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("location"))
            {
                resp = resp.with_header("location", loc);
            }
            // §16.6 / WP-r10-1 / B-05: a middleware that mutates
            // `locals.session` AND answers must persist the mutation.
            // The post-chain write-back (below) never runs for an
            // answer, so this return path is the only place the cookie
            // can ride.
            if let Some(codec) = session_codec.as_deref() {
                let after = out.locals.as_ref().and_then(|l| l.get("session")).cloned();
                if after.is_some() && after != session_before {
                    let mut sess = oxide_fw_server::Session::new();
                    if let Some(obj) = after.as_ref().and_then(|v| v.as_object()) {
                        for (k, v) in obj {
                            sess.insert(k.clone(), v.clone());
                        }
                    }
                    if let Ok(token) = sess.encode(codec) {
                        resp = resp.with_header(
                            "set-cookie",
                            format!(
                                "oxide_session={token}; Path=/; HttpOnly; SameSite=Lax; \
                                 Max-Age={}",
                                session_ttl_secs()
                            )
                            .replace("  ", " "),
                        );
                    }
                }
            }
            return Flow::Respond(resp);
        }
        // §8.5: honour the rewrite by re-dispatching the request to
        // the new URL through the normal match → loader → render
        // path. The rewrite-hop header prevents an infinite loop when
        // a middleware always rewrites. The chain ended at the
        // rewriting layer, so the re-dispatch carries the B-22 resume
        // marker; the layers before the marker do not run twice.
        Ok(ChainOutcome::Rewrite {
            to,
            locals,
            headers,
            resume_after,
        }) => {
            if let Some(re_dispatch) = self_dispatch_for_closure.get() {
                let mut new_req = req_owned.clone();
                new_req.url = to;
                // S-20: bump the internal counter, not a header, so it
                // cannot be forged.
                new_req.rewrite_hops = (rewrite_hop + 1) as u8;
                new_req.mw_resume_after = resume_after;
                return Flow::Respond(re_dispatch(new_req).await);
            }
            tracing::warn!(
                route = %label,
                rewrite = %to,
                "middleware rewrite: self-dispatch unavailable; serving with the chain's \
                 accumulated state",
            );
            // Unreachable in production (the dispatch wrapper
            // publishes the closure before the first request): the
            // rewrite target is ignored and the chain's accumulated
            // locals/headers flow to the loader/render phases.
            finish_middleware_stage(
                cx,
                locals,
                headers,
                stage_middleware_start,
                &session_codec,
                session_before.as_ref(),
            )
        }
        Ok(ChainOutcome::Continue { locals, headers }) => finish_middleware_stage(
            cx,
            locals,
            headers,
            stage_middleware_start,
            &session_codec,
            session_before.as_ref(),
        ),
    }
}

/// The Continue-path postlude: B-05 session write-back, PERF-06 timing,
/// the F15 record capture and the context writes. Shared by the
/// no-dispatcher early return, the unreachable rewrite fallback and the
/// chain's Continue outcome so the three paths cannot drift.
fn finish_middleware_stage(
    cx: &mut RequestCtx,
    middleware_locals: serde_json::Value,
    mut middleware_headers: Vec<(String, String)>,
    stage_middleware_start: std::time::Instant,
    session_codec: &Option<std::sync::Arc<oxide_fw_server::SessionCodec>>,
    session_before: Option<&serde_json::Value>,
) -> Flow {
    // B-05: write-back. If a middleware layer changed
    // `locals.session`, seal it and ride a `Set-Cookie` on every
    // response path via `middleware_headers`.
    if let Some(codec) = session_codec.as_deref() {
        let after = middleware_locals.get("session").cloned();
        if after.as_ref() != session_before && after.is_some() {
            let mut sess = oxide_fw_server::Session::new();
            if let Some(obj) = after.as_ref().and_then(|v| v.as_object()) {
                for (k, v) in obj {
                    sess.insert(k.clone(), v.clone());
                }
            }
            match sess.encode(codec) {
                Ok(token) => middleware_headers.push((
                    "set-cookie".to_string(),
                    format!(
                        "oxide_session={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
                        session_ttl_secs()
                    ),
                )),
                Err(e) => tracing::warn!(
                    error = %e.0.message,
                    "session encode failed; cookie not set",
                ),
            }
        }
    }
    // PERF-06: the middleware phase is done — record its duration
    // so `Server-Timing` reports the real cost (§12.1 step 8).
    let middleware_ms = stage_middleware_start.elapsed().as_secs_f64() * 1000.0;
    // V2-F15: the middleware locals are final — the record captures
    // them (the loader's threaded copy rides the loader inputs).
    if let Some(rec) = cx.req_recording.as_ref() {
        rec.set_locals(&middleware_locals);
    }
    cx.middleware_locals = middleware_locals;
    cx.middleware_headers = middleware_headers;
    cx.middleware_ms = middleware_ms;
    cx.session_before = session_before.cloned();
    Flow::Continue
}

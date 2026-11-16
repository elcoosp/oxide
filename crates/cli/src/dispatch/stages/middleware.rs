//! Moved verbatim from dispatch/entry.rs by scripts/build_stage_split.py
//! (V2-A01 phase 3).
//!
//! stage_middleware — the §8.5 ancestor middleware chain (the
//! /// sketch's `stage_middleware`): session decode, B-22 resume, the
//! /// dispatcher walk with locals/headers merge, rewrite re-dispatch,
//! /// B-05 session write-back, PERF-06 timing.

use crate::*;
use oxide_fw_dispatch::flow::Flow;

// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
pub(crate) async fn stage_middleware(d: &DispatchDeps, cx: &mut RequestCtx) -> Flow {
    let req_owned = &cx.req;
    let metrics = std::sync::Arc::clone(&d.metrics);
    let label = &cx.label;
    let params = cx.params.as_ref().expect("the stage order sets params");
    let middleware_module_paths = cx.middleware_module_paths.clone();
    let endpoint_params_json = &mut cx.endpoint_params_json;
    let endpoint_body = &cx.endpoint_body;
    let self_dispatch_for_closure = std::sync::Arc::clone(&d.self_dispatch);
    let session_codec = d.session_codec.clone();
    let middleware_for_dispatch = d.middleware_dispatcher.clone();
    let rewrite_hop = cx.rewrite_hop;
    let started = cx.started;
    let req_recording = &cx.req_recording;
    // <<< stage-wrapper
    // §8.1 middleware.ts / §8.5: run the route's FULL ancestor
    // middleware chain, OUTERMOST first, BEFORE the loader/render
    // phase — and before the /__data branch (§8.4 parity: client
    // nav data must see the same middleware locals/headers/
    // short-circuits as SSR). Each middleware can:
    //   * short-circuit with a `Response` or `{ redirect }`;
    //   * inject `locals` (merged into the accumulated map,
    //     delivered to the next middleware AND the loader via
    //     `LoaderRunInput.locals`);
    //   * add response headers (accumulated).
    // A `{ rewrite }` re-dispatches through the normal path. An
    // error surfaces as a 500 with a `MIDDLEWARE`-coded envelope.
    let stage_middleware_start = std::time::Instant::now();
    let mut middleware_locals: serde_json::Value = serde_json::json!({});
    // §16.6: decode the encrypted session cookie into
    // `locals.session` so a loader/action reads it as an object. A
    // missing or tampered cookie is an empty session, never an error.
    // B-05: the sealed token rides the `Cookie` header, never a
    // header named `oxide_session`; read it through the cookie parser.
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
    let mut middleware_headers: Vec<(String, String)> = Vec::new();
    // B-22: resume after the layer that issued the rewrite.
    let resume_after = req_owned.mw_resume_after.clone();
    let mut pending_resume = resume_after.as_ref().and_then(|_| {
        // Only honour the marker when the target layer is present in
        // this route's chain; otherwise run the chain normally.
        let present = middleware_module_paths
            .iter()
            .any(|p| p.to_string_lossy() == resume_after.as_deref().unwrap_or(""));
        present.then(|| resume_after.clone().unwrap_or_default())
    });
    if let Some(dispatcher) = middleware_for_dispatch.as_deref() {
        for mp in &middleware_module_paths {
            let module_path_s = mp.to_string_lossy().to_string();
            if let Some(target) = pending_resume.as_ref() {
                if &module_path_s == target {
                    // The rewriting layer itself must not run twice.
                    pending_resume = None;
                }
                continue;
            }
            // Pass the full request (headers + body) so an auth /
            // upload middleware sees what it needs, plus the
            // locals accumulated by outer middlewares (None when
            // nothing has accumulated — see the trait doc).
            let locals_for_call: Option<&serde_json::Value> =
                if middleware_locals.as_object().is_some_and(|o| !o.is_empty()) {
                    Some(&middleware_locals)
                } else {
                    None
                };
            let mw_res = dispatcher
                .dispatch(MiddlewareDispatchArgs {
                    module_path: &module_path_s,
                    method: &req_owned.method,
                    url: &req_owned.url,
                    params: endpoint_params_json.get_or_insert_with(|| build_params_json(params)),
                    headers: &req_owned.headers,
                    body: endpoint_body.as_ref(),
                    locals: locals_for_call,
                })
                .await;
            match mw_res {
                Ok(out) => {
                    if out.short {
                        metrics.observe(label, started.elapsed(), out.status);
                        let mut resp = oxide_fw_server::OxideResponse::new(
                            out.status,
                            bytes::Bytes::from(out.body),
                        );
                        for (k, v) in &out.headers {
                            resp = resp.with_header(k, v.clone());
                        }
                        // §8.5: the worker returns a middleware
                        // redirect as a sibling `redirect` field
                        // (`{ short: true, status, redirect }`), not
                        // as a `location` header. Without this the
                        // browser got a 302 with no destination.
                        if let Some(loc) = out.redirect.as_deref()
                            && !resp
                                .headers
                                .iter()
                                .any(|(k, _)| k.eq_ignore_ascii_case("location"))
                        {
                            resp = resp.with_header("location", loc);
                        }
                        // §16.6 / WP-r10-1 / B-05: a middleware that
                        // mutates `locals.session` AND short-circuits
                        // must persist the mutation. The post-loop
                        // write-back (below) never runs for a
                        // short-circuit, so this return path is the
                        // only place the cookie can ride.
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
                                            "oxide_session={token}; Path=/; \
                                                 HttpOnly; SameSite=Lax; Max-Age={}",
                                            session_ttl_secs()
                                        )
                                        .replace(
                                            "\
", "",
                                        )
                                        .replace("  ", " "),
                                    );
                                }
                            }
                        }
                        return Flow::Respond(resp);
                    }
                    // B-06: last-writer-wins per header name — an
                    // inner middleware refines what an outer one
                    // set, matching the locals merge below. Appending
                    // both produced duplicate header lines.
                    for (k, v) in &out.headers {
                        let lower = k.to_ascii_lowercase();
                        match middleware_headers
                            .iter_mut()
                            .find(|(ek, _)| ek.eq_ignore_ascii_case(&lower))
                        {
                            Some((_, ev)) => *ev = v.clone(),
                            None => middleware_headers.push((k.clone(), v.clone())),
                        }
                    }
                    // P-08: consume the locals by value — the merge
                    // used to deep-clone every key/value per hop.
                    if let Some(serde_json::Value::Object(src)) = out.locals {
                        let dst = middleware_locals
                            .as_object_mut()
                            .expect("middleware_locals is always an object");
                        for (k, v) in src {
                            dst.insert(k, v);
                        }
                    }
                    if let Some(rw) = out.rewrite.as_deref() {
                        // §8.5: honour the rewrite by re-dispatching
                        // the request to the new URL through the
                        // normal match → loader → render path. The
                        // rewrite-hop header prevents an infinite
                        // loop when a middleware always rewrites.
                        if let Some(re_dispatch) = self_dispatch_for_closure.get() {
                            let mut new_req = req_owned.clone();
                            new_req.url = rw.to_string();
                            // S-20: bump the internal counter, not a
                            // header, so it cannot be forged.
                            new_req.rewrite_hops = (rewrite_hop + 1) as u8;
                            new_req.mw_resume_after = Some(module_path_s.clone());
                            return Flow::Respond(re_dispatch(new_req).await);
                        }
                        tracing::warn!(
                            route = %label,
                            rewrite = %rw,
                            "middleware rewrite: self-dispatch unavailable",
                        );
                    }
                }
                Err(e) => {
                    metrics.observe(label, started.elapsed(), e.status);
                    return Flow::Respond(oxide_fw_server::OxideResponse::json(
                        e.status,
                        serde_json::json!({"error": {
                        "code": e.code,
                        "message": e.message,
                        "route": label,
                        }}),
                    ));
                }
            }
        }
    }
    // B-05: write-back. If a middleware layer changed
    // `locals.session`, seal it and ride a `Set-Cookie` on every
    // response path via `middleware_headers`.
    if let Some(codec) = session_codec.as_deref() {
        let after = middleware_locals.get("session").cloned();
        if after.is_some() && after != session_before {
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
    if let Some(rec) = req_recording.as_ref() {
        rec.set_locals(&middleware_locals);
    }
    // >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
    cx.middleware_locals = middleware_locals;
    cx.middleware_headers = middleware_headers;
    cx.middleware_ms = middleware_ms;
    cx.session_before = session_before;
    Flow::Continue
    // <<< stage-wrapper
}

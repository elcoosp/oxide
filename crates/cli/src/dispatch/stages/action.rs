//! Moved verbatim from dispatch/entry.rs by scripts/build_stage_split.py
//! (V2-A01 phase 3).
//!
//! stage_action — the §9.3 form action arm (the sketch's
//! /// `stage_action`): run_action, tag/path invalidations, the PPR/L0
//! /// invalidation fan-out, B6 cookie forwarding, the 200 fall-through.

use crate::*;
use oxide_fw_dispatch::flow::Flow;

// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
pub(crate) async fn stage_action(d: &DispatchDeps, cx: &mut RequestCtx) -> Flow {
    let req_owned = &cx.req;
    let action_name = &cx.action_name;
    let module_path = &cx.module_path;
    let action_dispatcher = &cx.action_dispatcher;
    let action_params = &cx.action_params;
    let action_route = &cx.action_route;
    let middleware_locals = &cx.middleware_locals;
    let middleware_headers = &mut cx.middleware_headers;
    let waterfall = cx.waterfall.clone();
    let l0 = d.l0.clone();
    let ppr_for_request = d.ppr_shells.clone();
    let live_for_request = d.live.clone();
    let metrics = std::sync::Arc::clone(&d.metrics);
    let label = &cx.label;
    let started = cx.started;
    let middleware_ms = cx.middleware_ms;
    // <<< stage-wrapper
    // §9.3: a form action POST runs the route's `actions[name]`.
    // A successful action does NOT return its JSON to the browser —
    // the spec's form-first contract is "runs, and re-renders the
    // page with `form` result data". The result is carried into the
    // render phase through this variable; a redirect or error status
    // still returns directly (POST/redirect/GET, and a failed action
    // must not render a success page).
    let mut action_form: Option<serde_json::Value> = None;
    if let (Some(action), Some(module_path)) = (action_name.as_deref(), module_path.as_deref())
        && let Some(dispatcher) = action_dispatcher.as_deref()
    {
        let form_json = {
            // §V2-A02: the ONE definition site is the dispatch crate's url
            // module (the CLI's duplicate copy is deleted with this diff).
            let entries = oxide_fw_dispatch::url::parse_form_urlencoded(&req_owned.body);
            serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string())
        };
        let action_trace = req_owned.trace.as_ref().and_then(|t| {
            t.to_traceparent(&t.parent_id)
                .map(|tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}))
        });
        let resp = oxide_fw_server::run_action(RunActionArgs {
            dispatcher: Some(dispatcher),
            module_path: &module_path.to_string_lossy(),
            route: action_route,
            action,
            form_json: &form_json,
            params: action_params,
            url: &req_owned.url,
            method: &req_owned.method,
            headers: &req_owned.headers,
            locals: middleware_locals,
            trace: action_trace.as_ref(),
        })
        .await;
        // B-17: actions get their own waterfall row so the
        // inspector shows form-POST latency next to renders.
        if let Some(rec) = waterfall.as_ref() {
            let total = started.elapsed().as_secs_f64() * 1000.0;
            rec.record_with_trace(
                &req_owned.request_id,
                vec![
                    ("middleware".to_string(), middleware_ms),
                    ("action".to_string(), (total - middleware_ms).max(0.0)),
                ],
                req_owned.trace.as_ref().map(|t| t.trace_id.clone()),
            );
        }
        // §11.3: apply any tag/path invalidations the action
        // asked for, then return the response.
        if let Some(rev) = resp.header("x-oxide-revalidate") {
            let events: Vec<(String, String)> = rev
                .split(',')
                .filter_map(|e| {
                    e.split_once(':')
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                })
                .collect();
            if let Some(cache) = l0.as_ref() {
                // BUG-01: before this fix, `tag` events were
                // dropped and `path` events were passed to
                // `invalidate_tag` raw (no `path:` prefix), so
                // neither ever matched a cached entry.
                for (kind, value) in &events {
                    match kind.as_str() {
                        "tag" => {
                            cache.invalidate_tag(value, true).await;
                            // V2-F05 step 6: the revalidation hook —
                            // every live connection that depends on
                            // the tag re-runs its loaders and pushes
                            // the new snapshot to its tab.
                            if let Some(live) = live_for_request.as_ref() {
                                live.notify_tag(value);
                            }
                        }
                        "path" => {
                            cache.invalidate_path(value, true).await;
                            // The `path` spelling fans out under the
                            // `path:<url>` tag (`serve::live`).
                            if let Some(live) = live_for_request.as_ref() {
                                live.notify_path(value);
                            }
                        }
                        _ => {}
                    }
                }
            }
            // PPR shells carry the same tags; drop them with the
            // L0 entries so a revalidate action cannot leave a
            // stale shell serving the old document.
            if let Some(shells) = ppr_for_request.as_ref() {
                for (kind, value) in &events {
                    if let Ok(mut store) = shells.lock() {
                        match kind.as_str() {
                            "tag" => {
                                store.invalidate_tag(value);
                            }
                            "path" => {
                                store.invalidate_path(value);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        // §8.5: headers the middleware phase added ride the
        // action response (they were accumulated but never
        // applied — the previous drop made a CSP/security
        // header set in middleware vanish on POSTs).
        let mut resp = resp;
        for (k, v) in middleware_headers.iter() {
            resp = resp.with_header(k, v.clone());
        }
        // §9.3: 200 → carry the result into the render and fall
        // through. Anything else (a redirect, a validation
        // failure) is the action's own answer and returns.
        if resp.status == 200 {
            // B6 fix: copy the action's Set-Cookie headers into
            // middleware_headers before the response is dropped —
            // the render path applies middleware_headers to every
            // response it produces, so the cookie rides the
            // re-rendered response. Non-200 responses return `resp`
            // directly (they already carry the headers).
            forward_action_set_cookies(&resp.headers, middleware_headers);
            action_form = serde_json::from_slice::<serde_json::Value>(&resp.body).ok();
        } else {
            metrics.observe(label, started.elapsed(), resp.status);
            return Flow::Respond(resp);
        }
    }
    // >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
    cx.action_form = action_form;
    Flow::Continue
    // <<< stage-wrapper
}

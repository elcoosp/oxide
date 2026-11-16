//! Moved verbatim from dispatch/entry.rs by scripts/build_stage_split.py
//! (V2-A01 phase 3).
//!
//! stage_match — path normalization + route match (the sketch's
//! /// `stage_match`): trailing-slash fold, BUG-01 path tag, §16.4 locale
//! /// resolution, the PERF-01 `/__data` target detection, candidate match
//! /// over the archived or live tree, status/label.

use crate::*;
use oxide_fw_dispatch::flow::Flow;
use oxide_fw_router::MatchedView;

// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
pub(crate) async fn stage_match(d: &DispatchDeps, cx: &mut RequestCtx) -> Flow {
    let req = &cx.req;
    let i18n_cfg = &d.i18n_cfg;
    let archived = &d.archived;
    let tree = &cx.tree;
    let raw = cx.raw.clone();
    // <<< stage-wrapper
    // Route conventions: the root page module is `index`, so `/` maps to
    // the `/index` pattern (spec §8.1); trailing slashes are folded too.
    let normalized = if raw.len() > 1 && raw.ends_with('/') {
        raw.trim_end_matches('/').to_string()
    } else {
        raw
    };
    // §11.3 / BUG-01: remember the request path. The L0 entry for
    // this request is tagged `path:<req_path>` so a `revalidate.path`
    // event (which reaches the dispatch via `x-oxide-revalidate`)
    // actually invalidates it. Without this, entries are tagged with
    // the *route id* only, and path events match nothing.
    let req_path = normalized.clone();
    // SPEC-10 / §16.4: resolve the request locale when the project
    // configures i18n — path prefix first, then `Accept-Language`,
    // then the default. Empty when no i18n is configured.
    // `i18n_cfg` is captured by the dispatch closure (built from config
    // before any request); `req` supplies the header.
    let resolved_locale: String = match i18n_cfg.as_ref() {
        Some(a) => {
            let stripped = oxide_fw_assets::i18n::strip_locale_prefix(a, &req_path);
            let from_path = stripped.0 != a.default && stripped.1 != req_path.as_str();
            if from_path {
                stripped.0
            } else {
                oxide_fw_assets::i18n::negotiate(a, req.header("accept-language").unwrap_or(""))
            }
        }
        None => String::new(),
    };
    // PERF-01 / §8.4: a `/__data` GET names its target route via
    // `?route=<path>`. The endpoint re-matches THAT path against the
    // route tree (below), so the whole prelude — route id, params,
    // layout chain, nearest middleware, error boundary — describes
    // the route whose data the client router is fetching. That is
    // what makes client-side navigation data match the SSR render
    // (§8.4 parity); previously /__data matched nothing, ran no
    // middleware, no layout loads, and no defer resolution.
    let data_route_q: String = if req.method.eq_ignore_ascii_case("GET") && req_path == "/__data" {
        let raw_q = req.url.split_once('?').map(|(_, q)| q).unwrap_or("");
        raw_q
            .split('&')
            .find_map(|kv| {
                kv.strip_prefix("route=")
                    .map(oxide_fw_server::percent_decode)
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/".to_string())
    } else {
        String::new()
    };
    let is_data_request = !data_route_q.is_empty();
    // §8.4: for a data request, match the TARGET route (trailing
    // slash folded like a real URL path); otherwise the request path.
    let match_path = if is_data_request {
        let stripped = data_route_q.trim_end_matches('/');
        if stripped.is_empty() {
            "/".to_string()
        } else {
            stripped.to_string()
        }
    } else {
        normalized.clone()
    };
    let candidates = if match_path == "/" {
        vec!["/".to_string(), "/index".to_string()]
    } else {
        vec![match_path]
    };
    // V2-B02: the match goes through the unified view seam — one shape
    // over `RouteTree::match_ref` (borrowed entry) and
    // `ArchivedTree::match_ref` (index into the mmap), reached through the
    // candidate index on both trees. `into_match_result` produces the
    // byte-identical owned shape the stages consume; the prelude's live
    // gate reads `is_live_route` in place with no materialization.
    let matched: Option<oxide_fw_router::MatchResult> = candidates.iter().find_map(|p| {
        match archived {
            Some(a) => MatchedView::archived(a, p),
            None => MatchedView::live(tree, p),
        }
        .map(|v| v.into_match_result())
    });
    // V2-B01: the per-route context is one table hit — the metrics label
    // is the precomputed pattern (no per-request `pattern_of` walk), and
    // the route step reads the chain answers from the same hit.
    let route_ctx: Option<std::sync::Arc<oxide_fw_router::RouteContext>> =
        match cx.ctx_table.as_ref() {
            Some(table) => match &matched {
                Some(m) => table.get(&m.route.id.0).cloned(),
                None => table.unmatched().cloned(),
            },
            None => None,
        };
    let (status, label) = match (&matched, &route_ctx) {
        (Some(_), Some(rc)) => (200u16, rc.pattern.to_string()),
        (Some(m), None) => (200u16, pattern_of(&m.route.segments)),
        (None, _) => (404u16, "_unmatched".to_string()),
    };
    // >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
    cx.normalized = normalized;
    cx.req_path = req_path;
    cx.resolved_locale = resolved_locale;
    cx.is_data_request = is_data_request;
    cx.matched = matched;
    cx.route_ctx = route_ctx;
    cx.status = status;
    cx.label = label;
    Flow::Continue
    // <<< stage-wrapper
}

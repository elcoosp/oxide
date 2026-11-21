//! Moved verbatim from dispatch/entry.rs by scripts/build_stage_split.py
//! (V2-A01 phase 3).
//!
//! stage_match — path normalization + route match (the sketch's
//! `stage_match`): trailing-slash fold, BUG-01 path tag, §16.4 locale
//! resolution, the PERF-01 `/__data` target detection, candidate match
//! over the archived or live tree, status/label.
//!
//! V2-A02 route-context adoption (this file deliberately diverges from the
//! verbatim split; recorded in `docs/deviations.md`): the pure decision
//! core — normalization, the `/__data` target re-point, the root candidate
//! pair, the V2-B02 unified match, the context-table hit and the
//! status/label pair — became the tested V8-free substrate call
//! `oxide_fw_dispatch::route_ctx::resolve_match` (pinned by
//! `crates/dispatch/tests/route_ctx_tdd.rs` against the inline logic this
//! stage shipped). The §16.4 locale resolution STAYS here: it reads
//! `oxide-fw-assets`' i18n helpers, and the substrate's dependency budget
//! must not grow a syntect/image link just to resolve a locale
//! (CONVENTIONS §8).

use crate::*;
use oxide_fw_dispatch::flow::Flow;

// >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
pub(crate) async fn stage_match(d: &DispatchDeps, cx: &mut RequestCtx) -> Flow {
    let req = &cx.req;
    let i18n_cfg = &d.i18n_cfg;
    let archived = &d.archived;
    let tree = &cx.tree;
    let raw = cx.raw.clone();
    // <<< stage-wrapper
    // V2-A02: the pure decision core is one substrate call — the semantics
    // the inline arms shipped, pinned substrate-side without a V8 runtime.
    let decision = oxide_fw_dispatch::route_ctx::resolve_match(
        &raw,
        &req.method,
        &req.url,
        tree,
        archived.as_deref(),
        cx.ctx_table.as_deref(),
    );
    // SPEC-10 / §16.4: resolve the request locale when the project
    // configures i18n — path prefix first, then `Accept-Language`,
    // then the default. Empty when no i18n is configured.
    // `i18n_cfg` is captured by the dispatch closure (built from config
    // before any request); `req` supplies the header.
    let resolved_locale: String = match i18n_cfg.as_ref() {
        Some(a) => {
            let stripped = oxide_fw_assets::i18n::strip_locale_prefix(a, &decision.req_path);
            let from_path = stripped.0 != a.default && stripped.1 != decision.req_path.as_str();
            if from_path {
                stripped.0
            } else {
                oxide_fw_assets::i18n::negotiate(a, req.header("accept-language").unwrap_or(""))
            }
        }
        None => String::new(),
    };
    // >>> stage-wrapper (not part of the moved body; split_selfcheck --stages strips this block)
    cx.normalized = decision.normalized;
    cx.req_path = decision.req_path;
    cx.resolved_locale = resolved_locale;
    cx.is_data_request = decision.is_data_request;
    cx.matched = decision.matched;
    cx.route_ctx = decision.route_ctx;
    cx.status = decision.status;
    cx.label = decision.label;
    Flow::Continue
    // <<< stage-wrapper
}

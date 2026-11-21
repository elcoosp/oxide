//! The route-context resolution leg of the dispatch prelude (spec v0.2,
//! V2-A02 — the last pure request decision that still lived inline in the
//! CLI's stage files).
//!
//! Two decisions moved here from `crates/cli/src/dispatch/stages/`:
//!
//! - **The match decision** ([`resolve_match`]) — `stage_match`'s core:
//!   the trailing-slash fold (BUG-01 path tag), the `GET /__data?route=…`
//!   target re-point (PERF-01 / §8.4: the whole prelude must describe the
//!   route whose data the client router fetches), the `["/", "/index"]`
//!   root candidate pair (§8.1), the match through the unified V2-B02 view
//!   seam (live tree or mmap archive), the per-generation context-table
//!   hit (V2-B01) and the 404/`_unmatched` status/label pair. The request
//!   locale resolution STAYS in the CLI stage: it reads `oxide-fw-assets`'
//!   i18n helpers, and this crate's dependency budget must not grow a
//!   syntect/image link just to resolve a locale (CONVENTIONS §8 — the
//!   substrate's tests have to compile in seconds); the CLI passes nothing
//!   locale-shaped in, and the decision's outputs are locale-free.
//! - **The per-route answers** ([`resolve_answers`]) — `stage_route`'s
//!   middle section: the ancestor middleware chain paths (§8.5, outermost
//!   first), the not-found pair (the project's `not-found.tsx` route id
//!   plus its layout chain), the nearest ancestor `error.tsx` boundary,
//!   and the §8.2/§16.3 layout chain with the §9.2 load specs. Every
//!   answer reads the precomputed [`RouteContextTable`] first (V2-B01 —
//!   one boot-time answer per route id) and falls back to the tree's
//!   dispatch index exactly where the inline arms did.
//!
//! Both functions are pure: no clock, no I/O, no executor. Semantics are
//! pinned by `crates/dispatch/tests/route_ctx_tdd.rs` against the CLI's
//! inline behavior at the extraction base commit.

use oxide_fw_router::{
    ArchivedTree, MatchResult, MatchedView, RouteContext, RouteContextTable, RouteTree,
};
use std::path::Path;
use std::sync::Arc;

/// The metrics label for a route's segments (§20): `/` for the root,
/// `/<segments joined by />` otherwise. Moved verbatim from the CLI's
/// `config_load::pattern_of` (the same shape router's
/// `default_pattern_of` builds, kept here as the degenerate-fallback
/// oracle the match decision uses when no context table rides along).
#[must_use]
pub fn pattern_of(segments: &[String]) -> String {
    if segments.is_empty() {
        "/".to_string()
    } else {
        let joined = segments.join("/");
        format!("/{joined}")
    }
}

/// The match-stage decision: everything `stage_match` derived from the
/// request line, the trees and the generation's context table, in one
/// owned value the stage writes into its per-request context.
pub struct MatchDecision {
    /// The trailing-slash-folded path (the BUG-01 path tag and the match
    /// key — query and fragment already stripped upstream).
    pub normalized: String,
    /// The request path as the L0 tags it (`path:<req_path>` — BUG-01: a
    /// `revalidate.path` event must reach this request's entry).
    pub req_path: String,
    /// Whether this is a `GET /__data` request (PERF-01 / §8.4) — the
    /// match path re-pointed at the `route=` target.
    pub is_data_request: bool,
    /// The winning match (candidate-pair semantics: `/` tries `/index`
    /// too), through the V2-B02 unified view — the owned shape the stages
    /// consume.
    pub matched: Option<MatchResult>,
    /// The matched (or unmatched not-found) route's precomputed context,
    /// one table hit within the generation; `None` without a table.
    pub route_ctx: Option<Arc<RouteContext>>,
    /// 200 matched, 404 unmatched (the §10.2 step 5 status).
    pub status: u16,
    /// The metrics label: the table's precomputed pattern, the
    /// [`pattern_of`] fallback without a table, `_unmatched` on a miss.
    pub label: String,
}

/// Resolve `stage_match`'s decision for one request line.
///
/// `raw` is the request path (query/fragment stripped by the prelude),
/// `method` and `url` feed the `/__data` target parse, `tree` is the
/// generation's live tree, `archived` the production archive (the match
/// goes through whichever rides along — the archived tree wins, exactly
/// the stage's arm), and `ctx_table` the generation's precomputed answers
/// (`None` in the degenerate test shapes).
#[must_use]
pub fn resolve_match(
    raw: &str,
    method: &str,
    url: &str,
    tree: &RouteTree,
    archived: Option<&ArchivedTree>,
    ctx_table: Option<&RouteContextTable>,
) -> MatchDecision {
    let normalized = crate::url::normalize_path(raw);
    let req_path = normalized.clone();
    // PERF-01 / §8.4: the `/__data` target parse — `url::data_route_of`
    // is the verbatim extraction of the stage's inline query walk (the
    // GET-only gate, the percent-decoded `route=` value, the `/` default).
    let data_target = crate::url::data_route_of(method, &req_path, url);
    let is_data_request = data_target.is_some();
    // §8.4: a data request matches the TARGET route (its trailing slash
    // folded like a real URL path); otherwise the request path.
    let match_path = match data_target.as_deref() {
        Some(target) => {
            let stripped = target.trim_end_matches('/');
            if stripped.is_empty() {
                "/".to_string()
            } else {
                stripped.to_string()
            }
        }
        None => normalized.clone(),
    };
    let candidates: Vec<String> = if match_path == "/" {
        vec!["/".to_string(), "/index".to_string()]
    } else {
        vec![match_path.clone()]
    };
    let matched: Option<MatchResult> = candidates.iter().find_map(|p| {
        match archived {
            Some(a) => MatchedView::archived(a, p),
            None => MatchedView::live(tree, p),
        }
        .map(|v| v.into_match_result())
    });
    // V2-B01: the per-route context is one table hit — the metrics label
    // is the precomputed pattern, and the route step reads the chain
    // answers from the same hit. An unmatched request gets the table's
    // not-found context (the render stage resolves the project's
    // `not-found.tsx` through it).
    let route_ctx: Option<Arc<RouteContext>> = match ctx_table {
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
    MatchDecision {
        normalized,
        req_path,
        is_data_request,
        matched,
        route_ctx,
        status,
        label,
    }
}

/// One layout's §9.2 load spec — the route id, the on-disk
/// `layout.server.ts` path (routes-root joined) and the scanner's
/// token-exact `parent()` verdict. Moved from the CLI's `LayoutLoadSpec`
/// (the render/data-route readers keep the name through the re-export).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutLoadSpec {
    /// The layout route id (`routes/blog/layout`), as the loader input's
    /// route field expects.
    pub id: String,
    /// The `layout.server.ts` path on disk (routes-root joined).
    pub path: std::path::PathBuf,
    /// Whether this layout's load may call `await parent()` — the load
    /// graph serializes it behind its predecessor when true.
    pub needs_parent: bool,
}

/// The per-route dispatch answers `stage_route` resolves once per request:
/// the chain/layout/error/not-found quartet, table-first with the tree's
/// dispatch index as the degenerate fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteAnswers {
    /// Middleware module files wrapping the route, outermost first
    /// (§8.5), joined with the routes root.
    pub middleware_paths: Vec<std::path::PathBuf>,
    /// The project's not-found route id, precomputed for the 404 arm
    /// (`None` when the project declares none).
    pub unmatched_route_id: Option<String>,
    /// The not-found route's layout chain, outermost first.
    pub unmatched_layout_ids: Vec<String>,
    /// The nearest ancestor `error.tsx` route id (innermost boundary
    /// wins — a `routes/blog/error` shadows `routes/error`).
    pub error_route_id: Option<String>,
    /// The layout chain ids, outermost first (§8.2 / §16.3); empty
    /// without a context (the degenerate inline shape).
    pub layout_ids: Vec<String>,
    /// The layouts' §9.2 load specs (V2-A06) — only layouts with a
    /// server module, the `parent()` verdict carried.
    pub layout_loads: Vec<LayoutLoadSpec>,
}

/// Resolve `stage_route`'s per-route answers.
///
/// `route_ctx` is the matched (or unmatched) route's precomputed context
/// from [`resolve_match`]; `ctx_table` rides separately because the
/// not-found pair reads the table's unmatched context even for MATCHED
/// requests (the 404 arm consumes it later without a re-scan); `tree`
/// backs every degenerate fallback, `route_id` feeds the tree's index
/// lookups and `routes_root` joins the module paths.
#[must_use]
pub fn resolve_answers(
    route_ctx: Option<&RouteContext>,
    ctx_table: Option<&RouteContextTable>,
    tree: &RouteTree,
    route_id: Option<&str>,
    routes_root: &Path,
) -> RouteAnswers {
    // §8.1 middleware.ts / §8.5: the full ancestor chain, outermost
    // first, from the precomputed context (P-06 → V2-B01); the tree's
    // dispatch index stays as the degenerate fallback.
    let middleware_paths: Vec<std::path::PathBuf> = match route_ctx {
        Some(rc) => rc
            .middleware_chain
            .iter()
            .map(|f| routes_root.join(f))
            .collect(),
        None => tree
            .middleware_files_for(route_id.unwrap_or(""))
            .into_iter()
            .map(|f| routes_root.join(f))
            .collect(),
    };
    // §10.2 step 5: the not-found pair — the table's unmatched context
    // first, the tree's index as the fallback, the default (None, [])
    // last. The arm order is the stage's, verbatim.
    let (unmatched_route_id, unmatched_layout_ids) = ctx_table
        .and_then(|t| t.unmatched())
        .map(|c| {
            (
                Some(c.route_id.0.clone()),
                c.layout_chain.iter().map(|i| i.0.clone()).collect(),
            )
        })
        .or_else(|| {
            tree.not_found_route()
                .map(|(id, chain)| (Some(id), chain.into_iter().map(|r| r.0).collect()))
        })
        .unwrap_or_default();
    // §10.2 step 5: the NEAREST ancestor `error.tsx` for the matched
    // route (innermost boundary wins) — table first, tree fallback.
    let error_route_id: Option<String> = match route_ctx {
        Some(rc) => rc.error_route.as_ref().map(|i| i.0.clone()),
        None => tree.error_route_for(route_id.unwrap_or("")),
    };
    // §8.2 / §16.3 → §9.2: the layout chain and the load specs from the
    // precomputed context (the paths joined with the routes root, the
    // `parent()` verdict carried); unmatched requests have no layout
    // chain without a table (the specs are empty, as the walk produced).
    let (layout_ids, layout_loads) = match route_ctx {
        Some(rc) => (
            rc.layout_chain.iter().map(|i| i.0.clone()).collect(),
            rc.layout_loads
                .iter()
                .map(|l| LayoutLoadSpec {
                    id: l.id.0.clone(),
                    path: routes_root.join(&l.server_file),
                    needs_parent: l.needs_parent,
                })
                .collect(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    RouteAnswers {
        middleware_paths,
        unmatched_route_id,
        unmatched_layout_ids,
        error_route_id,
        layout_ids,
        layout_loads,
    }
}

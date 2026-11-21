//! V2-A02 (the route-context leg): the match decision and the per-route
//! dispatch answers are pure functions of the request line, the trees and
//! the context table — pinned here WITHOUT a V8 runtime (CONVENTIONS §8: the
//! substrate's tests must compile in seconds; the CLI's dispatch pins pay a
//! V8 link for logic that never touches it).
//!
//! The oracle is the CLI's inline logic at `base_commit` (`dispatch/stages/
//! matcher.rs` `stage_match` + `dispatch/stages/route.rs` `stage_route`):
//! every pin restates an answer those stages produced, byte-identically.
//! The fixture mirrors the scanner's conventions: the root page has empty
//! segments (`scan.rs`: "root page matches /"), ids carry the file role
//! suffix (`routes/blog/[slug]/page`), scoped modules sit in the route's
//! directory, and the blog layout declares `layout.server.ts` with a
//! `parent()` verdict (V2-A06).

use oxide_fw_dispatch::route_ctx::{LayoutLoadSpec, pattern_of, resolve_answers, resolve_match};
use oxide_fw_router::{RouteContextTable, RouteEntry, RouteId, RouteKind, RouteTree};
use std::path::{Path, PathBuf};

const ROUTES_ROOT: &str = "/srv/app";

/// A fixture entry with the scanner's shapes (defaults everywhere; the two
/// flags the §9.2 graph reads are flipped on the blog layout below).
fn entry(id: &str, kind: RouteKind, segments: &[&str], file: &str) -> RouteEntry {
    RouteEntry {
        id: RouteId(id.to_string()),
        kind,
        segments: segments.iter().map(|s| (*s).to_string()).collect(),
        file: PathBuf::from(file),
        render_mode: None,
        tags: vec![],
        has_server_module: false,
        needs_parent: false,
        slot: None,
        live: None,
        priority: None,
    }
}

struct Fixture {
    tree: RouteTree,
    table: RouteContextTable,
}

/// Root page + blog pages (one param), root/blog layouts (blog's carries the
/// server module + `parent()` verdict), root/blog middleware, root/blog error
/// boundaries and one not-found page.
fn fixture() -> Fixture {
    let mut blog_layout = entry(
        "routes/blog/layout",
        RouteKind::Layout,
        &["blog"],
        "routes/blog/layout.tsx",
    );
    blog_layout.has_server_module = true;
    blog_layout.needs_parent = true;
    let entries = vec![
        entry(
            "routes/index/page",
            RouteKind::Page,
            &[],
            "routes/index/page.tsx",
        ),
        entry(
            "routes/blog/page",
            RouteKind::Page,
            &["blog"],
            "routes/blog/page.tsx",
        ),
        entry(
            "routes/blog/[slug]/page",
            RouteKind::Page,
            &["blog", ":slug"],
            "routes/blog/[slug]/page.tsx",
        ),
        entry("routes/layout", RouteKind::Layout, &[], "routes/layout.tsx"),
        blog_layout,
        entry(
            "routes/middleware",
            RouteKind::Middleware,
            &[],
            "routes/middleware.ts",
        ),
        entry(
            "routes/blog/middleware",
            RouteKind::Middleware,
            &["blog"],
            "routes/blog/middleware.ts",
        ),
        entry("routes/error", RouteKind::Error, &[], "routes/error.tsx"),
        entry(
            "routes/blog/error",
            RouteKind::Error,
            &["blog"],
            "routes/blog/error.tsx",
        ),
        entry(
            "routes/not-found",
            RouteKind::NotFound,
            &[],
            "routes/not-found.tsx",
        ),
    ];
    let mut tree = RouteTree::new();
    for e in &entries {
        tree.insert(e.clone());
    }
    let table = RouteContextTable::build_with_default_pattern(&entries);
    Fixture { tree, table }
}

// ---------------------------------------------------------------- match

/// `stage_match`: a trailing slash folds before matching, and the metrics
/// label comes from the generation's context-table hit (the precomputed
/// pattern), never a per-request walk.
#[test]
fn match_trailing_slash_folds_and_labels_from_table() {
    let f = fixture();
    let m = resolve_match(
        "/blog/",
        "GET",
        "/blog/?utm=x",
        &f.tree,
        None,
        Some(&f.table),
    );
    assert_eq!(m.normalized, "/blog");
    assert_eq!(m.req_path, "/blog");
    assert!(!m.is_data_request);
    let matched = m.matched.as_ref().expect("blog page matches");
    assert_eq!(matched.route.id.0, "routes/blog/page");
    assert_eq!(m.status, 200);
    assert_eq!(m.label, "/blog");
}

/// `stage_match`: the root path's `["/", "/index"]` candidate pair hits the
/// index page, and the empty-segment pattern label is `/`.
#[test]
fn match_root_candidates_hit_index() {
    let f = fixture();
    let m = resolve_match("/", "GET", "/", &f.tree, None, Some(&f.table));
    let matched = m.matched.as_ref().expect("root matches via candidates");
    assert_eq!(matched.route.id.0, "routes/index/page");
    assert_eq!(m.status, 200);
    assert_eq!(m.label, "/");
}

/// `stage_match`: with no context table (the degenerate fallback arm) the
/// label falls back to the per-request `pattern_of` walk over the matched
/// route's segments — `/blog/:slug` for the param route.
#[test]
fn match_param_route_labels_pattern_of_without_table() {
    let f = fixture();
    let m = resolve_match("/blog/hello", "GET", "/blog/hello", &f.tree, None, None);
    let matched = m.matched.as_ref().expect("param route matches");
    assert_eq!(matched.route.id.0, "routes/blog/[slug]/page");
    assert_eq!(
        matched.params.get("slug").map(String::as_str),
        Some("hello")
    );
    assert!(m.route_ctx.is_none(), "no table, no context hit");
    assert_eq!(m.status, 200);
    assert_eq!(m.label, "/blog/:slug");
}

/// `stage_match`: an unmatched URL 404s with the `_unmatched` label and the
/// table's not-found context rides along (the render stage resolves the
/// project's `not-found.tsx` through it).
#[test]
fn match_unmatched_404s_with_not_found_context() {
    let f = fixture();
    let m = resolve_match("/nope", "GET", "/nope", &f.tree, None, Some(&f.table));
    assert!(m.matched.is_none());
    assert_eq!(m.status, 404);
    assert_eq!(m.label, "_unmatched");
    let ctx = m.route_ctx.as_ref().expect("table unmatched context");
    assert_eq!(ctx.route_id.0, "routes/not-found");
}

/// `stage_match` (PERF-01 / §8.4): `GET /__data?route=/blog` is a data
/// request — the match path re-points at the TARGET route so the whole
/// prelude describes the route whose data the client router fetches.
#[test]
fn match_data_route_targets_the_query_route() {
    let f = fixture();
    let m = resolve_match(
        "/__data",
        "GET",
        "/__data?route=/blog",
        &f.tree,
        None,
        Some(&f.table),
    );
    assert_eq!(m.normalized, "/__data");
    assert_eq!(m.req_path, "/__data");
    assert!(m.is_data_request);
    let matched = m.matched.as_ref().expect("target route matches");
    assert_eq!(matched.route.id.0, "routes/blog/page");
    assert_eq!(m.status, 200);
    assert_eq!(m.label, "/blog");
}

/// `stage_match`: a `GET /__data` without a usable `route=` value targets
/// `/` (the endpoint's documented default) — still a data request.
#[test]
fn match_data_route_defaults_to_root() {
    let f = fixture();
    let m = resolve_match(
        "/__data",
        "GET",
        "/__data?other=1",
        &f.tree,
        None,
        Some(&f.table),
    );
    assert!(m.is_data_request);
    let matched = m.matched.as_ref().expect("default target matches");
    assert_eq!(matched.route.id.0, "routes/index/page");
}

/// `stage_match`: only a GET is a data request — a POST to `/__data` falls
/// through to ordinary matching (and matches nothing here).
#[test]
fn match_data_route_requires_get() {
    let f = fixture();
    let m = resolve_match(
        "/__data",
        "POST",
        "/__data?route=/blog",
        &f.tree,
        None,
        Some(&f.table),
    );
    assert!(!m.is_data_request);
    assert!(m.matched.is_none());
    assert_eq!(m.status, 404);
    assert_eq!(m.label, "_unmatched");
}

/// `stage_match`: the target is percent-decoded and its trailing slash
/// folds like a real URL path before matching.
#[test]
fn match_data_route_decodes_and_folds_target() {
    let f = fixture();
    let m = resolve_match(
        "/__data",
        "GET",
        "/__data?route=%2Fblog%2F",
        &f.tree,
        None,
        Some(&f.table),
    );
    let matched = m.matched.as_ref().expect("decoded target matches");
    assert_eq!(matched.route.id.0, "routes/blog/page");
}

// ------------------------------------------------------------- answers

/// `stage_route`: the matched route's per-route answers compose from the
/// precomputed context — middleware paths outer→inner joined with the
/// routes root, the layout chain ids, the §9.2 load specs (only layouts
/// with a server module; the `parent()` verdict carried), the innermost
/// error boundary — and the not-found pair rides the table for every
/// request (the 404 arm reads it later without a re-scan).
#[test]
fn answers_matched_route_compose_chain_layout_error() {
    let f = fixture();
    let m = resolve_match(
        "/blog/hello",
        "GET",
        "/blog/hello",
        &f.tree,
        None,
        Some(&f.table),
    );
    let ctx = m.route_ctx.as_deref().expect("table hit");
    let a = resolve_answers(
        Some(ctx),
        Some(&f.table),
        &f.tree,
        m.matched.as_ref().map(|r| r.route.id.0.as_str()),
        Path::new(ROUTES_ROOT),
    );
    assert_eq!(
        a.middleware_paths,
        vec![
            PathBuf::from("/srv/app/routes/middleware.ts"),
            PathBuf::from("/srv/app/routes/blog/middleware.ts"),
        ]
    );
    assert_eq!(a.layout_ids, vec!["routes/layout", "routes/blog/layout"]);
    assert_eq!(
        a.layout_loads,
        vec![LayoutLoadSpec {
            id: "routes/blog/layout".to_string(),
            path: PathBuf::from("/srv/app/routes/blog/layout.server.ts"),
            needs_parent: true,
        }]
    );
    assert_eq!(a.error_route_id.as_deref(), Some("routes/blog/error"));
    assert_eq!(a.unmatched_route_id.as_deref(), Some("routes/not-found"));
    assert_eq!(a.unmatched_layout_ids, vec!["routes/layout"]);
}

/// `stage_route`: with no context and no table (the degenerate arms) the
/// answers come from the tree's dispatch index verbatim — the middleware
/// files join the routes root, the nearest error boundary resolves, and the
/// not-found pair is the tree's — while the layout answers stay EMPTY (the
/// old inline shape: without the table the per-request walk produced no
/// layout chain).
#[test]
fn answers_without_table_fall_back_to_the_tree() {
    let f = fixture();
    let a = resolve_answers(
        None,
        None,
        &f.tree,
        Some("routes/blog/[slug]/page"),
        Path::new(ROUTES_ROOT),
    );
    let root = Path::new(ROUTES_ROOT);
    let expected: Vec<PathBuf> = f
        .tree
        .middleware_files_for("routes/blog/[slug]/page")
        .into_iter()
        .map(|p| root.join(p))
        .collect();
    assert_eq!(a.middleware_paths, expected);
    assert_eq!(a.error_route_id.as_deref(), Some("routes/blog/error"));
    assert_eq!(a.layout_ids, Vec::<String>::new());
    assert_eq!(a.layout_loads, Vec::<LayoutLoadSpec>::new());
    let (nf_id, nf_chain) = f
        .tree
        .not_found_route()
        .expect("fixture declares not-found");
    assert_eq!(a.unmatched_route_id.as_deref(), Some(nf_id.as_str()));
    assert_eq!(
        a.unmatched_layout_ids,
        nf_chain.iter().map(|r| r.0.clone()).collect::<Vec<_>>()
    );
}

/// The full 404 composition: `resolve_match` answers unmatched, and the
/// answers stage resolves the not-found route through the table's context
/// while the root middleware STILL wraps the 404 (the chain answer the old
/// stage shipped).
#[test]
fn answers_unmatched_compose_the_not_found_pair() {
    let f = fixture();
    let m = resolve_match("/nope", "GET", "/nope", &f.tree, None, Some(&f.table));
    assert_eq!(m.status, 404);
    let a = resolve_answers(
        m.route_ctx.as_deref(),
        Some(&f.table),
        &f.tree,
        None,
        Path::new(ROUTES_ROOT),
    );
    assert_eq!(a.unmatched_route_id.as_deref(), Some("routes/not-found"));
    assert_eq!(a.unmatched_layout_ids, vec!["routes/layout"]);
    assert_eq!(
        a.middleware_paths,
        vec![PathBuf::from("/srv/app/routes/middleware.ts")]
    );
}

// ----------------------------------------------------------- pattern_of

/// The metrics label helper: root is `/`, otherwise the segments joined
/// under a leading slash (the §20 shape, moved verbatim from the CLI's
/// `config_load::pattern_of`).
#[test]
fn pattern_of_shapes() {
    assert_eq!(pattern_of(&[]), "/");
    assert_eq!(pattern_of(&["blog".to_string()]), "/blog");
    assert_eq!(
        pattern_of(&["blog".to_string(), ":slug".to_string()]),
        "/blog/:slug"
    );
}

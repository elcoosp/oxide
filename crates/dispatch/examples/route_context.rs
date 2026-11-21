//! The V2-B01 route-context walkthrough: ONE boot-time `RouteContextTable`
//! answers every per-request dispatch question. The example builds a small
//! route tree, precomputes the table the way the boot does
//! (`SwappableRouteTree` on the serving side, `phase_prerender` on the
//! build side), then answers a matched page request, a 404 and the
//! degenerate no-table shapes through the [`resolve_match`] /
//! [`resolve_answers`] substrate.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dispatch --example route_context
//! ```

use oxide_fw_dispatch::route_ctx::{resolve_answers, resolve_match};
use oxide_fw_router::{RouteContextTable, RouteEntry, RouteId, RouteKind, RouteTree};
use std::path::Path;

/// A hand-built route entry — the same field shape the scanner writes:
/// `segments` carry ONLY the URL segments (the trailing `page` is the file
/// role, not a URL segment) in the encoded `:param` spelling
/// (`segment.rs`: `[slug]` → `:slug`); `file` is routes-root-RELATIVE (the
/// table's module paths are joined onto the routes root by the consumers).
fn entry(id: &str, kind: RouteKind) -> RouteEntry {
    let rel = id.strip_prefix("routes/").unwrap_or(id);
    let segments: Vec<String> = match kind {
        RouteKind::Page => rel
            .strip_suffix("/page")
            .unwrap_or(rel)
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| {
                if let Some(param) = s.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                    format!(":{param}")
                } else {
                    s.to_string()
                }
            })
            .collect(),
        _ => Vec::new(),
    };
    RouteEntry {
        id: RouteId(id.to_string()),
        kind,
        segments,
        file: std::path::PathBuf::from(format!("{rel}.tsx")),
        render_mode: None,
        tags: Vec::new(),
        // The blog layout carries a `layout.server.ts` — the §9.2 load
        // specs only include layouts with a server module.
        has_server_module: id == "routes/blog/layout",
        needs_parent: false,
        live: None,
        priority: None,
        slot: None,
    }
}

fn main() {
    // The scanned route tree: a root layout, a blog section with its own
    // layout, middleware, error boundary and a param page, plus the
    // project's not-found route.
    let entries = vec![
        entry("routes/layout", RouteKind::Layout),
        entry("routes/blog/layout", RouteKind::Layout),
        entry("routes/middleware", RouteKind::Middleware),
        entry("routes/blog/middleware", RouteKind::Middleware),
        entry("routes/error", RouteKind::Error),
        entry("routes/blog/error", RouteKind::Error),
        entry("routes/blog/page", RouteKind::Page),
        entry("routes/blog/[slug]/page", RouteKind::Page),
        entry("routes/not-found", RouteKind::NotFound),
    ];
    let mut tree = RouteTree::new();
    for e in &entries {
        tree.insert(e.clone());
    }

    // V2-B01: the table precomputes every per-route dispatch answer ONCE —
    // the layout chain, the middleware chain, the error boundary, the
    // §9.2 load specs and the metrics pattern. Per request the answers are
    // one HashMap hit instead of O(entries) scans (spec perf #1 / #10).
    let table = RouteContextTable::build_with_default_pattern(&entries);
    println!(
        "boot: the route-context table is built over {} entries",
        entries.len()
    );

    // A matched page request: the match decision carries the table's
    // precomputed pattern as the metrics label and the route's context as
    // one hit.
    let decision = resolve_match(
        "/blog/hello",
        "GET",
        "/blog/hello",
        &tree,
        None,
        Some(&table),
    );
    println!(
        "GET /blog/hello -> status {} label {} (route `{}`)",
        decision.status,
        decision.label,
        decision
            .matched
            .as_ref()
            .map(|m| m.route.id.0.clone())
            .unwrap_or_default(),
    );
    let rc = decision
        .route_ctx
        .as_deref()
        .expect("a matched page has a context row");
    let answers = resolve_answers(
        Some(rc),
        Some(&table),
        &tree,
        decision.matched.as_ref().map(|m| m.route.id.0.as_str()),
        Path::new("/proj/src/routes"),
    );
    println!(
        "  middleware chain (outermost first): {:?}",
        answers.middleware_paths
    );
    println!("  layout chain: {:?}", answers.layout_ids);
    println!("  §9.2 load specs: {:?}", answers.layout_loads);
    println!("  nearest error boundary: {:?}", answers.error_route_id);
    println!(
        "  the 404 arm will render `{:?}`",
        answers.unmatched_route_id
    );

    // A 404: the table's unmatched context answers the render arm — the
    // not-found route id and ITS layout chain, still one hit.
    let miss = resolve_match("/nope", "GET", "/nope", &tree, None, Some(&table));
    println!("GET /nope -> status {} label {}", miss.status, miss.label);
    let answers404 = resolve_answers(
        miss.route_ctx.as_deref(),
        Some(&table),
        &tree,
        None,
        Path::new("/proj/src/routes"),
    );
    println!(
        "  renders `{}` over layouts {:?}",
        answers404.unmatched_route_id.unwrap_or_default(),
        answers404.layout_ids,
    );

    // The degenerate shape: no table (a hand-built tree in a test) — the
    // decisions fall back to the tree's dispatch index and the `pattern_of`
    // walk, the exact arms the pre-B01 stages ran.
    let degenerate = resolve_match("/blog/hello", "GET", "/blog/hello", &tree, None, None);
    println!(
        "without a table -> status {} label {} (the pattern_of fallback)",
        degenerate.status, degenerate.label
    );
    let answers_degenerate = resolve_answers(
        None,
        None,
        &tree,
        degenerate.matched.as_ref().map(|m| m.route.id.0.as_str()),
        Path::new("/proj/src/routes"),
    );
    println!(
        "  tree-index fallbacks: middleware {:?}, error {:?}, no layout specs ({})",
        answers_degenerate.middleware_paths,
        answers_degenerate.error_route_id,
        answers_degenerate.layout_loads.len(),
    );
}

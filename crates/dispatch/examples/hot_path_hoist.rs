//! WP B03 (hot-path hoists) walkthrough: the zero-alloc matched label
//! (spec item 2) in one runnable demo. The match decision's label is the
//! table's precomputed `Arc<str>` pattern, shared by refcount bump — not a
//! fresh `String` cloned per request. The three arms:
//!
//! - **matched with table** (the hot path): `Arc::clone(&rc.pattern)` —
//!   pointer-identical to the table's pattern, zero allocation.
//! - **unmatched (404)**: a cached `Arc<str>` (`_unmatched`) — one allocation
//!   shared across every 404, never per-request.
//! - **no-table fallback** (a test shape): the `pattern_of` `String`, the
//!   only arm that still allocates (rare — production boots always carry a
//!   `RouteContextTable`).
//!
//! The lock-free dynamic-label `observe` (spec item 4) is pinned by
//! `crates/server/src/metrics.rs::register_routes_freezes_dynamic_labels`
//! and `observe_registered_route_takes_no_lock`; this example stays in the
//! V8-free dispatch substrate.
//!
//! Run: `cargo run -p oxide-fw-dispatch --example hot_path_hoist`

use oxide_fw_dispatch::route_ctx::resolve_match;
use oxide_fw_router::{RouteContextTable, RouteEntry, RouteId, RouteKind, RouteTree};
use std::path::PathBuf;
use std::sync::Arc;

/// A hand-built page entry — the scanner's field shape (`segments` carry
/// only the URL segments in the encoded `:param` spelling; `file` is
/// routes-root-relative).
fn page_entry(id: &str, segments: &[&str]) -> RouteEntry {
    let rel = id.strip_prefix("routes/").unwrap_or(id);
    RouteEntry {
        id: RouteId(id.to_string()),
        kind: RouteKind::Page,
        segments: segments.iter().map(|s| (*s).to_string()).collect(),
        file: PathBuf::from(format!("{rel}.tsx")),
        render_mode: None,
        tags: Vec::new(),
        has_server_module: false,
        needs_parent: false,
        live: None,
        priority: None,
        slot: None,
    }
}

fn main() {
    let entries = vec![
        page_entry("routes/index/page", &[]),
        page_entry("routes/blog/page", &["blog"]),
        page_entry("routes/blog/[slug]/page", &["blog", ":slug"]),
    ];
    let mut tree = RouteTree::new();
    for entry in &entries {
        tree.insert(entry.clone());
    }
    let table = RouteContextTable::build_with_default_pattern(&entries);
    println!(
        "boot: route-context table built over {} entries (the precomputed patterns ride the table)",
        entries.len()
    );

    // The hot path: a matched page request. The decision's label IS the
    // table's `Arc<str>` pattern — pointer identity proves zero allocation
    // (a `String` clone would land at a different address every request).
    let matched = resolve_match(
        "/blog/hello",
        "GET",
        "/blog/hello",
        &tree,
        None,
        Some(&table),
    );
    let matched_ctx = matched
        .route_ctx
        .as_deref()
        .expect("a matched page has a context row");
    let shared = Arc::ptr_eq(&matched.label, &matched_ctx.pattern);
    println!(
        "GET /blog/hello -> status {} label {:?}",
        matched.status,
        matched.label.as_ref()
    );
    println!(
        "  label shares the table's pattern Arc: {shared} (Arc::ptr_eq — zero-alloc hot path)"
    );

    // A 404: the unmatched label is a cached `Arc<str>` — one allocation
    // shared across every unmatched request, never a per-request `String`.
    let miss = resolve_match("/nope", "GET", "/nope", &tree, None, Some(&table));
    println!(
        "GET /nope -> status {} label {:?} (a cached Arc<str> — shared across every 404)",
        miss.status,
        miss.label.as_ref()
    );

    // The degenerate no-table fallback (a test shape): the `pattern_of`
    // `String` — the only arm that still allocates. Production boots carry a
    // table, so the matched hot path above is what every request pays.
    let degenerate = resolve_match("/blog/hello", "GET", "/blog/hello", &tree, None, None);
    println!(
        "without a table -> status {} label {:?} (the pattern_of String fallback — allocates)",
        degenerate.status,
        degenerate.label.as_ref()
    );
}

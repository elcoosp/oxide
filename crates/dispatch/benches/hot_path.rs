//! WP B03 (hot-path hoists): criterion benches for the matched-label
//! zero-alloc path and the degenerate `pattern_of` fallback.
//!
//! The matched-with-table label resolves as an `Arc::clone` of the
//! precomputed `RouteContext.pattern` — one refcount bump, zero heap
//! allocation (the prior `rc.pattern.to_string()` allocated a fresh
//! `String` per matched request). The no-table fallback still builds the
//! `pattern_of` `String` (rare — a table-less tree is a test shape), and
//! the isolated `old_to_string_baseline` restates the per-request cost the
//! hoist replaced so the contrast is explicit.
//!
//! Run: `cargo bench -p oxide-fw-dispatch --features bench-util`

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_dispatch::route_ctx::resolve_match;
use oxide_fw_router::{RouteContextTable, RouteEntry, RouteId, RouteKind, RouteTree};
use std::hint::black_box;
use std::path::PathBuf;

/// A hand-built page entry — the scanner's field shape (`segments` carry
/// only the URL segments in the encoded `:param` spelling; `file` is
/// routes-root-relative). Mirrors `crates/dispatch/examples/route_context.rs`.
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

/// A small route tree + its precomputed `RouteContextTable` — the boot-time
/// shape `stage_match` reads one hit from per request.
fn hot_fixture() -> (RouteTree, RouteContextTable) {
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
    (tree, table)
}

/// The OLD per-request label cost the hoist replaced: `rc.pattern.to_string()`
/// — a fresh `String` allocation + byte copy per matched request. Kept here as
/// the contrast baseline the `Arc::clone` hot path is measured against.
fn old_string_label(pattern: &str) -> String {
    pattern.to_string()
}

fn bench_hot_path(c: &mut Criterion) {
    let (tree, table) = hot_fixture();
    let mut group = c.benchmark_group("hot_path");
    group.sample_size(40);
    // The matched-with-table label: `Arc::clone(&rc.pattern)` (zero-alloc).
    group.bench_function("label/matched_arc_clone", |bencher| {
        bencher.iter(|| {
            let decision = resolve_match(
                black_box("/blog/hello"),
                "GET",
                "/blog/hello",
                black_box(&tree),
                None,
                Some(black_box(&table)),
            );
            black_box(decision.label);
        });
    });
    // The degenerate no-table fallback: still allocates the `pattern_of`
    // `String` (rare — a table-less tree is a test shape).
    group.bench_function("label/matched_string_fallback", |bencher| {
        bencher.iter(|| {
            let decision = resolve_match(
                black_box("/blog/hello"),
                "GET",
                "/blog/hello",
                black_box(&tree),
                None,
                None,
            );
            black_box(decision.label);
        });
    });
    // The contrast baseline: the OLD `to_string()` shape, isolated — the
    // per-request allocation the matched `Arc::clone` path no longer pays.
    let context = table
        .get("routes/blog/[slug]/page")
        .cloned()
        .expect("the param route has a context row");
    group.bench_function("label/old_to_string_baseline", |bencher| {
        bencher.iter(|| black_box(old_string_label(black_box(&context.pattern))));
    });
    group.finish();
}

criterion_group!(benches, bench_hot_path);
criterion_main!(benches);

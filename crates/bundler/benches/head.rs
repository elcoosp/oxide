//! Criterion benches for the adapter-level head producer (the V2-E04 tail,
//! `bench-util` gated like the A02/B09/F18/E11 benches — plain `cargo test`
//! never pays the criterion build).
//!
//! The bench-relevant question: `head_for` runs once per prerendered route
//! (and once per live-SSR route once the live path adopts the producer), so
//! its cost must stay noise next to the render itself. The groups:
//! - `head_for/static`: an exact manifest-keyed route (the `<route>/page`
//!   shape the bundler writes).
//! - `head_for/dynamic`: the `[slug]` → `_slug_` normalized shape — the
//!   normalization walk plus the miss-then-hit key resolution.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_bundler::head::{HeadProducer as _, ManifestHeadProducer};
use oxide_fw_bundler::{ClientManifest, RouteBundle};

/// A synthetic manifest: 64 page routes in the bundler's key spelling
/// (eight preload files + three stylesheets each) plus one dynamic route in
/// the sanitized `_param_` form — the shape a mid-size app's client
/// manifest carries.
fn synthetic_manifest() -> ClientManifest {
    let mut map = std::collections::BTreeMap::new();
    for i in 0..64 {
        map.insert(
            format!("routes/page-{i}/page"),
            RouteBundle {
                entry_chunk: format!("page-{i}-entry.js"),
                preload: (0..8).map(|j| format!("page-{i}-chunk-{j}.js")).collect(),
                css: (0..3).map(|j| format!("page-{i}-style-{j}.css")).collect(),
                js_bytes: 48 * 1024,
                modules: Vec::new(),
                layout_chain: Vec::new(),
            },
        );
    }
    map.insert(
        "routes/blog/_slug_/page".to_string(),
        RouteBundle {
            entry_chunk: "blog-entry.js".to_string(),
            preload: (0..8).map(|j| format!("blog-chunk-{j}.js")).collect(),
            css: (0..3).map(|j| format!("blog-style-{j}.css")).collect(),
            js_bytes: 48 * 1024,
            modules: Vec::new(),
            layout_chain: Vec::new(),
        },
    );
    ClientManifest {
        build_id: "bench-build".to_string(),
        routes: map,
        assets: std::collections::BTreeMap::new(),
        integrity: std::collections::BTreeMap::new(),
    }
}

fn bench_head_for(c: &mut Criterion) {
    let producer = ManifestHeadProducer::new(synthetic_manifest());
    let mut g = c.benchmark_group("head_for");
    for (id, route) in [
        ("static", "routes/page-7"),
        ("dynamic", "routes/blog/[slug]"),
    ] {
        g.bench_with_input(BenchmarkId::from_parameter(id), &route, |b, route| {
            b.iter(|| producer.head_for(black_box(route)));
        });
    }
    g.finish();
}

criterion_group!(benches, bench_head_for);
criterion_main!(benches);

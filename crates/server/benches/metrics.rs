//! Criterion bench for the §20 metrics endpoint's render, with the F37
//! cache-plane slot attached (`bench-util` gated like every other
//! criterion bench in the workspace — plain `cargo test` never pays the
//! criterion build).
//!
//! The wiring-relevant questions:
//! - `exposition/render`: the endpoint's render cost bare — the baseline
//!   every scrape pays.
//! - `exposition/render-cache-plane`: the render with the slot attached —
//!   the delta the cache-plane families add per scrape (one closure call
//!   + the append; the closure here renders the F30 exporter's real
//!   family count, nine families × three lines).
//! - `exposition/attach`: the `set_cache_plane` attach itself (boot-time,
//!   once — pinned for completeness, not for the dashboards).

use std::hint::black_box;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_server::Metrics;

/// The F30 exporter's exposition shape — nine `oxide_cache_plane_*`
/// families, `# HELP`/`# TYPE`/row each — the text the boot's closure
/// hands the slot per scrape.
fn cache_plane_exposition() -> String {
    let families = [
        "memory_entries",
        "memory_bytes",
        "memory_capacity_bytes",
        "disk_attached",
        "disk_bytes",
        "remote_attached",
        "tags_indexed",
        "keys_tracked",
        "observer_attached",
    ];
    let mut out = String::with_capacity(1200);
    for name in families {
        out.push_str("# HELP oxide_cache_plane_");
        out.push_str(name);
        out.push_str(" the cache plane posture\n");
        out.push_str("# TYPE oxide_cache_plane_");
        out.push_str(name);
        out.push_str(" gauge\n");
        out.push_str("oxide_cache_plane_");
        out.push_str(name);
        out.push_str(" 0\n");
    }
    out
}

fn bench_render(c: &mut Criterion) {
    let mut group = c.benchmark_group("exposition");

    let bare = Metrics::new();
    bare.observe("/bench", std::time::Duration::from_millis(3), 200);
    group.bench_function("render", |b| b.iter(|| bare.render()));

    let wired = Metrics::new();
    wired.observe("/bench", std::time::Duration::from_millis(3), 200);
    wired.set_cache_plane(cache_plane_exposition);
    group.bench_function("render-cache-plane", |b| {
        b.iter(|| {
            let text = wired.render();
            black_box(&text);
        })
    });

    let attach_target = Metrics::new();
    group.bench_function("attach", |b| {
        b.iter(|| {
            attach_target.set_cache_plane(cache_plane_exposition);
        })
    });

    group.finish();
}

/// The slot survives sharing: the wired registry crosses the thread
/// boundary (the endpoint arm and the boot hold the same `Arc`).
fn bench_render_shared(c: &mut Criterion) {
    let mut group = c.benchmark_group("exposition");
    let wired = Arc::new(Metrics::new());
    wired.set_cache_plane(cache_plane_exposition);
    let shared = Arc::clone(&wired);
    group.bench_function("render-through-arc", |b| {
        b.iter(|| {
            let text = shared.render();
            black_box(&text);
        })
    });
    group.finish();
}

criterion_group!(benches, bench_render, bench_render_shared);
criterion_main!(benches);

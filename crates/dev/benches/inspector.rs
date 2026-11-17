//! Criterion bench for the §V2-E08 inspector render path (the B09
//! pattern: plain `cargo test` never pays the criterion build — run
//! behind `--features bench-util`).
//!
//! The wiring-relevant question is the per-hit cost of the two surfaces
//! the dev server renders on every `/__oxide` (HTML) and `/__oxide/state`
//! (JSON) view, now that the live (F05) and generation (F10) tables ride
//! the same state:
//!
//! - `inspector_state/{routes}`: building the `InspectorState` from a
//!   boot-shaped `RouteTableInspector` — the route clone plus the live
//!   and generation rows the callers wired.
//! - `inspector_html/{routes}`: the full self-contained HTML render of
//!   the same state (route/live/generation/waterfall tables, the cell
//!   escaping).
//!
//! Run with:
//!
//! ```text
//! cargo bench -p oxide-fw-dev --features bench-util -- inspector
//! ```

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_dev::{GenerationRow, LiveRow, RouteTableInspector, render_inspector_html};

/// A boot-shaped inspector: `n` routes (a third of them dynamic with a
/// cache policy), a live table over 32 exact tag keys plus one wildcard
/// pool, and the two F10 generation rows.
fn inspector(n: usize) -> RouteTableInspector {
    let routes = (0..n)
        .map(|i| {
            let dynamic = i % 8 == 0;
            let pattern = if dynamic {
                "/blog/[slug]".to_string()
            } else {
                format!("/p{i}")
            };
            let mode = if dynamic { "dynamic" } else { "static" };
            let tags = if i % 3 == 0 {
                vec!["no-cache".to_string()]
            } else {
                Vec::new()
            };
            (pattern, mode.to_string(), tags)
        })
        .collect();
    let live = (0..32)
        .map(|i| LiveRow {
            tag: format!("post:{i}"),
            wildcard: false,
            subscribers: u64::try_from(i % 5).unwrap_or(0),
        })
        .chain(std::iter::once(LiveRow {
            tag: "comments:".to_string(),
            wildcard: true,
            subscribers: 16,
        }))
        .collect();
    let generations = vec![
        GenerationRow {
            id: "deadbeef0000000f".to_string(),
            role: "current".to_string(),
            window_ms_remaining: None,
        },
        GenerationRow {
            id: "deadbeef0000000e".to_string(),
            role: "previous".to_string(),
            window_ms_remaining: Some(30_000),
        },
    ];
    RouteTableInspector::from_router_table(routes)
        .with_live(live)
        .with_generations(generations)
}

fn bench_inspector_state(c: &mut Criterion) {
    let mut group = c.benchmark_group("inspector_state");
    // The realistic boot shapes: a small site and a scaled one.
    for n in [64usize, 256] {
        let insp = inspector(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &insp, |b, insp| {
            b.iter(|| black_box(insp.state()));
        });
    }
    group.finish();
}

fn bench_inspector_html(c: &mut Criterion) {
    let mut group = c.benchmark_group("inspector_html");
    for n in [64usize, 256] {
        let insp = inspector(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &insp, |b, insp| {
            b.iter(|| black_box(render_inspector_html(&insp.state())));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_inspector_state, bench_inspector_html);
criterion_main!(benches);

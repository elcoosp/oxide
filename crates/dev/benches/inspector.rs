//! Criterion bench for the §V2-E08 inspector render path (the B09
//! pattern: plain `cargo test` never pays the criterion build — run
//! behind `--features bench-util`).
//!
//! The wiring-relevant question is the per-hit cost of the two surfaces
//! the dev server renders on every `/__oxide` (HTML) and `/__oxide/state`
//! (JSON) view, now that the live (F05) and generation (F10) tables ride
//! the same state — plus, since the module-graph search landed, the
//! search the `?q=` arm pays per hit:
//!
//! - `inspector_state/{routes}`: building the `InspectorState` from a
//!   boot-shaped `RouteTableInspector` — the route clone plus the live
//!   and generation rows the callers wired.
//! - `inspector_html/{routes}`: the full self-contained HTML render of
//!   the same state (route/live/generation/waterfall tables, the cell
//!   escaping).
//! - `module_search/{modules}`: [`oxide_fw_dev::search_module_graph`]
//!   over the harvest-shaped graph (module → importers) at the query the
//!   `?q=` arm passes — the per-hit cost of the search, the case-insensitive
//!   scan over the module and importer paths.
//! - `inspector_html_search/{modules}`: the filtered render — the search
//!   plus the narrowed modules table and the `k of n` note.
//!
//! Run with:
//!
//! ```text
//! cargo bench -p oxide-fw-dev --features bench-util -- inspector
//! ```

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_dev::{
    GenerationRow, LiveRow, RouteTableInspector, render_inspector_html,
    render_inspector_html_with_query, search_module_graph,
};

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

/// The harvest-shaped graph (§V2-E08 item 2): `n` modules, each with two
/// importer edges (the CLI's `BTreeMap` walk shape, sorted by module).
fn module_graph(n: usize) -> Vec<(String, Vec<String>)> {
    (0..n)
        .map(|i| {
            let module = if i % 4 == 0 {
                format!("src/components/comp{i}.tsx")
            } else {
                format!("src/lib/mod{i}.ts")
            };
            let importers = vec![
                "src/app.tsx".to_string(),
                format!("src/routes/r{}/page.tsx", i % 8),
            ];
            (module, importers)
        })
        .collect()
}

fn bench_module_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("module_search");
    for n in [256usize, 1024] {
        let g = module_graph(n);
        // The narrow query (a component name) and the wide one (the `src/`
        // prefix every row matches) — the two shapes the search box sees.
        group.bench_with_input(BenchmarkId::from_parameter(n), &g, |b, g| {
            b.iter(|| {
                black_box(search_module_graph(g, "comp"));
                black_box(search_module_graph(g, "src/"))
            });
        });
    }
    group.finish();
}

fn bench_inspector_html_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("inspector_html_search");
    for n in [256usize, 1024] {
        let state = RouteTableInspector::from_router_table(Vec::new())
            .with_module_graph(module_graph(n))
            .state();
        group.bench_with_input(BenchmarkId::from_parameter(n), &state, |b, state| {
            b.iter(|| black_box(render_inspector_html_with_query(state, Some("comp"))));
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_inspector_state,
    bench_inspector_html,
    bench_module_search,
    bench_inspector_html_search
);
criterion_main!(benches);

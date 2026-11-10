//! Criterion bench for the V2-E01 HMR boundary planner (the B09 pattern:
//! plain `cargo test` never pays the criterion build — run behind
//! `--features bench-util`).
//!
//! The wiring-relevant question is the per-watcher-tick cost of the planner
//! the `cmd_dev` fan-out runs on every component-relevant file event, across
//! the graph shapes a real dev tree produces:
//!
//! - `deep_chain`: a 50-deep non-boundary chain ending at one component
//!   boundary — the walk-up the one-hop simplification never did.
//! - `wide_fanout`: one changed module with 100 direct component importers.
//! - `diamond`: the shared-boundary case (dedup + sort dominate).
//! - `reload_fallback`: the orphan module (the `FullReload` early return).
//! - `css_terminal` / `css_edge`: the phase-2 classified planner over a
//!   stylesheet change — the §13.3 collapse (orphan) and the own-boundary
//!   shape (post-harvest), against a Js control on the same deep chain.
//!
//! Run with:
//!
//! ```text
//! cargo bench -p oxide-fw-dev --features bench-util
//! ```

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_dev::ChangeKind;
use oxide_fw_dev::boundary::plan_classified;
use oxide_fw_dev::plan;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// `src/`-rooted path helper.
fn p(rel: &str) -> PathBuf {
    PathBuf::from(format!("src/{rel}"))
}

/// Deep non-boundary chain: `src/lib/util.ts <- mod_1 ... mod_{n}` (no
/// module in the chain self-accepts) with one component boundary at the top.
fn deep_chain(depth: usize) -> (PathBuf, HashMap<PathBuf, Vec<PathBuf>>, HashSet<PathBuf>) {
    let mut importers: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for i in 0..depth {
        let module = if i == 0 {
            p("lib/util.ts")
        } else {
            p(&format!("lib/chain/mod_{i}.ts"))
        };
        let importer = if i + 1 == depth {
            p("routes/index/page.tsx")
        } else {
            p(&format!("lib/chain/mod_{}.ts", i + 1))
        };
        importers.insert(module, vec![importer]);
    }
    let accepting: HashSet<PathBuf> = [p("routes/index/page.tsx")].into_iter().collect();
    (p("lib/util.ts"), importers, accepting)
}

/// One changed module imported by `n` component boundaries.
fn wide_fanout(n: usize) -> (PathBuf, HashMap<PathBuf, Vec<PathBuf>>, HashSet<PathBuf>) {
    let mut importers: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    let mut accepting: HashSet<PathBuf> = HashSet::new();
    let mut targets = Vec::with_capacity(n);
    for i in 0..n {
        let comp = p(&format!("routes/section{i}/page.tsx"));
        targets.push(comp.clone());
        accepting.insert(comp);
    }
    importers.insert(p("lib/util.ts"), targets);
    (p("lib/util.ts"), importers, accepting)
}

/// Diamond: two mid boundaries both reaching one shared top boundary.
fn diamond() -> (PathBuf, HashMap<PathBuf, Vec<PathBuf>>, HashSet<PathBuf>) {
    let mut importers: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    importers.insert(p("lib/util.ts"), vec![p("comp/a.tsx"), p("comp/b.tsx")]);
    importers.insert(p("comp/a.tsx"), vec![p("routes/index/page.tsx")]);
    importers.insert(p("comp/b.tsx"), vec![p("routes/index/page.tsx")]);
    let accepting: HashSet<PathBuf> = [
        p("comp/a.tsx"),
        p("comp/b.tsx"),
        p("routes/index/page.tsx"),
    ]
    .into_iter()
    .collect();
    (p("lib/util.ts"), importers, accepting)
}

fn bench_deep_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("boundary_deep_chain");
    for depth in [10, 50, 200] {
        let (changed, importers, accepting) = deep_chain(depth);
        group.bench_with_input(BenchmarkId::from_parameter(depth), &depth, |b, _| {
            b.iter(|| plan(&changed, &importers, &accepting, &HashSet::new()))
        });
    }
    group.finish();
}

fn bench_wide_fanout(c: &mut Criterion) {
    let mut group = c.benchmark_group("boundary_wide_fanout");
    for n in [10, 100] {
        let (changed, importers, accepting) = wide_fanout(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| plan(&changed, &importers, &accepting, &HashSet::new()))
        });
    }
    group.finish();
}

fn bench_diamond(c: &mut Criterion) {
    let mut group = c.benchmark_group("boundary_diamond");
    let (changed, importers, accepting) = diamond();
    group.bench_function("shared_top_boundary", |b| {
        b.iter(|| plan(&changed, &importers, &accepting, &HashSet::new()))
    });
    group.finish();
}

fn bench_reload_fallback(c: &mut Criterion) {
    let mut group = c.benchmark_group("boundary_reload_fallback");
    let importers: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    group.bench_function("orphan_module", |b| {
        b.iter(|| {
            plan(
                Path::new("src/lib/orphan.ts"),
                &importers,
                &HashSet::new(),
                &HashSet::new(),
            )
        })
    });
    group.finish();
}

/// The phase-2 shapes: a stylesheet change through `plan_classified`. The
/// orphan case exercises the §13.3 collapse (the walk returns a reload, the
/// terminal rewrites it to the swap); the edge case stops at the stylesheet
/// boundary the harvest marked via `css_self_accepts`.
fn bench_css_terminal(c: &mut Criterion) {
    let mut group = c.benchmark_group("boundary_css_terminal");
    let importers: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    group.bench_function("orphan_stylesheet_collapse", |b| {
        b.iter(|| {
            plan_classified(
                ChangeKind::Css,
                Path::new("src/styles/main.css"),
                &importers,
                &HashSet::new(),
                &HashSet::new(),
            )
        })
    });
    group.finish();
}

fn bench_css_edge(c: &mut Criterion) {
    let mut group = c.benchmark_group("boundary_css_edge");
    let mut importers: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    importers.insert(p("styles/main.css"), vec![p("routes/index/page.tsx")]);
    let accepting: HashSet<PathBuf> = [p("styles/main.css")].into_iter().collect();
    group.bench_function("stylesheet_own_boundary", |b| {
        b.iter(|| {
            plan_classified(
                ChangeKind::Css,
                &p("styles/main.css"),
                &importers,
                &accepting,
                &HashSet::new(),
            )
        })
    });
    group.finish();
}

fn bench_classified_js(c: &mut Criterion) {
    // The Js control: the classified planner must cost the same as the plain
    // walk for a component change (one `update_kind` map + one pass-through).
    let mut group = c.benchmark_group("boundary_classified_js");
    let (changed, importers, accepting) = deep_chain(50);
    group.bench_function("deep_chain_50_js", |b| {
        b.iter(|| {
            plan_classified(
                ChangeKind::Component,
                &changed,
                &importers,
                &accepting,
                &HashSet::new(),
            )
        })
    });
    group.finish();
}

criterion_group!(
    boundary,
    bench_deep_chain,
    bench_wide_fanout,
    bench_diamond,
    bench_reload_fallback,
    bench_css_terminal,
    bench_css_edge,
    bench_classified_js
);
criterion_main!(boundary);

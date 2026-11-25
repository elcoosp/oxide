//! Criterion benches for the PWA worker emitter (V2-F14, `bench-util`
//! gated — plain `cargo test` never pays the criterion build).
//!
//! The bench-relevant question: the emitter runs once per build, but the
//! plan fold walks the whole manifest (every route's chunk/preload/css
//! lists plus the asset map) and the source render walks the precache
//! list — both must stay proportional to the dist, not the codebase, and
//! stay noise next to the bundling they ride. The groups:
//! - `pwa_plan/routes_8` / `pwa_plan/routes_32`: the shell-only fold at
//!   the smoke-app and full-site shell counts.
//! - `pwa_plan/all_32`: the `all` breadth over a 32-route manifest (the
//!   route-map walk + the asset union).
//! - `pwa_plan/fallback_gap_32`: the same shape with an override
//!   `sw.offlineFallback` the shells do not carry — the r9-K3 contract's
//!   plan-side insert (the fallback joins the set undeduped).
//! - `pwa_source`: the deterministic render over a 32-route precache list.

use std::collections::{BTreeMap, BTreeSet};
use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_bundler::pwa::{ShellSource, plan_precache, sw_source};
use oxide_fw_bundler::{ClientManifest, RouteBundle};
use oxide_fw_config::{OfflineConfig, OfflinePrecache, SwConfig};

/// A manifest with `routes` route bundles (each with an entry, two
/// preloads and a css file) and one shared asset — the shape the emitter
/// folds.
fn manifest_fixture(routes: usize) -> ClientManifest {
    let mut map = BTreeMap::new();
    for i in 0..routes {
        map.insert(
            format!("routes/r{i}/page"),
            RouteBundle {
                entry_chunk: format!("r{i}-a1b2c3d4.js"),
                preload: vec![format!("preload-r{i}.js"), "shared-e5f6a7b8.js".to_string()],
                css: vec![format!("r{i}-c9d0e1f2.css")],
                js_bytes: 120,
                modules: Vec::new(),
                layout_chain: Vec::new(),
            },
        );
    }
    let mut assets = BTreeMap::new();
    assets.insert(
        "src/styles/global.css".to_string(),
        "global-1a2b3c4d.css".to_string(),
    );
    ClientManifest {
        build_id: "b17e5f0a4c9d2e8f".to_string(),
        routes: map,
        assets,
        integrity: BTreeMap::new(),
    }
}

struct FixedShells(BTreeSet<String>);

impl ShellSource for FixedShells {
    fn shell_urls(&self) -> BTreeSet<String> {
        self.0.clone()
    }
}

fn shells_fixture(routes: usize) -> FixedShells {
    let mut shells = BTreeSet::new();
    shells.insert("/".to_string());
    shells.insert("/offline".to_string());
    for i in 0..routes {
        shells.insert(format!("/r{i}"));
    }
    FixedShells(shells)
}

fn bench_plan(c: &mut Criterion) {
    let mut g = c.benchmark_group("pwa_plan");
    let offline_routes = OfflineConfig {
        enabled: true,
        precache: OfflinePrecache::Routes,
        outbox: true,
    };
    let offline_all = OfflineConfig {
        enabled: true,
        precache: OfflinePrecache::All,
        outbox: true,
    };
    let sw = SwConfig::default();
    for count in [8, 32] {
        let manifest = manifest_fixture(count);
        let shells = shells_fixture(count);
        g.bench_with_input(
            BenchmarkId::new("routes", count),
            &(&manifest, &shells),
            |b, (m, sh)| b.iter(|| plan_precache(black_box(m), &sw, &offline_routes, sh)),
        );
    }
    let manifest = manifest_fixture(32);
    let shells = shells_fixture(32);
    g.bench_with_input("all_32", &(&manifest, &shells), |b, (m, sh)| {
        b.iter(|| plan_precache(black_box(m), &sw, &offline_all, sh));
    });
    // The r9-K3 contract's plan-side cost: the fallback joins the set —
    // the override spelling the shells do NOT carry (one insert, no
    // dedup), the shape a site with an explicit `sw.offlineFallback`
    // rides on every build.
    let sw_gap = SwConfig {
        offline_fallback: Some("/help-offline".to_string()),
        ..SwConfig::default()
    };
    g.bench_with_input("fallback_gap_32", &(&manifest, &shells), |b, (m, sh)| {
        b.iter(|| plan_precache(black_box(m), &sw_gap, &offline_all, sh));
    });
    g.finish();
}

fn bench_sw_source(c: &mut Criterion) {
    let mut g = c.benchmark_group("pwa_source");
    let manifest = manifest_fixture(32);
    let shells = shells_fixture(32);
    let offline = OfflineConfig {
        enabled: true,
        precache: OfflinePrecache::All,
        outbox: true,
    };
    let plan = plan_precache(&manifest, &SwConfig::default(), &offline, &shells);
    g.bench_function("all_32", |b| b.iter(|| sw_source(black_box(&plan))));
    g.finish();
}

criterion_group!(pwa, bench_plan, bench_sw_source);
criterion_main!(pwa);

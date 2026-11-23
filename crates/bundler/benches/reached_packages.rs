//! Criterion benches for the V2-F20 module→package map
//! (`oxide_fw_bundler::packages_reached`, `bench-util` gated like the
//! E04/A02/B09/F18 benches — plain `cargo test` never pays the criterion
//! build).
//!
//! The bench-relevant question (the map rides every `write_attestation`
//! call once, over the union of every route's source modules): how does
//! the per-module classification cost scale with the route count and the
//! node_modules nesting depth? The walk is one `Path::components` pass per
//! module plus a `BTreeSet` insert per distinct package — allocation-bounded
//! by the module count, with the set's dedup as the only allocation beyond
//! the input slice.
//!
//! - `packages_reached/{100,500}`: the classification over a synthetic
//!   route's module set at two sizes — 100 modules (a typical app route's
//!   transitive deps) and 500 (a heavy route that pulls a deep vendor
//!   tree). Half the modules are first-party (no `node_modules` segment,
//!   contributing nothing), a quarter are bare packages, and a quarter are
//!   scoped packages under the pnpm virtual store layout — the three
//!   arms the classifier walks.
//! - `reached_packages/10-routes`: the `ClientManifest::reached_packages`
//!   aggregate over a ten-route manifest, each route carrying a 50-module
//!   set with overlapping packages — the one-read fold the attest calls.

use std::hint::black_box;
use std::path::PathBuf;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_bundler::{ClientManifest, RouteBundle, packages_reached};

/// A synthetic module set of `n` entries: half first-party, a quarter bare
/// `node_modules/<pkg>` packages, a quarter scoped packages under the pnpm
/// virtual store — the three classifier arms in the proportions a real
/// route's transitive deps roughly carry.
fn module_set(n: usize) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let arm = i % 4;
        let module = match arm {
            0 | 1 => format!("/proj/src/component-{i}.tsx"),
            2 => format!("/proj/node_modules/pkg-{i}/index.js"),
            // A scoped package under the pnpm virtual store — the nested
            // `node_modules/.pnpm/<pkg>@<ver>/node_modules/<pkg>` layout.
            3 => format!(
                "/proj/node_modules/.pnpm/@scope/pkg-{i}@1.0.0/node_modules/@scope/pkg-{i}/sub.js"
            ),
            _ => unreachable!("arm is i % 4"),
        };
        out.push(PathBuf::from(module));
    }
    out
}

/// A ten-route manifest, each route carrying a 50-module set with
/// overlapping packages (every route reaches `pkg-shared` plus its own
/// `pkg-<route>` — the dedup the aggregate folds).
fn ten_route_manifest() -> ClientManifest {
    let mut routes = std::collections::BTreeMap::new();
    for r in 0..10 {
        let mut modules = vec![PathBuf::from("/proj/node_modules/pkg-shared/index.js")];
        for m in module_set(49) {
            modules.push(m);
        }
        // The route's own package, so each route contributes one distinct
        // name the set did not already carry.
        modules.push(PathBuf::from(format!(
            "/proj/node_modules/pkg-route-{r}/index.js"
        )));
        routes.insert(
            format!("route-{r}"),
            RouteBundle {
                entry_chunk: format!("route-{r}.js"),
                modules,
                ..RouteBundle::default()
            },
        );
    }
    ClientManifest {
        routes,
        ..ClientManifest::default()
    }
}

fn bench_packages_reached(c: &mut Criterion) {
    let mut g = c.benchmark_group("packages_reached");
    for size in [100, 500] {
        let modules = module_set(size);
        g.throughput(criterion::Throughput::Elements(size as u64));
        g.bench_with_input(BenchmarkId::from_parameter(size), &modules, |b, modules| {
            b.iter(|| packages_reached(black_box(modules)));
        });
    }
    g.finish();
}

fn bench_reached_packages(c: &mut Criterion) {
    let manifest = ten_route_manifest();
    let mut g = c.benchmark_group("reached_packages");
    g.throughput(criterion::Throughput::Elements(10));
    g.bench_function("10-routes", |b| {
        b.iter(|| black_box(&manifest).reached_packages());
    });
    g.finish();
}

criterion_group!(benches, bench_packages_reached, bench_reached_packages);
criterion_main!(benches);

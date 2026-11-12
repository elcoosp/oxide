//! Criterion bench for the §V2-F11 delta-deploy engine — the costs a
//! deploy pays on every invocation and the ones only the first deploy
//! pays.
//!
//! The shapes: the pure [`plan`] diff over a mid-size ledger (the
//! per-deploy planning cost: 500 hashed files, the steady state where
//! most are `keep`), the full first deploy through [`DirStore`] (the
//! hashing + write cost a fresh store pays) and the delta re-deploy (the
//! steady-state cost: one changed chunk, the rest kept).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::hint::black_box;
use std::path::Path;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_adapters::{
    DEFAULT_SKEW_WINDOW, DeployOptions, DirStore, FileEntry, PruneOptions, deploy_dir, plan, prune,
    scan_dist,
};

/// A deterministic pseudo-random body (no RNG dep): the bytes are a
/// function of the index, so a "changed" file differs predictably.
fn body(n: usize) -> String {
    let mut s = String::with_capacity(1024);
    for i in 0..32 {
        let _ = writeln!(s, "chunk {n} line {i} padding padding");
    }
    s
}

/// Write `n` immutable hashed chunks + one HTML page under `dist/`.
fn make_dist(dist: &Path, n: usize) -> u64 {
    std::fs::create_dir_all(dist.join("assets")).expect("dist mkdir");
    for i in 0..n {
        std::fs::write(
            dist.join(format!("assets/chunk-{i:04}.deadbeef.js")),
            body(i),
        )
        .expect("chunk write");
    }
    std::fs::write(dist.join("index.html"), "<html>index</html>").expect("html write");
    n as u64 * 1024 + 17
}

/// The local scan as a ledger-shaped map (rel → hash), for the pure plan
/// bench.
fn ledger(dist: &Path) -> BTreeMap<String, FileEntry> {
    scan_dist(dist).expect("the scan walks")
}

fn bench_plan(c: &mut Criterion) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().join("dist");
    let total = make_dist(&dist, 500);
    let local = ledger(&dist);
    let remote: BTreeMap<String, String> = local
        .iter()
        .map(|(rel, e)| (rel.clone(), e.hash.clone()))
        .collect();

    let mut group = c.benchmark_group("delta");
    group.bench_function(BenchmarkId::new("plan_500_files_all_keep", total), |b| {
        b.iter(|| plan(black_box(&local), black_box(&remote)));
    });

    // The changed-build shape: one hash flips, the rest keep.
    let mut changed = remote.clone();
    changed.insert(
        "assets/chunk-0007.deadbeef.js".to_string(),
        "flipped".to_string(),
    );
    changed.insert("assets/gone.deadbeef.js".to_string(), "orphan".to_string());
    group.bench_function(BenchmarkId::new("plan_500_files_one_change", total), |b| {
        b.iter(|| plan(black_box(&local), black_box(&changed)));
    });
    group.finish();
}

fn bench_deploy_dir(c: &mut Criterion) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().join("dist");
    let total = make_dist(&dist, 200);

    let mut group = c.benchmark_group("delta");
    group.throughput(criterion::Throughput::Bytes(total));
    // The full first deploy: scan + hash every file + write every object.
    group.bench_function("deploy_first_full_200_files", |b| {
        b.iter_batched(
            || tempfile::tempdir_in(tmp.path()).expect("store tempdir"),
            |store_tmp| {
                let store = DirStore::new(store_tmp.path().join("store"));
                deploy_dir(
                    &dist,
                    &store,
                    &DeployOptions {
                        build_id: "0123456789abcdef".to_string(),
                        now_ms: 1_000,
                    },
                )
                .expect("the deploy lands")
            },
            criterion::BatchSize::SmallInput,
        );
    });

    // The delta re-deploy: the ledger matches except one chunk, so the
    // write set is one immutable upload + the HTML mirror.
    let store_tmp = tempfile::tempdir_in(tmp.path()).expect("store tempdir");
    let store = DirStore::new(store_tmp.path().join("store"));
    deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: "0123456789abcdef".to_string(),
            now_ms: 1_000,
        },
    )
    .expect("the baseline deploy lands");
    std::fs::write(dist.join("assets/chunk-0007.deadbeef.js"), body(10_007))
        .expect("chunk rewrite");
    group.bench_function("deploy_delta_one_change_200_files", |b| {
        b.iter(|| {
            deploy_dir(
                &dist,
                black_box(&store),
                &DeployOptions {
                    build_id: "2222222222222222".to_string(),
                    now_ms: 2_000,
                },
            )
            .expect("the delta deploy lands");
        });
    });

    // Prune with nothing expired (the steady-state cost: a ledger walk).
    deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: "3333333333333333".to_string(),
            now_ms: 3_000,
        },
    )
    .expect("the v3 deploy lands");
    group.bench_function("prune_young_marks", |b| {
        b.iter(|| {
            prune(
                black_box(&store),
                &PruneOptions {
                    now_ms: 3_000,
                    window: DEFAULT_SKEW_WINDOW,
                },
            )
            .expect("the prune runs");
        });
    });
    group.finish();
}

/// Wall-clock guard the bench harness needs no part of: the engine takes
/// a skew window as a plain `Duration` (the F10 config value flows in
/// unchanged).
const _: Duration = DEFAULT_SKEW_WINDOW;

criterion_group!(delta_benches, bench_plan, bench_deploy_dir);
criterion_main!(delta_benches);

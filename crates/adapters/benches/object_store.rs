//! Criterion bench for the object-store deploy target (§V2-F11 remaining
//! half) — the same shapes the `delta` group runs through [`DirStore`],
//! re-run through the [`ObjectStoreStore`] seam over `object_store`'s
//! in-memory backend: the full first deploy (the scan + hash + per-object
//! put cost) and the steady-state re-run (the idempotent no-op) plus the
//! ledger/pointer round-trip the swap pays.

use std::hint::black_box;
use std::path::Path;

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_adapters::{
    DEFAULT_SKEW_WINDOW, DeployOptions, ObjectStoreStore, PruneOptions, deploy_dir, prune,
};

/// A deterministic pseudo-random body (no RNG dep): the bytes are a
/// function of the index, so a "changed" file differs predictably.
fn body(n: usize) -> String {
    let mut s = String::with_capacity(1024);
    for i in 0..32 {
        let _ = std::fmt::Write::write_fmt(
            &mut s,
            format_args!("chunk {n} line {i} padding padding\n"),
        );
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

fn bench_deploy_object_store(c: &mut Criterion) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().join("dist");
    let total = make_dist(&dist, 200);

    let mut group = c.benchmark_group("object_store");
    group.throughput(criterion::Throughput::Bytes(total));
    // The full first deploy through the in-memory backend: scan + hash
    // every file + one object-store put per object (fresh store per
    // iteration, the setup cost excluded by `iter_batched`).
    group.bench_function("deploy_first_full_200_files", |b| {
        b.iter_batched(
            || ObjectStoreStore::new_in_memory().expect("the mem:// target constructs"),
            |store| {
                deploy_dir(
                    &dist,
                    black_box(&store),
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

    // The steady-state re-run: v2 landed once outside the loop, so every
    // iteration is the idempotent no-op (ledger read + pointer read, zero
    // uploads — the per-deploy floor an `oxide deploy` pays).
    let store = ObjectStoreStore::new_in_memory().expect("the mem:// target constructs");
    deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: "0123456789abcdef".to_string(),
            now_ms: 1_000,
        },
    )
    .expect("the baseline deploy lands");
    deploy_dir(
        &dist,
        &store,
        &DeployOptions {
            build_id: "2222222222222222".to_string(),
            now_ms: 2_000,
        },
    )
    .expect("the v2 deploy lands");
    group.bench_function("deploy_redo_idempotent_200_files", |b| {
        b.iter(|| {
            deploy_dir(
                &dist,
                black_box(&store),
                &DeployOptions {
                    build_id: "2222222222222222".to_string(),
                    now_ms: 2_000,
                },
            )
            .expect("the idempotent re-run lands");
        });
    });

    // Prune with nothing expired (the steady-state ledger walk).
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

criterion_group!(object_store_benches, bench_deploy_object_store);
criterion_main!(object_store_benches);

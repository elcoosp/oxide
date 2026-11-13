//! Criterion bench for the §V2-B07 parallel-precompile engine — the
//! orchestration cost per job and the overlap structure the WP exists to
//! create.
//!
//! The shapes: the engine at the spec's window sizing for a 2-worker pool
//! (width 4) against the sequential baseline (width 1) over the same 20
//! fake-compilation jobs whose latency models a tooling-pool round trip —
//! the structure the cold-build acceptance (a 200-Svelte-component fixture
//! at least 4x faster on 8 cores) rides — plus the pure orchestration
//! overhead on 200 instant jobs (no fake latency: the engine's own cost).
//!
//! The cold-build e2e number belongs to the CLI adoption's fixture build
//! (the real tooling pool); this bench pins the engine.

use std::path::PathBuf;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_compiler::precompile_engine::{PrecompileJob, compile_jobs};

fn jobs(n: usize) -> Vec<PrecompileJob> {
    (0..n)
        .map(|i| PrecompileJob {
            path: PathBuf::from(format!("src/routes/comp-{i:03}.vue")),
            kind: "vue",
            source: format!("<template><p>{i}</p></template>"),
        })
        .collect()
}

fn bench_precompile_engine(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("the bench runtime builds");

    let mut group = c.benchmark_group("precompile_engine");

    // The overlap structure: 20 jobs, each parking on a 10 ms fake
    // tooling round trip. Width 1 is the sequential baseline (the
    // pre-B07 shape); width 4 is the spec's sizing for a 2-worker pool —
    // the wall-clock difference is the overlap the engine buys.
    for width in [1, 4] {
        group.throughput(criterion::Throughput::Elements(20));
        group.bench_function(format!("fake_pool_20_jobs_width_{width}"), |b| {
            b.iter(|| {
                let batch = jobs(20);
                rt.block_on(async {
                    let rows = compile_jobs(batch, width, |job| async move {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        Ok::<String, String>(format!("js for {}", job.path.display()))
                    })
                    .await;
                    assert_eq!(rows.len(), 20);
                });
            });
        });
    }

    // The pure orchestration overhead: 200 instant jobs (no fake latency)
    // through the spec's window — the engine's own per-job cost.
    group.throughput(criterion::Throughput::Elements(200));
    group.bench_function("orchestration_200_instant_jobs", |b| {
        b.iter(|| {
            let batch = jobs(200);
            rt.block_on(async {
                let rows = compile_jobs(batch, 4, |job| async move {
                    Ok::<String, String>(format!("js for {}", job.path.display()))
                })
                .await;
                assert_eq!(rows.len(), 200);
            });
        });
    });

    group.finish();
}

criterion_group!(benches, bench_precompile_engine);
criterion_main!(benches);

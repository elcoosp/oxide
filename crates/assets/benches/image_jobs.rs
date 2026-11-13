//! Criterion bench for the §V2-C09 image job model — the per-request cost
//! the images endpoint pays around an encode, and the single-flight
//! convergence the acceptance names.
//!
//! The shapes: a CAS hit (the lookup the hot path pays when a variant is
//! already stored), the single-flight join (8 waiters on one key — the
//! marginal per-waiter cost once the encode is in flight), and the fresh
//! encode through `spawn_blocking` on a 1×1 payload (the plumbing, not a
//! real encoder's CPU time). A real `oha -c 64` acceptance run belongs to
//! the CLI adoption (the endpoint wiring); this bench pins the model.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_assets::image_jobs::{ImageCas, ImageJobs};

/// An in-memory CAS (the L0 tier the endpoint feeds in).
#[derive(Default)]
struct MapCas(std::sync::Mutex<std::collections::BTreeMap<String, Arc<Vec<u8>>>>);

impl ImageCas for MapCas {
    fn get(&self, key: &str) -> Option<Arc<Vec<u8>>> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn put(&self, key: &str, bytes: Arc<Vec<u8>>) {
        self.0.lock().unwrap().insert(key.to_string(), bytes);
    }
}

fn bench_image_jobs(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("the bench runtime builds");

    let mut group = c.benchmark_group("image_jobs");
    group.throughput(criterion::Throughput::Elements(1));

    // The CAS hit: what every repeat request for a stored variant pays.
    let cas = Arc::new(MapCas::default());
    cas.put("warm-key", Arc::new(b"cached".to_vec()));
    let jobs = ImageJobs::new(cas);
    group.bench_function("cas_hit", |b| {
        b.iter(|| {
            rt.block_on(async {
                let bytes = jobs
                    .run(
                        "warm-key",
                        false,
                        || -> Result<Vec<u8>, oxide_fw_diagnostics::OxideError> {
                            unreachable!("a CAS hit never runs the job")
                        },
                    )
                    .await
                    .expect("the stored bytes answer");
                assert_eq!(bytes.as_ref(), b"cached");
            });
        });
    });

    // The fresh encode path: admission (lane), spawn_blocking, CAS store —
    // a trivial payload so the bench measures the PLUMBING per request.
    let jobs_fresh = ImageJobs::new(Arc::new(MapCas::default()));
    let seq = AtomicUsize::new(0);
    group.bench_function("fresh_encode", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = format!("bench-{}", seq.fetch_add(1, Ordering::SeqCst));
                jobs_fresh
                    .run(&key, false, || Ok(b"tiny".to_vec()))
                    .await
                    .expect("the encode succeeds");
            });
        });
    });

    // The single-flight convergence: 8 waiters join one encode — the
    // marginal cost per waiter after the leader is in flight.
    let jobs_dedupe = Arc::new(ImageJobs::new(Arc::new(MapCas::default())));
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_for_job = Arc::clone(&counter);
    group.bench_function("single_flight_join_8", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut handles = Vec::new();
                for _ in 0..8 {
                    let jobs = Arc::clone(&jobs_dedupe);
                    let counter = Arc::clone(&counter_for_job);
                    handles.push(tokio::spawn(async move {
                        jobs.run("dedupe-key", false, move || {
                            counter.fetch_add(1, Ordering::SeqCst);
                            Ok(b"once".to_vec())
                        })
                        .await
                    }));
                }
                for h in handles {
                    h.await.unwrap().expect("the shared encode answers");
                }
            });
        });
    });
    // The model's whole point: one encode per burst.
    let encodes = counter.load(Ordering::SeqCst);
    assert!(
        encodes <= 2,
        "single-flight dedupes the burst (got {encodes})"
    );

    group.finish();
}

criterion_group!(benches, bench_image_jobs);
criterion_main!(benches);

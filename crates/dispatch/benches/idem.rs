//! Criterion bench for the §V2-F14 idempotency stage — the per-request
//! cost the offline outbox's `x-oxide-idem` replay key adds to a POST.
//!
//! The shapes: the MISS path (store probe → handler → the store-time
//! set-cookie projection + the 24 h put — the first submit's cost), the
//! HIT path (store probe → the stored response handed back, the handler
//! never runs — the replay's cost, the stage's point) and the
//! pass-through (no header, the stage is invisible).

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use oxide_fw_diagnostics::OxideError;
use oxide_fw_dispatch::idem::{
    ExecutedResponse, IdempotencyStore, IdempotencyVerdict, StoredResponse, stage_idempotency,
};

/// The in-memory store (the L1 fake; the same shape the tests drive).
#[derive(Default)]
struct MemoryStore {
    entries: std::sync::Mutex<std::collections::BTreeMap<String, StoredResponse>>,
}

impl IdempotencyStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<StoredResponse>, OxideError> {
        Ok(self
            .entries
            .lock()
            .expect("the bench store lock")
            .get(key)
            .cloned())
    }

    fn put(&self, key: &str, response: &StoredResponse, _ttl_secs: u64) -> Result<(), OxideError> {
        self.entries
            .lock()
            .expect("the bench store lock")
            .insert(key.to_string(), response.clone());
        Ok(())
    }
}

/// The handler the stage drives: a session-bearing JSON answer (the
/// set-cookie strip is part of the store-time projection).
async fn handler() -> Result<ExecutedResponse, OxideError> {
    Ok(ExecutedResponse {
        status: 200,
        headers: vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("set-cookie".to_string(), "sid=abc; Secure".to_string()),
        ],
        body: vec![0x7B; 512],
    })
}

fn bench_stage(c: &mut Criterion) {
    let mut group = c.benchmark_group("idem_stage");

    // The MISS path: the first submit pays the probe, the handler and
    // the put — each iteration a fresh key so every iteration misses.
    // The store clears at EVERY routine invocation (criterion may run
    // the routine more than once; the counter restarts with it, so a
    // persisted entry would turn the miss into a replay).
    let store = MemoryStore::default();
    group.bench_function("miss_first_submit", |b| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("the bench runtime");
        store.entries.lock().expect("the bench store lock").clear();
        let mut n = 0u64;
        b.iter(|| {
            n += 1;
            let key = format!("submit-{n}");
            runtime.block_on(async {
                let verdict = stage_idempotency(black_box(&store), Some(&key), || handler())
                    .await
                    .expect("the bench stage runs");
                assert!(
                    matches!(verdict, IdempotencyVerdict::Executed(_)),
                    "the miss bench got {verdict:?} for key {key}"
                );
            });
        });
    });

    // The HIT path: the replay after a warm entry — the handler never
    // runs (the stage's point: an offline retry replays, never re-runs).
    let warm = MemoryStore::default();
    warm.put(
        // The key the stage probes: the `idem:` prefix (the same
        // spelling `idem_key` produces from the header).
        "idem:submit",
        &StoredResponse {
            status: 200,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: vec![0x7B; 512],
        },
        86_400,
    )
    .expect("the warm entry stores");
    group.bench_function("hit_replay", |b| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("the bench runtime");
        b.iter(|| {
            runtime.block_on(async {
                let verdict = stage_idempotency(black_box(&warm), Some("submit"), || handler())
                    .await
                    .expect("the bench stage runs");
                assert!(matches!(verdict, IdempotencyVerdict::Replayed(_)));
            });
        });
    });

    // The pass-through: no `x-oxide-idem` header, the stage is a
    // probe-free pass to the handler.
    group.bench_function("passthrough_no_header", |b| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("the bench runtime");
        b.iter(|| {
            runtime.block_on(async {
                let verdict = stage_idempotency(black_box(&store), None, || handler())
                    .await
                    .expect("the bench stage runs");
                assert!(matches!(verdict, IdempotencyVerdict::Executed(_)));
            });
        });
    });

    group.finish();
}

criterion_group!(benches, bench_stage);
criterion_main!(benches);

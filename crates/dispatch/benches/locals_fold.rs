//! V2-A05 step 4 bench: the per-request locals fold the pipeline rides —
//! the adopted `RequestLocals` shape (session-slot extraction +
//! `apply_middleware_locals` per layer + one final `to_value()` snapshot,
//! the exact operations `stage_middleware::finish_middleware_stage` runs)
//! against the pre-A05 ad-hoc shape it replaced (a raw `serde_json::Value`
//! map seeded with the session key, each layer's payload merged verbatim,
//! stored verbatim — no reserved-key ownership, no DSP-101 report). Both
//! shapes at `{1, 4, 8}` middleware layers staging one user key.
//!
//! Run: `cargo bench -p oxide-fw-dispatch --features bench-util`
//! (or in test mode: `cargo test -p oxide-fw-dispatch --features
//! bench-util --benches -- --test`).

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_dispatch::locals::RequestLocals;
use std::hint::black_box;

/// The `K` staged locals payloads the fold consumes (one user key per
/// layer — the representative chain shape).
fn staged_layers(layers: usize) -> Vec<serde_json::Value> {
    (0..layers)
        .map(|i| serde_json::json!({ format!("key{i}"): format!("v{i}") }))
        .collect()
}

/// The adopted pipeline shape: seed the slot (the prelude's session),
/// fold every layer through `apply_middleware_locals` (the session
/// extraction + the collision report the stage runs), then ONE final
/// `to_value()` snapshot.
fn fold_pipeline(layers: &[serde_json::Value]) -> serde_json::Value {
    let mut locals = RequestLocals::default();
    locals.session = Some(serde_json::json!({ "user": "ada" }));
    for payload in layers {
        let mut rest = payload.clone();
        let staged_session = rest.get("session").cloned();
        if let Some(obj) = rest.as_object_mut() {
            obj.remove("session");
        }
        if let Some(sess) = staged_session {
            locals.session = Some(sess);
        }
        let collisions = locals.apply_middleware_locals(rest);
        black_box(collisions);
    }
    locals.to_value()
}

/// The pre-A05 ad-hoc shape: a raw `Value` map seeded with the session
/// key, every layer's payload merged verbatim (later-wins insert), stored
/// verbatim as the consumers' snapshot.
fn fold_adhoc(layers: &[serde_json::Value]) -> serde_json::Value {
    let mut middleware_locals = serde_json::json!({});
    middleware_locals["session"] = serde_json::json!({ "user": "ada" });
    for payload in layers {
        if let (Some(into), Some(from)) = (middleware_locals.as_object_mut(), payload.as_object()) {
            for (k, v) in from {
                into.insert(k.clone(), v.clone());
            }
        }
    }
    middleware_locals
}

fn bench_locals_fold(c: &mut Criterion) {
    let mut group = c.benchmark_group("locals_fold");
    for layers in [1usize, 4, 8] {
        let staged = staged_layers(layers);
        group.bench_with_input(
            BenchmarkId::new("pipeline_shape", layers),
            &staged,
            |b, staged| b.iter(|| fold_pipeline(black_box(staged))),
        );
        group.bench_with_input(
            BenchmarkId::new("adhoc_shape", layers),
            &staged,
            |b, staged| b.iter(|| fold_adhoc(black_box(staged))),
        );
    }
    group.finish();
}

criterion_group!(benches, bench_locals_fold);
criterion_main!(benches);

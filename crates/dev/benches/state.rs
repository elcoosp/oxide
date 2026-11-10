//! Criterion bench for the V2-E01 phase-4 state-preservation substrate
//! (the B09 pattern: plain `cargo test` never pays the criterion build —
//! run behind `--features bench-util`).
//!
//! The wiring-relevant question is the per-watcher-tick cost of the
//! preservation seam the `cmd_dev` fan-out runs on every publish:
//!
//! - `state_preservation/verdict_batch_{1,16,64}`: the
//!   [`update_frame`] seam end to end — the policy verdict for an
//!   N-boundary `Update` plan plus the frame build (the serde body is
//!   benchmarked by the `wire_encode` group; this group isolates the
//!   seam's own decision cost, which must be noise against the walk).
//! - `state_preservation/verdict_full_reload`: the `Clear` path — the
//!   planner's reason clones into the verdict (the only allocation the
//!   clear path makes).
//! - `state_preservation/verdict_serde`: the verdict's own wire cost —
//!   the internally-tagged JSON the client narrows on.
//!
//! Run with:
//!
//! ```text
//! cargo bench -p oxide-fw-dev --features bench-util -- state_preservation
//! ```

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_dev::boundary::{ClassifiedPlan, Plan};
use oxide_fw_dev::state::{DefaultStatePolicy, StatePolicy, update_frame};
use oxide_fw_dev::{UpdateEntry, UpdateKind};

/// One v1 entry shaped like the fan-out's (same fixture the wire bench
/// uses, so the two groups' frame bodies are comparable).
fn entry(i: usize) -> UpdateEntry {
    UpdateEntry {
        kind: UpdateKind::Js,
        path: format!("/abs/src/comp/mod_{i}.tsx"),
        accepted_path: format!("/src/comp/mod_{i}.tsx"),
        timestamp: 1_760_000_000_000,
        hash: "0123456789abcdef".to_string(),
    }
}

fn bench_state(c: &mut Criterion) {
    let mut group = c.benchmark_group("state_preservation");
    for n in [1usize, 16, 64] {
        group.bench_with_input(BenchmarkId::new("verdict_batch", n), &n, |b, &n| {
            let plan = ClassifiedPlan {
                kind: UpdateKind::Js,
                plan: Plan::Update(
                    (0..n)
                        .map(|i| std::path::PathBuf::from(format!("src/comp/mod_{i}.tsx")))
                        .collect(),
                ),
            };
            let updates: Vec<UpdateEntry> = (0..n).map(entry).collect();
            b.iter(|| update_frame(&plan, &DefaultStatePolicy, updates.clone()));
        });
    }
    group.bench_function("verdict_full_reload", |b| {
        let plan = ClassifiedPlan {
            kind: UpdateKind::Js,
            plan: Plan::FullReload("src/lib/util.ts is not imported".to_string()),
        };
        b.iter(|| update_frame(&plan, &DefaultStatePolicy, vec![entry(0)]));
    });
    group.bench_function("verdict_serde", |b| {
        let verdict = DefaultStatePolicy.preservation(&ClassifiedPlan {
            kind: UpdateKind::Css,
            plan: Plan::Update(vec!["src/styles/app.css".into()]),
        });
        b.iter(|| serde_json::to_string(&verdict));
    });
    group.finish();
}

criterion_group!(benches, bench_state);
criterion_main!(benches);

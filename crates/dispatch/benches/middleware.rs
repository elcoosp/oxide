//! V2-A05: criterion benchmarks for the middleware-chain and unified-locals
//! substrate (`crates/dispatch`).
//!
//! Two groups:
//! - `chain/*`: [`run_chain`] orchestration cost on zero-work middleware —
//!   the per-layer price the phase-3 `stage_middleware` wiring pays on top
//!   of each real middleware execution (`noop/{1,2,4}`), plus the
//!   locals-merge path every real chain hits (`locals_merge_4`: four
//!   layers each staging one key). These set the baseline the CLI
//!   adoption is compared against, mirroring the `loads/overhead` group.
//! - `locals/*`: [`RequestLocals::to_value`] composition and
//!   [`RequestLocals::apply_middleware_locals`] at a representative
//!   request shape (session + locale + experiments + 8 user keys) — the
//!   per-request price of the unified-locals composition.
//!
//! Run: `cargo bench -p oxide-fw-dispatch --features bench-util`

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_dispatch::chain::{ChainRequest, run_chain};
use oxide_fw_dispatch::locals::RequestLocals;
use oxide_fw_server::{
    MiddlewareDispatchArgs, MiddlewareDispatcher, MiddlewareRunError, MiddlewareRunResult,
};
use std::hint::black_box;
use std::path::PathBuf;
use std::time::Duration;

/// A zero-work middleware: the stand-in for the real worker round trip,
/// reduced to the substrate's own dispatch hop.
struct NoopMiddleware {
    locals_per_call: bool,
}

#[async_trait::async_trait]
impl MiddlewareDispatcher for NoopMiddleware {
    async fn dispatch(
        &self,
        _args: MiddlewareDispatchArgs<'_>,
    ) -> std::result::Result<MiddlewareRunResult, MiddlewareRunError> {
        Ok(if self.locals_per_call {
            MiddlewareRunResult {
                locals: Some(serde_json::json!({ "staged": "once" })),
                ..Default::default()
            }
        } else {
            MiddlewareRunResult::default()
        })
    }
}

/// `n` layer paths for the chain: `routes/mw0.ts` … `routes/mw{n}.ts`.
fn layers(n: usize) -> Vec<PathBuf> {
    (0..n)
        .map(|i| PathBuf::from(format!("routes/mw{i}.ts")))
        .collect()
}

/// The constant request the bench chains run against.
fn bench_request() -> ChainRequest<'static> {
    static PARAMS: std::sync::OnceLock<serde_json::Map<String, serde_json::Value>> =
        std::sync::OnceLock::new();
    ChainRequest {
        method: "GET",
        url: "/blog/post",
        params: PARAMS.get_or_init(serde_json::Map::new),
        headers: &[],
        body: &[],
    }
}

/// A representative composed locals value: seeded session, resolved
/// locale, two experiments, eight user keys — the shape a real request
/// hands to `to_value` per stage call.
fn representative_locals() -> RequestLocals {
    let mut locals = RequestLocals {
        session: Some(serde_json::json!({ "user": "elmo", "roles": ["admin"] })),
        locale: "fr".into(),
        ..RequestLocals::default()
    };
    locals
        .experiments
        .insert("hero".into(), serde_json::json!("b"));
    locals
        .experiments
        .insert("checkout".into(), serde_json::json!("v2"));
    for i in 0..8 {
        locals.user.insert(format!("key{i}"), serde_json::json!(i));
    }
    locals
}

fn bench_chain(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("bench tokio runtime");
    let noop: std::sync::Arc<dyn MiddlewareDispatcher> = std::sync::Arc::new(NoopMiddleware {
        locals_per_call: false,
    });
    let mut g = c.benchmark_group("chain");
    g.sample_size(40);
    g.measurement_time(Duration::from_secs(4));
    // The substrate's own cost per layer: async dispatch hop + locals
    // threading + header accumulation, zero middleware work.
    for n in [1, 2, 4] {
        g.bench_with_input(BenchmarkId::new("noop", n), &n, |b, &n| {
            let chain = layers(n);
            b.iter(|| {
                rt.block_on(async {
                    let outcome = run_chain(
                        noop.as_ref(),
                        &chain,
                        bench_request(),
                        serde_json::json!({}),
                        None,
                    )
                    .await;
                    black_box(outcome.is_ok())
                })
            });
        });
    }
    // The merge path: four layers each staging one locals key — the
    // per-hop clone + merge every real chain pays.
    let merging: std::sync::Arc<dyn MiddlewareDispatcher> = std::sync::Arc::new(NoopMiddleware {
        locals_per_call: true,
    });
    g.bench_function("locals_merge_4", |b| {
        let chain = layers(4);
        b.iter(|| {
            rt.block_on(async {
                let outcome = run_chain(
                    merging.as_ref(),
                    &chain,
                    bench_request(),
                    serde_json::json!({}),
                    None,
                )
                .await;
                black_box(outcome.is_ok())
            })
        });
    });
    g.finish();
}

fn bench_locals(c: &mut Criterion) {
    let mut g = c.benchmark_group("locals");
    g.sample_size(40);
    g.measurement_time(Duration::from_secs(4));
    g.bench_function("to_value", |b| {
        let locals = representative_locals();
        b.iter(|| black_box(locals.to_value()));
    });
    g.bench_function("apply_middleware", |b| {
        let mut locals = representative_locals();
        let payload = serde_json::json!({
            "section": "blog",
            "flag": true,
        });
        b.iter(|| {
            let staged = payload.clone();
            black_box(locals.apply_middleware_locals(staged).is_empty())
        });
    });
    g.finish();
}

criterion_group!(benches, bench_chain, bench_locals);
criterion_main!(benches);

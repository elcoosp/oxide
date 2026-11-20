//! Criterion bench for the client's inflight bound (F68): the cost the
//! permit acquire + release adds to an exchange against the pre-F68
//! unbounded client — the bound's whole price on the happy path (one
//! semaphore acquire per exchange, the permit held for its life, one
//! release at completion; the generous cap never queues in this
//! single-client bench).
//!
//! Run with: `cargo bench -p oxide-fw-cache --features bench-util --bench client_inflight`

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheHandler, HttpCacheService, Policy, TieredCache,
};

fn bench_client_inflight(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("bench runtime");

    // The plane: a real socket shell over an in-memory tier holding one
    // entry; both clients exchange the same GET.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    let entry = Entry {
        body: b"<html>client-inflight-v1</html>".to_vec(),
        status: 200,
        headers: vec![],
        tags: vec![],
        policy: Policy {
            ttl: Duration::from_secs(3600),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: now,
        etag: String::new(),
    };
    let tier = TieredCache::new_memory();
    rt.block_on(async {
        tier.set("route:/bench-inflight", entry)
            .await
            .expect("the bench entry stores");
    });
    let listener = rt
        .block_on(async { tokio::net::TcpListener::bind("127.0.0.1:0").await })
        .expect("bench listener binds");
    let addr = listener.local_addr().expect("the bound address");
    let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>);
    rt.spawn(async move {
        svc.serve(listener).await.expect("the accept loop serves");
    });

    // The clients: the capped client (the F68 shape, a generous cap — the
    // measured cost is the acquire/release ride, never a queue) and the
    // pre-F68 unbounded client (the contrast).
    let capped = HttpCacheHandler::new(format!("http://{addr}")).with_inflight_cap(64);
    let uncapped = HttpCacheHandler::new(format!("http://{addr}"));

    let mut group = c.benchmark_group("client_inflight");
    group.throughput(criterion::Throughput::Elements(1));

    // The bound's cost: the same GET through the gated client — one
    // permit acquire before the socket opens, one release at completion,
    // the exchange itself untouched.
    group.bench_function(BenchmarkId::new("capped-client", 200), |b| {
        b.iter(|| {
            let got = rt.block_on(capped.get("route:/bench-inflight"));
            black_box(
                got.expect("the capped client answers")
                    .expect("the bench entry hits"),
            );
        });
    });

    // The pre-F68 contrast: the unbounded client — no gate, no permit,
    // the delta IS the bound's price.
    group.bench_function(BenchmarkId::new("uncapped-client", 200), |b| {
        b.iter(|| {
            let got = rt.block_on(uncapped.get("route:/bench-inflight"));
            black_box(
                got.expect("the uncapped client answers")
                    .expect("the bench entry hits"),
            );
        });
    });

    group.finish();
}

criterion_group!(benches, bench_client_inflight);
criterion_main!(benches);

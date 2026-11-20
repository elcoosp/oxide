//! Criterion bench for the client's keep-alive connection reuse (F70):
//! the dial tax the pool removes.
//!
//! The pre-F70 client opened one fresh socket per exchange (`Connection:
//! close`) — every exchange paid the TCP handshake. The F70 keep-alive
//! client parks its socket in the per-host idle pool after the exchange
//! and pops it for the next one: the reuse shape's whole price is the
//! pool pop + push, and the delta against the close shape IS the dial
//! (the measured saving).
//!
//! Run with: `cargo bench -p oxide-fw-cache --features bench-util --bench client_keepalive`

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheHandler, HttpCacheService, Policy, TieredCache,
};

fn bench_client_keepalive(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("bench runtime");

    // The plane: a real F53 service over an in-memory tier holding one
    // entry (the shell honors keep-alive since F70 — both clients talk
    // to the same honored shell; the close client just never asks).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    let entry = Entry {
        body: b"<html>keepalive-v1</html>".to_vec(),
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
        tier.set("route:/bench-keepalive", entry)
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

    // The clients: the keep-alive client (a generous idle bound — the
    // measured cost is the pool pop/push ride, never a queue or a park
    // overflow) and the pre-F70 close client (the contrast).
    let reused = HttpCacheHandler::new(format!("http://{addr}")).with_keep_alive(8);
    let closed = HttpCacheHandler::new(format!("http://{addr}"));

    let mut group = c.benchmark_group("client_keepalive");
    group.throughput(criterion::Throughput::Elements(1));

    // The reuse shape: the same GET through the pooled client — after
    // the first dial the socket rides the pool, the exchange pays the
    // pop + the CL-driven read.
    group.bench_function(BenchmarkId::new("keepalive-get", 200), |b| {
        b.iter(|| {
            let got = rt.block_on(reused.get("route:/bench-keepalive"));
            black_box(
                got.expect("the keep-alive client answers")
                    .expect("the bench entry hits"),
            );
        });
    });

    // The pre-F70 contrast: the close client — one fresh dial per
    // exchange, the delta IS the dial tax the pool removes.
    group.bench_function(BenchmarkId::new("close-get", 200), |b| {
        b.iter(|| {
            let got = rt.block_on(closed.get("route:/bench-keepalive"));
            black_box(
                got.expect("the close client answers")
                    .expect("the bench entry hits"),
            );
        });
    });

    group.finish();
}

criterion_group!(benches, bench_client_keepalive);
criterion_main!(benches);

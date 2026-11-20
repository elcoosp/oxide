//! Criterion bench for the client's read bound (F65): the cost the
//! per-chunk cap comparison adds to a legit read against the refusal arm
//! it buys — the over-cap answer stops at the cap (one exchange, 64 KiB
//! buffered) where the pre-F65 unbounded client buffered the whole body.
//!
//! Run with: `cargo bench -p oxide-fw-cache --features bench-util --bench client_read_limit`

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheHandler, HttpCacheService, Policy, TieredCache,
};

fn bench_client_read_limit(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("bench runtime");

    // The plane: a real socket shell over an in-memory tier, holding one
    // 2 MiB entry — far past the 64 KiB cap, well under the codec's 32
    // MiB bound, so the over-cap answer is a REAL plane answer.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    let listener = rt
        .block_on(async { tokio::net::TcpListener::bind("127.0.0.1:0").await })
        .expect("bench listener binds");
    let addr = listener.local_addr().expect("the bound address");
    let big = Entry {
        body: vec![b'a'; 2 * 1024 * 1024],
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
        tier.set("route:/bench-big", big)
            .await
            .expect("the bench entry stores");
    });
    let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>);
    rt.spawn(async move {
        svc.serve(listener).await.expect("the accept loop serves");
    });

    // The clients: the capped reader (the F65 shape), the over-cap
    // reader (the refusal floor's own cap) and the pre-F65 unbounded
    // client (the contrast).
    let capped = HttpCacheHandler::new(format!("http://{addr}")).with_read_limit(1024 * 1024);
    let refusing = HttpCacheHandler::new(format!("http://{addr}")).with_read_limit(64 * 1024);
    let unbounded = HttpCacheHandler::new(format!("http://{addr}"));

    let mut group = c.benchmark_group("client_read_limit");
    group.throughput(criterion::Throughput::Elements(1));

    // The legit read under a generous cap: the bound costs one length
    // comparison per chunk — the full exchange (connect + request +
    // codec/MAC round) rides unchanged.
    group.bench_function(BenchmarkId::new("read-under-cap", 200), |b| {
        b.iter(|| {
            let got = rt.block_on(capped.get("route:/bench-big"));
            black_box(
                got.expect("the capped read answers")
                    .expect("the bench entry hits"),
            );
        });
    });

    // The refusal floor: the same 2 MiB answer against the 64 KiB cap —
    // the exchange stops at the cap (the bytes past it never land), the
    // get degrades to a miss (the §11.2 transport-class posture).
    group.bench_function(BenchmarkId::new("over-cap-refusal", 404), |b| {
        b.iter(|| {
            let got = rt.block_on(refusing.get("route:/bench-big"));
            black_box(got.expect("the degrade is a plain miss"));
        });
    });

    // The pre-F65 contrast: the unbounded client buffers the whole 2 MiB
    // answer — the memory the cap buys down, measured.
    group.bench_function(BenchmarkId::new("unbounded-drain", 200), |b| {
        b.iter(|| {
            let got = rt.block_on(unbounded.get("route:/bench-big"));
            black_box(
                got.expect("the unbounded read answers")
                    .expect("the bench entry hits"),
            );
        });
    });

    group.finish();
}

criterion_group!(benches, bench_client_read_limit);
criterion_main!(benches);

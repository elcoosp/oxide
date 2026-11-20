//! Criterion bench for the HTTP client's deadline arm (F60): the happy
//! path's cost under a whole-exchange deadline versus the pre-F60
//! unbounded client — the wrapper's overhead against a real plane
//! service on a local socket.
//!
//! Run with: `cargo bench -p oxide-fw-cache --features bench-util --bench handler_deadline`

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheHandler, HttpCacheService, Policy, TieredCache,
};

fn entry() -> Entry {
    Entry {
        body: b"<html>deadline-bench</html>".to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: vec!["bench-tag".to_string()],
        policy: Policy {
            ttl: Duration::from_secs(3600),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        // The tier classifies freshness against the REAL clock (unix ms) —
        // the bench's entry must be born fresh, not a fixed stamp.
        stored_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        etag: "\"deadline-bench\"".into(),
    }
}

fn bench_deadline(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("bench runtime");

    // A real plane on a local socket: both clients dial the SAME service
    // so the comparison isolates the deadline wrapper's overhead (the
    // pre-F60 client vs the F60 client on the identical wire).
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .expect("bench listener");
    let addr = listener.local_addr().expect("bench addr");
    let tier = TieredCache::new_memory();
    let service = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>);
    rt.block_on(async {
        tier.set("route:/bench", entry()).await.expect("warm entry");
        tokio::spawn(service.serve(listener));
    });

    let bare = HttpCacheHandler::new(format!("http://{addr}"));
    let bounded =
        HttpCacheHandler::new(format!("http://{addr}")).with_timeout(Duration::from_secs(5));

    let mut group = c.benchmark_group("cache_handler_deadline");
    group.throughput(criterion::Throughput::Elements(1));

    // The pre-F60 shape: unbounded exchange (the base the deadline's
    // overhead compares against).
    group.bench_function(BenchmarkId::new("get", 0), |b| {
        b.iter(|| {
            let got = rt.block_on(bare.get("route:/bench"));
            black_box(got.expect("the unbounded get answers"));
        });
    });

    // The F60 shape: the same exchange under a 5s deadline — connect,
    // write and read share one budget; the happy path pays only the
    // wrapper (one `timeout` future) and must stay indistinguishable
    // from the base.
    group.bench_function(BenchmarkId::new("get-deadline", 5_000), |b| {
        b.iter(|| {
            let got = rt.block_on(bounded.get("route:/bench"));
            black_box(got.expect("the bounded get answers"));
        });
    });

    group.finish();
}

criterion_group!(benches, bench_deadline);
criterion_main!(benches);

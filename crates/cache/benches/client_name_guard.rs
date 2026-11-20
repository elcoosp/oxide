//! Criterion bench for the client's request-line trust boundary (F66):
//! the guard's per-name scan cost — the pass arm over a canonical route
//! key, the refuse arm stopping at the offending byte — beside the full
//! guarded `set` exchange, where the scan rides the verb's real cost.
//!
//! Run with: `cargo bench -p oxide-fw-cache --features bench-util --bench client_name_guard`

use std::hint::black_box;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheHandler, HttpCacheService, Policy, TieredCache,
};

fn bench_client_name_guard(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("bench runtime");

    let listener = rt
        .block_on(async { tokio::net::TcpListener::bind("127.0.0.1:0").await })
        .expect("bench listener binds");
    let addr = listener.local_addr().expect("the bound address");
    let tier = TieredCache::new_memory();
    let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>);
    rt.spawn(async move {
        svc.serve(listener).await.expect("the accept loop serves");
    });
    let client = HttpCacheHandler::new(format!("http://{addr}"));
    let entry = Entry {
        body: b"<html>guard-bench</html>".to_vec(),
        status: 200,
        headers: vec![],
        tags: vec![],
        policy: Policy::default(),
        stored_at: 0,
        etag: String::new(),
    };

    let mut group = c.benchmark_group("client_name_guard");
    group.throughput(criterion::Throughput::Elements(1));

    // The pass arm: the canonical route key scans clean — the whole
    // name walks, the verb proceeds.
    group.bench_function(BenchmarkId::new("pass-scan", 1), |b| {
        b.iter(|| {
            let ok = HttpCacheHandler::plane_name("route:/blog/42?v=2");
            black_box(ok.is_ok());
        });
    });

    // The refuse arm: the space stops the scan at the first offending
    // byte — the corrupt exchange never leaves the client.
    group.bench_function(BenchmarkId::new("refuse-scan", 1), |b| {
        b.iter(|| {
            let bad = HttpCacheHandler::plane_name("route:/a b");
            black_box(bad.expect_err("the space refuses"));
        });
    });

    // The guarded verb: the full set exchange — the scan rides the
    // connect + request + codec/MAC round, the boundary's real cost.
    group.bench_function(BenchmarkId::new("guarded-set", 204), |b| {
        b.iter(|| {
            let put = rt.block_on(client.set("route:/bench-guard", entry.clone()));
            black_box(put.is_ok());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_client_name_guard);
criterion_main!(benches);

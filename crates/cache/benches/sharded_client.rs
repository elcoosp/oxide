//! Criterion bench for the sharded multi-plane client (F69): the cost
//! the routing hop adds to an exchange, and the fan-out's price on the
//! tag verbs.
//!
//! The composite's entry verbs are ONE routed exchange (the routing is
//! argmax over N rendezvous scores — single digits of planes, a
//! nanosecond-scale pass beside a network exchange), so the sharded
//! client's `get` should ride within noise of a direct client's `get`
//! against the same plane. The tag verbs fan out over every shard by
//! design — the fan's cost is the N exchanges it is.
//!
//! Run with: `cargo bench -p oxide-fw-cache --features bench-util --bench sharded_client`

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheHandler, HttpCacheService, Policy, ShardedCacheHandler,
    TieredCache,
};

fn bench_sharded_client(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("bench runtime");

    // Four real planes on loopback; the tier handles stay out so the
    // bench can seed the ROUTED plane (the routing is deterministic —
    // seed where the composite will look, the recorded fixture class).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    let entry = Entry {
        body: b"<html>sharded-v1</html>".to_vec(),
        status: 200,
        headers: vec![],
        tags: vec!["bench".into()],
        policy: Policy {
            ttl: Duration::from_secs(3600),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: now,
        etag: String::new(),
    };
    let mut tiers = Vec::new();
    let mut addrs = Vec::new();
    for _ in 0..4 {
        tiers.push(TieredCache::new_memory());
        let listener = rt
            .block_on(async { tokio::net::TcpListener::bind("127.0.0.1:0").await })
            .expect("bench listener binds");
        addrs.push(listener.local_addr().expect("the bound address"));
        let svc =
            HttpCacheService::new(Arc::clone(&tiers[tiers.len() - 1]) as Arc<dyn CacheHandler>);
        rt.spawn(async move {
            svc.serve(listener).await.expect("the accept loop serves");
        });
    }

    let clients: Vec<Arc<dyn CacheHandler>> = addrs
        .iter()
        .map(|a| Arc::new(HttpCacheHandler::new(format!("http://{a}"))) as Arc<dyn CacheHandler>)
        .collect();
    let sharded = ShardedCacheHandler::new(clients).expect("the composite composes");

    // The bench entry (and its tag) ride the plane the key routes to.
    let routed = sharded.shard_of("route:/bench-sharded");
    rt.block_on(async {
        tiers[routed]
            .set("route:/bench-sharded", entry)
            .await
            .expect("the bench entry stores");
    });
    let direct = HttpCacheHandler::new(format!("http://{}", addrs[routed]));

    let mut group = c.benchmark_group("sharded_client");
    group.throughput(criterion::Throughput::Elements(1));

    // The routing hop's cost: the same GET hit through the composite
    // (routing + the exchange) vs the direct client (the exchange alone)
    // — the delta is the rendezvous pass.
    group.bench_function(BenchmarkId::new("sharded-get", 200), |b| {
        b.iter(|| {
            let got = rt.block_on(sharded.get("route:/bench-sharded"));
            black_box(
                got.expect("the sharded client answers")
                    .expect("the bench entry hits"),
            );
        });
    });
    group.bench_function(BenchmarkId::new("single-get", 200), |b| {
        b.iter(|| {
            let got = rt.block_on(direct.get("route:/bench-sharded"));
            black_box(
                got.expect("the direct client answers")
                    .expect("the bench entry hits"),
            );
        });
    });

    // The fan-out's price: tag_keys over four planes through the
    // composite (four exchanges, concatenated) vs the single-plane list.
    group.bench_function(BenchmarkId::new("sharded-tag-fanout", 200), |b| {
        b.iter(|| {
            let keys = rt.block_on(sharded.tag_keys("bench"));
            black_box(keys.expect("the fan-out answers"));
        });
    });
    group.bench_function(BenchmarkId::new("single-tag", 200), |b| {
        b.iter(|| {
            let keys = rt.block_on(direct.tag_keys("bench"));
            black_box(keys.expect("the single-plane list answers"));
        });
    });

    group.finish();
}

criterion_group!(benches, bench_sharded_client);
criterion_main!(benches);

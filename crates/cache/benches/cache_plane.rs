//! Criterion bench for the cache plane's HTTP service (F53, §11.2/§20):
//! the per-exchange cost a node's cache plane adds over the backing
//! handler's own call.
//!
//! The service's pure core (`HttpCacheService::respond`) is the whole
//! story: the socket shell parses bytes and writes statuses, so what a
//! hosted plane pays per exchange is the core's route + the codec/MAC
//! round the payload takes (encode + seal on GET, open + decode on PUT).
//! Measured over a real in-memory `TieredCache` so the backing handler's
//! own hit cost rides the number — the delta over a direct handler call
//! is the plane's tax, the honest §20 shape.

use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{
    Entry, HttpCacheService, PlaneRequest, Policy, TieredCache, encode_entry, seal_entry,
};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn entry(body: &[u8], tag: &str) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: vec![tag.into()],
        policy: Policy {
            ttl: Duration::from_secs(60),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: now_ms(),
        etag: "\"v1\"".into(),
    }
}

fn bench_serve(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let tier = TieredCache::new_memory();
    rt.block_on(tier.set("k", entry(b"x".repeat(1024).as_slice(), "t")))
        .expect("the seeding set lands");
    let service = HttpCacheService::new(tier.clone() as Arc<dyn oxide_fw_cache::CacheHandler>);
    let sealed =
        seal_entry(&encode_entry(&entry(b"x".repeat(1024).as_slice(), "t")).expect("encodes"));

    let mut group = c.benchmark_group("cache_plane_serve");
    group.throughput(criterion::Throughput::Elements(1));
    let rt_handle = &rt;

    // The GET-hit exchange: route + handler hit + encode + seal — the
    // full cost one node pays to serve one entry to another.
    group.bench_with_input(BenchmarkId::new("get-hit", 1024), &service, |b, s| {
        b.iter(|| {
            rt_handle.block_on(async {
                let resp = s.respond(black_box(&PlaneRequest::get("/entry/k"))).await;
                black_box(resp.status);
            });
        });
    });

    // The PUT exchange: route + open + decode + handler set — the cost
    // the receiving node pays to land one entry (the trust boundary's
    // open/decode rides every write).
    group.bench_with_input(BenchmarkId::new("put", 1024), &service, |b, s| {
        b.iter(|| {
            rt_handle.block_on(async {
                let resp = s
                    .respond(black_box(&PlaneRequest::put(
                        "/entry/put-k",
                        sealed.clone(),
                    )))
                    .await;
                black_box(resp.status);
            });
        });
    });

    // The miss exchange: route + handler miss — the floor a warm plane
    // answers at.
    group.bench_with_input(BenchmarkId::new("get-miss", 0), &service, |b, s| {
        b.iter(|| {
            rt_handle.block_on(async {
                let resp = s
                    .respond(black_box(&PlaneRequest::get("/entry/nope")))
                    .await;
                black_box(resp.status);
            });
        });
    });

    group.finish();
}

criterion_group!(benches, bench_serve);
criterion_main!(benches);

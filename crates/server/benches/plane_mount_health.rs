//! Criterion bench for the mounted cache plane's health arm (F59): what a
//! balancer probe pays on the mounted surface
//! ([`Pipeline::serve_request`] `GET /health` with the provider riding
//! the mount) versus the service's pure core pull alone, and what the
//! two floor shapes pay — a provider-less mount's `/health` fall-through
//! (the user-route contract's cost) and the unmounted pipeline's.
//!
//! Run with: `cargo bench -p oxide-fw-server --features bench-util --bench plane_mount_health`

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use futures::future::BoxFuture;
use oxide_fw_cache::{Entry, HttpCacheService, PlaneRequest, Policy, TieredCache};
use oxide_fw_server::{OxideRequest, OxideResponse, Pipeline};

fn entry() -> Entry {
    Entry {
        body: b"<html>the cached exchange</html>".to_vec(),
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
        etag: "\"mount-health-v1\"".into(),
    }
}

fn oxide_req(method: &str, url: &str, body: Bytes) -> OxideRequest {
    OxideRequest {
        method: method.into(),
        url: url.into(),
        headers: Arc::new(Vec::new()),
        header_index: Default::default(),
        body,
        request_id: "bench-mount-health".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

fn echo_dispatch() -> Arc<dyn Fn(OxideRequest) -> BoxFuture<'static, OxideResponse> + Send + Sync> {
    Arc::new(|r: OxideRequest| {
        Box::pin(async move { OxideResponse::html(200, format!("dispatch:{}", r.url)) })
    })
}

fn bench_mount_health(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("bench runtime");
    // The tier the plane roots in — warmed with one stored entry so the
    // health pull's posture fields carry a realistic resident shape (the
    // F32 pull reads the LIVE tier, not a zeroed stand-in). The tier
    // stores the Entry directly — the seal is the WIRE's shape, not the
    // store's.
    let tier = TieredCache::new_memory();
    rt.block_on(async {
        tier.set("route:/bench", entry()).await.expect("warm entry");
    });
    // The healthy mount: the service over the boot's own tier, the tier
    // doubling as the F58 health provider (the F59 composition verbatim).
    let healthy = Arc::new(
        HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>)
            .with_health(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::PlaneHealth>),
    );
    // The provider-less mount: the user-route contract's floor shape.
    let plain = Arc::new(HttpCacheService::new(
        Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>
    ));
    let healthy_mounted = Arc::new(Pipeline::default().with_cache_plane(Arc::clone(&healthy)));
    let plain_mounted = Arc::new(Pipeline::default().with_cache_plane(Arc::clone(&plain)));
    let unmounted = Arc::new(Pipeline::default());
    let dispatch = echo_dispatch();
    let dispatch_ref: &dyn Fn(OxideRequest) -> BoxFuture<'static, OxideResponse> =
        dispatch.as_ref();

    let mut group = c.benchmark_group("cache_plane_mount_health");
    group.throughput(criterion::Throughput::Elements(1));

    // The balancer probe through the mount: the native prefix's family
    // match (the provider check included), the PlaneRequest build, the
    // routing core's health arm (the live pull + its JSON), and the
    // finish_response header composition.
    group.bench_function(BenchmarkId::new("serve_request-get", 200), |b| {
        b.iter(|| {
            let resp = rt.block_on(
                healthy_mounted
                    .serve_request(oxide_req("GET", "/health", Bytes::new()), dispatch_ref),
            );
            black_box(resp.status);
        });
    });

    // The service's pure core alone — the delta against `serve_request-get`
    // is the mount's own overhead (the variant hop + the finish shape).
    group.bench_function(BenchmarkId::new("respond-core-get", 200), |b| {
        b.iter(|| {
            let resp = rt.block_on(healthy.respond(&PlaneRequest {
                method: "GET".into(),
                path: "/health".into(),
                body: Vec::new(),
            }));
            black_box(resp.status);
        });
    });

    // The provider-less floor: a provider-attached mount costs one extra
    // `Arc::is_some` check; this shape answers through dispatch (the
    // application's own /health route survives — the F40 doctrine).
    group.bench_function(BenchmarkId::new("plain-mount-fallthrough", 0), |b| {
        b.iter(|| {
            let resp = rt.block_on(
                plain_mounted
                    .serve_request(oxide_req("GET", "/health", Bytes::new()), dispatch_ref),
            );
            black_box(resp.status);
        });
    });

    // The unmounted floor: no mount at all — the pre-F57 shape.
    group.bench_function(BenchmarkId::new("unmounted-fallthrough", 0), |b| {
        b.iter(|| {
            let resp = rt.block_on(
                unmounted.serve_request(oxide_req("GET", "/health", Bytes::new()), dispatch_ref),
            );
            black_box(resp.status);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_mount_health);
criterion_main!(benches);

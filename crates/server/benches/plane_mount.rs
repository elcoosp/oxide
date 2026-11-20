//! Criterion bench for the §11.2 cache plane's mount (F57): what the app
//! server pays to answer a plane exchange through [`Pipeline::serve_request`]
//! versus the service's pure core alone, and what an unmounted pipeline
//! pays for the plane-shaped path (the None check's fall-through floor).
//!
//! Run with: `cargo bench -p oxide-fw-server --features bench-util --bench plane_mount`

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use futures::future::BoxFuture;
use oxide_fw_cache::{
    Entry, HttpCacheService, PlaneRequest, Policy, TieredCache, encode_entry, seal_entry,
};
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
        etag: "\"mount-v1\"".into(),
    }
}

fn oxide_req(method: &str, url: &str, body: Bytes) -> OxideRequest {
    OxideRequest {
        method: method.into(),
        url: url.into(),
        headers: Arc::new(Vec::new()),
        header_index: Default::default(),
        body,
        request_id: "bench-mount".into(),
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

fn bench_mount(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("bench runtime");
    let sealed = seal_entry(&encode_entry(&entry()).unwrap());
    let tier = TieredCache::new_memory();
    let service = Arc::new(HttpCacheService::new(
        Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>
    ));
    let mounted = Arc::new(Pipeline::default().with_cache_plane(Arc::clone(&service)));
    let unmounted = Arc::new(Pipeline::default());
    let dispatch = echo_dispatch();
    let dispatch_ref: &dyn Fn(OxideRequest) -> BoxFuture<'static, OxideResponse> =
        dispatch.as_ref();

    let mut group = c.benchmark_group("cache_plane_mount");
    group.throughput(criterion::Throughput::Elements(1));

    // The full pipeline exchange through the mount: the native prefix's
    // family match, the PlaneRequest build, the routing core, and the
    // finish_response header composition — the whole per-request cost a
    // mounted node pays for one GET.
    group.bench_function(BenchmarkId::new("serve_request-get", 200), |b| {
        rt.block_on(async {
            let put = oxide_req("PUT", "/entry/route:/bench", Bytes::from(sealed.clone()));
            mounted.serve_request(put, dispatch_ref).await;
        });
        b.iter(|| {
            let resp = rt.block_on(mounted.serve_request(
                oxide_req("GET", "/entry/route:/bench", Bytes::new()),
                dispatch_ref,
            ));
            black_box(resp.status);
        });
    });

    // The service's pure core alone — the delta against `serve_request-get`
    // is the mount's own overhead (the variant hop + the finish shape).
    group.bench_function(BenchmarkId::new("respond-core-get", 200), |b| {
        b.iter(|| {
            let resp = rt.block_on(service.respond(&PlaneRequest {
                method: "GET".into(),
                path: "/entry/route:/bench".into(),
                body: Vec::new(),
            }));
            black_box(resp.status);
        });
    });

    // The unmounted floor: a plane-shaped path on a pipeline with no mount
    // — one `Option` branch, then the fall-through dispatch echo (the F40
    // doctrine's cost, zero).
    group.bench_function(BenchmarkId::new("unmounted-fallthrough", 0), |b| {
        b.iter(|| {
            let resp = rt.block_on(unmounted.serve_request(
                oxide_req("GET", "/entry/route:/bench", Bytes::new()),
                dispatch_ref,
            ));
            black_box(resp.status);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_mount);
criterion_main!(benches);

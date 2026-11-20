//! Criterion bench for the plane's exchange-bounds seam (F64): the cost
//! the drain's bound adds to a legit exchange (the one `Content-Length`
//! comparison) against the refusal arm it buys — the 413 floor that stops
//! a hostile exchange before the first body byte lands, and the
//! unbounded drain's codec 400 the pre-F64 shape answered (the escape
//! hatch's own cost, for contrast).
//!
//! Run with: `cargo bench -p oxide-fw-cache --features bench-util --bench exchange_bounds`

use std::hint::black_box;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{ExchangeLimits, HttpCacheService, TieredCache};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// One raw exchange against a live shell: request bytes in, the status
/// line + body out (the socket arms are the seam's home, so the bench
/// drives real sockets — the `handler_deadline` shape).
async fn exchange(addr: std::net::SocketAddr, req: &[u8]) -> u16 {
    let mut sock = tokio::net::TcpStream::connect(addr)
        .await
        .expect("the bench dials");
    sock.write_all(req).await.expect("the bench writes");
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.expect("the bench reads");
    let head = String::from_utf8_lossy(&buf);
    head.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// A plane on a real socket with the caller's limits.
async fn bounded_plane(
    rt: &tokio::runtime::Runtime,
    limits: ExchangeLimits,
) -> std::net::SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the bench listener binds");
    let addr = l.local_addr().expect("the bound address");
    let tier = TieredCache::new_memory();
    let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>)
        .with_limits(limits);
    rt.spawn(async move {
        svc.serve(l).await.expect("the accept loop serves");
    });
    addr
}

fn bench_exchange_bounds(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("bench runtime");
    let default_addr = rt.block_on(bounded_plane(&rt, ExchangeLimits::default()));
    let tightened_addr = rt.block_on(bounded_plane(&rt, ExchangeLimits { max_body: 4096 }));
    let unbounded_addr = rt.block_on(bounded_plane(&rt, ExchangeLimits::unbounded()));

    // The hostile body every refusal/drain bench shape carries: past the
    // tightened cap, well under the default bound — 1 MiB per iteration
    // keeps the drain's cost measurable without paying the codec bound's
    // full 32 MiB per iter.
    let hostile = vec![0u8; 1024 * 1024];
    let hostile_len = hostile.len();

    let mut group = c.benchmark_group("exchange_bounds");
    group.throughput(criterion::Throughput::Elements(1));

    // The legit PUT under the default cap: the bound costs one
    // comparison — the full store path (seal open + decode + tier set)
    // rides unchanged.
    group.bench_function(BenchmarkId::new("put-under-cap", 204), |b| {
        b.iter(|| {
            let sealed = vec![0u8; 4096];
            let head = format!(
                "PUT /entry/route:/bench-bound HTTP/1.1\r\nHost: t\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                sealed.len()
            );
            let mut req = head.into_bytes();
            req.extend_from_slice(&sealed);
            let status = rt.block_on(exchange(default_addr, &req));
            black_box(status);
        });
    });

    // The refusal floor: a length past the cap — 413 before the first
    // body byte lands. The cost the hostile exchange is held to.
    group.bench_function(BenchmarkId::new("over-cap-413", 413), |b| {
        b.iter(|| {
            let head = format!(
                "PUT /entry/route:/bench-flood HTTP/1.1\r\nHost: t\r\nContent-Length: {hostile_len}\r\nConnection: close\r\n\r\n"
            );
            let status = rt.block_on(exchange(tightened_addr, head.as_bytes()));
            black_box(status);
        });
    });

    // The unbounded drain: the same oversized length reaches the codec —
    // the pre-F64 400 the escape hatch restores. One MiB of drain per
    // iter is the contrast the 413 floor buys down.
    group.bench_function(BenchmarkId::new("unbounded-400", 400), |b| {
        b.iter(|| {
            let head = format!(
                "PUT /entry/route:/bench-unbounded HTTP/1.1\r\nHost: t\r\nContent-Length: {hostile_len}\r\nConnection: close\r\n\r\n"
            );
            let mut req = head.into_bytes();
            req.extend_from_slice(&hostile);
            let status = rt.block_on(exchange(unbounded_addr, &req));
            black_box(status);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_exchange_bounds);
criterion_main!(benches);

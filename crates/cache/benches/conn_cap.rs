//! Criterion bench for the shell's concurrency bound (F67): the cost the
//! permit acquire + release adds to an exchange against the uncapped
//! pre-F67 shell — the bound's whole price on the happy path (one
//! semaphore acquire per connection, the permit held for the exchange's
//! life, one release at task end).
//!
//! Run with: `cargo bench -p oxide-fw-cache --features bench-util --bench conn_cap`

use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use oxide_fw_cache::{CacheHandler, Entry, HttpCacheService, Policy, TieredCache};

fn bench_conn_cap(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("bench runtime");

    // The backing tier holds one entry; both shells serve the same GET.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    let entry = Entry {
        body: b"<html>conn-cap-v1</html>".to_vec(),
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
        tier.set("route:/bench-conn", entry)
            .await
            .expect("the bench entry stores");
    });

    // The shells: the capped plane (the F67 shape, a generous cap so the
    // permit never queues in this single-client bench — the measured cost
    // is the acquire/release ride) and the pre-F67 uncapped plane.
    let plane = |cap: Option<usize>| {
        let listener = rt
            .block_on(async { tokio::net::TcpListener::bind("127.0.0.1:0").await })
            .expect("bench listener binds");
        let addr = listener.local_addr().expect("the bound address");
        let mut svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>);
        if let Some(n) = cap {
            svc = svc.with_max_connections(n);
        }
        rt.spawn(async move {
            svc.serve(listener).await.expect("the accept loop serves");
        });
        addr
    };
    let capped = plane(Some(64));
    let uncapped = plane(None);

    let mut group = c.benchmark_group("conn_cap");
    group.throughput(criterion::Throughput::Elements(1));

    // The bound's cost: the same GET through the capped shell — one
    // permit acquire before the spawn, one release at task end, the
    // exchange itself untouched.
    group.bench_function(BenchmarkId::new("capped-shell", 200), |b| {
        b.iter(|| {
            let got = rt.block_on(gated_get(capped));
            black_box(got.expect("the capped shell answers"));
        });
    });

    // The pre-F67 contrast: the uncapped shell — no gate, no permit, the
    // delta IS the bound's price.
    group.bench_function(BenchmarkId::new("uncapped-shell", 200), |b| {
        b.iter(|| {
            let got = rt.block_on(gated_get(uncapped));
            black_box(got.expect("the uncapped shell answers"));
        });
    });

    group.finish();
}

/// One client GET over a plain socket (the bench rides the shell's own
/// exchange shape — the raw request the client half speaks).
async fn gated_get(addr: std::net::SocketAddr) -> std::io::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut sock = tokio::net::TcpStream::connect(addr).await?;
    sock.write_all(
        b"GET /entry/route:/bench-conn HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n",
    )
    .await?;
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

criterion_group!(benches, bench_conn_cap);
criterion_main!(benches);

//! Consumer walkthrough for the V2-F61 cache plane's metrics arm: the §11.2
//! service answers the §20 exposition on its own wire.
//!
//! F58 gave the plane its health arm; the counters had no wire. The §20
//! exporter (`MetricsCacheObserver`) and the F52 posture's gauges rendered
//! only in-process — the CLI's `/_oxide/metrics` scrape behind the boot's
//! opt-ins. `HttpCacheService::with_metrics` attaches a
//! [`oxide_fw_cache::PlaneMetrics`] provider and `GET /metrics` answers the
//! joined exposition: the counter families (the F33 order, the F51 eviction
//! pair last) then the gauge families (the F52 order), both LIVE pulls — a
//! scrape diffs two pulls, nothing freezes. The natural provider is the
//! [`oxide_fw_cache::MetricsPlaneSource`]: the boot's exporter plus the
//! rooted tier — one value, both halves of the §20 story.
//!
//! Shows the story end to end:
//!
//! 1. Two entries land through the plane's `PUT` arm; one `GET` serves a
//!    hit — the plane's own traffic is the traffic the exposition counts.
//! 2. `GET /metrics` — the joined exposition reads them (hits, sets,
//!    residents).
//! 3. `POST /metrics` — 405 (the surface is GET-only).
//! 4. A service with no provider — 404 (the F40 floor, byte-preserved).
//! 5. The arm rides the real socket shell: one raw `GET /metrics`.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example plane_metrics
//! ```

use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheService, MetricsCacheObserver, MetricsPlaneSource, PlaneHealth,
    PlaneMetrics, PlaneRequest, Policy, TieredCache,
};
use std::sync::Arc;
use std::time::Duration;

fn entry(body: &[u8]) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: vec![],
        policy: Policy {
            ttl: Duration::from_secs(3600),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        // The tier classifies freshness against the REAL clock (unix ms).
        stored_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        etag: "\"metrics-v1\"".into(),
    }
}

async fn respond(svc: &HttpCacheService, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let resp = svc
        .respond(&PlaneRequest {
            method: method.into(),
            path: path.into(),
            body: body.to_vec(),
        })
        .await;
    (resp.status, resp.body)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    // A plane rooted at the tier it serves, the exporter fanned onto it —
    // the natural provider shape (the boot's own construction).
    let metrics = Arc::new(MetricsCacheObserver::new());
    let tier = Arc::new(
        TieredCache::new(64 * 1024 * 1024)
            .with_observer(metrics.clone() as Arc<dyn oxide_fw_cache::CacheObserver>),
    );
    let source = MetricsPlaneSource::new(metrics.clone(), tier.clone() as Arc<dyn PlaneHealth>);
    let svc = HttpCacheService::new(tier.clone() as Arc<dyn CacheHandler>)
        .with_metrics(Arc::new(source) as Arc<dyn PlaneMetrics>);
    let bare = HttpCacheService::new(tier.clone() as Arc<dyn CacheHandler>);

    rt.block_on(async {
        // 1. Two entries land through the plane; one GET serves the hit
        //    the exposition will count.
        for (name, body) in [("route:/metrics-1", "one"), ("route:/metrics-2", "two")] {
            let sealed = oxide_fw_cache::seal_entry(
                &oxide_fw_cache::encode_entry(&entry(body.as_bytes())).unwrap(),
            );
            let (status, _) = respond(&svc, "PUT", &format!("/entry/{name}"), &sealed).await;
            println!("PUT /entry/{name} -> {status}");
        }
        let (status, _) = respond(&svc, "GET", "/entry/route:/metrics-1", &[]).await;
        println!("GET /entry/route:/metrics-1 -> {status}");

        // 2. The joined exposition reads the live traffic.
        let (status, body) = respond(&svc, "GET", "/metrics", &[]).await;
        let text = String::from_utf8_lossy(&body);
        println!(
            "GET /metrics -> {status} ({} exposition lines)",
            text.lines().count()
        );
        for line in text.lines().filter(|l| {
            l.starts_with("oxide_cache_plane_hits_total{layer=\"L0\"}")
                || l.starts_with("oxide_cache_plane_sets_total")
                || l.starts_with("oxide_cache_plane_memory_entries")
        }) {
            println!("  {line}");
        }

        // 3. The surface is GET-only.
        let (status, _) = respond(&svc, "POST", "/metrics", &[]).await;
        println!("POST /metrics -> {status} (the surface is GET-only)");

        // 4. No provider, no arm: the F40 floor.
        let (status, _) = respond(&bare, "GET", "/metrics", &[]).await;
        println!("GET /metrics (no provider) -> {status} (the pre-F61 404)");

        // 5. The arm rides the real socket shell.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            svc.serve(listener).await.expect("the accept loop serves");
        });
        let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        sock.write_all(b"GET /metrics HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut buf = Vec::new();
        sock.read_to_end(&mut buf).await.unwrap();
        let head = String::from_utf8_lossy(&buf);
        println!("wire GET /metrics -> {}", head.lines().next().unwrap_or(""));
    });
    Ok(())
}

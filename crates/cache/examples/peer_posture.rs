//! Consumer walkthrough for the V2-F63 client's peer-posture probes: the
//! wired node asks its plane how it is doing.
//!
//! F58 gave the plane `GET /health` and F61 gave it `GET /metrics`, but
//! both answered a direct socket caller only. The node whose
//! `cache.handler = { type: 'http', url }` points at the plane — the
//! §11.2 two-node story's client half — could not answer "how is my
//! remote cache doing?" without hand-rolling a raw exchange.
//! [`oxide_fw_cache::PlanePeer`] is the pull side: the client implements
//! it, the probes ride the same `request` core the entry verbs ride (the
//! F60 deadline and the `CACHE-HTTP` error context come with them), and
//! every failure is loud — a probe that lied would be worse than one
//! that fails.
//!
//! Shows the story end to end:
//!
//! 1. Node A: the full plane on a socket (the tier rooted as the health
//!    provider, the exporter composed into the metrics source).
//! 2. Node B: the client probes `peer_health()` — the live posture pulls
//!    as JSON, and the entry B stores through the plane moves the NEXT
//!    pull (a probe diffs two pulls, nothing freezes).
//! 3. Node B probes `peer_metrics()` — the §20 exposition comes back
//!    byte-verbatim, B's own set counted in it.
//! 4. A provider-less plane answers the F40 404 — the probe is a LOUD
//!    error naming the status, never a fabricated zeroed posture.
//! 5. A dead peer with a deadline attached — the probe fails under
//!    `CACHE-HTTP` within the deadline (the F60 teeth ride the probes).
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example peer_posture
//! ```

use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheHandler, HttpCacheService, MetricsCacheObserver,
    MetricsPlaneSource, PlaneHealth, PlaneMetrics, PlanePeer, Policy, TieredCache,
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
        etag: "\"peer-v1\"".into(),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        // 1. Node A boots the full plane: the tier is the backing store
        //    AND the health provider, the exporter the metrics source —
        //    the boot's own construction.
        let metrics = Arc::new(MetricsCacheObserver::new());
        let tier = Arc::new(
            TieredCache::new(64 * 1024 * 1024)
                .with_observer(metrics.clone() as Arc<dyn oxide_fw_cache::CacheObserver>),
        );
        let source = MetricsPlaneSource::new(metrics.clone(), tier.clone() as Arc<dyn PlaneHealth>);
        let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>)
            .with_health(Arc::clone(&tier) as Arc<dyn PlaneHealth>)
            .with_metrics(Arc::new(source) as Arc<dyn PlaneMetrics>);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            svc.serve(listener).await.expect("the accept loop serves");
        });

        // 2. Node B points its handler at the plane and probes the
        //    posture: the live pull equals the tier's own health(), and
        //    B's own traffic moves the next pull.
        let client =
            HttpCacheHandler::new(format!("http://{addr}")).with_timeout(Duration::from_secs(5));
        let pulled = client.peer_health().await.expect("the plane answers");
        println!(
            "peer_health -> memory_entries={} disk_attached={} (the tier's live posture)",
            pulled.memory_entries, pulled.disk_attached
        );
        client
            .set("route:/peer-1", entry(b"one"))
            .await
            .expect("the set lands through the plane");
        let after = client.peer_health().await.expect("the second pull answers");
        println!(
            "peer_health after B's set -> memory_entries={} (a probe diffs two pulls)",
            after.memory_entries
        );

        // 3. The exposition comes back byte-verbatim — B's set counted.
        let text = client.peer_metrics().await.expect("the plane answers");
        println!(
            "peer_metrics -> {} exposition lines; B's set rides it:",
            text.lines().count()
        );
        for line in text
            .lines()
            .filter(|l| l.starts_with("oxide_cache_plane_sets_total"))
        {
            println!("  {line}");
        }

        // 4. A provider-less plane keeps its F40 404 — and the probe is
        //    LOUD: the error names the status instead of fabricating a
        //    zeroed posture.
        let bare_tier = TieredCache::new_memory();
        let bare = HttpCacheService::new(bare_tier as Arc<dyn CacheHandler>);
        let bare_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bare_addr = bare_listener.local_addr().unwrap();
        tokio::spawn(async move {
            bare.serve(bare_listener)
                .await
                .expect("the accept loop serves");
        });
        let bare_client = HttpCacheHandler::new(format!("http://{bare_addr}"));
        match bare_client.peer_health().await {
            Ok(_) => println!("unexpected: a provider-less plane answered"),
            Err(e) => println!("peer_health (no provider) -> loud: {}", e.0.message),
        }

        // 5. The deadline rides: a hung peer fails the probe within the
        //    operator's budget under the file's stable code.
        let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_addr = dead.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                if let Ok((s, _)) = dead.accept().await {
                    // Accept and hold: the probe's exchange never completes.
                    let () = tokio::time::sleep(Duration::from_secs(3600)).await;
                    drop(s);
                }
            }
        });
        let hung_client = HttpCacheHandler::new(format!("http://{dead_addr}"))
            .with_timeout(Duration::from_millis(250));
        let started = std::time::Instant::now();
        match hung_client.peer_health().await {
            Ok(_) => println!("unexpected: the hung peer answered"),
            Err(e) => println!(
                "peer_health (hung, deadline 250ms) -> {} after {:?}: {}",
                e.0.code,
                started.elapsed(),
                e.0.message
            ),
        }
    });
    Ok(())
}

//! TDD pins for the client's peer-posture seam (V2-F63): the §11.2
//! `HttpCacheHandler` gains the pull side of the observability surfaces
//! F58/F61 gave the service.
//!
//! F58 gave the plane `GET /health` and F61 gave it `GET /metrics` — but
//! both surfaces answered only a DIRECT scrape or socket caller. The
//! client half — the node pointing `cache.handler = { type: 'http', url }`
//! at the plane — could not answer "how is my remote cache doing?" without
//! hand-rolling a raw socket exchange, exactly the duplication the F53
//! record's split exists to prevent. The [`PlanePeer`] seam is the pull
//! side: the trait the client implements, the probes riding the same
//! `request` core the entry verbs ride (the F60 deadline and the
//! `CACHE-HTTP` error context come with it, for free).
//!
//! Written FIRST (red): the pins fail with E0405 over the unresolved
//! `PlanePeer` trait and E0599 over the missing `peer_health`/`peer_metrics`
//! probes.

use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheHandler, HttpCacheService, MetricsCacheObserver,
    MetricsPlaneSource, PlaneHealth, PlaneMetrics, PlanePeer, Policy, TieredCache,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn entry() -> Entry {
    Entry {
        body: b"<html>peer-v1</html>".to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: vec![],
        policy: Policy {
            ttl: Duration::from_secs(3600),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        etag: "\"peer-v1\"".into(),
    }
}

/// A full plane on a real socket: the tier rooted as both the backing
/// store and the health provider, the exporter fanned onto it and
/// composed into the metrics source — the boot's own construction.
async fn peer_plane() -> (std::net::SocketAddr, Arc<TieredCache>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let metrics = Arc::new(MetricsCacheObserver::new());
    let tier = Arc::new(
        TieredCache::new(64 * 1024 * 1024)
            .with_observer(metrics.clone() as Arc<dyn oxide_fw_cache::CacheObserver>),
    );
    let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>)
        .with_health(Arc::clone(&tier) as Arc<dyn PlaneHealth>)
        .with_metrics(Arc::new(MetricsPlaneSource::new(
            metrics,
            tier.clone() as Arc<dyn PlaneHealth>,
        )) as Arc<dyn PlaneMetrics>);
    tokio::spawn(async move {
        svc.serve(l).await.expect("the accept loop serves");
    });
    (addr, tier)
}

/// The probe answers the provider's LIVE pull: the decoded `CacheHealth`
/// equals the tier's own `health()` sampled at the same moment — the F58
/// pull contract transposed onto the client half (a PULL, never a value
/// frozen at attach).
#[tokio::test]
async fn peer_health_pulls_the_live_posture() {
    let (addr, tier) = peer_plane().await;
    let client = HttpCacheHandler::new(format!("http://{addr}"));
    let pulled = client
        .peer_health()
        .await
        .expect("a provider-attached peer answers the probe");
    assert_eq!(pulled, tier.health(), "the pull IS the tier's live posture");
    // Live: traffic through the CLIENT moves the NEXT pull — a probe
    // diffs two pulls, nothing freezes.
    client
        .set("route:/peer-1", entry())
        .await
        .expect("the set lands through the plane");
    let after = client.peer_health().await.expect("the second pull answers");
    assert_eq!(
        after.memory_entries, 1,
        "the entry the client stored rides the next pull"
    );
}

/// The metrics probe answers the §20 exposition text — both halves
/// (counters then gauges), and the client's own traffic counts in the
/// NEXT pull.
#[tokio::test]
async fn peer_metrics_pulls_the_live_exposition() {
    let (addr, _tier) = peer_plane().await;
    let client = HttpCacheHandler::new(format!("http://{addr}"));
    let text = client
        .peer_metrics()
        .await
        .expect("a metrics-attached peer answers the probe");
    assert!(
        text.contains("# TYPE oxide_cache_plane_hits_total counter"),
        "the counters' half rides the pull:\n{text}"
    );
    assert!(
        text.contains("# TYPE oxide_cache_plane_memory_entries gauge"),
        "the gauges' half rides the pull:\n{text}"
    );
    client
        .set("route:/peer-2", entry())
        .await
        .expect("the set lands through the plane");
    let after = client
        .peer_metrics()
        .await
        .expect("the second pull answers");
    assert!(
        after.contains("oxide_cache_plane_sets_total 1"),
        "the client's set rides the next pull:\n{after}"
    );
}

/// A provider-less peer answers the F40 floor (the plain 404) — and the
/// probe turns it into a LOUD error naming the status, never a silent
/// default posture: an operator probe that lied would be worse than one
/// that fails.
#[tokio::test]
async fn probes_fail_loudly_against_a_provider_less_peer() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let tier = TieredCache::new_memory();
    let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>);
    tokio::spawn(async move {
        svc.serve(l).await.expect("the accept loop serves");
    });
    let client = HttpCacheHandler::new(format!("http://{addr}"));
    let health = client.peer_health().await;
    let err = health.expect_err("a provider-less peer's 404 is a loud error");
    assert_eq!(err.0.code, "CACHE-HTTP", "the file's stable code rides");
    assert!(
        err.0.message.contains("404"),
        "the error names the status the peer answered: {err:?}"
    );
    let metrics = client.peer_metrics().await;
    assert!(
        metrics.is_err(),
        "the metrics probe is loud against a provider-less peer too"
    );
}

/// The probes ride the F60 deadline: a hung peer fails the probe within
/// the deadline under the file's stable code — never a hang. The probe
/// reuses the same `request` core the entry verbs ride, so the deadline
/// and the error context come with it (the wire-up's whole point).
#[tokio::test]
async fn probes_ride_the_exchange_deadline() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            // Accept and hold: the probe's exchange never completes.
            if let Ok((_s, _)) = l.accept().await {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        }
    });
    let client =
        HttpCacheHandler::new(format!("http://{addr}")).with_timeout(Duration::from_millis(250));
    let started = Instant::now();
    let outcome = client.peer_health().await;
    let elapsed = started.elapsed();
    let err = outcome.expect_err("the hung probe fails loudly");
    assert_eq!(err.0.code, "CACHE-HTTP", "the deadline failure is coded");
    assert!(
        elapsed < Duration::from_secs(5),
        "the probe fails within the deadline (took {elapsed:?})"
    );
}

/// A 200 that is not a posture is a LOUD decode error: a proxy that
/// answered 200 with HTML (a captive portal, a misrouted front) must not
/// surface as a zeroed posture — the probe refuses under the stable code.
#[tokio::test]
async fn a_garbage_health_body_is_a_loud_decode_err() {
    // A raw peer that answers 200 with a non-JSON body.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = l.accept() {
            let mut buf = [0u8; 1024];
            let _ = std::io::Read::read(&mut s, &mut buf);
            let _ = std::io::Write::write_all(
                &mut s,
                b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot json\n",
            );
        }
    });
    let client = HttpCacheHandler::new(format!("http://{addr}"));
    let err = client
        .peer_health()
        .await
        .expect_err("a non-JSON 200 is a loud decode error");
    assert_eq!(err.0.code, "CACHE-HTTP", "the decode arm is coded");
}

/// The seam is a trait: a host that wraps the client (a failover pair, a
/// test double) composes through `Arc<dyn PlanePeer>` — the F58 provider
/// shape transposed onto the client half.
#[tokio::test]
async fn the_peer_trait_rides_dyn() {
    let (addr, _tier) = peer_plane().await;
    let peer: Arc<dyn PlanePeer> = Arc::new(HttpCacheHandler::new(format!("http://{addr}")));
    let pulled = peer.peer_health().await.expect("the trait object probes");
    assert_eq!(
        pulled.memory_entries, 0,
        "the empty tier's posture rides the trait object"
    );
}

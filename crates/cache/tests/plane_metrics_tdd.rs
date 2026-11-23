//! TDD pins for the cache plane's metrics arm (V2-F61): the §11.2 service's
//! own scrape surface.
//!
//! F58 gave the plane its health arm, but the plane still had NO metrics
//! story: the §20 exporter's counters (`MetricsCacheObserver`) and the F52
//! posture's gauges (`CacheHealth::render_prometheus`) rendered only
//! in-process — the CLI's `/_oxide/metrics` scrape behind the boot's opt-ins.
//! A dedicated plane socket (or a mounted one) answered nothing an operator
//! could scrape. The metrics arm is the plane's own surface:
//! `HttpCacheService::with_metrics` attaches a [`PlaneMetrics`] provider and
//! `GET /metrics` answers the live exposition — the F58 pull contract
//! transposed onto the counters' half (the unconfigured path 404s before
//! anything else, the surface is GET-only, the body is the LIVE pull — a
//! scrape diffs two pulls, nothing freezes it). The natural provider is the
//! [`MetricsPlaneSource`]: the boot's exporter behind the §20 fan plus the
//! tier's own posture pull — one value, both halves of the §20 story.
//!
//! Written FIRST (red): the pins fail with E0432 over the unresolved
//! `PlaneMetrics`/`MetricsPlaneSource` seams and E0599 over the missing
//! `with_metrics`/`has_metrics_provider` builders.

use oxide_fw_cache::{
    CacheHealth, HttpCacheService, MetricsCacheObserver, PlaneMetrics, TieredCache,
};
use std::sync::Arc;

// ------------------------------------------------------------------ helpers

/// A tier with the exporter fanned onto it — the boot's own construction
/// (`with_observer` consumes the tier, so the owned `new` shape feeds it).
fn observed_tier(metrics: &Arc<MetricsCacheObserver>) -> Arc<TieredCache> {
    Arc::new(
        TieredCache::new(64 * 1024 * 1024)
            .with_observer(metrics.clone() as Arc<dyn oxide_fw_cache::CacheObserver>),
    )
}

/// A fake provider: the exposition a host with its own counter store would
/// inject (tests and embedded hosts inject fakes — the F49 provider shape,
/// now for the metrics half).
struct FakeMetrics;

impl oxide_fw_cache::PlaneMetrics for FakeMetrics {
    fn plane_metrics(&self) -> oxide_fw_cache::CacheMetricsSnapshot {
        oxide_fw_cache::CacheMetricsSnapshot {
            hits_l0: 2,
            ..Default::default()
        }
    }

    fn plane_gauges(&self) -> CacheHealth {
        CacheHealth {
            memory_entries: 3,
            ..Default::default()
        }
    }
}

fn plain_service() -> HttpCacheService {
    let tier = TieredCache::new_memory();
    HttpCacheService::new(tier as Arc<dyn oxide_fw_cache::CacheHandler>)
}

fn source_service() -> (Arc<MetricsCacheObserver>, HttpCacheService) {
    let metrics = Arc::new(MetricsCacheObserver::new());
    let tier = observed_tier(&metrics);
    let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>)
        .with_metrics(Arc::new(oxide_fw_cache::MetricsPlaneSource::new(
            metrics.clone(),
            tier.clone() as Arc<dyn oxide_fw_cache::PlaneHealth>,
        )) as Arc<dyn oxide_fw_cache::PlaneMetrics>);
    (metrics, svc)
}

async fn respond(
    svc: &HttpCacheService,
    method: &str,
    path: &str,
) -> oxide_fw_cache::PlaneResponse {
    svc.respond(&oxide_fw_cache::PlaneRequest {
        method: method.into(),
        path: path.into(),
        body: Vec::new(),
    })
    .await
}

// ------------------------------------------------------------- the metrics arm

/// The F40 doctrine on the plane's own wire: an unconfigured `/metrics`
/// keeps the pre-F61 answer (route's `Unknown` 404) — a boot that never
/// attached a provider leaks nothing, byte-identically, not even a method
/// table (the 404 answers BEFORE the method check).
#[tokio::test]
async fn unconfigured_metrics_answers_404() {
    let svc = plain_service();
    let resp = respond(&svc, "GET", "/metrics").await;
    assert_eq!(resp.status, 404, "the unconfigured arm stays a plain 404");
    assert_eq!(resp.body, [] as [u8; 0]);

    let resp = respond(&svc, "POST", "/metrics").await;
    assert_eq!(
        resp.status, 404,
        "the unconfigured path 404s before the method check"
    );
}

/// The configured arm answers the provider's pull as the §20 text
/// exposition: a PULL — the body reflects the counters as they stand NOW,
/// never a value frozen at attach time.
#[tokio::test]
async fn configured_metrics_answers_the_provider_exposition() {
    let (_metrics, svc) = source_service();
    let resp = respond(&svc, "GET", "/metrics").await;
    assert_eq!(resp.status, 200);
    let body = String::from_utf8_lossy(&resp.body);
    assert!(
        body.contains("# TYPE oxide_cache_plane_hits_total counter"),
        "the counters' half rides the body:\n{body}"
    );
    assert!(
        body.contains("# TYPE oxide_cache_plane_memory_entries gauge"),
        "the gauges' half rides the body:\n{body}"
    );
}

/// The pull reads the LIVE traffic: the plane's own entry GET drives the
/// observer (one L0 hit through the tier the exporter fans onto), and the
/// next scrape answers the delta — a scrape diffs two pulls, the plane
/// never freezes the counters.
#[tokio::test]
async fn the_metrics_pull_reads_live_traffic() {
    let metrics = Arc::new(MetricsCacheObserver::new());
    let tier = observed_tier(&metrics);
    let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>)
        .with_metrics(Arc::new(oxide_fw_cache::MetricsPlaneSource::new(
            metrics.clone(),
            tier.clone() as Arc<dyn oxide_fw_cache::PlaneHealth>,
        )) as Arc<dyn oxide_fw_cache::PlaneMetrics>);
    tier.set("route:/m-1", test_entry())
        .await
        .expect("the tier stores");
    let miss = respond(&svc, "GET", "/entry/route:/m-1").await;
    assert_eq!(miss.status, 200, "the plane serves the entry");
    let resp = respond(&svc, "GET", "/metrics").await;
    assert_eq!(resp.status, 200);
    let body = String::from_utf8_lossy(&resp.body);
    assert!(
        body.contains("oxide_cache_plane_hits_total{layer=\"L0\"} 1"),
        "the L0 hit the plane itself served rides the exposition:\n{body}"
    );
    assert!(
        body.contains("oxide_cache_plane_memory_entries 1"),
        "the resident the plane stored rides the gauges' half:\n{body}"
    );
}

/// The exposition order is the plane's contract (the F33 rule transposed to
/// the joined body): the counter families in the exporter's established
/// order — ending with the F51 eviction pair — precede the gauge families in
/// the posture's established order — ending with the F52 eviction gauge.
/// A scraper's offsets never move.
#[tokio::test]
async fn the_exposition_order_is_counters_then_gauges() {
    let (_metrics, svc) = source_service();
    let resp = respond(&svc, "GET", "/metrics").await;
    let body = String::from_utf8_lossy(&resp.body);
    let last_counter = body
        .find("oxide_cache_plane_evicted_bytes_total ")
        .expect("the F51 eviction counter pair is the counters' half's last family");
    let first_gauge = body
        .find("# HELP oxide_cache_plane_memory_entries ")
        .expect("the posture's first gauge family joins the body");
    let last_gauge = body
        .find("oxide_cache_plane_disk_evictions ")
        .expect("the F52 eviction gauge is the gauges' half's last family");
    assert!(
        last_counter < first_gauge,
        "the counters' half renders before the gauges' half:\n{body}"
    );
    assert!(
        first_gauge < last_gauge,
        "the gauges' half keeps the posture's order (F52's gauge appends last):\n{body}"
    );
}

/// Two scrapes with no traffic between are byte-equal — the deterministic
/// render contract (`render_prometheus`'s module doc), now on the wire: a
/// diff of two scrapes is a diff of the traffic, not of the renderer.
#[tokio::test]
async fn two_scrapes_without_traffic_are_byte_equal() {
    let (_metrics, svc) = source_service();
    let first = respond(&svc, "GET", "/metrics").await;
    let second = respond(&svc, "GET", "/metrics").await;
    assert_eq!(first.status, 200);
    assert_eq!(
        first.body, second.body,
        "the exposition is deterministic over the same counters"
    );
}

/// The surface is GET-only (405 otherwise) — the plane's method table owns
/// the answer, the same posture the health arm rides.
#[tokio::test]
async fn configured_non_get_is_405() {
    let (_metrics, svc) = source_service();
    for method in ["POST", "PUT", "DELETE"] {
        let resp = respond(&svc, method, "/metrics").await;
        assert_eq!(
            resp.status, 405,
            "the {method} exchange rides the plane's 405, not a fall-through"
        );
    }
}

/// An injected provider counts the same as the natural one: the trait is
/// the seam — a host with its own counter store (a test harness, an
/// embedded exporter) attaches it verbatim and the arm answers ITS pull.
#[tokio::test]
async fn an_injected_provider_answers_verbatim() {
    let tier = TieredCache::new_memory();
    let svc = HttpCacheService::new(tier as Arc<dyn oxide_fw_cache::CacheHandler>)
        .with_metrics(Arc::new(FakeMetrics) as Arc<dyn oxide_fw_cache::PlaneMetrics>);
    let resp = respond(&svc, "GET", "/metrics").await;
    assert_eq!(resp.status, 200);
    let body = String::from_utf8_lossy(&resp.body);
    assert!(
        body.contains("oxide_cache_plane_hits_total{layer=\"L0\"} 2"),
        "the fake's counter pull answers verbatim:\n{body}"
    );
    assert!(
        body.contains("oxide_cache_plane_memory_entries 3"),
        "the fake's gauge pull answers verbatim:\n{body}"
    );
}

/// The arm rides the socket shell too: the ONE call a host makes to host
/// the plane answers metrics on the same listener — the wire shape the
/// client's `request` half parses.
#[tokio::test]
async fn the_metrics_arm_rides_the_socket_shell() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (_metrics, svc) = source_service();
    tokio::spawn(async move {
        svc.serve(listener).await.expect("the accept loop serves");
    });

    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    sock.write_all(b"GET /metrics HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.unwrap();
    let head = String::from_utf8_lossy(&buf);
    assert!(
        head.starts_with("HTTP/1.1 200 OK"),
        "the socket shell answers the metrics arm: {head}"
    );
    let body = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map_or_default(|i| &buf[i + 4..]);
    let text = String::from_utf8_lossy(body);
    assert!(
        text.contains("# TYPE oxide_cache_plane_hits_total counter"),
        "the wire body is the exposition:\n{text}"
    );
}

// ------------------------------------------------------------------- the source

/// The natural provider composes the boot's two halves: the exporter's
/// snapshot rides the counter pull, the tier's F52 pull rides the gauge
/// pull — one value, both halves of the §20 story, zero extra state.
#[test]
fn the_metrics_plane_source_composes_the_boot_halves() {
    let metrics = Arc::new(MetricsCacheObserver::new());
    let tier = TieredCache::new_memory();
    let source = oxide_fw_cache::MetricsPlaneSource::new(
        metrics.clone(),
        tier.clone() as Arc<dyn oxide_fw_cache::PlaneHealth>,
    );
    assert_eq!(
        source.plane_metrics(),
        metrics.snapshot(),
        "the counter pull IS the exporter's snapshot"
    );
    assert_eq!(
        source.plane_gauges(),
        tier.health(),
        "the gauge pull IS the tier's F52 pull"
    );
}

// ------------------------------------------------------------------- the mount's routing seam (V2-F61)

/// The F57 mount's routing seam grows its metrics half: the mount must
/// answer `/metrics` through the plane ONLY when a metrics provider rides
/// the service — a provider-less mount must never shadow an application's
/// own `/metrics` route, while a provider-attached mount is exactly the
/// scrape surface the F61 record promises a collector. The accessor is the
/// decision's input: the service knows its own counters, the mount asks
/// instead of guessing (the F59 `has_health_provider` shape).
///
/// Written FIRST (red): E0599 over the unresolved `has_metrics_provider`.
#[test]
fn has_metrics_provider_reflects_the_attached_state() {
    let plain =
        HttpCacheService::new(TieredCache::new_memory() as Arc<dyn oxide_fw_cache::CacheHandler>);
    assert!(
        !plain.has_metrics_provider(),
        "no provider attached — the mount must not route /metrics into the plane"
    );

    let (metrics, tier) = source_parts();
    let attached =
        HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>)
            .with_metrics(Arc::new(oxide_fw_cache::MetricsPlaneSource::new(
                metrics,
                tier.clone() as Arc<dyn oxide_fw_cache::PlaneHealth>,
            )) as Arc<dyn oxide_fw_cache::PlaneMetrics>);
    assert!(
        attached.has_metrics_provider(),
        "the natural provider rides — the mount routes /metrics through the plane"
    );

    let fake_attached =
        HttpCacheService::new(TieredCache::new_memory() as Arc<dyn oxide_fw_cache::CacheHandler>)
            .with_metrics(Arc::new(FakeMetrics) as Arc<dyn oxide_fw_cache::PlaneMetrics>);
    assert!(
        fake_attached.has_metrics_provider(),
        "an injected provider counts the same as the natural one"
    );
}

/// The source parts for the accessor pin: the exporter handle plus the
/// tier it fans onto.
fn source_parts() -> (Arc<MetricsCacheObserver>, Arc<TieredCache>) {
    let metrics = Arc::new(MetricsCacheObserver::new());
    let tier = observed_tier(&metrics);
    (metrics, tier)
}

fn test_entry() -> oxide_fw_cache::Entry {
    oxide_fw_cache::Entry {
        body: b"ok".to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: vec![],
        policy: oxide_fw_cache::Policy {
            ttl: std::time::Duration::from_secs(3600),
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
        etag: "\"metrics-v1\"".into(),
    }
}

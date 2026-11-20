//! TDD pins for the mounted cache plane's metrics arm (V2-F61): the F57
//! mount × the F61 arm, composed.
//!
//! F59 taught the mount to route the F58 health arm's surface (`/health`
//! behind [`HttpCacheService::has_health_provider`]), but the mount's path
//! check still names only the three protocol families plus health — so the
//! F61 metrics arm, "the scrape a collector can point at the shared node",
//! was unreachable on the mount. The F61 wire closes that seam with the
//! F58 contract verbatim: a mount whose service carries a metrics provider
//! answers `GET /metrics` with the live §20 exposition (counters then
//! gauges), a mount WITHOUT a provider leaves `/metrics` to the app (an
//! unconfigured plane must never shadow an application route — the F40
//! doctrine), and the method table rides the plane's own (a POST answers
//! 405, an unconfigured plane's `/metrics` never reaches the 405 check).
//!
//! Written FIRST (red): the pins compile against the F59 surface and fail
//! behaviorally — the pre-F61 mount routes `/metrics` to dispatch, so the
//! provider-attached pin sees the dispatch echo instead of the exposition,
//! and the method-table pin sees a dispatch 200 instead of the plane's 405.

use futures::future::BoxFuture;
use oxide_fw_cache::{HttpCacheService, MetricsCacheObserver, MetricsPlaneSource, TieredCache};
use oxide_fw_server::{OxideRequest, OxideResponse, Pipeline, Server};
use std::sync::Arc;
use std::time::Duration;

// ------------------------------------------------------------------ helpers

async fn free_port_addr() -> std::net::SocketAddr {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    addr
}

/// The natural provider shape a real deployment mounts: the service over
/// the boot's own tier, the boot's §20 exporter fanned onto that tier, the
/// composed [`MetricsPlaneSource`] carrying both halves (the F61 natural
/// provider — one `Arc`, both pulls).
fn wired_mounted_service() -> Arc<HttpCacheService> {
    let metrics = Arc::new(MetricsCacheObserver::new());
    let tier = Arc::new(
        TieredCache::new(64 * 1024 * 1024)
            .with_observer(metrics.clone() as Arc<dyn oxide_fw_cache::CacheObserver>),
    );
    let source = MetricsPlaneSource::new(
        metrics,
        tier.clone() as Arc<dyn oxide_fw_cache::PlaneHealth>,
    );
    Arc::new(
        HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>)
            .with_health(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::PlaneHealth>)
            .with_metrics(Arc::new(source) as Arc<dyn oxide_fw_cache::PlaneMetrics>),
    )
}

/// The provider-less shape (a host that mounted the plane for its entry
/// families only): the metrics arm must stay OUT of the mount's path
/// families so an app's own `/metrics` route survives.
fn plain_mounted_service() -> Arc<HttpCacheService> {
    Arc::new(HttpCacheService::new(
        TieredCache::new_memory() as Arc<dyn oxide_fw_cache::CacheHandler>
    ))
}

fn echo_dispatch() -> Arc<dyn Fn(OxideRequest) -> BoxFuture<'static, OxideResponse> + Send + Sync> {
    Arc::new(|r: OxideRequest| {
        Box::pin(async move { OxideResponse::html(200, format!("dispatch:{}", r.url)) })
    })
}

/// Spawn the server with the given pipeline; wait for the listener;
/// return the address and the shutdown sender (alive for the test's
/// lifetime).
async fn spawn_server(p: Pipeline) -> (std::net::SocketAddr, tokio::sync::watch::Sender<bool>) {
    let addr = free_port_addr().await;
    let p = Arc::new(p);
    let dispatch = echo_dispatch();
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(Server::serve(addr, p, dispatch, rx));
    for _ in 0..200 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return (addr, tx);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the server never bound its listener");
}

/// One raw HTTP/1.1 exchange (`Connection: close`): the status code and
/// the body bytes.
async fn exchange(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    body: &[u8],
) -> (u16, Vec<u8>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).await.unwrap();
    if !body.is_empty() {
        s.write_all(body).await.unwrap();
    }
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let head_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a header terminator");
    let head = String::from_utf8_lossy(&buf[..head_end]);
    let status: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .expect("a status line");
    (status, buf[head_end + 4..].to_vec())
}

// ------------------------------------------------------------------- the metrics mount

/// The collector's surface: a provider-attached mount answers `GET
/// /metrics` with the live §20 exposition — the F61 arm through the F57
/// mount, one socket, no dedicated plane listener.
#[tokio::test]
async fn mounted_plane_with_a_provider_answers_metrics_on_the_mount() {
    let p = Pipeline::default().with_cache_plane(wired_mounted_service());
    let (addr, _tx) = spawn_server(p).await;
    let (status, body) = exchange(addr, "GET", "/metrics", &[]).await;
    assert_eq!(
        status, 200,
        "the provider-attached mount answers the scrape"
    );
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("# TYPE oxide_cache_plane_hits_total counter"),
        "the counters' half rides the mount's answer:\n{text}"
    );
    assert!(
        text.contains("# TYPE oxide_cache_plane_memory_entries gauge"),
        "the gauges' half rides the mount's answer:\n{text}"
    );
}

/// The user-route contract: a provider-less mount never shadows an
/// application `/metrics` route — the path falls through to dispatch (the
/// echo answers), exactly the pre-F61 shape for the paths the mount does
/// not own.
#[tokio::test]
async fn mounted_plane_without_a_provider_leaves_metrics_to_dispatch() {
    let p = Pipeline::default().with_cache_plane(plain_mounted_service());
    let (addr, _tx) = spawn_server(p).await;
    let (status, body) = exchange(addr, "GET", "/metrics", &[]).await;
    assert_eq!(status, 200, "the fall-through answers through dispatch");
    assert_eq!(
        body,
        b"dispatch:/metrics".to_vec(),
        "the app's own /metrics route survives a provider-less mount"
    );
}

/// The method table rides the plane's own: with a provider attached, a
/// POST to the mounted surface is a 405 (the F61 contract — the surface
/// is GET-only), NOT a dispatch fall-through.
#[tokio::test]
async fn mounted_plane_metrics_answers_only_get() {
    let p = Pipeline::default().with_cache_plane(wired_mounted_service());
    let (addr, _tx) = spawn_server(p).await;
    let (status, _) = exchange(addr, "POST", "/metrics", &[]).await;
    assert_eq!(status, 405, "the plane's method table owns the 405");
}

/// No mount at all: `/metrics` falls through to dispatch — every pre-F57
/// byte preserved (the F40 doctrine over the whole family).
#[tokio::test]
async fn unmounted_metrics_falls_through_to_dispatch() {
    let p = Pipeline::default();
    let (addr, _tx) = spawn_server(p).await;
    let (status, body) = exchange(addr, "GET", "/metrics", &[]).await;
    assert_eq!(status, 200);
    assert_eq!(body, b"dispatch:/metrics".to_vec(), "the unmounted shape");
}

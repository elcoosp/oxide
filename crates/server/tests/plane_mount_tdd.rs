//! TDD pins for the §11.2 cache plane's mount on the app server (V2-F57).
//!
//! F53 minted the plane's HTTP service ([`HttpCacheService`]) — the client's
//! missing server half — but nothing in the workspace HOSTS it: the composer
//! set is the service's own example and pins. The recorded pickup ("run the
//! two-node serve with `HttpCacheService` hosting the shared plane") needs a
//! production host, and the production host is the app server itself: this
//! file pins [`Pipeline::with_cache_plane`], the mount that answers the
//! plane's three path families (`/entry/`, `/tag/`, `/invalidate/`) through
//! the service's pure routing core — one socket, one TLS/firewall surface,
//! the site AND the shared plane on the same node. A second node points its
//! `cache.handler = { type: 'http', url }` at the mount.
//!
//! Written FIRST (red): every pin fails with E0599 over the unresolved
//! `with_cache_plane` builder before the seam exists.
//!
//! The shape is the F49 posture-probe precedent verbatim: an `Option` field
//! (the F40 doctrine — unattached, every byte of the pre-mount shape is
//! preserved, the plane paths fall through to dispatch), a chainable
//! builder, and a `NativePrefix` variant whose async arm runs the service's
//! `respond` core exactly where the `ServerFn`/`LocalImage`/`RemoteImage`
//! arms run theirs.

use futures::future::BoxFuture;
use oxide_fw_cache::{Entry, HttpCacheService, Policy, TieredCache, encode_entry, seal_entry};
use oxide_fw_server::{OxideRequest, OxideResponse, Pipeline, Server, StaticAssets};
use std::sync::Arc;
use std::time::Duration;

// ------------------------------------------------------------------ helpers

async fn free_port_addr() -> std::net::SocketAddr {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    addr
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn entry(body: &[u8], tags: &[&str]) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: tags.iter().copied().map(str::to_string).collect(),
        policy: Policy {
            ttl: Duration::from_secs(60),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: now_ms(),
        etag: "\"mount-v1\"".into(),
    }
}

/// The backing store a real deployment hosts: a `TieredCache` IS a
/// `CacheHandler`, and the service composes it behind the plane protocol.
fn mounted_service() -> Arc<HttpCacheService> {
    let tier = TieredCache::new_memory();
    Arc::new(HttpCacheService::new(
        tier as Arc<dyn oxide_fw_cache::CacheHandler>,
    ))
}

fn echo_dispatch() -> Arc<dyn Fn(OxideRequest) -> BoxFuture<'static, OxideResponse> + Send + Sync> {
    Arc::new(|r: OxideRequest| {
        Box::pin(async move { OxideResponse::html(200, format!("dispatch:{}", r.url)) })
    })
}

/// Spawn the server with the given pipeline; wait for the listener; return
/// the address and the shutdown sender (alive for the test's lifetime).
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

/// One raw HTTP/1.1 exchange (`Connection: close`): the status code and the
/// body bytes. The plane's bodies are sealed binary — bytes, never a String.
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

// ------------------------------------------------------------------- the mount

/// The mount's round trip: a client PUTs the sealed entry it would PUT at a
/// dedicated plane socket, the mount lands it through the service, and a GET
/// answers the same sealed bytes (the codec + MAC are deterministic — the
/// §11.2 wire contract, byte for byte).
#[tokio::test]
async fn mounted_plane_put_then_get_round_trips_the_sealed_entry() {
    let p = Pipeline::default().with_cache_plane(mounted_service());
    let (addr, _tx) = spawn_server(p).await;
    let e = entry(b"<h1>mount-1</h1>", &[]);
    let sealed = seal_entry(&encode_entry(&e).unwrap());

    let (status, body) = exchange(addr, "PUT", "/entry/route:/mount-1", &sealed).await;
    assert_eq!(status, 204, "the mounted plane stores: {status}");
    assert!(body.is_empty(), "204 carries no body");

    let (status, body) = exchange(addr, "GET", "/entry/route:/mount-1", &[]).await;
    assert_eq!(status, 200, "the mounted plane serves");
    assert_eq!(body, sealed, "the sealed bytes survive the mount verbatim");
}

/// A miss on the mounted plane is the service's own 404 — the mount is
/// transparent, the plane's statuses answer unmodified.
#[tokio::test]
async fn mounted_plane_miss_answers_404() {
    let p = Pipeline::default().with_cache_plane(mounted_service());
    let (addr, _tx) = spawn_server(p).await;
    let (status, body) = exchange(addr, "GET", "/entry/route:/absent", &[]).await;
    assert_eq!(status, 404);
    assert!(body.is_empty());
}

/// The trust boundary holds through the mount: a forged (unsealed) PUT is a
/// 400 and NEVER enters the store — the same refusal the service's own
/// socket shell answers, enforced where the payload writes.
#[tokio::test]
async fn mounted_plane_forged_put_is_400_and_never_stores() {
    let p = Pipeline::default().with_cache_plane(mounted_service());
    let (addr, _tx) = spawn_server(p).await;
    let (status, body) = exchange(addr, "PUT", "/entry/route:/forged", b"garbage-not-sealed").await;
    assert_eq!(status, 400, "the forged payload is refused");
    assert!(body.is_empty());
    let (status, body) = exchange(addr, "GET", "/entry/route:/forged", &[]).await;
    assert_eq!(status, 404, "the refusal stored nothing");
    assert!(body.is_empty());
}

/// A known family with the wrong verb is the service's 405 — the mount
/// forwards the exchange, the plane's method table answers.
#[tokio::test]
async fn mounted_plane_wrong_method_is_405() {
    let p = Pipeline::default().with_cache_plane(mounted_service());
    let (addr, _tx) = spawn_server(p).await;
    let (status, body) = exchange(addr, "POST", "/entry/route:/mount-1", &[]).await;
    assert_eq!(status, 405);
    assert!(body.is_empty());
}

/// The two-node story, in one process: the entry rides a tag, the tag lists
/// the key, the hard invalidation crosses to the entry, and the plane
/// answers coherent misses after — the §11.2 shared-plane contract through
/// the mount.
#[tokio::test]
async fn mounted_plane_tag_list_and_hard_invalidation_crosses_nodes() {
    let p = Pipeline::default().with_cache_plane(mounted_service());
    let (addr, _tx) = spawn_server(p).await;
    let e = entry(b"<h1>tagged</h1>", &["mount-tag"]);
    let sealed = seal_entry(&encode_entry(&e).unwrap());
    let (status, _) = exchange(addr, "PUT", "/entry/route:/mount-2", &sealed).await;
    assert_eq!(status, 204);

    let (status, body) = exchange(addr, "GET", "/tag/mount-tag", &[]).await;
    assert_eq!(status, 200, "the tag lists through the mount");
    let keys: Vec<String> = serde_json::from_slice(&body).unwrap();
    assert_eq!(keys, vec!["route:/mount-2".to_string()]);

    let (status, body) = exchange(addr, "POST", "/invalidate/mount-tag", b"{\"soft\":false}").await;
    assert_eq!(status, 200, "the invalidation rides the mount");
    let answer: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(answer["count"], 1, "one key invalidated");

    let (status, _) = exchange(addr, "GET", "/entry/route:/mount-2", &[]).await;
    assert_eq!(status, 404, "the hard invalidation crossed nodes");
    let (status, body) = exchange(addr, "GET", "/tag/mount-tag", &[]).await;
    assert_eq!(status, 200);
    assert_eq!(body, b"[]".to_vec(), "the tag index emptied");
}

/// The F40 doctrine: no mount, no seam — the plane paths fall through to
/// dispatch byte-identically with the pre-F57 shape (the §11.2 protocol is
/// opt-in; an app serving its own `/entry/…` route is untouched).
#[tokio::test]
async fn unmounted_plane_paths_fall_through_to_dispatch() {
    let p = Pipeline::default();
    let (addr, _tx) = spawn_server(p).await;
    let (status, body) = exchange(addr, "GET", "/entry/route:/unmounted", &[]).await;
    assert_eq!(status, 200);
    assert_eq!(
        String::from_utf8_lossy(&body),
        "dispatch:/entry/route:/unmounted",
        "the plane path reached the dispatch delegate, not a plane"
    );
}

/// The mounted plane answers BEFORE the static store: an asset that happens
/// to live at a plane-shaped path is shadowed for the plane's families —
/// the mount is explicit, and when it is attached the plane owns its
/// protocol paths (an asset at `entry/x.html` stays reachable only on an
/// unmounted node).
#[tokio::test]
async fn mounted_plane_paths_answer_from_the_plane_not_the_asset_store() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("entry")).unwrap();
    std::fs::write(dir.path().join("entry").join("x.html"), b"ASSET").unwrap();
    let assets = StaticAssets::scan(dir.path()).unwrap();
    let p = Pipeline {
        assets: Some(assets),
        ..Default::default()
    }
    .with_cache_plane(mounted_service());
    let (addr, _tx) = spawn_server(p).await;
    let (status, body) = exchange(addr, "GET", "/entry/x.html", &[]).await;
    assert_eq!(
        status, 404,
        "the plane owns the family: a miss, not the asset"
    );
    assert!(body.is_empty());
}

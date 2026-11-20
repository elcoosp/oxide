//! TDD pins for the cache plane's health arm (V2-F58): the §11.2 service's
//! own health surface.
//!
//! F53's service composed any [`CacheHandler`] behind the client's protocol,
//! but the plane itself had NO health story: a dedicated plane socket (or a
//! mounted one) answered nothing an operator could probe — the F52 posture
//! pull (`TieredCache::health()` → `CacheHealth`) rides the app server's
//! scrape and the `/-/health` probe, both behind the CLI's opt-ins. The
//! health arm is the plane's own surface: `HttpCacheService::with_health`
//! attaches a [`PlaneHealth`] provider and `GET /health` answers the pull's
//! JSON — the F49 probe's contract transposed onto the plane's wire (the
//! unconfigured path 404s before anything else, the surface is GET-only,
//! the body is the live pull — a PULL, never a frozen snapshot).
//!
//! Written FIRST (red): the pins fail with E0432 over the unresolved
//! `PlaneHealth` trait and E0599 over the missing `with_health` builder.

use oxide_fw_cache::{
    CacheHealth, Entry, HttpCacheService, PlaneRequest, PlaneResponse, Policy, TieredCache,
};
use std::sync::Arc;
use std::time::Duration;

// ------------------------------------------------------------------ helpers

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
            ttl: Duration::from_secs(3600),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: now_ms(),
        etag: "\"health-v1\"".into(),
    }
}

/// A fake provider: the value a host with its own posture would inject
/// (tests and embedded hosts inject fakes — the F49 provider shape).
struct FakeHealth(CacheHealth);

impl oxide_fw_cache::PlaneHealth for FakeHealth {
    fn plane_health(&self) -> CacheHealth {
        self.0.clone()
    }
}

fn fake() -> CacheHealth {
    CacheHealth {
        memory_entries: 7,
        memory_bytes: 4_096,
        memory_capacity_bytes: 65_536,
        disk_attached: false,
        disk_bytes: 0,
        remote_attached: false,
        remote_kind: None,
        ..Default::default()
    }
}

fn plain_service() -> HttpCacheService {
    let tier = TieredCache::new_memory();
    HttpCacheService::new(tier as Arc<dyn oxide_fw_cache::CacheHandler>)
}

fn healthy_service() -> HttpCacheService {
    let tier = TieredCache::new_memory();
    HttpCacheService::new(tier as Arc<dyn oxide_fw_cache::CacheHandler>)
        .with_health(Arc::new(FakeHealth(fake())))
}

async fn respond(svc: &HttpCacheService, method: &str, path: &str) -> PlaneResponse {
    svc.respond(&PlaneRequest {
        method: method.into(),
        path: path.into(),
        body: Vec::new(),
    })
    .await
}

// ------------------------------------------------------------- the health arm

/// The F40 doctrine on the plane's own wire: an unconfigured `/health` keeps
/// the pre-F58 answer (route's `Unknown` 404) — a boot that never attached a
/// provider leaks nothing, byte-identically.
#[tokio::test]
async fn unconfigured_health_answers_404() {
    let svc = plain_service();
    let resp = respond(&svc, "GET", "/health").await;
    assert_eq!(resp.status, 404, "the unconfigured arm stays a plain 404");
    assert!(resp.body.is_empty());
}

/// The configured arm answers the provider's pull as JSON: a PULL — the
/// body reflects the posture as it stands NOW, never a value frozen at
/// attach time (the F49 probe's contract, verbatim).
#[tokio::test]
async fn configured_health_answers_the_provider_json() {
    let svc = healthy_service();
    let resp = respond(&svc, "GET", "/health").await;
    assert_eq!(resp.status, 200);
    let answered: CacheHealth =
        serde_json::from_slice(&resp.body).expect("the body is the CacheHealth JSON");
    assert_eq!(answered, fake(), "the provider's pull answers verbatim");
}

/// The surface is GET-only (405 otherwise), and the unconfigured path 404s
/// BEFORE the method check (it leaks nothing, not even a method table).
#[tokio::test]
async fn configured_non_get_is_405_unconfigured_stays_404() {
    let svc = healthy_service();
    let resp = respond(&svc, "POST", "/health").await;
    assert_eq!(resp.status, 405, "the configured surface is GET-only");

    let plain = plain_service();
    let resp = respond(&plain, "DELETE", "/health").await;
    assert_eq!(resp.status, 404, "the unconfigured path 404s first");
}

/// The plane's own entries feed the pull: three stored entries show up as
/// three residents on the next GET /health — a scrape diffs the count, the
/// plane never freezes it.
#[tokio::test]
async fn the_health_pull_reads_live_state() {
    let tier = TieredCache::new_memory();
    let provider: Arc<dyn oxide_fw_cache::PlaneHealth> = tier.clone();
    let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>)
        .with_health(provider);
    for i in 0..3 {
        tier.set(&format!("route:/health-{i}"), entry(b"ok", &[]))
            .await
            .expect("the tier stores");
    }
    let resp = respond(&svc, "GET", "/health").await;
    assert_eq!(resp.status, 200);
    let answered: CacheHealth = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(
        answered.memory_entries, 3,
        "the pull reads the tier's live resident count"
    );
}

/// `TieredCache` IS a `PlaneHealth` provider — its pull is the F52
/// `health()` value unchanged, so a plane rooted at a tier reports the same
/// posture the app server's probe reports.
#[test]
fn the_tiered_cache_is_a_plane_health_provider() {
    let tier = TieredCache::new_memory();
    let provider: Arc<dyn oxide_fw_cache::PlaneHealth> = tier.clone();
    assert_eq!(
        provider.plane_health(),
        tier.health(),
        "the provider pull IS the F52 pull"
    );
    let json = serde_json::to_string(&provider.plane_health()).unwrap();
    for field in [
        "memory_entries",
        "memory_bytes",
        "memory_capacity_bytes",
        "disk_attached",
        "remote_attached",
    ] {
        assert!(json.contains(field), "the {field} row rides the JSON");
    }
}

/// The arm rides the socket shell too: the ONE call a host makes to host
/// the plane answers health on the same listener — the wire shape the
/// client's `request` half parses.
#[tokio::test]
async fn the_health_arm_rides_the_socket_shell() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let svc = healthy_service();
    tokio::spawn(async move {
        svc.serve(listener).await.expect("the accept loop serves");
    });

    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    sock.write_all(b"GET /health HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.unwrap();
    let head = String::from_utf8_lossy(&buf);
    assert!(
        head.starts_with("HTTP/1.1 200 OK"),
        "the socket shell answers the health arm: {head}"
    );
    let body = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| &buf[i + 4..])
        .unwrap_or_default();
    let answered: CacheHealth =
        serde_json::from_slice(body).expect("the wire body is the CacheHealth JSON");
    assert_eq!(answered, fake());
}

// ------------------------------------------------------------------- the mount's routing seam (V2-F59)

/// The F57 mount's routing seam (V2-F59): the mount must answer `/health`
/// through the plane ONLY when a provider rides the service — a
/// provider-less mount must never shadow an app's own `/health` route,
/// while a provider-attached mount is exactly the surface the F58 record
/// promised a balancer ("the probe a balancer can point at the shared
/// node"). The accessor is the decision's input: the service knows its
/// own posture, the mount asks instead of guessing.
///
/// Written FIRST (red): E0599 over the unresolved `has_health_provider`.
#[test]
fn has_health_provider_reflects_the_attached_state() {
    let plain =
        HttpCacheService::new(TieredCache::new_memory() as Arc<dyn oxide_fw_cache::CacheHandler>);
    assert!(
        !plain.has_health_provider(),
        "no provider attached — the mount must not route /health into the plane"
    );

    let tier = TieredCache::new_memory();
    let attached =
        HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>)
            .with_health(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::PlaneHealth>);
    assert!(
        attached.has_health_provider(),
        "the natural provider rides — the mount routes /health through the plane"
    );

    let fake_attached =
        HttpCacheService::new(TieredCache::new_memory() as Arc<dyn oxide_fw_cache::CacheHandler>)
            .with_health(Arc::new(FakeHealth(fake())) as Arc<dyn oxide_fw_cache::PlaneHealth>);
    assert!(
        fake_attached.has_health_provider(),
        "an injected provider counts the same as the natural one"
    );
}

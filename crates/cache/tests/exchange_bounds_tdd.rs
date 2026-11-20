//! TDD pins for the plane's exchange-bounds seam (V2-F64): the socket
//! shell's body drain learns the cap its header read already had.
//!
//! The F53 shell bounded the HEADER read (`HEADER_CAP`, the hostile-client
//! refusal) but the file's own comment recorded the gap: "The cap guards
//! the PARSER, not the codec — the codec re-checks the decompressed size
//! itself." The body drain ran `while body.len() < clen` — a peer writing
//! `Content-Length: 1099511627776` and trickling bytes grew the buffer
//! without bound: the OOM the header cap exists to prevent, one header
//! line later. The codec's `MAX_ENTRY_BYTES` check happens only AFTER the
//! drain — too late for the memory. [`ExchangeLimits`] is the seam: the
//! shell refuses an oversized `Content-Length` with 413 BEFORE draining,
//! the default cap rides the codec's own bound (every exchange the codec
//! would accept drains; every exchange it would refuse is refused before
//! the bytes land), and `with_limits`/`limits` are the operator knob and
//! the introspection seam (the F60 `deadline()` shape).
//!
//! Written FIRST (red): the pins fail with E0433 over the unresolved
//! `ExchangeLimits` and E0599 over the missing `with_limits`/`limits`.

use oxide_fw_cache::{ExchangeLimits, HttpCacheService, MAC_LEN, MAX_ENTRY_BYTES, TieredCache};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A plane on a real socket, the caller picks the limits.
async fn bounded_plane(limits: ExchangeLimits) -> std::net::SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let tier = TieredCache::new_memory();
    let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>)
        .with_limits(limits);
    tokio::spawn(async move {
        svc.serve(l).await.expect("the accept loop serves");
    });
    addr
}

/// One raw exchange: request bytes in, the status line out.
async fn raw_exchange(addr: std::net::SocketAddr, req: &[u8]) -> String {
    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    sock.write_all(req).await.unwrap();
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.unwrap();
    String::from_utf8_lossy(&buf).into_owned()
}

/// The default cap rides the codec's own bound — the arithmetic is the
/// contract: every body the codec could decode drains; a body the codec
/// would refuse (a decompressed entry past `MAX_ENTRY_BYTES`) is refused
/// BEFORE the bytes land. The slack covers the envelope's non-body JSON
/// (status, headers, tags, etag) the codec's body cap does not count.
#[test]
fn the_default_limit_rides_the_codec_bound() {
    let d = ExchangeLimits::default();
    assert!(
        d.max_body >= MAX_ENTRY_BYTES + MAC_LEN,
        "the default never refuses a body the codec itself accepts"
    );
    assert_eq!(
        d.max_body,
        MAX_ENTRY_BYTES + MAC_LEN + 64 * 1024,
        "the documented arithmetic: the codec bound + the MAC + the envelope slack"
    );
}

/// The hostile shape the seam exists for: `Content-Length` past the cap
/// answers 413 BEFORE the drain — the response arrives while the client
/// has sent only headers, the bytes never land, the buffer never grows.
#[tokio::test]
async fn an_oversized_content_length_answers_413_without_draining() {
    let cap = ExchangeLimits::default().max_body;
    let addr = bounded_plane(ExchangeLimits::default()).await;
    // Headers ONLY: if the shell drained first, this exchange would hang
    // until the body arrived; the 413 arriving proves the refusal beats
    // the drain.
    let head = format!(
        "PUT /entry/route:/flood HTTP/1.1\r\nHost: t\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        cap + 16
    );
    let resp = raw_exchange(addr, head.as_bytes()).await;
    assert!(
        resp.starts_with("HTTP/1.1 413"),
        "the oversized exchange is refused: {resp}"
    );
    assert!(
        resp.contains("Content Too Large"),
        "the reason phrase names the refusal: {resp}"
    );
}

/// The default is byte-compatible with legit traffic: a normal sealed
/// entry PUTs and GETs through the bounded shell exactly as it did
/// pre-F64 — the cap refuses what the codec would refuse, nothing else.
#[tokio::test]
async fn a_legit_exchange_under_the_cap_still_serves() {
    let addr = bounded_plane(ExchangeLimits::default()).await;
    let entry = oxide_fw_cache::Entry {
        body: b"<html>bounded-v1</html>".to_vec(),
        status: 200,
        headers: vec![],
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
        etag: "\"bounded-v1\"".into(),
    };
    let sealed = oxide_fw_cache::seal_entry(&oxide_fw_cache::encode_entry(&entry).unwrap());
    let head = format!(
        "PUT /entry/route:/bounded HTTP/1.1\r\nHost: t\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        sealed.len()
    );
    let mut req = head.into_bytes();
    req.extend_from_slice(&sealed);
    let resp = raw_exchange(addr, &req).await;
    assert!(
        resp.starts_with("HTTP/1.1 204"),
        "the legit PUT serves under the default cap: {resp}"
    );
    let get = raw_exchange(
        addr,
        b"GET /entry/route:/bounded HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        get.starts_with("HTTP/1.1 200"),
        "the stored entry serves after the bounded PUT: {get}"
    );
}

/// The operator knob: `with_limits` carries a custom cap and `limits()`
/// pins the state (the F60 accessor shape) — a tightened cap refuses a
/// body the default would serve.
#[tokio::test]
async fn a_custom_cap_refuses_a_body_the_default_would_serve() {
    let fresh =
        HttpCacheService::new(TieredCache::new_memory() as Arc<dyn oxide_fw_cache::CacheHandler>);
    assert_eq!(
        fresh.limits(),
        ExchangeLimits::default(),
        "a fresh service rides the default bounds"
    );
    let tightened = fresh.with_limits(ExchangeLimits { max_body: 64 });
    assert_eq!(
        tightened.limits(),
        ExchangeLimits { max_body: 64 },
        "the accessor carries the attached bounds"
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        tightened.serve(l).await.expect("the accept loop serves");
    });
    let entry = oxide_fw_cache::Entry {
        body: vec![0u8; 256],
        status: 200,
        headers: vec![],
        tags: vec![],
        policy: oxide_fw_cache::Policy::default(),
        stored_at: 0,
        etag: String::new(),
    };
    let sealed = oxide_fw_cache::seal_entry(&oxide_fw_cache::encode_entry(&entry).unwrap());
    assert!(sealed.len() > 64, "the fixture exceeds the tightened cap");
    let head = format!(
        "PUT /entry/route:/tight HTTP/1.1\r\nHost: t\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        sealed.len()
    );
    let resp = raw_exchange(addr, head.as_bytes()).await;
    assert!(
        resp.starts_with("HTTP/1.1 413"),
        "the tightened cap refuses the sealed entry: {resp}"
    );
}

/// The escape hatch is explicit: `unbounded()` restores the pre-F64
/// drain for a host that fronts the plane with its own bound (a proxy
/// that caps bodies upstream) — the oversized body then reaches the
/// codec and answers the codec's 400, never the shell's 413.
#[tokio::test]
async fn unbounded_restores_the_pre_f64_drain() {
    assert_eq!(
        ExchangeLimits::unbounded().max_body,
        usize::MAX,
        "the escape hatch is the honest maximum"
    );
    let addr = bounded_plane(ExchangeLimits::unbounded()).await;
    // A 5 MiB garbage body: past the default cap, under the drain — the
    // unbounded shell drains it and the codec refuses it (400), the
    // pre-F64 shape verbatim.
    let body = vec![0u8; 5 * 1024 * 1024];
    let head = format!(
        "PUT /entry/route:/unbounded HTTP/1.1\r\nHost: t\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut req = head.into_bytes();
    req.extend_from_slice(&body);
    let resp = raw_exchange(addr, &req).await;
    assert!(
        resp.starts_with("HTTP/1.1 400"),
        "the garbage body drains to the codec's refusal, not the shell's 413: {resp}"
    );
}

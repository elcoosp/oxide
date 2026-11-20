//! TDD pins for the client's keep-alive connection reuse (V2-F70): the
//! §11.2 client stops paying a fresh dial per exchange.
//!
//! Since F53 the client opened ONE fresh socket per exchange (`Connection:
//! close`, `read_to_end`-until-close) and the F53 shell answered the same
//! posture — every exchange cost a TCP handshake, every answer cost the
//! close. The bounds arc fenced the exchange's TIME (F60), MEMORY
//! (F64/F65), NAMES (F66) and COUNT (F67/F68); the DIAL is the last
//! per-exchange tax, and the plane's answers always carry
//! `Content-Length`, so the body's exact length is knowable on the wire —
//! the socket can serve the next exchange instead of dying. F70 is the
//! both-halves seam: the client's `with_keep_alive(max_idle)` speaks
//! `Connection: keep-alive`, reads the Content-Length-driven body, and
//! parks the socket in a per-host idle pool bounded at `max_idle`; the
//! shell honors the request's keep-alive by looping the exchange read on
//! the same connection. The DEFAULT stays the pre-F70 close shape —
//! byte-identical for every existing deployment (the F60 posture).
//!
//! Written FIRST (red): the pins fail with E0599 over the missing
//! `with_keep_alive`/`keep_alive`.

use oxide_fw_cache::{CacheHandler, Entry, HttpCacheHandler, HttpCacheService, Policy, Result};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

/// A recording plane fixture: counts ACCEPTED CONNECTIONS, captures the
/// request's `Connection` header per exchange, and answers the cache
/// protocol's GET shape minimally (a 404 miss is a valid answer — the
/// pins measure dials and headers, not cache contents). `honor_keepalive`
/// flips the fixture's own posture: the close shape (the pre-F70 world)
/// or the honoring shape (the F70 shell).
struct RecordingPlane {
    connections: Arc<std::sync::atomic::AtomicUsize>,
    connection_headers: Arc<Mutex<Vec<String>>>,
    honor_keepalive: bool,
}

impl RecordingPlane {
    fn spawn(honor_keepalive: bool) -> (std::net::SocketAddr, Self) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let plane = Self {
            connections: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            connection_headers: Arc::new(Mutex::new(Vec::new())),
            honor_keepalive,
        };
        let headers = Arc::clone(&plane.connection_headers);
        let connections = Arc::clone(&plane.connections);
        let honor = plane.honor_keepalive;
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut s) = conn else { continue };
                connections.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                loop {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 1024];
                    let head_end = loop {
                        let n = s.read(&mut tmp).unwrap_or(0);
                        if n == 0 {
                            break None;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break Some(i);
                        }
                    };
                    let Some(head_end) = head_end else { break };
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                    let mut conn_header = String::new();
                    let mut clen = 0usize;
                    for line in head.lines().skip(1) {
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("connection:") {
                            conn_header = v.trim().to_string();
                        }
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                            clen = v.trim().parse().unwrap_or(0);
                        }
                    }
                    headers.lock().unwrap().push(conn_header.clone());
                    let mut body = buf[head_end + 4..].to_vec();
                    while body.len() < clen {
                        let n = s.read(&mut tmp).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        body.extend_from_slice(&tmp[..n]);
                    }
                    // A valid 404 miss (Content-Length always written, the
                    // plane's own shape).
                    let reusable = honor && conn_header.eq_ignore_ascii_case("keep-alive");
                    let resp_conn = if reusable { "keep-alive" } else { "close" };
                    let _ = s.write_all(
                        format!("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: {resp_conn}\r\n\r\n").as_bytes(),
                    );
                    if !reusable {
                        // This connection is done (the close IS the answer)
                        // — the ACCEPT LOOP lives on.
                        break;
                    }
                }
            }
        });
        (addr, plane)
    }

    fn connection_count(&self) -> usize {
        self.connections.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// The default is the pre-F70 shape — no pool — and the builder carries
/// the attached idle bound for the introspection seam (the F60
/// `deadline()` shape).
#[tokio::test]
async fn the_default_has_no_pool_and_the_builder_carries_the_bound() {
    let bare = HttpCacheHandler::new("http://127.0.0.1:1");
    assert_eq!(
        bare.keep_alive(),
        None,
        "the default shape is the pre-F70 close-per-exchange client"
    );
    let pooled = HttpCacheHandler::new("http://127.0.0.1:1").with_keep_alive(4);
    assert_eq!(
        pooled.keep_alive(),
        Some(4),
        "the accessor carries the attached idle bound"
    );
}

/// The default client still speaks close: two sequential exchanges open
/// TWO connections, each request's `Connection` header says `close` —
/// the pre-F70 shape byte-identical (the F60 posture).
#[tokio::test]
async fn the_default_client_still_speaks_close() {
    let (addr, plane) = RecordingPlane::spawn(true);
    let h = HttpCacheHandler::new(format!("http://{addr}"));
    h.get("route:/a").await.expect("exchange one answers");
    h.get("route:/b").await.expect("exchange two answers");
    assert_eq!(
        plane.connection_count(),
        2,
        "the close shape dials per exchange"
    );
    let headers = plane.connection_headers.lock().unwrap();
    assert!(
        headers.iter().all(|v| v.eq_ignore_ascii_case("close")),
        "every request carries Connection: close: {headers:?}"
    );
}

/// The keep-alive client reuses: two sequential exchanges ride ONE
/// connection, the requests say `Connection: keep-alive`, and both
/// answer correctly — the dial tax paid once.
#[tokio::test]
async fn keep_alive_reuses_one_connection_for_two_exchanges() {
    let (addr, plane) = RecordingPlane::spawn(true);
    let h = HttpCacheHandler::new(format!("http://{addr}")).with_keep_alive(2);
    h.get("route:/a").await.expect("exchange one answers");
    h.get("route:/b").await.expect("exchange two answers");
    assert_eq!(
        plane.connection_count(),
        1,
        "the second exchange rode the pooled socket"
    );
    let headers = plane.connection_headers.lock().unwrap();
    assert!(
        headers.iter().all(|v| v.eq_ignore_ascii_case("keep-alive")),
        "every request speaks keep-alive: {headers:?}"
    );
}

/// A pool bound of ZERO never parks a socket: the client still speaks
/// keep-alive on the wire but every socket closes after its exchange —
/// two exchanges, two connections (the honest "reuse off, wire on"
/// shape, the operator's kill switch).
#[tokio::test]
async fn a_zero_bound_never_parks_a_socket() {
    let (addr, plane) = RecordingPlane::spawn(true);
    let h = HttpCacheHandler::new(format!("http://{addr}")).with_keep_alive(0);
    h.get("route:/a").await.expect("exchange one answers");
    h.get("route:/b").await.expect("exchange two answers");
    assert_eq!(
        plane.connection_count(),
        2,
        "max_idle = 0 parks nothing: the dial-per-exchange shape remains"
    );
}

/// Honesty against a close-shape peer: a plane that refuses keep-alive
/// (answers `Connection: close`, the pre-F70 world) still serves every
/// exchange — the client reads the close, parks nothing, and dials
/// fresh for the next exchange (2 connections, 2 correct answers).
#[tokio::test]
async fn a_close_shape_peer_still_serves_the_keepalive_client() {
    let (addr, plane) = RecordingPlane::spawn(false);
    let h = HttpCacheHandler::new(format!("http://{addr}")).with_keep_alive(2);
    h.get("route:/a").await.expect("exchange one answers");
    h.get("route:/b").await.expect("exchange two answers");
    assert_eq!(
        plane.connection_count(),
        2,
        "the close-shape peer gets one dial per exchange — the client honors the answer"
    );
}

/// The shell honors keep-alive END TO END: the F53 service's socket loop
/// reads exchange after exchange on the honored connection (two raw
/// exchanges on one connection, two 200 answers), and a close-shaped
/// request still gets the pre-F70 byte shape (one answer, then EOF).
#[tokio::test]
async fn the_shell_honors_keepalive_and_keeps_the_close_shape() {
    let tier = oxide_fw_cache::TieredCache::new_memory();
    let svc = HttpCacheService::new(tier as Arc<dyn CacheHandler>);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { svc.serve(listener).await.expect("the accept loop serves") });

    // The raw-socket story is BLOCKING std I/O — it rides the blocking
    // pool: a std read (or a `join`) on the current-thread runtime would
    // stall the very task (the spawned service) that must answer (the
    // recorded fixture class).
    let story = tokio::task::spawn_blocking(move || {
        // The honored shape: one connection, two exchanges, both answered.
        let mut s = std::net::TcpStream::connect(addr).unwrap();
        let get = "GET /entry/missing HTTP/1.1\r\nHost: plane\r\nConnection: keep-alive\r\nContent-Length: 0\r\n\r\n";
        s.write_all(get.as_bytes()).unwrap();
        let mut buf = Vec::new();
        let read_answer = |s: &mut std::net::TcpStream, buf: &mut Vec<u8>| -> String {
            loop {
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..i]).to_string();
                    let mut clen = 0usize;
                    for line in head.lines().skip(1) {
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                            clen = v.trim().parse().unwrap_or(0);
                        }
                    }
                    while buf.len() < i + 4 + clen {
                        let mut t = [0u8; 1024];
                        let n = s.read(&mut t).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&t[..n]);
                    }
                    buf.drain(..i + 4 + clen);
                    return head;
                }
                let mut t = [0u8; 1024];
                let n = s.read(&mut t).unwrap_or(0);
                if n == 0 {
                    return String::new();
                }
                buf.extend_from_slice(&t[..n]);
            }
        };
        let first = read_answer(&mut s, &mut buf);
        assert!(
            first.starts_with("HTTP/1.1 404"),
            "the miss answers: {first}"
        );
        assert!(
            first
                .to_ascii_lowercase()
                .contains("connection: keep-alive"),
            "the honored answer speaks keep-alive: {first}"
        );
        s.write_all(get.as_bytes()).unwrap();
        let second = read_answer(&mut s, &mut buf);
        assert!(
            second.starts_with("HTTP/1.1 404"),
            "the second exchange rode the SAME connection: {second}"
        );
        drop(s);

        // The close shape: one answer, then EOF (the pre-F70 byte shape).
        let mut s2 = std::net::TcpStream::connect(addr).unwrap();
        let close_get = "GET /entry/missing HTTP/1.1\r\nHost: plane\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";
        s2.write_all(close_get.as_bytes()).unwrap();
        let mut buf2 = Vec::new();
        let head = read_answer(&mut s2, &mut buf2);
        assert!(head.starts_with("HTTP/1.1 404"));
        assert!(
            head.to_ascii_lowercase().contains("connection: close"),
            "the close shape answers close: {head}"
        );
        let mut tail = [0u8; 16];
        let n = s2.read(&mut tail).unwrap_or(0);
        assert_eq!(n, 0, "the close-shaped connection ends after its answer");
    });
    story.await.expect("the raw-socket story joins");
}

/// The deadline rides the reused exchange too: a keep-alive client with
/// a deadline attached degrades LOUD inside the clock when the peer
/// hangs (the F60 teeth on the F70 socket — the pool never suspends the
/// contract).
#[tokio::test]
async fn the_deadline_bounds_the_keepalive_exchange() {
    // The parked plane: the F68 fixture's shape — a real service whose
    // backing handler parks on a gate the test holds shut.
    struct GateHandler {
        gate: Arc<tokio::sync::Mutex<()>>,
    }
    #[async_trait::async_trait]
    impl CacheHandler for GateHandler {
        async fn get(&self, _key: &str) -> Result<Option<Entry>> {
            let _held = self.gate.lock().await;
            Ok(None)
        }
        async fn set(&self, _key: &str, _entry: Entry) -> Result<()> {
            let _held = self.gate.lock().await;
            Ok(())
        }
        async fn delete(&self, _key: &str) -> Result<()> {
            Ok(())
        }
        async fn tag_keys(&self, _tag: &str) -> Result<Vec<String>> {
            Ok(Vec::new())
        }
        async fn invalidate_tag(&self, _tag: &str, _soft: bool) -> Result<usize> {
            Ok(0)
        }
        async fn publish_invalidation(&self, _tag: &str, _soft: bool) -> Result<()> {
            Ok(())
        }
    }
    let gate = Arc::new(tokio::sync::Mutex::<()>::new(()));
    let handler = Arc::new(GateHandler {
        gate: Arc::clone(&gate),
    });
    let svc = HttpCacheService::new(handler as Arc<dyn CacheHandler>);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { svc.serve(listener).await.expect("the accept loop serves") });

    let h = HttpCacheHandler::new(format!("http://{addr}"))
        .with_keep_alive(2)
        .with_timeout(Duration::from_millis(250));
    let guard = gate.lock().await;
    let out = h
        .set(
            "route:/hung",
            Entry {
                body: b"<html>hung</html>".to_vec(),
                status: 200,
                headers: vec![],
                tags: vec![],
                policy: Policy::default(),
                stored_at: 0,
                etag: String::new(),
            },
        )
        .await;
    drop(guard);
    let err = out.expect_err("the hung exchange degrades LOUD inside the deadline");
    assert!(
        err.0.message.contains("deadline"),
        "the degradation names the arm that fired: {}",
        err.0.message
    );
}

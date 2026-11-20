//! Consumer walkthrough for the V2-F70 client's keep-alive connection
//! reuse: the §11.2 client stops paying a fresh dial per exchange.
//!
//! Since F53 every client exchange opened ONE fresh socket (`Connection:
//! close`) and the F53 shell answered the same posture — every exchange
//! cost the TCP handshake. The plane's answers always carry
//! `Content-Length`, so the body's exact end is knowable on the wire;
//! F70 is the both-halves seam: the shell HONORS a request's
//! `Connection: keep-alive` (the next exchange rides the same
//! connection), and the client's `with_keep_alive(max_idle)` speaks it,
//! reads the Content-Length-driven body, and parks the socket in a
//! per-host idle pool bounded at `max_idle`. The default stays the
//! pre-F70 close shape — byte-identical, the F60 posture.
//!
//! Shows the story end to end:
//!
//! 1. Counting planes (the fixture shape) answer the cache protocol and
//!    count ACCEPTED CONNECTIONS.
//! 2. The default client: two exchanges, TWO connections, every request
//!    speaking `Connection: close` (the pre-F70 shape, byte-identical).
//! 3. The keep-alive client: two exchanges, ONE connection — the dial
//!    tax paid once.
//! 4. The bounded pool: `max_idle = 0` parks nothing (the kill switch),
//!    and a close-shape peer still serves every exchange honestly.
//! 5. The real plane rides the same story: a pooled client round-trips
//!    a real entry through its pooled socket.
//! 6. The transport arm stays loud: a dead dial is a named error, never
//!    a hang.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example client_keep_alive
//! ```

use oxide_fw_cache::{CacheHandler, Entry, HttpCacheHandler, HttpCacheService, TieredCache};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

/// A counting plane: one thread per connection, the accept count shared.
/// The `honor` flag flips the plane's own posture (the pre-F70 close
/// shape vs the F70 honored shape) — the client must follow the ANSWER,
/// not its wish.
struct CountingPlane {
    connections: Arc<std::sync::atomic::AtomicUsize>,
}

impl CountingPlane {
    fn spawn(honor: bool) -> (std::net::SocketAddr, Self) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let plane = Self {
            connections: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        let connections = Arc::clone(&plane.connections);
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut s) = conn else { continue };
                connections.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                while let Some(requested) = read_exchange(&mut s) {
                    let reusable = honor && requested.eq_ignore_ascii_case("keep-alive");
                    let resp_conn = if reusable { "keep-alive" } else { "close" };
                    let _ = s.write_all(
                        format!("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: {resp_conn}\r\n\r\n")
                            .as_bytes(),
                    );
                    if !reusable {
                        break;
                    }
                }
            }
        });
        (addr, plane)
    }

    fn connections(&self) -> usize {
        self.connections.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Read one raw exchange (head + Content-Length body) and return the
/// request's Connection header value; `None` when the peer went away.
fn read_exchange(s: &mut std::net::TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let head_end = loop {
        let n = s.read(&mut tmp).unwrap_or(0);
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut requested = String::new();
    let mut clen = 0usize;
    for line in head.lines().skip(1) {
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("connection:") {
            requested = v.trim().to_string();
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            clen = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < clen {
        let n = s.read(&mut tmp).unwrap_or(0);
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    Some(requested)
}

/// The fixture story: the default close shape, the pool's ride, the
/// kill switch, and the close-shape honesty — each measured by the
/// plane's own accept count.
async fn fixture_story() {
    let (addr, plane) = CountingPlane::spawn(true);
    let bare = HttpCacheHandler::new(format!("http://{addr}"));
    bare.get("route:/a").await.expect("exchange one answers");
    bare.get("route:/b").await.expect("exchange two answers");
    println!(
        "== the default client: 2 exchanges over {} connections (the pre-F70 close shape)",
        plane.connections()
    );

    let (addr, plane) = CountingPlane::spawn(true);
    let pooled = HttpCacheHandler::new(format!("http://{addr}")).with_keep_alive(4);
    pooled.get("route:/a").await.expect("exchange one answers");
    pooled.get("route:/b").await.expect("exchange two answers");
    println!(
        "== the keep-alive client: the same 2 exchanges over {} connection (the pool rode)",
        plane.connections()
    );

    let (addr, plane) = CountingPlane::spawn(true);
    let tight = HttpCacheHandler::new(format!("http://{addr}")).with_keep_alive(0);
    tight.get("route:/a").await.expect("exchange one answers");
    tight.get("route:/b").await.expect("exchange two answers");
    println!(
        "== max_idle = 0: the kill switch — 2 exchanges over {} connections (nothing parked)",
        plane.connections()
    );

    let (addr, plane) = CountingPlane::spawn(false);
    let hopeful = HttpCacheHandler::new(format!("http://{addr}")).with_keep_alive(4);
    hopeful.get("route:/a").await.expect("exchange one answers");
    hopeful.get("route:/b").await.expect("exchange two answers");
    println!(
        "== a close-shape peer: 2 exchanges over {} connections (the client honored every answer)",
        plane.connections()
    );
}

/// The real-plane story: the F53 shell honors keep-alive, so a pooled
/// client round-trips a real entry; a dead dial stays loud.
async fn real_plane_story() {
    let tier = TieredCache::new_memory();
    let svc = HttpCacheService::new(tier as Arc<dyn CacheHandler>);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { svc.serve(listener).await.expect("the accept loop serves") });
    let client = HttpCacheHandler::new(format!("http://{addr}")).with_keep_alive(4);
    client
        .set(
            "route:/real",
            Entry {
                body: b"<html>one socket, many exchanges</html>".to_vec(),
                status: 200,
                headers: vec![],
                tags: vec![],
                policy: oxide_fw_cache::Policy {
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
                etag: "\"keepalive-v1\"".into(),
            },
        )
        .await
        .expect("the set stores");
    let got = client
        .get("route:/real")
        .await
        .expect("the get answers")
        .expect("the entry hits");
    println!(
        "== the real plane: set + get through the pooled socket ({:?})",
        String::from_utf8_lossy(&got.body)
    );

    let parked = HttpCacheHandler::new("http://127.0.0.1:1").with_keep_alive(2);
    match parked
        .set(
            "route:/hung",
            Entry {
                body: b"hung".to_vec(),
                status: 200,
                headers: vec![],
                tags: vec![],
                policy: oxide_fw_cache::Policy::default(),
                stored_at: 0,
                etag: String::new(),
            },
        )
        .await
    {
        Ok(()) => println!("== UNREACHABLE: a refused dial answered"),
        Err(e) => println!("== a dead dial stays loud: [{}] {}", e.0.code, e.0.message),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        fixture_story().await;
        real_plane_story().await;
    });
    Ok(())
}

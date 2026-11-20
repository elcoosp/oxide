//! TDD pins for the HTTP client's deadline arm (V2-F60): the §11.2
//! "a remote cache is an optimization" contract gains its teeth.
//!
//! The `HttpCacheHandler` client degrades every transport error to a
//! miss/no-op (`get` → `Ok(None)`, the trait's §11.2 posture), but the
//! degradation only ever fired on a REFUSED connection — a peer that
//! HANGS (a node behind a firewall dropping SYNs, a wedged service, a
//! half-open socket) held the render path forever: `TcpStream::connect`
//! and the `read_to_end` had no deadline, so one dead peer stalled every
//! request it touched. The deadline arm bounds the WHOLE exchange
//! (connect through read) to the operator's duration; an exchange that
//! outlives it degrades through the file's existing arms — `get` to a
//! miss, `set` to a loud `Err` — and the local tier keeps serving, the
//! same posture the crate's every remote driver documents.
//!
//! Written FIRST (red): E0599 over the unresolved `with_timeout`
//! builder (and the `deadline` accessor the builder's state pin rides).

use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheHandler, HttpCacheService, Policy, TieredCache,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn entry() -> Entry {
    Entry {
        body: b"<html>deadline-v1</html>".to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: vec!["deadline-tag".to_string()],
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
        etag: "\"deadline-v1\"".into(),
    }
}

/// A peer that accepts and NEVER answers: the dropped-SYN's stand-in —
/// the exact shape a refused connection does not cover and the §11.2
/// contract must bound. The socket holds every exchange open until the
/// test drops the listener.
async fn hang_peer() -> std::net::SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            // Accept and hold: the exchange never completes.
            if let Ok((_s, _)) = l.accept().await {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        }
    });
    addr
}

/// The builder's state: a fresh client is UNBOUNDED (the pre-F60 shape —
/// every existing deployment keeps its byte behavior), and
/// `with_timeout` carries the operator's duration.
#[test]
fn the_deadline_state_rides_the_builder() {
    let bare = HttpCacheHandler::new("http://127.0.0.1:1");
    assert_eq!(bare.deadline(), None, "the default shape is unbounded");
    let bounded =
        HttpCacheHandler::new("http://127.0.0.1:1").with_timeout(Duration::from_millis(250));
    assert_eq!(
        bounded.deadline(),
        Some(Duration::from_millis(250)),
        "the builder carries the operator's duration"
    );
}

/// The contract's teeth: a hung peer degrades `get` to a MISS within the
/// deadline — not a hang, not an error (the local tier keeps serving,
/// §11.2). The elapsed bound is generous (the deadline plus scheduler
/// slack) but the pin's real assertion is the miss itself: the
/// pre-F60 shape never returns here.
#[tokio::test]
async fn hung_peer_degrades_a_get_to_a_miss_within_the_deadline() {
    let addr = hang_peer().await;
    let client =
        HttpCacheHandler::new(format!("http://{addr}")).with_timeout(Duration::from_millis(250));
    let started = Instant::now();
    let outcome = client
        .get("route:/hung")
        .await
        .expect("the degrade is a miss, not an error");
    let elapsed = started.elapsed();
    assert!(outcome.is_none(), "the hung peer degrades to a miss");
    assert!(
        elapsed < Duration::from_secs(5),
        "the miss lands within the deadline (took {elapsed:?})"
    );
}

/// The loud half: a hung peer FAILS a `set` within the deadline — a
/// write the plane never acknowledged is an error the caller sees (the
/// file's `set` arm is loud; the deadline rides it), never a hang.
#[tokio::test]
async fn hung_peer_fails_a_set_within_the_deadline() {
    let addr = hang_peer().await;
    let client =
        HttpCacheHandler::new(format!("http://{addr}")).with_timeout(Duration::from_millis(250));
    let started = Instant::now();
    let outcome = client.set("route:/hung", entry()).await;
    let elapsed = started.elapsed();
    assert!(outcome.is_err(), "the unacknowledged set is loud");
    assert_eq!(
        outcome.unwrap_err().0.code,
        "CACHE-HTTP",
        "the deadline failure carries the file's stable code"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "the failure lands within the deadline (took {elapsed:?})"
    );
}

/// A healthy exchange under a deadline is untouched: the bound clips
/// dead peers, not live traffic — set/get round-trip through a real
/// plane service with a 2-second deadline attached.
#[tokio::test]
async fn a_healthy_exchange_round_trips_under_the_deadline() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let tier = TieredCache::new_memory();
    let service = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>);
    tokio::spawn(async move {
        let _ = service.serve(l).await;
    });
    let client =
        HttpCacheHandler::new(format!("http://{addr}")).with_timeout(Duration::from_secs(2));
    client
        .set("route:/live", entry())
        .await
        .expect("the set lands under the deadline");
    let got = client
        .get("route:/live")
        .await
        .expect("the get answers under the deadline");
    assert_eq!(
        got.expect("the entry is resident").body,
        b"<html>deadline-v1</html>".to_vec(),
        "the deadline never clips a healthy exchange"
    );
}

/// A refused connection degrades to a miss exactly as before — the
/// deadline arm composes with the file's existing arms, it does not
/// replace them.
#[tokio::test]
async fn refused_connect_degrades_to_a_miss_under_the_deadline() {
    // Bind and drop: the port is closed by the time the client dials.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    let client =
        HttpCacheHandler::new(format!("http://{addr}")).with_timeout(Duration::from_secs(2));
    let outcome = client.get("route:/refused").await;
    assert!(
        outcome.is_ok() && outcome.unwrap().is_none(),
        "the refused dial is a miss"
    );
}

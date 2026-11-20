//! TDD pins for the client's read bound (V2-F65): the §11.2
//! `HttpCacheHandler` learns the bound its deadline arm implies.
//!
//! F64 bounded the SERVICE half of the wire — the shell refuses an
//! oversized `Content-Length` before the first body byte lands. The
//! CLIENT half stayed unbounded on the same exchange: `exchange` drained
//! the peer's answer with `read_to_end` and scanned for the header
//! terminator over everything it had buffered — a hostile or misrouted
//! peer (the F63 record's own captive-portal shape) answering an endless
//! or gigantic body grew the client's buffer without bound. The F60
//! deadline bounds the exchange's TIME; nothing bounded its MEMORY. The
//! read bound is the seam: `with_read_limit` attaches the cap, the read
//! loop refuses past it LOUD under the file's stable `CACHE-HTTP` code,
//! and `read_limit()` is the introspection accessor (the F60 `deadline()`
//! shape). The default stays the pre-F65 unbounded shape — byte-identical
//! for every existing deployment (the F60 posture for the client half);
//! the operator opts in, the choice is greppable.
//!
//! Written FIRST (red): the pins fail with E0599 over the missing
//! `with_read_limit`/`read_limit` (and, past the compile wall, the
//! over-cap pins fail behaviorally — the pre-F65 drain answers the
//! decode/no-terminator errors instead of the cap's refusal).

use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheHandler, HttpCacheService, PlaneHealth, PlanePeer, Policy,
    TieredCache,
};
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
fn entry() -> Entry {
    Entry {
        body: b"<html>read-cap-v1</html>".to_vec(),
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
        etag: "\"read-cap-v1\"".into(),
    }
}

/// A full plane on a real socket (the F63 fixture shape): the tier rooted
/// as the backing store AND the health provider (the cap pin probes the
/// health arm), nothing else attached.
async fn plain_plane() -> std::net::SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let tier = TieredCache::new_memory();
    let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>)
        .with_health(Arc::clone(&tier) as Arc<dyn PlaneHealth>);
    tokio::spawn(async move {
        svc.serve(l).await.expect("the accept loop serves");
    });
    addr
}

/// The accessor pins the state: a fresh client rides the pre-F65
/// unbounded shape (`None`), `with_read_limit` attaches the cap and
/// `read_limit()` carries it back — the F60 `deadline()` introspection
/// shape, the seam tests and hosts pin instead of guessing.
#[tokio::test]
async fn the_accessor_pins_the_read_bound_state() {
    let fresh = HttpCacheHandler::new("http://127.0.0.1:1");
    assert_eq!(
        fresh.read_limit(),
        None,
        "a fresh client rides the pre-F65 unbounded read"
    );
    let capped = fresh.with_read_limit(64 * 1024);
    assert_eq!(
        capped.read_limit(),
        Some(64 * 1024),
        "the attached cap rides the accessor"
    );
}

/// The hostile shape the seam exists for: a peer answers 200 with a body
/// past the cap — the bounded client refuses the exchange LOUD (the
/// stable code, the cap named) instead of draining it into memory. The
/// probes carry the same refusal (the F63 loud contract); the entry
/// `get` degrades to a miss like every transport-class failure (§11.2),
/// the cap failure included.
#[tokio::test]
async fn an_answer_past_the_cap_is_refused_loudly() {
    // A raw peer that answers a normal-looking 200 with a 2 MiB body —
    // past the 64 KiB cap, under the pre-F65 drain.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = l.accept() {
            let mut buf = [0u8; 1024];
            let _ = Read::read(&mut s, &mut buf);
            let body = vec![b'a'; 2 * 1024 * 1024];
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = Write::write_all(&mut s, head.as_bytes());
            let _ = Write::write_all(&mut s, &body);
        }
    });
    let cap = 64 * 1024;
    let client = HttpCacheHandler::new(format!("http://{addr}")).with_read_limit(cap);

    // The probe is LOUD: the error names the stable code and the cap.
    let err = client
        .peer_health()
        .await
        .expect_err("an over-cap answer is a loud refusal");
    assert_eq!(err.0.code, "CACHE-HTTP", "the file's stable code rides");
    assert!(
        err.0.message.contains("read limit"),
        "the error names the bound the exchange refused: {err:?}"
    );
    assert!(
        err.0.message.contains(&cap.to_string()),
        "the error carries the cap's value: {err:?}"
    );

    // The entry verbs keep their §11.2 contracts: `get` degrades to a
    // miss (the local tier keeps serving), `set` is a loud Err.
    let got = client.get("route:/over-cap").await.unwrap();
    assert!(
        got.is_none(),
        "the over-cap answer degrades to a miss, never a hit"
    );
    let set = client.set("route:/over-cap", entry()).await;
    let set_err = set.expect_err("the set's over-cap answer is a loud Err");
    assert_eq!(set_err.0.code, "CACHE-HTTP", "the set's refusal is coded");
}

/// The hostile shape with NO terminator: a peer that streams headers
/// forever and never completes the block. The pre-F65 drain accumulated
/// everything it read and only THEN failed with "no header terminator" —
/// the bounded client refuses at the cap and the error says so, the
/// buffer never grows past it.
#[tokio::test]
async fn a_terminatorless_flood_stops_at_the_cap() {
    // Headers, then 2 MiB of continuation bytes, never the blank line —
    // the pre-F65 client read ALL of it before the parse error.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = l.accept() {
            let mut buf = [0u8; 1024];
            let _ = Read::read(&mut s, &mut buf);
            let _ = Write::write_all(&mut s, b"HTTP/1.1 200 OK\r\nX-Flood: ");
            let _ = Write::write_all(&mut s, &vec![b'A'; 2 * 1024 * 1024]);
        }
    });
    let cap = 64 * 1024;
    let client = HttpCacheHandler::new(format!("http://{addr}")).with_read_limit(cap);
    let started = Instant::now();
    let err = client
        .peer_metrics()
        .await
        .expect_err("a terminatorless flood is refused at the cap");
    assert_eq!(err.0.code, "CACHE-HTTP", "the refusal is coded");
    assert!(
        err.0.message.contains("read limit"),
        "the cap's refusal names the bound, not the parser's after-the-fact \
         terminator error: {err:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the refusal stops at the cap instead of draining the flood (took {:?})",
        started.elapsed()
    );
}

/// The default is byte-compatible with legit traffic: a bounded client
/// with a generous cap answers a normal exchange exactly as the
/// unbounded client does — the bound changes no answer it should not.
#[tokio::test]
async fn a_bounded_client_answers_legit_traffic_unchanged() {
    let addr = plain_plane().await;
    let unbounded = HttpCacheHandler::new(format!("http://{addr}"));
    let capped = HttpCacheHandler::new(format!("http://{addr}")).with_read_limit(4 * 1024 * 1024);

    unbounded
        .set("route:/cap-legit", entry())
        .await
        .expect("the set lands through the plane");
    let from_unbounded = unbounded
        .get("route:/cap-legit")
        .await
        .unwrap()
        .expect("the unbounded client hits");
    let from_capped = capped
        .get("route:/cap-legit")
        .await
        .unwrap()
        .expect("the bounded client hits the same entry");
    assert_eq!(
        from_unbounded.body, from_capped.body,
        "the bound changes no legit answer"
    );
}

/// The deadline and the read cap compose: the cap failure answers with
/// the cap's message (the bound that fired), inside the deadline — the
/// two budgets ride one exchange, each naming its own arm.
#[tokio::test]
async fn the_deadline_and_the_cap_compose() {
    // The over-cap 200 peer again: the cap fires long before any
    // deadline could.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = l.accept() {
            let mut buf = [0u8; 1024];
            let _ = Read::read(&mut s, &mut buf);
            let body = vec![b'a'; 1024 * 1024];
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = Write::write_all(&mut s, head.as_bytes());
            let _ = Write::write_all(&mut s, &body);
        }
    });
    let client = HttpCacheHandler::new(format!("http://{addr}"))
        .with_timeout(Duration::from_secs(30))
        .with_read_limit(64 * 1024);
    let err = client.peer_health().await.expect_err("the cap fires first");
    assert!(
        err.0.message.contains("read limit"),
        "the cap's arm names itself, not the deadline: {err:?}"
    );
    assert!(
        !err.0.message.contains("deadline"),
        "the deadline never fired — the cap answered: {err:?}"
    );
}

/// The cap rides the trait object: a host that composes the client
/// behind `Arc<dyn PlanePeer>` (the F63 shape) keeps the bound — the
/// seam is the client's, not the call site's.
#[tokio::test]
async fn the_cap_rides_the_peer_trait_object() {
    let addr = plain_plane().await;
    let peer: Arc<dyn PlanePeer> =
        Arc::new(HttpCacheHandler::new(format!("http://{addr}")).with_read_limit(1024 * 1024));
    peer.peer_health()
        .await
        .expect("the capped trait object probes the plane");
}

/// A wire byte counter: the guard pins prove the refusal happens BEFORE
/// the exchange (no bytes leave the client) for M2's seam; this fixture
/// is shared shape, kept here for the cap's own before-the-drain proof.
#[tokio::test]
async fn the_refusal_costs_no_wire_roundtrip() {
    let hits = Arc::new(AtomicUsize::new(0));
    let hits2 = Arc::clone(&hits);
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for conn in l.incoming() {
            hits2.fetch_add(1, Ordering::SeqCst);
            drop(conn);
        }
    });
    let client = HttpCacheHandler::new(format!("http://{addr}")).with_read_limit(64);
    // The cap never fires on an EMPTY answer — the refusal is the
    // over-cap arm's, not a connection-count artifact. The exchange
    // itself still dials (the transport connects, the answer is short).
    let got = client.get("route:/short").await.unwrap();
    assert!(got.is_none(), "a short answer is a plain miss");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the capped client still speaks the wire for a legit answer"
    );
}

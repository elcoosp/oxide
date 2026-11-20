//! TDD pins for the client's request-line trust boundary (V2-F66): the
//! §11.2 `HttpCacheHandler` refuses to speak a key or tag the request
//! line cannot carry.
//!
//! The client builds its URLs by interpolation — `format!(
//! "{base}/entry/{key}")` — and the wire has no escaping: the service's
//! own `route` contract is "everything after the family prefix IS the
//! name" (the client writes keys and tags verbatim). A name carrying a
//! CR, LF, space or other ASCII control byte did not fail — it CORRUPTED
//! the exchange: a space split the request line (the plane stored the
//! key's head, dropped the tail), a CRLF injected a second exchange into
//! the same connection stream. Keys are app-facing (`route:` keys embed
//! request paths; a slug is user input), so the boundary is real. The
//! guard is the seam: every verb checks the name BEFORE a single wire
//! byte leaves and fails LOUD under the file's stable `CACHE-HTTP` code
//! (a silent miss would hide the caller bug; a corrupted exchange would
//! poison the plane), and the public `plane_name` predicate pins the
//! table so hosts that compose names can pre-validate.
//!
//! Written FIRST (red): the pins fail with E0599 over the missing
//! `plane_name` and behaviorally — the pre-F66 verbs speak the corrupt
//! exchange (the space key `get` answered a silent miss, the set answered
//! the plane's 404 with a generic status error instead of the guard).

use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheHandler, HttpCacheService, Policy, TieredCache,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

fn entry() -> Entry {
    Entry {
        body: b"<html>name-guard-v1</html>".to_vec(),
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
        etag: "\"name-guard-v1\"".into(),
    }
}

/// A counting plane: every dial is one wire byte-leak. The guard's whole
/// point is the refusal happening BEFORE the exchange — the bad-name
/// pins count ZERO dials.
fn counting_plane() -> (std::net::SocketAddr, Arc<AtomicUsize>) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let dials = Arc::new(AtomicUsize::new(0));
    let dials2 = Arc::clone(&dials);
    std::thread::spawn(move || {
        for conn in l.incoming() {
            dials2.fetch_add(1, Ordering::SeqCst);
            drop(conn);
        }
    });
    (addr, dials)
}

/// A CRLF key never reaches the wire: `set`, `get` and `delete` all
/// refuse LOUD under the stable code, and the counting plane answers
/// ZERO dials — the corrupt exchange is never spoken, not merely
/// misrouted.
#[tokio::test]
async fn a_crlf_key_is_refused_before_any_wire_byte() {
    let (addr, dials) = counting_plane();
    let client = HttpCacheHandler::new(format!("http://{addr}"));
    let key = "route:/x\r\nX-Injected: 1";

    let set = client.set(key, entry()).await;
    let err = set.expect_err("a CRLF key is a loud refusal, not a corrupt exchange");
    assert_eq!(err.0.code, "CACHE-HTTP", "the file's stable code rides");

    let got = client.get(key).await;
    let err = got.expect_err("the get is loud too — never a silent miss");
    assert_eq!(err.0.code, "CACHE-HTTP", "the get's refusal is coded");

    let deleted = client.delete(key).await;
    deleted.expect_err("the delete refuses the same boundary");
    assert_eq!(
        dials.load(Ordering::SeqCst),
        0,
        "the refusals cost zero dials — nothing left the client"
    );
}

/// A space splits the request line the same way (`split_whitespace`):
/// the plane would store the key's head and drop the tail — the guard
/// refuses the name before the exchange.
#[tokio::test]
async fn a_space_key_is_refused_before_any_wire_byte() {
    let (addr, dials) = counting_plane();
    let client = HttpCacheHandler::new(format!("http://{addr}"));
    let key = "route:/a b";
    let set = client.set(key, entry()).await;
    let err = set.expect_err("a space key would split the request line");
    assert_eq!(err.0.code, "CACHE-HTTP", "the refusal is coded");
    let got = client.get(key).await;
    got.expect_err("the get refuses too — never the pre-F66 silent miss");
    assert_eq!(dials.load(Ordering::SeqCst), 0, "zero dials");
}

/// The guard's error names the offending byte — a caller debugging a
/// poisoned key sees WHICH byte broke the boundary, not a bare refusal.
#[tokio::test]
async fn the_guard_names_the_offending_byte() {
    let err = HttpCacheHandler::plane_name("bad\nname").expect_err("the LF is refused");
    assert!(
        err.0.message.contains("0x0a"),
        "the error names the byte value: {err:?}"
    );
    let err = HttpCacheHandler::plane_name("bad name").expect_err("the space is refused");
    assert!(
        err.0.message.contains("0x20"),
        "the space is named too: {err:?}"
    );
}

/// The tag verbs ride the same boundary: `tag_keys` and `invalidate_tag`
/// refuse a bad tag loudly, and `publish_invalidation` keeps its
/// documented best-effort contract (`Ok(())`, the gap loud in the warn)
/// with ZERO dials — the corrupt publish is never spoken.
#[tokio::test]
async fn a_bad_tag_refuses_the_tag_verbs() {
    let (addr, dials) = counting_plane();
    let client = HttpCacheHandler::new(format!("http://{addr}"));
    let tag = "products\r\ncache-poison";

    let keys = client.tag_keys(tag).await;
    let err = keys.expect_err("the tag listing refuses a bad tag");
    assert_eq!(err.0.code, "CACHE-HTTP", "the refusal is coded");

    let invalidated = client.invalidate_tag(tag, true).await;
    invalidated.expect_err("the invalidate refuses the same boundary");

    let published = client.publish_invalidation(tag, true).await;
    published.expect("the publish keeps its best-effort Ok contract");
    assert_eq!(
        dials.load(Ordering::SeqCst),
        0,
        "the tag verbs' refusals cost zero dials"
    );
}

/// The public predicate pins the table: everything the request line
/// speaks verbatim passes — route keys, query punctuation, percent
/// escapes, UTF-8 multi-byte (a continuation byte is never an ASCII
/// control); every byte that breaks or injects into the line fails.
#[test]
fn the_plane_name_predicate_pins_the_table() {
    for ok in [
        "route:/blog/1",
        "cache:user:42:v2",
        "route:/_blog/über?x=1&y=2",
        "%2Fencoded-slash",
        "k#hash",
    ] {
        assert!(
            HttpCacheHandler::plane_name(ok).is_ok(),
            "the request line speaks {ok:?} verbatim"
        );
    }
    for bad in ["a\rb", "a\nb", "a b", "a\tb", "a\u{1f}b", "a\u{7f}b"] {
        assert!(
            HttpCacheHandler::plane_name(bad).is_err(),
            "the request line cannot speak {bad:?}"
        );
    }
}

/// The guard does not over-refuse: a UTF-8 + punctuation key round-trips
/// through a REAL plane byte-identical — the multi-byte sequences never
/// contain an ASCII control byte, so the wire speaks them unchanged.
#[tokio::test]
async fn utf8_and_punctuation_names_roundtrip_over_the_wire() {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let tier = TieredCache::new_memory();
    let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>);
    tokio::spawn(async move {
        svc.serve(l).await.expect("the accept loop serves");
    });
    let client = HttpCacheHandler::new(format!("http://{addr}"));
    let key = "route:/_blog/über?x=1&y=2";
    client
        .set(key, entry())
        .await
        .expect("the guarded client still speaks the full name table");
    let got = client
        .get(key)
        .await
        .unwrap()
        .expect("the UTF-8 key round-trips verbatim");
    assert_eq!(got.body, b"<html>name-guard-v1</html>");
}

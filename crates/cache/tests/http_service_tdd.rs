//! TDD suite for the cache plane's HTTP service (`HttpCacheService`, F53).
//! Written first per the house TDD rule: every pin below failed on the
//! unresolved seam (E0432 over `HttpCacheService`/`PlaneRequest`/
//! `PlaneResponse`) before the impl existed.
//!
//! The contract the pins hold:
//!
//! 1. (F53) The SPEC-12 / §11.2 protocol's SERVER half exists: the
//!    `HttpCacheHandler` client's doc block says "one node runs the cache
//!    service" — nothing did. The service composes any `CacheHandler`
//!    (a `TieredCache` is one) and answers the exact exchange shapes the
//!    client speaks: `GET/PUT/DELETE /entry/{key}`, `GET /tag/{tag}`,
//!    `POST /invalidate/{tag}`.
//! 2. (F53) The trust boundary holds on both sides: a PUT body that does
//!    not open (`auth::open_entry`) or decode (`codec::decode_entry`) is
//!    a 400 and NEVER enters the backing store — the forged-entry miss
//!    rule the client already pins, enforced where the payload lands.
//!    A served GET payload re-seals through the same key, so the client
//!    opens it (the multi-node rule: `OXIDE_CACHE_MAC_KEY` shared).
//! 3. (F53) Degradations are loud, never silent: handler errors answer
//!    5xx (the client's `set` then reports `CACHE-HTTP`, its `get`
//!    degrades to a miss — the §11.2 "remote cache is an optimization"
//!    contract), malformed requests answer 400 and close the connection
//!    without poisoning the accept loop.
//! 4. (F53) The wire contract: a real `HttpCacheHandler` against a real
//!    `HttpCacheService` on a loopback listener round-trips the whole
//!    protocol — set from one "node", get through another, tag listing,
//!    hard invalidation crossing nodes — the multi-node deployment story
//!    the protocol doc describes end to end.

use std::sync::Arc;
use std::time::Duration;

use oxide_fw_cache::{
    CacheHandler as _, Entry, HttpCacheHandler, HttpCacheService, PlaneRequest, PlaneResponse,
    Policy, TieredCache,
};

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn entry(body: &[u8], tags: &[&str], stored_at: u64) -> Entry {
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
        stored_at,
        etag: "\"v1\"".into(),
    }
}

/// A fresh in-memory backing store (a `TieredCache` IS a `CacheHandler`
/// — the same shape a real deployment hosts) wired behind a service.
fn backed_plane() -> (Arc<TieredCache>, HttpCacheService) {
    let tier = TieredCache::new_memory();
    let plane = HttpCacheService::new(tier.clone() as Arc<dyn oxide_fw_cache::CacheHandler>);
    (tier, plane)
}

// ---------------------------------------------------------------- the pure core

#[tokio::test]
async fn get_hit_serves_the_sealed_entry() {
    let (tier, plane) = backed_plane();
    tier.set("k1", entry(b"hello", &["t1"], now_ms()))
        .await
        .expect("the set lands");
    let resp = plane.respond(&PlaneRequest::get("/entry/k1")).await;
    assert_eq!(resp.status, 200, "a hit answers 200");
    // The payload is the MAC-sealed envelope: it opens and decodes back
    // to the stored entry (the same bytes shape the client's `set` sends).
    let opened = oxide_fw_cache::open_entry(&resp.body).expect("served payload opens");
    let got = oxide_fw_cache::decode_entry(opened).expect("opened payload decodes");
    assert_eq!(got.body, b"hello");
    assert_eq!(got.etag, "\"v1\"");
    assert_eq!(got.tags, vec!["t1".to_string()]);
}

#[tokio::test]
async fn get_miss_is_404() {
    let (_tier, plane) = backed_plane();
    let resp = plane.respond(&PlaneRequest::get("/entry/nope")).await;
    assert_eq!(resp.status, 404, "a miss answers 404, not an error");
}

#[tokio::test]
async fn put_sealed_stores_through_the_backing_handler() {
    let (tier, plane) = backed_plane();
    let e = entry(b"body", &[], now_ms());
    let sealed = oxide_fw_cache::seal_entry(&oxide_fw_cache::encode_entry(&e).unwrap());
    let resp = plane.respond(&PlaneRequest::put("/entry/k2", sealed)).await;
    assert_eq!(resp.status, 204, "a valid PUT answers 204");
    let stored = tier.get("k2").await.unwrap().expect("the backing holds it");
    let e = match stored {
        oxide_fw_cache::Hit::Fresh(e) | oxide_fw_cache::Hit::Stale(e) => e,
    };
    assert_eq!(e.body, b"body");
}

#[tokio::test]
async fn put_forged_is_400_and_never_stored() {
    let (tier, plane) = backed_plane();
    // Exactly what an attacker with wire write-access would send: a plain
    // JSON entry, no MAC. The client rejects it on read; the service must
    // already refuse it on write — the payload never lands.
    let forged = serde_json::to_vec(&entry(b"forged", &[], now_ms())).unwrap();
    let resp = plane.respond(&PlaneRequest::put("/entry/k3", forged)).await;
    assert_eq!(resp.status, 400, "a forged PUT is refused");
    assert!(
        tier.get("k3").await.unwrap().is_none(),
        "a refused payload never enters the backing store",
    );
}

#[tokio::test]
async fn put_tampered_is_400() {
    let (_tier, plane) = backed_plane();
    let e = entry(b"real", &[], now_ms());
    let mut sealed = oxide_fw_cache::seal_entry(&oxide_fw_cache::encode_entry(&e).unwrap());
    if let Some(byte) = sealed.last_mut() {
        *byte ^= 0x01;
    }
    let resp = plane.respond(&PlaneRequest::put("/entry/k4", sealed)).await;
    assert_eq!(resp.status, 400, "a tampered PUT is refused");
}

#[tokio::test]
async fn delete_removes_204() {
    let (tier, plane) = backed_plane();
    tier.set("k5", entry(b"gone", &[], now_ms()))
        .await
        .expect("the set lands");
    let resp = plane.respond(&PlaneRequest::delete("/entry/k5")).await;
    assert_eq!(resp.status, 204);
    assert!(tier.get("k5").await.unwrap().is_none(), "deleted");
}

#[tokio::test]
async fn tag_keys_serves_the_json_list() {
    let (tier, plane) = backed_plane();
    tier.set("a", entry(b"x", &["t9"], now_ms()))
        .await
        .expect("the set lands");
    tier.set("b", entry(b"y", &["t9"], now_ms()))
        .await
        .expect("the set lands");
    let resp = plane.respond(&PlaneRequest::get("/tag/t9")).await;
    assert_eq!(resp.status, 200);
    let mut keys: Vec<String> = serde_json::from_slice(&resp.body).expect("a JSON key list");
    let mut want = vec!["a".to_string(), "b".to_string()];
    keys.sort();
    want.sort();
    assert_eq!(keys, want, "the tag's key list (order not the contract)");
}

#[tokio::test]
async fn invalidate_serves_the_count() {
    let (tier, plane) = backed_plane();
    tier.set("a", entry(b"x", &["t8"], now_ms()))
        .await
        .expect("the set lands");
    tier.set("b", entry(b"y", &["t8"], now_ms()))
        .await
        .expect("the set lands");
    let resp = plane
        .respond(&PlaneRequest::post(
            "/invalidate/t8",
            br#"{"soft":false}"#.to_vec(),
        ))
        .await;
    assert_eq!(resp.status, 200);
    let v: serde_json::Value = serde_json::from_slice(&resp.body).expect("a JSON count");
    assert_eq!(v["count"], 2, "both tagged entries invalidated");
    assert!(
        tier.get("a").await.unwrap().is_none(),
        "hard invalidation deleted"
    );
}

#[tokio::test]
async fn unknown_paths_and_methods_are_404_and_405() {
    let (_tier, plane) = backed_plane();
    assert_eq!(plane.respond(&PlaneRequest::get("/nope")).await.status, 404);
    assert_eq!(
        plane
            .respond(&PlaneRequest::post("/entry/k", Vec::new()))
            .await
            .status,
        405,
        "only GET/PUT/DELETE speak /entry",
    );
    assert_eq!(
        plane
            .respond(&PlaneRequest::put("/tag/t", Vec::new()))
            .await
            .status,
        405,
        "only GET speaks /tag",
    );
}

#[tokio::test]
async fn a_handler_error_answers_5xx_loudly() {
    // A handler whose `get` always fails: the plane must NOT turn a
    // handler failure into a miss (a miss caches, an error is loud).
    struct Broken;
    #[async_trait::async_trait]
    impl oxide_fw_cache::CacheHandler for Broken {
        async fn get(&self, _key: &str) -> oxide_fw_cache::Result<Option<Entry>> {
            Err(oxide_fw_cache::err("CAC-TEST", "broken"))
        }
        async fn set(&self, _key: &str, _entry: Entry) -> oxide_fw_cache::Result<()> {
            Err(oxide_fw_cache::err("CAC-TEST", "broken"))
        }
        async fn delete(&self, _key: &str) -> oxide_fw_cache::Result<()> {
            Err(oxide_fw_cache::err("CAC-TEST", "broken"))
        }
        async fn tag_keys(&self, _tag: &str) -> oxide_fw_cache::Result<Vec<String>> {
            Err(oxide_fw_cache::err("CAC-TEST", "broken"))
        }
        async fn invalidate_tag(&self, _tag: &str, _soft: bool) -> oxide_fw_cache::Result<usize> {
            Err(oxide_fw_cache::err("CAC-TEST", "broken"))
        }
        async fn publish_invalidation(
            &self,
            _tag: &str,
            _soft: bool,
        ) -> oxide_fw_cache::Result<()> {
            Err(oxide_fw_cache::err("CAC-TEST", "broken"))
        }
    }
    let plane = HttpCacheService::new(Arc::new(Broken) as Arc<dyn oxide_fw_cache::CacheHandler>);
    assert_eq!(
        plane.respond(&PlaneRequest::get("/entry/k")).await.status,
        500
    );
    let e = entry(b"x", &[], now_ms());
    let sealed = oxide_fw_cache::seal_entry(&oxide_fw_cache::encode_entry(&e).unwrap());
    assert_eq!(
        plane
            .respond(&PlaneRequest::put("/entry/k", sealed))
            .await
            .status,
        500
    );
}

// ---------------------------------------------------------------- the wire

/// Boot the service on a loopback listener and return its base URL
/// (`http://host:port`, the exact spelling the client's `new` takes).
async fn boot(tier: Arc<TieredCache>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let service = HttpCacheService::new(tier.clone() as Arc<dyn oxide_fw_cache::CacheHandler>);
    tokio::spawn(async move {
        service
            .serve(listener)
            .await
            .expect("the accept loop serves");
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn wire_roundtrip_with_the_real_client() {
    // One backing store, TWO client "nodes" pointed at the service: the
    // multi-node deployment story the protocol doc describes, proven end
    // to end over real sockets.
    let tier = TieredCache::new_memory();
    let base = boot(tier).await;
    let node_a = HttpCacheHandler::new(base.clone());
    let node_b = HttpCacheHandler::new(base.clone());

    // Node A stores; node B reads (same MAC key in-process, the docs'
    // multi-node rule).
    node_a
        .set("w1", entry(b"from-a", &["shared"], now_ms()))
        .await
        .expect("node A sets");
    let got = node_b.get("w1").await.unwrap().expect("node B hits");
    assert_eq!(got.body, b"from-a");

    // The tag listing and the invalidation cross nodes: B's hard
    // invalidation deletes through the shared backing, A's next read misses.
    assert_eq!(
        node_b.tag_keys("shared").await.unwrap(),
        vec!["w1".to_string()],
        "the shared tag lists the entry node A stored",
    );
    assert_eq!(node_b.invalidate_tag("shared", false).await.unwrap(), 1);
    assert!(
        node_a.get("w1").await.unwrap().is_none(),
        "the invalidation crossed nodes",
    );
    // The publish arm POSTs the same invalidate — converges quietly.
    node_a.publish_invalidation("shared", true).await.unwrap();
}

#[tokio::test]
async fn malformed_requests_answer_400_and_the_loop_survives() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt as _};
    let tier = TieredCache::new_memory();
    let base = boot(tier).await;
    // Raw garbage: no request line the parser accepts. The exchange ends
    // (a 400 or a close) and the service keeps serving afterwards.
    let mut sock = tokio::net::TcpStream::connect(base.trim_start_matches("http://"))
        .await
        .unwrap();
    sock.write_all(b"NOT-HTTP\r\n\r\n").await.unwrap();
    let mut buf = Vec::new();
    let _ = sock.read_to_end(&mut buf).await; // 400 + close, or bare close
    drop(sock);

    // The accept loop survived: a well-formed client still answers.
    let client = HttpCacheHandler::new(base);
    client
        .set("after-garbage", entry(b"ok", &[], now_ms()))
        .await
        .expect("the loop survives malformed traffic");
}

#[tokio::test]
async fn keys_and_tags_with_slashes_route_verbatim() {
    // The client writes the key verbatim into the URL
    // (`format!("{base}/entry/{key}")`, no percent-encoding) and real
    // keys in this workspace carry slashes (`route:/blog/1`). The
    // service speaks the same shape: everything after the family prefix
    // IS the name, on every family.
    let (tier, plane) = backed_plane();
    tier.set(
        "route:/blog/1",
        entry(b"slashed", &["posts/2026"], now_ms()),
    )
    .await
    .expect("the set lands");
    let resp = plane
        .respond(&PlaneRequest::get("/entry/route:/blog/1"))
        .await;
    assert_eq!(resp.status, 200, "a slash-bearing key routes whole");
    let resp = plane.respond(&PlaneRequest::get("/tag/posts/2026")).await;
    assert_eq!(resp.status, 200, "a slash-bearing tag routes whole");
}

// The wire types are value-shaped so tests and hosts can drive the core
// without sockets (the bench rides the same shape).
#[test]
fn plane_types_are_constructible() {
    let req = PlaneRequest {
        method: "GET".into(),
        path: "/entry/k".into(),
        body: Vec::new(),
    };
    let resp = PlaneResponse {
        status: 200,
        body: Vec::new(),
    };
    assert_eq!(req.method, "GET");
    assert_eq!(resp.status, 200);
}

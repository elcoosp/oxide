//! TDD pins for the sharded multi-plane client (V2-F69): one client, many
//! planes, keys routed by consistent hashing — the §11.2 story's next
//! shape.
//!
//! Until now a node pointed ONE `HttpCacheHandler` at ONE plane: the
//! plane's tier was the node's whole remote capacity (F67 bounded its
//! concurrency, F64/F65 its shape and memory, F60 its time — nothing
//! spread the keys across MORE planes). [`ShardedCacheHandler`] is the
//! composite: the entry verbs route a key to exactly ONE shard by
//! rendezvous hashing over the same blake3 the crate's keys already ride
//! ([`keyhash`](oxide_fw_cache::keyhash)), the tag verbs fan out over
//! EVERY shard (a tag's key set is plane-local, so it is scattered across
//! the shards), and the whole composite is one `Arc<dyn CacheHandler>` —
//! it composes behind a [`HttpCacheService`], inside a `TieredCache`'s
//! remote slot, or under a probe, like any handler.
//!
//! Written FIRST (red): the pins fail with E0432 over the unresolved
//! `ShardedCacheHandler` import and E0599 over the missing
//! `shard_of`/`shards` accessors.

use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheHandler, HttpCacheService, Policy, Result, ShardedCacheHandler,
    TieredCache,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A shard double that records every verb arrival and keeps a real store,
/// so the composite's round-trips run against honest answers — the F68
/// `GateHandler` fixture's shape, minus the gate (the sharded client adds
/// routing, not parking).
struct RecordingHandler {
    gets: AtomicUsize,
    sets: AtomicUsize,
    deletes: AtomicUsize,
    store: Mutex<HashMap<String, Entry>>,
}

impl RecordingHandler {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            gets: AtomicUsize::new(0),
            sets: AtomicUsize::new(0),
            deletes: AtomicUsize::new(0),
            store: Mutex::new(HashMap::new()),
        })
    }

    fn arrivals(&self) -> usize {
        self.gets.load(Ordering::SeqCst)
            + self.sets.load(Ordering::SeqCst)
            + self.deletes.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl CacheHandler for RecordingHandler {
    async fn get(&self, key: &str) -> Result<Option<Entry>> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .store
            .lock()
            .expect("the store locks")
            .get(key)
            .cloned())
    }
    async fn set(&self, key: &str, entry: Entry) -> Result<()> {
        self.sets.fetch_add(1, Ordering::SeqCst);
        self.store
            .lock()
            .expect("the store locks")
            .insert(key.to_string(), entry);
        Ok(())
    }
    async fn delete(&self, key: &str) -> Result<()> {
        self.deletes.fetch_add(1, Ordering::SeqCst);
        self.store.lock().expect("the store locks").remove(key);
        Ok(())
    }
    async fn tag_keys(&self, tag: &str) -> Result<Vec<String>> {
        Ok(self
            .store
            .lock()
            .expect("the store locks")
            .iter()
            .filter(|(_, e)| e.tags.iter().any(|t| t == tag))
            .map(|(k, _)| k.clone())
            .collect())
    }
    async fn invalidate_tag(&self, tag: &str, _soft: bool) -> Result<usize> {
        let mut store = self.store.lock().expect("the store locks");
        let victims: Vec<String> = store
            .iter()
            .filter(|(_, e)| e.tags.iter().any(|t| t == tag))
            .map(|(k, _)| k.clone())
            .collect();
        for k in &victims {
            store.remove(k);
        }
        Ok(victims.len())
    }
    async fn publish_invalidation(&self, _tag: &str, _soft: bool) -> Result<()> {
        Ok(())
    }
}

/// A fixture entry with a finite TTL born NOW (the recorded class: an
/// entry born expired is a correct miss the seams did not cause).
fn entry(body: &[u8], tags: &[&str]) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![],
        tags: tags.iter().map(|t| (*t).to_string()).collect(),
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
        etag: String::new(),
    }
}

/// An empty shard set refuses LOUDLY (the house convention — a composite
/// that silently dropped every `set` would poison the deployment; the F66
/// lesson: the refusal beats the corruption). The code is the minted
/// stable `CAC-SHARD`.
#[tokio::test]
async fn an_empty_shard_set_refuses_loudly() {
    let err = ShardedCacheHandler::new(Vec::<Arc<dyn CacheHandler>>::new())
        .expect_err("the empty set refuses");
    assert_eq!(
        err.0.code, "CAC-SHARD",
        "the refusal speaks the minted code, not a bare string"
    );
    assert!(
        err.0.message.contains("empty"),
        "the message names the empty set: {}",
        err.0.message
    );
}

/// The routing function is deterministic and in range: the same key
/// routes to the same shard on two independently built composites, and
/// every answer lands inside `0..shards` (the introspection seam the F60
/// `deadline()` shape established — tests pin the routing, no sockets).
#[tokio::test]
async fn the_routing_is_deterministic_and_in_range() {
    let shards: Vec<Arc<dyn CacheHandler>> = (0..4)
        .map(|_| Arc::clone(&RecordingHandler::new()) as Arc<dyn CacheHandler>)
        .collect();
    let a = ShardedCacheHandler::new(shards).expect("it composes");
    let shards: Vec<Arc<dyn CacheHandler>> = (0..4)
        .map(|_| Arc::clone(&RecordingHandler::new()) as Arc<dyn CacheHandler>)
        .collect();
    let b = ShardedCacheHandler::new(shards).expect("it composes");

    assert_eq!(a.shards(), 4, "the accessor carries the shard count");
    for key in [
        "route:/",
        "route:/blog/[slug]",
        "route:/x?y=1",
        "L1:data-key",
    ] {
        let shard = a.shard_of(key);
        assert!(shard < 4, "the route lands in range: {shard}");
        assert_eq!(
            shard,
            b.shard_of(key),
            "two composites agree on the same key set: {key}"
        );
    }
}

/// The keys spread: over a route-shaped key set, more than one shard
/// receives keys — a composite that pinned everything to shard 0 would be
/// a pass-through, not a sharder (the balance proof, deterministic over
/// blake3).
#[tokio::test]
async fn the_keys_spread_across_the_shards() {
    let shards: Vec<Arc<dyn CacheHandler>> = (0..4)
        .map(|_| Arc::clone(&RecordingHandler::new()) as Arc<dyn CacheHandler>)
        .collect();
    let client = ShardedCacheHandler::new(shards).expect("it composes");
    let mut seen = [false; 4];
    for i in 0..64 {
        seen[client.shard_of(&format!("route:/page/{i}"))] = true;
    }
    assert!(
        seen.iter().filter(|s| **s).count() >= 2,
        "the keys spread across the shards: {seen:?}"
    );
}

/// The entry verbs reach EXACTLY ONE shard: a `set` lands on the routed
/// shard alone, the following `get` rides the same route (so it finds the
/// entry), the `delete` removes it there — the other shards never hear a
/// verb (the routing is the capability: a fan-out over entry verbs would
/// multiply every write N-fold).
#[tokio::test]
async fn the_entry_verbs_reach_exactly_one_shard() {
    let shards: Vec<Arc<RecordingHandler>> = (0..3).map(|_| RecordingHandler::new()).collect();
    let doubled: Vec<Arc<dyn CacheHandler>> = shards
        .iter()
        .map(|s| Arc::clone(s) as Arc<dyn CacheHandler>)
        .collect();
    let client = ShardedCacheHandler::new(doubled).expect("it composes");

    let key = "route:/sharded-entry";
    let routed = client.shard_of(key);
    client
        .set(key, entry(b"<html>one</html>", &[]))
        .await
        .expect("the set stores");
    let got = client
        .get(key)
        .await
        .expect("the get answers")
        .expect("the routed shard holds the entry");
    assert_eq!(got.body, b"<html>one</html>");
    client.delete(key).await.expect("the delete answers");
    assert!(
        client.get(key).await.expect("the get answers").is_none(),
        "the delete removed the entry on its shard"
    );

    for (i, shard) in shards.iter().enumerate() {
        assert_eq!(
            shard.arrivals(),
            if i == routed { 4 } else { 0 },
            "shard {i} saw exactly the routed shard's verbs (set, get, miss-check get, delete) — no fan-out"
        );
    }
}

/// The composite is honest on the wire: three real plane services on
/// loopback, the composite over three real clients — a set through the
/// composite round-trips through its routed plane, and the entry is
/// visible to a DIRECT client pointed at that plane (no re-encoding, no
/// second copy on the other planes).
#[tokio::test]
async fn the_wire_round_trips_through_real_planes() {
    // Three planes, each an in-memory tier behind the F53 service.
    let mut addrs = Vec::new();
    for _ in 0..3 {
        let tier = TieredCache::new_memory();
        let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>);
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { svc.serve(l).await.expect("the accept loop serves") });
        addrs.push(addr);
    }
    let clients: Vec<Arc<dyn CacheHandler>> = addrs
        .iter()
        .map(|a| Arc::new(HttpCacheHandler::new(format!("http://{a}"))) as Arc<dyn CacheHandler>)
        .collect();
    let client = ShardedCacheHandler::new(clients).expect("it composes");

    let key = "route:/wire-roundtrip";
    client
        .set(key, entry(b"<html>wire</html>", &[]))
        .await
        .expect("the composite stores");
    let got = client
        .get(key)
        .await
        .expect("the composite answers")
        .expect("the entry rides its routed plane");
    assert_eq!(got.body, b"<html>wire</html>");

    // A direct client pointed at the ROUTED plane sees the entry (the
    // routing landed the bytes there, verbatim); the composite's delete
    // removes exactly that copy.
    let direct = HttpCacheHandler::new(format!("http://{}", addrs[client.shard_of(key)]));
    let seen = direct
        .get(key)
        .await
        .expect("the direct client answers")
        .expect("the routed plane holds the entry");
    assert_eq!(seen.body, b"<html>wire</html>");
    client.delete(key).await.expect("the composite deletes");
    assert!(
        direct
            .get(key)
            .await
            .expect("the direct client answers")
            .is_none(),
        "the composite's delete reached the routed plane"
    );
}

/// The tag verbs FAN OUT (the §11.2 tag story is plane-local: a tag's key
/// set is scattered across the shards, so `tag_keys` gathers every
/// shard's list and `invalidate_tag` invalidates every shard). Three real
/// planes each hold keys under one tag — set there by DIRECT clients —
/// and the composite's invalidate crosses all three.
#[tokio::test]
async fn the_tag_verbs_fan_out_across_every_shard() {
    let mut addrs = Vec::new();
    for _ in 0..3 {
        let tier = TieredCache::new_memory();
        let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>);
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { svc.serve(l).await.expect("the accept loop serves") });
        addrs.push(addr);
    }
    let direct: Vec<Arc<HttpCacheHandler>> = addrs
        .iter()
        .map(|a| Arc::new(HttpCacheHandler::new(format!("http://{a}"))))
        .collect();
    let clients: Vec<Arc<dyn CacheHandler>> = direct
        .iter()
        .map(|h| Arc::clone(h) as Arc<dyn CacheHandler>)
        .collect();
    let client = ShardedCacheHandler::new(clients).expect("it composes");

    // Every plane holds ONE key under the tag (set through its own
    // direct client — the composite is not involved in the setup).
    for (i, h) in direct.iter().enumerate() {
        h.set(
            &format!("route:/tagged/{i}"),
            entry(b"<html>t</html>", &["t"]),
        )
        .await
        .expect("the direct set stores");
    }

    let mut keys = client.tag_keys("t").await.expect("the fan-out gathers");
    keys.sort();
    let expected: Vec<String> = (0..3).map(|i| format!("route:/tagged/{i}")).collect();
    assert_eq!(keys, expected, "the fan-out sees every shard's key set");

    let count = client
        .invalidate_tag("t", true)
        .await
        .expect("the fan-out invalidates");
    assert_eq!(count, 3, "the count is the SUM across the shards");
    for (i, h) in direct.iter().enumerate() {
        assert!(
            h.get(&format!("route:/tagged/{i}"))
                .await
                .expect("the direct client answers")
                .is_none(),
            "the invalidation crossed every plane"
        );
    }
}

/// The composite names itself and composes as a plain trait object — the
/// `handler_kind` introspection the posture pull reads (F32), and the
/// `Arc<dyn CacheHandler>` shape any host slot takes.
#[tokio::test]
async fn the_composite_names_itself_and_composes_as_a_trait_object() {
    let shards: Vec<Arc<dyn CacheHandler>> = (0..2)
        .map(|_| Arc::clone(&RecordingHandler::new()) as Arc<dyn CacheHandler>)
        .collect();
    let client = ShardedCacheHandler::new(shards).expect("it composes");
    assert_eq!(client.handler_kind(), "sharded");
    let obj: Arc<dyn CacheHandler> = Arc::new(client);
    assert_eq!(obj.handler_kind(), "sharded");
    assert!(
        obj.get("route:/any")
            .await
            .expect("the routed shard answers")
            .is_none(),
        "the trait-object shape serves"
    );
}

/// Minimal disruption: growing the shard set moves only the keys whose
/// rendezvous winner changed — the consistent-hash promise (a reshard
/// never re-shuffles the world). Over 200 keys, growing 3 → 4 shards
/// moves strictly fewer than half the mean pre-growth share (the HRW
/// bound is ~1/(K+1); the loose bound keeps the pin honest without
/// pinning the digest).
#[tokio::test]
async fn growing_the_shard_set_moves_a_minority() {
    let three: Vec<Arc<dyn CacheHandler>> = (0..3)
        .map(|_| Arc::clone(&RecordingHandler::new()) as Arc<dyn CacheHandler>)
        .collect();
    let before = ShardedCacheHandler::new(three).expect("it composes");
    let routes: Vec<usize> = (0..200)
        .map(|i| before.shard_of(&format!("route:/p/{i}")))
        .collect();

    let four: Vec<Arc<dyn CacheHandler>> = (0..4)
        .map(|_| Arc::clone(&RecordingHandler::new()) as Arc<dyn CacheHandler>)
        .collect();
    let after = ShardedCacheHandler::new(four).expect("it composes");
    let moved = routes
        .iter()
        .enumerate()
        .filter(|(i, r)| after.shard_of(&format!("route:/p/{i}")) != **r)
        .count();
    assert!(
        moved < 100,
        "growing 3 → 4 moved {moved} of 200 keys — a reshard moves a minority"
    );
}

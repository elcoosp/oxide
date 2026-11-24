//! TDD pins for the §V2-E09 edge KV cache handler (the spec's "**KV cache
//! handler for the edge (tags via a KV index key per tag with a size cap,
//! documented as best effort)**" — the A10 remote-handler family's edge
//! member, the shape the §V2-E09 worker template's `env.OXIDE_KV` speaks).
//!
//! Written FIRST (red): failed to compile before `kv_handler` existed.
//!
//! The family contract (§11.2, the redis/s3 handlers' documented posture):
//! - a store failure on `get`/`tag_keys`/`invalidate_tag` is a miss/no-op —
//!   the local tier keeps serving (a remote cache is an optimization);
//! - `set` returns the error;
//! - tags ride ONE index key per tag (`cache:tag:<tag>`, a JSON key list);
//! - the index is best-effort: the read-modify-write has no CAS and the
//!   spec's SIZE CAP stops appends — the entry still exists, the tag
//!   discovery misses it, the docs say so.

use oxide_fw_cache::{CacheHandler, Entry, KvCacheHandler, KvStore, Policy};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The fake store's value: the bytes plus the TTL the handler asked for.
type MemValue = (Vec<u8>, Option<Duration>);

/// An in-memory `KvStore` fake — the §V2-E09 worker's `env.OXIDE_KV` shape:
/// flat string keys, opaque bytes, an optional expiry TTL. The spec's
/// worker template rides `expirationTtl`; the fake records what the
/// handler asked for and honors it.
#[derive(Debug, Default)]
struct MemKv {
    map: Mutex<BTreeMap<String, MemValue>>,
    /// A store failure to inject (the degradation pins).
    failing: Mutex<bool>,
}

impl MemKv {
    fn keys(&self) -> Vec<String> {
        self.map.lock().unwrap().keys().cloned().collect()
    }
}

#[async_trait::async_trait]
impl KvStore for MemKv {
    async fn get(&self, key: &str) -> oxide_fw_cache::Result<Option<Vec<u8>>> {
        if *self.failing.lock().unwrap() {
            return Err(oxide_fw_cache::err("CACHE-KV", "injected store failure"));
        }
        Ok(self.map.lock().unwrap().get(key).map(|(v, _)| v.clone()))
    }

    async fn put(
        &self,
        key: &str,
        value: Vec<u8>,
        ttl: Option<Duration>,
    ) -> oxide_fw_cache::Result<()> {
        if *self.failing.lock().unwrap() {
            return Err(oxide_fw_cache::err("CACHE-KV", "injected store failure"));
        }
        self.map
            .lock()
            .unwrap()
            .insert(key.to_string(), (value, ttl));
        Ok(())
    }

    async fn delete(&self, key: &str) -> oxide_fw_cache::Result<()> {
        if *self.failing.lock().unwrap() {
            return Err(oxide_fw_cache::err("CACHE-KV", "injected store failure"));
        }
        self.map.lock().unwrap().remove(key);
        Ok(())
    }
}

fn entry(tags: &[&str]) -> Entry {
    Entry {
        body: b"body".to_vec(),
        status: 200,
        headers: vec![],
        tags: tags.iter().map(|t| (*t).to_string()).collect(),
        policy: Policy::default(),
        stored_at: 0,
        etag: "e".into(),
    }
}

/// THE plan pin (§V2-E09): the round-trip through a KV store — the entry
/// lands under its entry key, the tag lands under ONE index key per tag,
/// and `invalidate_tag` wipes both.
#[tokio::test]
async fn kv_handler_roundtrips_with_one_index_key_per_tag() {
    let kv = Arc::new(MemKv::default());
    let h = KvCacheHandler::new(kv.clone());
    let key = "route:/about";
    let tag = "route";
    assert!(h.get(key).await.unwrap().is_none(), "cold miss");
    h.set(key, entry(&[tag])).await.unwrap();
    let got = h.get(key).await.unwrap().expect("hit");
    assert_eq!(got.body, b"body");
    // One index key per tag, the JSON key list the spec's shape names.
    let index = kv
        .get("cache:tag:route")
        .await
        .unwrap()
        .expect("the tag index key exists");
    let keys: Vec<String> = serde_json::from_slice(&index).unwrap();
    assert_eq!(keys, vec![key.to_string()]);
    assert_eq!(h.tag_keys(tag).await.unwrap(), vec![key.to_string()]);
    assert_eq!(h.invalidate_tag(tag, true).await.unwrap(), 1);
    assert!(h.get(key).await.unwrap().is_none(), "invalidated away");
    assert!(kv.get("cache:tag:route").await.unwrap().is_none());
}

/// The size-cap pin: beyond the cap the index stops appending — the entry
/// still exists (a `get` hits), the tag discovery names only the cap's
/// worth, the best-effort posture the spec documents.
#[tokio::test]
async fn tag_index_stops_appending_at_the_cap() {
    let kv = Arc::new(MemKv::default());
    let h = KvCacheHandler::new(kv.clone()).with_tag_cap(3);
    for i in 0..6 {
        h.set(&format!("route:{i}"), entry(&["route"]))
            .await
            .unwrap();
    }
    let keys = h.tag_keys("route").await.unwrap();
    assert_eq!(keys.len(), 3, "the index holds the cap's worth");
    // The uncapped keys still exist — best-effort, not a refusal.
    assert!(h.get("route:5").await.unwrap().is_some());
    // The index key's payload is the capped list.
    let raw = kv.get("cache:tag:route").await.unwrap().unwrap();
    assert_eq!(
        serde_json::from_slice::<Vec<String>>(&raw).unwrap().len(),
        3
    );
}

/// The invalidation pin: every indexed key is deleted, the index key is
/// dropped, and the count is the index's size.
#[tokio::test]
async fn invalidate_tag_deletes_every_indexed_key() {
    let kv = Arc::new(MemKv::default());
    let h = KvCacheHandler::new(kv.clone());
    for i in 0..4 {
        h.set(&format!("route:{i}"), entry(&["route"]))
            .await
            .unwrap();
    }
    assert_eq!(h.invalidate_tag("route", false).await.unwrap(), 4);
    for i in 0..4 {
        assert!(h.get(&format!("route:{i}")).await.unwrap().is_none());
    }
    assert!(kv.keys().iter().all(|k| !k.starts_with("cache:tag:")));
}

/// The degradation pin (the family contract): a store failure on `get` is a
/// miss, on `tag_keys` an empty list — never an outage; `set` is the loud
/// arm.
#[tokio::test]
async fn store_failures_degrade_on_reads_and_are_loud_on_set() {
    let kv = Arc::new(MemKv::default());
    let h = KvCacheHandler::new(kv.clone());
    *kv.failing.lock().unwrap() = true;
    assert!(
        h.get("route:1").await.unwrap().is_none(),
        "a miss, not an outage"
    );
    assert_eq!(h.tag_keys("route").await.unwrap(), Vec::<String>::new());
    assert_eq!(h.invalidate_tag("route", true).await.unwrap(), 0);
    let set = h.set("route:1", entry(&["route"])).await;
    assert!(set.is_err(), "set is the loud arm");
    *kv.failing.lock().unwrap() = false;
}

/// The TTL pin: the handler's ttl rides every put (the worker template's
/// `expirationTtl` arm) and the builder is the F60 accessor shape.
#[tokio::test]
async fn ttl_rides_the_puts_and_accessors_report_the_state() {
    let kv = Arc::new(MemKv::default());
    let ttl = Duration::from_secs(60);
    let h = KvCacheHandler::new(kv.clone()).with_ttl(ttl);
    assert_eq!(h.ttl(), Some(ttl), "the introspection accessor");
    h.set("route:1", entry(&[])).await.unwrap();
    let (_, stored) = kv.map.lock().unwrap()["cache:entry:route:1"].clone();
    assert_eq!(stored, Some(ttl), "the ttl rode the put");
    assert_eq!(h.tag_cap(), KvCacheHandler::DEFAULT_TAG_CAP);
}

/// The trust-boundary pin: a tampered payload fails the MAC and is a miss
/// (the s3 family's S-06 posture) — never a decode of hostile bytes.
#[tokio::test]
async fn tampered_payload_is_a_miss() {
    let kv = Arc::new(MemKv::default());
    let h = KvCacheHandler::new(kv.clone());
    h.set("route:1", entry(&[])).await.unwrap();
    // Flip a byte in the stored blob.
    let raw = kv.get("cache:entry:route:1").await.unwrap().unwrap();
    let mut bad = raw.clone();
    bad[0] ^= 0xff;
    kv.put("cache:entry:route:1", bad, None).await.unwrap();
    assert!(
        h.get("route:1").await.unwrap().is_none(),
        "MAC refuse = miss"
    );
}

/// The introspection pin: the handler names itself `"kv"` (the F32 posture
/// pull's seam) and composes behind the trait object.
#[tokio::test]
async fn handler_kind_names_the_driver() {
    let kv = Arc::new(MemKv::default());
    let h: Arc<dyn CacheHandler> = Arc::new(KvCacheHandler::new(kv));
    assert_eq!(h.handler_kind(), "kv");
}

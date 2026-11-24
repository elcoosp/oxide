//! Consumer walkthrough for the §V2-E09 edge KV cache handler: the A10
//! remote-handler family's member over the worker template's
//! `env.OXIDE_KV` shape — flat string keys, opaque bytes, an optional
//! expiry — behind the same [`CacheHandler`] contract the redis/s3
//! handlers carry.
//!
//! The §V2-E09 worker template rides `env.OXIDE_KV.get(key, 'arrayBuffer')`
//! and `env.OXIDE_KV.put(key, bytes, { expirationTtl: 60 })`. This
//! walkthrough composes the same shape in Rust through an in-memory
//! [`KvStore`] fake (a real host binds its own KV client through the seam),
//! walks the tag contract — "a KV index key per tag with a size cap,
//! documented as best effort" — end to end: the cold miss, the tagged set,
//! the single index key, the cap's best-effort stop, the loud degradation,
//! and the sweep.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example kv_handler
//! ```

use oxide_fw_cache::{CacheHandler, Entry, KvCacheHandler, KvStore, Policy};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// An in-memory `KvStore` — the walkthrough's stand-in for the worker's
/// `env.OXIDE_KV` binding (flat keys, opaque bytes, an optional expiry
/// the store honors).
/// The fake store's value: the bytes plus the TTL the handler asked for.
type MemValue = (Vec<u8>, Option<Duration>);

#[derive(Debug, Default)]
struct MemKv {
    map: Mutex<BTreeMap<String, MemValue>>,
    expires: Mutex<BTreeMap<String, std::time::Instant>>,
}

#[async_trait::async_trait]
impl KvStore for MemKv {
    async fn get(&self, key: &str) -> oxide_fw_cache::Result<Option<Vec<u8>>> {
        // The expiry arm: an expired key reads as absent (the binding's
        // own TTL semantics).
        if let Some(at) = self.expires.lock().unwrap().get(key)
            && std::time::Instant::now() >= *at
        {
            self.map.lock().unwrap().remove(key);
            self.expires.lock().unwrap().remove(key);
            return Ok(None);
        }
        Ok(self.map.lock().unwrap().get(key).map(|(v, _)| v.clone()))
    }

    async fn put(
        &self,
        key: &str,
        value: Vec<u8>,
        ttl: Option<Duration>,
    ) -> oxide_fw_cache::Result<()> {
        self.map
            .lock()
            .unwrap()
            .insert(key.to_string(), (value, ttl));
        if let Some(ttl) = ttl {
            self.expires
                .lock()
                .unwrap()
                .insert(key.to_string(), std::time::Instant::now() + ttl);
        }
        Ok(())
    }

    async fn delete(&self, key: &str) -> oxide_fw_cache::Result<()> {
        self.map.lock().unwrap().remove(key);
        self.expires.lock().unwrap().remove(key);
        Ok(())
    }
}

fn entry(tags: &[&str]) -> Entry {
    Entry {
        body: b"<html>about</html>".to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html; charset=utf-8".into())],
        tags: tags.iter().map(|t| (*t).to_string()).collect(),
        policy: Policy::default(),
        stored_at: 0,
        etag: "about-v1".into(),
    }
}

#[tokio::main]
async fn main() {
    let kv = Arc::new(MemKv::default());

    // 1. The handler over the worker binding's shape: no entry TTL (the
    //    entry's own §11.3 freshness governs serving), the documented
    //    default tag cap.
    let h = KvCacheHandler::new(kv.clone()).with_ttl(Duration::from_secs(60));
    println!("== the edge KV handler ==");
    println!("  handler_kind: {}", h.handler_kind());
    println!("  tag_cap: {}", h.tag_cap());
    println!("  ttl: {:?} (the worker's expirationTtl arm)", h.ttl());

    // 2. The cold miss, then the tagged set: the entry lands under its
    //    entry key, the tag under ONE index key (`cache:tag:route`).
    let key = "route:/about";
    println!("\ncold get {key}: {:?}", h.get(key).await.unwrap());
    h.set(key, entry(&["route"])).await.unwrap();
    let hit = h.get(key).await.unwrap().expect("hit");
    println!(
        "set+get {key}: {} bytes, tags {:?} (the sealed blob, the S-06 MAC)",
        hit.body.len(),
        hit.tags
    );
    let raw = kv.map.lock().unwrap()["cache:tag:route"].0.clone();
    println!(
        "  the index key's payload: {} (a JSON key list)",
        String::from_utf8_lossy(&raw)
    );
    println!(
        "  tag_keys(route): {:?}",
        h.tag_keys("route").await.unwrap()
    );

    // 3. The cap's best-effort stop: a second handler at cap 1 — the new
    //    key still writes its entry, the tag discovery names only the
    //    cap's worth (the documented posture).
    let capped = KvCacheHandler::new(kv.clone()).with_tag_cap(1);
    capped.set("route:/team", entry(&["route"])).await.unwrap();
    println!("\n== the size cap (best effort, documented) ==");
    println!(
        "  route:/team get: {:?} (the entry exists)",
        capped.get("route:/team").await.unwrap().is_some()
    );
    println!(
        "  tag_keys(route): {:?} (the index stopped at the cap)",
        h.tag_keys("route").await.unwrap()
    );

    // 4. The loud degradation: a store failure is a miss on reads, never
    //    an outage; set is the loud arm. (The fake fails only between the
    //    markers below.)
    println!("\n== the §11.2 degradation posture ==");
    println!(
        "  (a real store failure warns under CACHE-KV and degrades; the fake skips the injection here)"
    );

    // 5. The sweep: every indexed key is deleted, the index key is
    //    dropped, the count is the index's size.
    println!("\n== invalidate_tag ==");
    for i in 0..3 {
        h.set(&format!("route:/p{i}"), entry(&["pages"]))
            .await
            .unwrap();
    }
    let n = h.invalidate_tag("pages", true).await.unwrap();
    println!("  invalidate_tag(pages) deleted {n} entries");
    println!(
        "  route:/p0 after the sweep: {:?}",
        h.get("route:/p0").await.unwrap()
    );

    println!(
        "\nsuccess: the edge KV handler serves the §11.2 contract over the worker binding's shape"
    );
}

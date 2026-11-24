//! SPEC §11.2 + §V2-E09: a KV-backed `CacheHandler` for the edge.
//!
//! `env.OXIDE_KV` (the §V2-E09 worker template's binding — a flat
//! string-keyed store with an optional expiry TTL) behind the same
//! [`CacheHandler`] contract the redis/s3 handlers carry, so a host that
//! speaks a KV shape composes the same cache semantics on the edge.
//!
//! Layout:
//!
//! - `cache:entry:<key>` → the sealed JSON blob of [`Entry`] (the S-06 MAC
//!   every L2 driver writes).
//! - `cache:tag:<tag>`   → JSON array of entry keys — the spec's "a KV
//!   index key per tag".
//!
//! ## Degradation (§11.2 contract, the family posture)
//!
//! A store failure on `get`/`tag_keys`/`invalidate_tag` is a miss/no-op —
//! the local tier keeps serving (a remote cache is an optimization, not a
//! correctness requirement). `set` returns the error. A tampered payload
//! fails the MAC and is a miss (S-06).
//!
//! ## Best effort, documented (the spec's own words)
//!
//! "Tags via a KV index key per tag with a size cap, documented as best
//! effort": the tag index is a read-modify-write over a store with no
//! compare-and-swap (a lost update under concurrency shrinks the tag's
//! discovery until the next write re-appends), and the index stops
//! appending at [`KvCacheHandler::DEFAULT_TAG_CAP`] — the entry still
//! exists and `get` serves it, the tag discovery names only the cap's
//! worth. This is the edge posture the spec accepts; the strict handler
//! (markers + rollback) is the s3 driver's.
//!
//! ## `with_store` shape
//!
//! [`KvCacheHandler::new`] takes any [`Arc<dyn KvStore>`] — the Cloudflare
//! binding, a test fake, or a host's own KV client through the same seam.

use crate::{CacheHandler, Entry, Result, auth, codec, err};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

/// The object-safe KV seam (the `env.OXIDE_KV` shape): flat string keys,
/// opaque bytes, an optional expiry. The handler owns the key layout and
/// the codec; the store owns nothing but bytes and TTLs.
#[async_trait]
pub trait KvStore: Send + Sync {
    /// Read the value at `key` (`None` — absent or expired).
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    /// Write `value` at `key`, expiring after `ttl` when the store speaks
    /// expiry (the worker template's `expirationTtl`; `None` = no expiry).
    async fn put(&self, key: &str, value: Vec<u8>, ttl: Option<Duration>) -> Result<()>;
    /// Remove `key` (absent = done).
    async fn delete(&self, key: &str) -> Result<()>;
}

/// A KV-backed [`CacheHandler`] (the §V2-E09 edge member of the A10
/// remote-handler family).
pub struct KvCacheHandler {
    store: Arc<dyn KvStore>,
    /// The spec's tag-index size cap: the index stops appending here.
    tag_cap: usize,
    /// The TTL every put carries (the worker's `expirationTtl`), `None` for
    /// an entry-lifetime-only store.
    ttl: Option<Duration>,
}

impl KvCacheHandler {
    /// The tag-index size cap when the host names none. The spec says "a
    /// size cap" and names no value — this milestone documents the default
    /// (the doctor session's precedent): a tag touching more than 1,024
    /// keys discovers the first 1,024 through the edge index.
    pub const DEFAULT_TAG_CAP: usize = 1_024;

    /// Wrap a KV store. The default posture: no entry TTL (the entry's own
    /// §11.3 freshness governs serving), the documented default tag cap.
    pub fn new(store: Arc<dyn KvStore>) -> Self {
        Self {
            store,
            tag_cap: Self::DEFAULT_TAG_CAP,
            ttl: None,
        }
    }
    /// Set the tag-index size cap (chainable, the F60 `#[must_use]` shape).
    #[must_use]
    pub fn with_tag_cap(mut self, cap: usize) -> Self {
        self.tag_cap = cap;
        self
    }
    /// Set the TTL every put carries (chainable; the worker's
    /// `expirationTtl` arm).
    #[must_use]
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }
    /// The tag-index size cap (the introspection seam, the F60 shape).
    #[must_use]
    pub fn tag_cap(&self) -> usize {
        self.tag_cap
    }
    /// The per-put TTL (the introspection seam, the F60 shape).
    #[must_use]
    pub fn ttl(&self) -> Option<Duration> {
        self.ttl
    }

    fn entry_key(key: &str) -> String {
        format!("cache:entry:{key}")
    }
    fn tag_key(tag: &str) -> String {
        format!("cache:tag:{tag}")
    }

    /// Read the tag index: a missing key is an empty set (the first write
    /// creates it), a corrupt blob is a data bug — the family's loud-then-
    /// empty convention (serve the empty set, say so). A store failure
    /// degrades to the empty set too (the family contract: the invalidation
    /// misses keys this round, the store self-heals on the next write).
    async fn read_tag_keys(&self, tag: &str) -> Result<Vec<String>> {
        let bytes = match self.store.get(&Self::tag_key(tag)).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(tag, error = %e, "CACHE-KV: the tag index read failed; treating as empty (the invalidation misses keys this round, the store self-heals on the next write)");
                return Ok(Vec::new());
            }
        };
        let Some(bytes) = bytes else {
            return Ok(Vec::new());
        };
        match serde_json::from_slice::<Vec<String>>(&bytes) {
            Ok(keys) => Ok(keys),
            Err(e) => {
                tracing::warn!(tag, error = %e, "CACHE-KV: corrupt tag index; treating as empty (the index re-materializes on the next write)");
                Ok(Vec::new())
            }
        }
    }

    /// Append `key` to the tag's index under the size cap. `Ok(true)` — the
    /// key is discoverable; `Ok(false)` — the cap stopped the append (the
    /// entry exists, the tag discovery misses it, the documented
    /// best-effort posture).
    async fn append_to_tag_index(&self, tag: &str, key: &str) -> Result<bool> {
        let mut keys = self.read_tag_keys(tag).await?;
        if keys.iter().any(|k| k == key) {
            return Ok(true);
        }
        if keys.len() >= self.tag_cap {
            tracing::debug!(
                tag,
                cap = self.tag_cap,
                key,
                "CACHE-KV: the tag index is at its size cap; the key stays discoverable only through the entry (best effort, documented)"
            );
            return Ok(false);
        }
        keys.push(key.to_string());
        let json =
            serde_json::to_vec(&keys).map_err(|e| err("CACHE-KV", format!("tag encode: {e}")))?;
        self.store.put(&Self::tag_key(tag), json, self.ttl).await?;
        Ok(true)
    }
}

#[async_trait]
impl CacheHandler for KvCacheHandler {
    async fn get(&self, key: &str) -> Result<Option<Entry>> {
        // The family degradation: a store failure is a miss, never an
        // outage (the local tier keeps serving).
        let bytes = match self.store.get(&Self::entry_key(key)).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(key, error = %e, "CACHE-KV: the entry read failed; treating as miss");
                return Ok(None);
            }
        };
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        // S-06: verify the keyed MAC before trusting the payload; a
        // tampered blob is a miss.
        let Some(verified) = auth::open_entry(&bytes) else {
            tracing::warn!("L2 (kv) entry failed MAC verification — treated as miss");
            return Ok(None);
        };
        match codec::decode_entry(verified) {
            Ok(e) => Ok(Some(e)),
            Err(e) => Err(err("CACHE-KV", format!("entry decode: {}", e.0.message))),
        }
    }

    async fn set(&self, key: &str, entry: Entry) -> Result<()> {
        // S-06: seal the encoded entry with a keyed MAC.
        let payload = auth::seal_entry(&codec::encode_entry(&entry)?);
        self.store
            .put(&Self::entry_key(key), payload, self.ttl)
            .await?;
        for tag in &entry.tags {
            // B-20's KV arm: the entry is written but a failed index
            // registration would make `invalidate_tag` the only way to
            // reclaim it — remove the entry and surface the error (the s3
            // family's rollback posture; the BEST-EFFORT surfaces are the
            // RMW race and the cap, not a silent write).
            if let Err(e) = self.append_to_tag_index(tag, key).await {
                let _ = self.store.delete(&Self::entry_key(key)).await;
                return Err(err(
                    "CACHE-KV",
                    format!(
                        "put tag {tag}: registration failed, entry rolled back ({})",
                        e.0.message
                    ),
                ));
            }
            // `Ok(false)` — the cap's best-effort stop: the entry stays,
            // the tag discovery names only the cap's worth.
        }
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<()> {
        // The family convention: a delete removes only the entry; the tag
        // index may briefly hold the stale key — harmless, `invalidate_tag`
        // skips missing entries.
        self.store.delete(&Self::entry_key(key)).await?;
        Ok(())
    }

    async fn tag_keys(&self, tag: &str) -> Result<Vec<String>> {
        self.read_tag_keys(tag).await
    }

    async fn invalidate_tag(&self, tag: &str, _soft: bool) -> Result<usize> {
        let keys = self.read_tag_keys(tag).await?;
        if keys.is_empty() {
            return Ok(0);
        }
        for key in &keys {
            // Best-effort deletion: an entry a peer already removed (or a
            // raced delete) must not abort the sweep — the index key is
            // dropped below either way, the family posture.
            let _ = self.store.delete(&Self::entry_key(key)).await;
        }
        self.store.delete(&Self::tag_key(tag)).await?;
        Ok(keys.len())
    }

    async fn publish_invalidation(&self, _tag: &str, _soft: bool) -> Result<()> {
        // KV has no pub/sub. Peers see the index change on their next
        // read; no delivery guarantee. §11.2 leaves cross-node
        // peer-revalidation implementation-defined.
        Ok(())
    }

    fn handler_kind(&self) -> &'static str {
        "kv"
    }
}

//! The sharded multi-plane client (V2-F69): one [`CacheHandler`] over
//! many planes, keys routed by consistent hashing — the §11.2 story's
//! next shape.
//!
//! Until now a node pointed ONE client at ONE plane (`cache.handler =
//! { type: 'http', url }`): the plane's tier was the node's whole remote
//! capacity. The bounds arc already fenced what one plane can take —
//! F67 its concurrency, F64 its body shape, F60 the client's clock —
//! but nothing spread the keys across MORE planes. A node outgrowing a
//! single plane (a render fanning out over a hundred keys against one
//! bounded shell) had to re-plumb the config, not compose. This module
//! is the composition: [`ShardedCacheHandler`] wraps N handlers (the
//! typical shards are [`HttpCacheHandler`](crate::HttpCacheHandler)s,
//! but the seam is the trait — any `CacheHandler` shards) and routes
//! every ENTRY verb to exactly ONE shard, deterministically.
//!
//! ## The routing
//!
//! [`ShardRing`] is rendezvous hashing (highest-random-weight) over the
//! same blake3 the crate's keys already ride
//! ([`keyhash`](crate::keyhash)): for a key, every shard scores
//! `blake3(key ++ 0x00 ++ shard-index-be)` and the highest score wins.
//! The NUL separator keeps the encoding prefix-free (the
//! [`key_of`](crate::keyhash::key_of) rule). Why rendezvous over a
//! vnode ring: the shard count is the number of PLANES a node fronts —
//! single digits, so the O(N) scoring pass is nanoseconds beside a
//! network exchange — and HRW balances without a virtual-node knob to
//! tune, with the same minimal-disruption promise (a shard added or
//! removed moves only the keys whose winner changed, the consistent-hash
//! contract; see the growth pin in `sharded_client_tdd.rs`).
//!
//! ## The verb split
//!
//! - **Entry verbs route** ([`CacheHandler::get`]/[`CacheHandler::get_arc`]/[`CacheHandler::set`]/[`CacheHandler::delete`]):
//!   an entry lives on exactly ONE shard, so the verb lands there alone
//!   and the shard's own `Result` passes through untouched — the
//!   composite re-wraps nothing (the http shard degrades its `get`s to
//!   misses and reports `set`s loud, the §11.2 contracts intact; the
//!   tiered shard serves zero-copy `get_arc`s).
//! - **Tag verbs fan out** ([`CacheHandler::tag_keys`]/[`CacheHandler::invalidate_tag`]/[`CacheHandler::publish_invalidation`]):
//!   a tag's key set is plane-local, so it is SCATTERED across the
//!   shards — the fan asks every shard, in construction order:
//!   `tag_keys` concatenates the lists, `invalidate_tag` sums the
//!   counts, `publish_invalidation` publishes everywhere. A shard that
//!   errors degrades the way the §11.2 contract degrades — LOUD under
//!   the file's stable code, the shard named, the other shards still
//!   serving — never a silent partial.
//!
//! The composite names itself `sharded` through
//! [`CacheHandler::handler_kind`] (the F32 introspection the posture
//! pull reads) and is one `Arc<dyn CacheHandler>` — it composes behind
//! an [`HttpCacheService`](crate::HttpCacheService), inside a
//! [`TieredCache`](crate::TieredCache)'s remote slot, under a probe, or
//! inside another sharder (a fan over fans is just children again, the
//! F34 fan rule).

use crate::{CacheHandler, Entry, Result, err};
use async_trait::async_trait;
use std::sync::Arc;

/// The rendezvous routing function (V2-F69): the key-to-shard map a
/// [`ShardedCacheHandler`] serves through, exposed as its own type so
/// tests and hosts pin the ROUTING without sockets (the introspection
/// seam the F60 `deadline()` accessor established).
///
/// Deterministic and stable: the same key over the same shard count
/// routes to the same shard forever (blake3, nothing else), and two
/// independently built rings agree. Minimal disruption: the growth pin
/// in `sharded_client_tdd.rs` proves growing the set moves only the
/// keys whose winner changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardRing {
    shard_count: usize,
}

impl ShardRing {
    /// Build the ring over `shard_count` shards.
    ///
    /// # Errors
    ///
    /// [`OxideError`](crate::OxideError) with code `CAC-SHARD` when
    /// `shard_count` is zero — an empty shard set serves nothing, and a
    /// composite that silently dropped every `set` would poison the
    /// deployment (the F66 lesson: the loud refusal beats the corruption).
    pub fn new(shard_count: usize) -> Result<Self> {
        if shard_count == 0 {
            return Err(err(
                "CAC-SHARD",
                "the shard set is empty: a sharded client must point at least one plane",
            ));
        }
        Ok(Self { shard_count })
    }

    /// The shard count the ring routes over.
    #[must_use]
    pub fn shard_count(&self) -> usize {
        self.shard_count
    }

    /// The shard a key lands on: the argmax of the rendezvous weights
    /// (one blake3 per shard — single digits of planes, nanoseconds
    /// beside a network exchange). Always in `0..shard_count`.
    #[must_use]
    pub fn route(&self, key: &str) -> usize {
        let mut best_shard = 0usize;
        let mut best_weight = weight(key, 0);
        for shard in 1..self.shard_count {
            let w = weight(key, shard);
            if w > best_weight {
                best_weight = w;
                best_shard = shard;
            }
        }
        best_shard
    }
}

/// One rendezvous score: `blake3(key ++ 0x00 ++ shard-index-be)`, the
/// first 8 digest bytes as a big-endian integer. The NUL separator keeps
/// the encoding prefix-free (the [`key_of`](crate::keyhash::key_of) rule)
/// — a key cannot end where another shard index begins.
fn weight(key: &str, shard: usize) -> u64 {
    let mut h = blake3::Hasher::new();
    h.update(key.as_bytes());
    h.update(&[0]);
    h.update(&shard.to_be_bytes());
    let digest = h.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_be_bytes(bytes)
}

/// The sharded multi-plane client (V2-F69): one [`CacheHandler`] over
/// many shards, entry verbs routed by [`ShardRing`], tag verbs fanned
/// out over every shard. Build it over the shard handlers in the order
/// they should be asked (the fan-out order, and the shard indexes the
/// routing speaks):
///
/// ```
/// use std::sync::Arc;
/// use oxide_fw_cache::{CacheHandler, ShardedCacheHandler};
///
/// # fn build() -> oxide_fw_cache::Result<()> {
/// let client = ShardedCacheHandler::new(vec![
///     Arc::new(oxide_fw_cache::HttpCacheHandler::new("http://plane-a:7800"))
///         as Arc<dyn CacheHandler>,
///     Arc::new(oxide_fw_cache::HttpCacheHandler::new("http://plane-b:7800"))
///         as Arc<dyn CacheHandler>,
/// ])?;
/// // Entry verbs land on `client.shard_of(key)` alone; tag verbs fan
/// // out over both planes.
/// # Ok(())
/// # }
/// ```
pub struct ShardedCacheHandler {
    shards: Vec<Arc<dyn CacheHandler>>,
    ring: ShardRing,
}

impl std::fmt::Debug for ShardedCacheHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The shard set is the composite's identity (the boot's attach
        // shape); each shard's own Debug carries its internals.
        f.debug_struct("ShardedCacheHandler")
            .field("shards", &self.shards.len())
            .finish_non_exhaustive()
    }
}

impl ShardedCacheHandler {
    /// Compose the sharded client over its shards.
    ///
    /// # Errors
    ///
    /// [`OxideError`](crate::OxideError) with code `CAC-SHARD` when the
    /// shard set is empty (see [`ShardRing::new`] — the loud refusal is
    /// the house convention: a composite that silently dropped every
    /// `set` would poison the deployment).
    pub fn new(shards: Vec<Arc<dyn CacheHandler>>) -> Result<Self> {
        let ring = ShardRing::new(shards.len())?;
        Ok(Self { shards, ring })
    }

    /// The shard count the client routes over (the F60 `deadline()`
    /// introspection shape: tests and hosts pin the state instead of
    /// guessing it).
    #[must_use]
    pub fn shards(&self) -> usize {
        self.ring.shard_count()
    }

    /// The shard a key routes to — the routing function exposed, so a
    /// host can answer "which plane holds this key?" without sockets.
    /// Always in `0..shards()`.
    #[must_use]
    pub fn shard_of(&self, key: &str) -> usize {
        self.ring.route(key)
    }
}

#[async_trait]
impl CacheHandler for ShardedCacheHandler {
    async fn get(&self, key: &str) -> Result<Option<Entry>> {
        self.shards[self.ring.route(key)].get(key).await
    }

    /// The routed shard's own shared-entry answer (the P-03 zero-copy
    /// seam passes THROUGH the composite — a tier-backed shard hands
    /// back its `Arc<Entry>` directly, no copy at this hop).
    async fn get_arc(&self, key: &str) -> Result<Option<Arc<Entry>>> {
        self.shards[self.ring.route(key)].get_arc(key).await
    }

    async fn set(&self, key: &str, entry: Entry) -> Result<()> {
        self.shards[self.ring.route(key)].set(key, entry).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.shards[self.ring.route(key)].delete(key).await
    }

    async fn tag_keys(&self, tag: &str) -> Result<Vec<String>> {
        // The fan: a tag's key set is plane-local, so it is scattered
        // across the shards — every shard is asked, in construction
        // order, and the lists concatenate in that order. A shard that
        // errors degrades LOUD (the file's stable code, the shard named)
        // and its keys are simply absent from the gather — the §11.2
        // contract degrades, the gap is visible, never silent.
        let mut keys = Vec::new();
        for (i, shard) in self.shards.iter().enumerate() {
            match shard.tag_keys(tag).await {
                Ok(mut shard_keys) => keys.append(&mut shard_keys),
                Err(e) => tracing::warn!(
                    code = "CAC-SHARD",
                    shard = i,
                    tag = %tag,
                    error = %e.0.message,
                    "the shard's tag-key gather failed; its keys are absent from this listing"
                ),
            }
        }
        Ok(keys)
    }

    async fn invalidate_tag(&self, tag: &str, soft: bool) -> Result<usize> {
        // The fan: every shard invalidates (the tag is scattered), the
        // answer is the SUM across the shards. A shard that errors warns
        // loudly (the shards that already served keep their result — the
        // local-tier-first contract) and the sum carries the rest.
        let mut count = 0usize;
        for (i, shard) in self.shards.iter().enumerate() {
            match shard.invalidate_tag(tag, soft).await {
                Ok(n) => count = count.saturating_add(n),
                Err(e) => tracing::warn!(
                    code = "CAC-SHARD",
                    shard = i,
                    tag = %tag,
                    soft = soft,
                    error = %e.0.message,
                    "the shard's tag invalidation failed; its keys may survive on that plane"
                ),
            }
        }
        Ok(count)
    }

    async fn publish_invalidation(&self, tag: &str, soft: bool) -> Result<()> {
        // The fan: the publish reaches every shard (peers of every plane
        // converge). Each shard's publish carries its own best-effort
        // contract (the http shard's zero-dial warn+Ok, the F66 shape);
        // a shard that still returns `Err` warns here — the composite's
        // own answer stays `Ok(())`, the local tiers already served.
        for (i, shard) in self.shards.iter().enumerate() {
            if let Err(e) = shard.publish_invalidation(tag, soft).await {
                tracing::warn!(
                    code = "CAC-SHARD",
                    shard = i,
                    tag = %tag,
                    soft = soft,
                    error = %e.0.message,
                    "the shard's invalidation publish failed; that plane's peers may still serve the tag"
                );
            }
        }
        Ok(())
    }

    fn handler_kind(&self) -> &'static str {
        "sharded"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_ring_refuses_with_the_stable_code() {
        let e = ShardRing::new(0).expect_err("the empty ring refuses");
        assert_eq!(e.0.code, "CAC-SHARD");
    }

    #[test]
    fn the_route_is_deterministic_in_range_and_agrees_between_rings() {
        let a = ShardRing::new(4).expect("4 shards build");
        let b = ShardRing::new(4).expect("4 shards build");
        for key in ["route:/", "route:/blog/[slug]", "L1:data", "x"] {
            assert!(a.route(key) < 4);
            assert_eq!(a.route(key), b.route(key), "two rings agree on {key}");
        }
    }

    #[test]
    fn every_component_of_the_weight_participates() {
        // The NUL separator: (key, shard) pairs must not collide across
        // key boundaries — "ab" on shard X vs "a" on a shard whose index
        // byte spells the rest are different encodings, so different
        // scores (the keyhash prefix-free rule).
        let ring = ShardRing::new(8).expect("8 shards build");
        let mut seen = std::collections::HashSet::new();
        for shard in 0..8 {
            seen.insert(weight("ab", shard));
        }
        assert_eq!(seen.len(), 8, "the shard index participates in the score");
        assert!(ring.route("ab") < 8);
    }

    #[test]
    fn the_keys_spread_across_a_four_shard_ring() {
        let ring = ShardRing::new(4).expect("4 shards build");
        let mut seen = [false; 4];
        for i in 0..64 {
            seen[ring.route(&format!("route:/page/{i}"))] = true;
        }
        assert!(seen.iter().filter(|s| **s).count() >= 2, "{seen:?}");
    }
}

//! Single-flight coalescing (spec §11.3): concurrent misses for the same key run one
//! render. The first caller becomes the [`FlightSlot::Owner`]; everyone else receives
//! a [`FlightSlot::Follower`] broadcast receiver and awaits the owner's result.
//!
//! # Cleanup ordering (deviation from the naive "publish, then remove")
//!
//! [`SingleFlight::publish_and_cleanup`] removes the map entry **before** sending on
//! the broadcast channel. This closes a subscribe/publish race deterministically:
//!
//! - a follower that finds the entry subscribes while still holding the `DashMap` shard
//!   read lock; the owner's `remove` needs the write lock on that shard, so the owner
//!   cannot remove-and-send until every in-flight follower has subscribed. Nobody who
//!   found the entry can miss the value;
//! - an acquirer arriving after the removal becomes the next owner — it may compute
//!   concurrently with the tail of the previous generation (a bounded, documented
//!   thundering-herd window) but can never hang waiting for a message that was sent
//!   before it subscribed.
//!
//! Followers that nevertheless observe a `RecvError` should treat it as a cache miss
//! and retry.

use crate::Entry;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Single-flight coordinator. Stateless — the in-flight map is supplied by the caller
/// (one map per scope, e.g. per server process), which keeps [`SingleFlight`] cheap to
/// share.
pub struct SingleFlight;

impl SingleFlight {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Acquire the right to compute `key`: `Owner(tx)` if no flight is in progress,
    /// otherwise `Follower(rx)` subscribed to the owner's channel.
    pub async fn acquire(
        &self,
        map: &DashMap<String, Arc<broadcast::Sender<Arc<Entry>>>>,
        key: &str,
    ) -> FlightSlot {
        // Fast path: subscribe while the shard read lock is held (see module docs).
        if let Some(tx) = map.get(key) {
            return FlightSlot::Follower(tx.subscribe());
        }
        match map.entry(key.to_string()) {
            dashmap::mapref::entry::Entry::Occupied(e) => FlightSlot::Follower(e.get().subscribe()),
            dashmap::mapref::entry::Entry::Vacant(e) => {
                let (tx, _rx) = broadcast::channel(1);
                e.insert(Arc::new(tx.clone()));
                FlightSlot::Owner(tx)
            }
        }
    }

    /// Owner-side completion: remove the in-flight entry, then publish `entry` to the
    /// followers. `entry = None` models an owner that failed to compute — the entry is
    /// still removed so the next acquirer can start a fresh flight. Returns the number
    /// of followers reached (send failures with no subscribers are ignored).
    pub async fn publish_and_cleanup(
        &self,
        map: &DashMap<String, Arc<broadcast::Sender<Arc<Entry>>>>,
        key: &str,
        slot: FlightSlot,
        entry: Option<Arc<Entry>>,
    ) -> usize {
        map.remove(key); // before publish — see module docs
        match (slot, entry) {
            (FlightSlot::Owner(tx), Some(value)) => tx.send(value).unwrap_or_default(),
            _ => 0, // failed owner or a follower slot: cleanup only
        }
    }
}

impl Default for SingleFlight {
    fn default() -> Self {
        Self::new()
    }
}

/// Slot returned by [`SingleFlight::acquire`].
pub enum FlightSlot {
    /// You compute; call `publish` when done.
    Owner(tokio::sync::broadcast::Sender<Arc<Entry>>),
    /// Await the owner's result via this receiver.
    Follower(tokio::sync::broadcast::Receiver<Arc<Entry>>),
}

/// S-15: an RAII guard for an owner slot. Every owner exit path — success,
/// loader `error(404)`/redirect, cookie short-circuit, render error — must
/// clear the in-flight map entry, or a short-circuited request leaks one
/// `DashMap` entry forever (attacker-mintable keys → unbounded growth, and the
/// key is single-flight-broken thereafter). A guard makes *all* early returns
/// correct without threading cleanup through each one.
///
/// Success path: call [`FlightOwnerGuard::publish`], which removes the entry
/// then broadcasts the entry to followers.
pub struct FlightOwnerGuard {
    map: Arc<DashMap<String, Arc<broadcast::Sender<Arc<Entry>>>>>,
    key: String,
    tx: Option<broadcast::Sender<Arc<Entry>>>,
}

impl FlightOwnerGuard {
    /// Wrap an owner sender (from [`FlightSlot::Owner`]).
    #[must_use]
    pub fn new(
        map: Arc<DashMap<String, Arc<broadcast::Sender<Arc<Entry>>>>>,
        key: String,
        tx: broadcast::Sender<Arc<Entry>>,
    ) -> Self {
        Self {
            map,
            key,
            tx: Some(tx),
        }
    }

    /// Success: publish the computed entry to followers. Removes the in-flight
    /// entry before sending (same ordering guarantee as
    /// [`SingleFlight::publish_and_cleanup`]).
    ///
    /// Returns the number of followers reached — the same count
    /// [`SingleFlight::publish_and_cleanup`] returns, which the F33
    /// emitter contract routes to
    /// [`TieredCache::record_coalesced`](crate::tiers::TieredCache::record_coalesced)
    /// ("the host's flight owner calls it on completion with the count
    /// the publish reached"). The count previously rode the send and was
    /// dropped on the floor, leaving the guard's owner with no honest
    /// join number to record.
    #[must_use = "the follower count is the F33 emitter's join number; record it (record_coalesced) or name the drop"]
    pub fn publish(mut self, entry: Arc<Entry>) -> usize {
        // Remove only our own generation; a later owner may have replaced it.
        self.map.remove(&self.key);
        if let Some(tx) = self.tx.take() {
            tx.send(entry).unwrap_or_default()
        } else {
            0
        }
    }

    /// Explicit failure: clear the entry so the next acquirer starts fresh.
    pub fn abandon(mut self) {
        self.map.remove(&self.key);
        self.tx = None;
    }
}

impl Drop for FlightOwnerGuard {
    fn drop(&mut self) {
        // `publish`/`abandon` consume `self`, so reaching here with `tx` still
        // `Some` means the owner exited early (an error, a short-circuit, a
        // panic) without publishing.
        //
        // A2: remove UNCONDITIONALLY. The previous `receiver_count() == 0`
        // guard was the bug: a follower that subscribed before the owner
        // short-circuited held a receiver forever, so the entry never cleared
        // and every future request for that key became a follower that
        // stalled to the 10s timeout - indefinitely.
        //
        // A follower whose receiver is left dangling (the sender dropped
        // without a send) gets `Err(Closed)` from `recv`, which the caller
        // already treats as "fall through and compute".
        if self.tx.is_some() {
            self.map.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Policy;
    use std::time::Duration;

    fn entry(body: &[u8]) -> Entry {
        Entry {
            body: body.to_vec(),
            status: 200,
            headers: vec![],
            tags: vec![],
            policy: Policy {
                ttl: Duration::from_secs(1),
                swr: None,
                stale_if_error: None,
                soft_tags: true,
            },
            stored_at: 0,
            etag: String::new(),
        }
    }

    #[tokio::test]
    async fn acquire_owner_then_followers_then_cleanup() {
        let sf = SingleFlight::new();
        let map: DashMap<String, Arc<broadcast::Sender<Arc<Entry>>>> = DashMap::new();

        // First acquire owns; follow-up acquires coalesce onto it.
        let FlightSlot::Owner(owner) = sf.acquire(&map, "k").await else {
            panic!("first acquire must own");
        };
        let f1 = sf.acquire(&map, "k").await;
        let f2 = sf.acquire(&map, "k").await;
        assert!(matches!(f1, FlightSlot::Follower(_)));
        assert!(matches!(f2, FlightSlot::Follower(_)));
        assert_eq!(map.len(), 1);

        // Publish reaches both followers and removes the in-flight entry.
        let n = sf
            .publish_and_cleanup(
                &map,
                "k",
                FlightSlot::Owner(owner),
                Some(Arc::new(entry(b"v"))),
            )
            .await;
        assert_eq!(n, 2);
        assert!(map.is_empty());

        // Next acquire starts a fresh flight.
        assert!(matches!(sf.acquire(&map, "k").await, FlightSlot::Owner(_)));
    }

    #[tokio::test]
    async fn owner_guard_dropped_on_early_exit_clears_the_map() {
        // S-15: an owner that exits without publishing (loader 404/redirect,
        // cookie short-circuit, render error) must not leak a map entry.
        let map: Arc<DashMap<String, Arc<broadcast::Sender<Arc<Entry>>>>> =
            Arc::new(DashMap::new());
        let sf = SingleFlight::new();
        let FlightSlot::Owner(tx) = sf.acquire(&map, "k").await else {
            panic!("owner");
        };
        assert_eq!(map.len(), 1);
        {
            let _guard = FlightOwnerGuard::new(Arc::clone(&map), "k".to_string(), tx);
            // guard drops here without publish/abandon
        }
        assert!(map.is_empty(), "early exit must clear the flight entry");
        // A fresh acquire owns again.
        assert!(matches!(sf.acquire(&map, "k").await, FlightSlot::Owner(_)));
    }

    #[tokio::test]
    async fn owner_guard_publish_delivers_and_clears() {
        let map: Arc<DashMap<String, Arc<broadcast::Sender<Arc<Entry>>>>> =
            Arc::new(DashMap::new());
        let sf = SingleFlight::new();
        let FlightSlot::Owner(tx) = sf.acquire(&map, "k").await else {
            panic!("owner");
        };
        let FlightSlot::Follower(mut follower) = sf.acquire(&map, "k").await else {
            panic!("follower");
        };
        let guard = FlightOwnerGuard::new(Arc::clone(&map), "k".to_string(), tx);
        let reached = guard.publish(Arc::new(entry(b"v")));
        assert_eq!(reached, 1, "the one subscribed follower was reached");
        assert!(map.is_empty());
        let got = follower.recv().await.unwrap();
        assert_eq!(got.body, b"v");
    }

    #[tokio::test]
    async fn owner_guard_publish_returns_the_follower_count() {
        // The F33 emitter contract routes the count the publish reached
        // to `record_coalesced` — the guard's publish must hand it back,
        // not drop it on the floor (the micro-debt the count-carrying
        // emitter exposed).
        let map: Arc<DashMap<String, Arc<broadcast::Sender<Arc<Entry>>>>> =
            Arc::new(DashMap::new());
        let sf = SingleFlight::new();
        let FlightSlot::Owner(tx) = sf.acquire(&map, "k").await else {
            panic!("owner");
        };
        let FlightSlot::Follower(mut follower) = sf.acquire(&map, "k").await else {
            panic!("follower");
        };
        let guard = FlightOwnerGuard::new(Arc::clone(&map), "k".to_string(), tx);
        let reached = guard.publish(Arc::new(entry(b"v")));
        assert_eq!(reached, 1, "one follower was reached");
        let _ = follower.recv().await;
    }

    #[tokio::test]
    async fn owner_guard_publish_with_no_followers_returns_zero() {
        // A zero-follower flight still completes and still records — the
        // count is honest zero, not a dropped send.
        let map: Arc<DashMap<String, Arc<broadcast::Sender<Arc<Entry>>>>> =
            Arc::new(DashMap::new());
        let sf = SingleFlight::new();
        let FlightSlot::Owner(tx) = sf.acquire(&map, "k").await else {
            panic!("owner");
        };
        let guard = FlightOwnerGuard::new(Arc::clone(&map), "k".to_string(), tx);
        assert_eq!(guard.publish(Arc::new(entry(b"v"))), 0);
        assert!(map.is_empty());
    }

    #[tokio::test]
    async fn follower_slot_cleanup_is_a_no_op_publish() {
        let sf = SingleFlight::new();
        let map: DashMap<String, Arc<broadcast::Sender<Arc<Entry>>>> = DashMap::new();
        let _owner = sf.acquire(&map, "k").await;
        let follower = sf.acquire(&map, "k").await;
        // A follower cannot publish; cleanup still removes the (now ownerless) entry.
        let n = sf
            .publish_and_cleanup(&map, "k", follower, Some(Arc::new(entry(b"v"))))
            .await;
        assert_eq!(n, 0);
        assert!(map.is_empty());
    }
}

//! The cache observability seam's composition arm: one
//! [`FanoutCacheObserver`] fans every event it receives into every child
//! observer, so the crate's shipped consumers compose behind the ONE
//! `with_observer` slot (§20, F34).
//!
//! ## Why the arm exists
//!
//! [`TieredCache::with_observer`](crate::tiers::TieredCache::with_observer)
//! is a single slot: the seam's contract is one synchronous callback per
//! event, and the tiers store one `Option<Arc<dyn CacheObserver>>`. The
//! crate ships three consumers — the tracing arm
//! ([`TracingCacheObserver`](crate::observer::TracingCacheObserver)), the
//! metrics exporter ([`MetricsCacheObserver`](crate::metrics::MetricsCacheObserver))
//! and the bounded feed ([`EventFeed`](crate::feed::EventFeed)) — and a
//! host that wants the exporter AND the feed attached at once had to
//! hand-roll a composite (the `coalesce` walkthrough carried exactly that
//! four-line inline impl, naming the server crate's posture it copied).
//! The request plane answered the same single-slot need with
//! `FanoutTraceSink` (F22); this module is that fan for the cache plane,
//! shipped once so no host re-rolls it.
//!
//! ## The contract
//!
//! - One event reaches every child, **in construction order** (the
//!   `FanoutTraceSink` rule): a filter or a pin can rely on child A's row
//!   landing before child B's for the same event. No cross-child
//!   re-ordering is attempted — events carry no global sequence number,
//!   so any merge would be fabricated.
//! - The event is **re-shared, not rebuilt**: `on_event` hands every
//!   child the same `&CacheEvent` borrow it received, so a fan over N
//!   children adds N virtual calls and zero copies (the seam's
//!   zero-alloc promise, kept for the composition).
//! - An **empty fan is the honest no-op**: it attaches (the posture's
//!   `observer_attached` reads `true` — the slot IS wired), delivers
//!   nowhere, and answers `children() == 0`. It never fabricates a
//!   consumer that is not there.
//! - Fanouts nest: a fan over fans is just children again — the trait
//!   contract, not a special shape.
//! - No error codes: the fan is infallible (the F21 posture — the
//!   registry gains no rows for a seam that cannot refuse).
//!
//! The children keep their own contracts: the exporter counts, the feed
//! bounds and warns, the tracing arm renders. The fan only moves the
//! event — it holds no state of its own.

use std::sync::Arc;

use crate::observer::{CacheEvent, CacheObserver};

/// The record fan: one event reaches every child observer. Build it with
/// the children in the order delivery should run — a dashboard boot
/// attaches `[tracing, metrics, feed]` and reads rows left to right:
///
/// ```
/// use std::sync::Arc;
/// use oxide_fw_cache::{EventFeed, FanoutCacheObserver, MetricsCacheObserver, TieredCache};
///
/// let metrics = Arc::new(MetricsCacheObserver::new());
/// let feed = Arc::new(EventFeed::bounded(1024));
/// let fan = FanoutCacheObserver::shared(vec![
///     Arc::new(oxide_fw_cache::TracingCacheObserver::new()) as Arc<dyn oxide_fw_cache::CacheObserver>,
///     metrics.clone() as Arc<dyn oxide_fw_cache::CacheObserver>,
///     feed.clone() as Arc<dyn oxide_fw_cache::CacheObserver>,
/// ]);
/// let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(fan));
/// // … one slot, three consumers: the scrape reads `metrics`, the task
/// // drains `feed`, the subscriber renders the tracing rows.
/// ```
pub struct FanoutCacheObserver {
    children: Vec<Arc<dyn CacheObserver>>,
}

impl std::fmt::Debug for FanoutCacheObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The children are the fan's identity (the boot's attach shape);
        // each observer's own Debug carries its internals.
        f.debug_struct("FanoutCacheObserver")
            .field("children", &self.children.len())
            .finish_non_exhaustive()
    }
}

impl FanoutCacheObserver {
    /// A fan over the child observers. An empty fan is the honest no-op:
    /// it attaches and delivers nowhere (the `FanoutTraceSink` decision).
    #[must_use]
    pub fn new(children: Vec<Arc<dyn CacheObserver>>) -> Self {
        Self { children }
    }

    /// The child count the fan was built with.
    #[must_use]
    pub fn children(&self) -> usize {
        self.children.len()
    }

    /// `Arc::new(FanoutCacheObserver::new(children))` with the
    /// attach-ready type spelled — the `Arc<dyn CacheObserver>` the
    /// tiers' field stores, ready for
    /// [`TieredCache::with_observer`](crate::tiers::TieredCache::with_observer)
    /// (the [`EventFeed::shared`](crate::feed::EventFeed::shared) shape).
    #[must_use]
    pub fn shared(children: Vec<Arc<dyn CacheObserver>>) -> Arc<Self> {
        Arc::new(Self::new(children))
    }
}

impl CacheObserver for FanoutCacheObserver {
    fn on_event(&self, event: &CacheEvent<'_>) {
        // The borrow is re-shared, not rebuilt: every child sees the same
        // `&CacheEvent` (zero copies), in construction order.
        for child in &self.children {
            child.on_event(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::MetricsCacheObserver;

    #[test]
    fn an_empty_fan_delivers_nowhere() {
        let fan = FanoutCacheObserver::new(Vec::new());
        // Takes every row without panic, delivers nowhere (no children).
        fan.on_event(&CacheEvent {
            key: "k",
            op: crate::observer::CacheOp::Miss,
        });
        assert_eq!(fan.children(), 0);
    }

    #[test]
    fn a_fan_over_the_exporter_counts_its_rows() {
        let metrics = Arc::new(MetricsCacheObserver::new());
        let fan = FanoutCacheObserver::new(vec![metrics.clone() as Arc<dyn CacheObserver>]);
        fan.on_event(&CacheEvent {
            key: "k",
            op: crate::observer::CacheOp::Set { bytes: 12 },
        });
        assert_eq!(metrics.snapshot().sets, 1);
        assert_eq!(metrics.snapshot().stored_bytes, 12);
    }
}

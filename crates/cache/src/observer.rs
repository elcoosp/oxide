//! The cache observability seam: a push channel for the cache plane's
//! events, beside the pull channel [`crate::tiers::TieredCache::stats`]
//! already serves (§11.2, §20).
//!
//! ## Why the seam exists
//!
//! The request plane grew its push seams over F22–F26 — the trace feed
//! (`/__oxide/traces`), the log feed (`/__oxide/logs`) and the export
//! arm — so a host can subscribe to what the pipeline *did* without
//! polling. The cache plane still answered only through pull: counters
//! (`CacheStats`) sampled by `oxide cache stats` and the §20 metrics.
//! Pull cannot answer the questions a dashboard asks ("which keys were
//! invalidated by this deploy", "how many disk-tier promotions did the
//! boot pay") unless the sampler stores every intermediate, and it
//! misses the events entirely between samples.
//!
//! [`CacheObserver`] is that push channel: one synchronous, infallible
//! callback per cache event, delivered while the event happens. The
//! record ([`CacheEvent`] / [`CacheOp`]) is value-shaped — the
//! [`crate::single_flight::SingleFlight`] posture, `Clone` + `Eq` — and
//! the key rides as a borrow, so a hot-path emission allocates nothing:
//! the no-observer path is one `None` branch, and the attached path is
//! one virtual call.
//!
//! ## The contract
//!
//! - Events are delivered for **completed** operations only: a `set`
//!   that wrote through, a `get` that served (or missed), a `delete`
//!   that finished, one aggregate `invalidate` row per completed
//!   [`TieredCache::invalidate_tag`](crate::tiers::TieredCache::invalidate_tag)
//!   call (the affected-key count rides the row — never one event per
//!   key), and one aggregate `coalesce` row per completed single-flight
//!   flight the host records through
//!   [`TieredCache::record_coalesced`](crate::tiers::TieredCache::record_coalesced)
//!   (the join count rides the row — the push twin of the pull channel's
//!   `coalesced` counter, F33). Push-only: no replay of pre-attach
//!   traffic.
//! - [`CacheOp::Hit`] names the tier that SERVED
//!   ([`Layer`](crate::Layer)`::L0` memory, `L1` disk, `L2` remote) and
//!   whether the serve was stale (the SWR window). An expired entry is
//!   invisible to the observer — the caller-visible outcome is a miss,
//!   and the lazy delete is not a `delete` event.
//! - An oversized `set` (over the memory cap, refused before every
//!   tier — the S-08 rule) stores nothing, so it emits nothing.
//!
//! [`TracingCacheObserver`] is the production impl: every row leaves as
//! one `tracing` event under the `oxide::cache` target (the same
//! convention as the §20 request log under `oxide::request`), debug for
//! the hot-path rows (hit/miss/set/delete) and info for the
//! deploy-visible arm (invalidations). The crate ships the structured
//! consumers the seam's doc named: the metrics exporter
//! ([`crate::metrics::MetricsCacheObserver`], the §20 exposition), the
//! bounded lossy feed ([`crate::feed::EventFeed`], enqueue here / drain
//! elsewhere), the per-key attribution ledger
//! ([`crate::hot_keys::HotKeyObserver`], behind its F39 admission gate —
//! the dashboard-batcher shape the sentence below named as a custom
//! impl, shipped once) — and a custom impl for anything else (a test
//! harness).

use crate::Layer;

/// The op payload one observer callback carries (§11.2/§11.3).
///
/// Value-shaped by design (`Clone` + `Eq`): a metrics exporter batches
/// records, a test pins records — neither holds a borrow on the cache.
/// The payload carries what the op DID (the serving
/// [`Layer`](crate::Layer), the stale flag, the payload size, the
/// affected-key count), not what the caller asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheOp {
    /// A lookup SERVED an entry: `layer` names the tier that served it,
    /// `stale` marks the SWR-window serve (§11.3), `bytes` is the entry
    /// body size.
    Hit {
        /// The tier that served: L0 memory, L1 disk, L2 remote.
        layer: Layer,
        /// Served stale (within the SWR window, past the ttl).
        stale: bool,
        /// The served entry's body size.
        bytes: u64,
    },
    /// A lookup exhausted every attached tier (an expired entry reads as
    /// a miss — the lazy delete is not a `delete` event).
    Miss,
    /// A write-through completed (`bytes` is the entry body size). An
    /// oversized entry refused before every tier (S-08) emits nothing.
    Set {
        /// The stored entry's body size.
        bytes: u64,
    },
    /// A delete completed across every attached tier.
    Delete,
    /// One aggregate row per completed tag invalidation (§11.3): `tag`
    /// is the invalidated tag, `soft` the soft/hard arm, `count` the
    /// affected-key count the call returned. Never one event per key.
    Invalidate {
        /// The invalidated tag.
        tag: String,
        /// The soft (serve-then-regen) vs hard (delete) arm.
        soft: bool,
        /// The number of distinct keys affected.
        count: usize,
    },
    /// One aggregate row per completed single-flight (§11.3): the owner's
    /// render finished and `followers` coalesced onto it (the push twin
    /// of the pull channel's `coalesced` counter, F33). The key names the
    /// flight's key; a zero-follower flight still completes and still
    /// records.
    Coalesce {
        /// The number of followers that joined the owner's result.
        followers: u64,
    },
}

/// One cache event: the borrowed key plus the op payload. The borrow is
/// the zero-alloc promise — the emit sites hand the `&str` they already
/// hold, the callback copies what it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEvent<'a> {
    /// The cache key the op ran against (canonical `KeyStr` on the
    /// route/data plane, the fetch-memo key on the L2 plane).
    pub key: &'a str,
    /// The op payload (see [`CacheOp`]).
    pub op: CacheOp,
}

impl CacheEvent<'_> {
    /// The stable row name a filter or exporter keys on: `"hit"`,
    /// `"miss"`, `"set"`, `"delete"`, `"invalidate"`, `"coalesce"`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self.op {
            CacheOp::Hit { .. } => "hit",
            CacheOp::Miss => "miss",
            CacheOp::Set { .. } => "set",
            CacheOp::Delete => "delete",
            CacheOp::Invalidate { .. } => "invalidate",
            CacheOp::Coalesce { .. } => "coalesce",
        }
    }
}

/// The push seam (§20): one synchronous, infallible callback per cache
/// event. `Send + Sync + 'static` so the observer rides the
/// `Arc`-shared cache across the pipeline's tasks.
///
/// Implementations must be cheap and never panic: the callback runs ON
/// the cache's call path (the request path for a hot hit), so a slow
/// observer slows the cache. The crate ships the bounded feed for the
/// heavy end — [`crate::feed::EventFeed`] enqueues the rows here and lets
/// the host's own task process them elsewhere, lossy past its bound (the
/// `FileTraceSink` posture: one warn, then degrade).
pub trait CacheObserver: Send + Sync + 'static {
    /// Called once per completed cache event. The `event` borrow lives
    /// only for the call.
    fn on_event(&self, event: &CacheEvent<'_>);
}

/// The production impl: every row leaves as one `tracing` event under
/// the `oxide::cache` target (the `oxide::request` convention), debug
/// for the hot-path rows and info for invalidations — the deploy-visible
/// arm a host wants in the default subscriber even at `info` level.
///
/// With no subscriber installed the events are no-ops (the `tracing`
/// contract), so the observer is safe to attach unconditionally in
/// hosts that own their subscriber.
#[derive(Debug, Default, Clone, Copy)]
pub struct TracingCacheObserver;

impl TracingCacheObserver {
    /// A new tracing-backed observer.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl CacheObserver for TracingCacheObserver {
    fn on_event(&self, event: &CacheEvent<'_>) {
        match &event.op {
            CacheOp::Hit {
                layer,
                stale,
                bytes,
            } => tracing::debug!(
                target: "oxide::cache",
                key = %event.key,
                layer = layer.as_str(),
                stale = *stale,
                bytes = *bytes,
                "cache hit"
            ),
            CacheOp::Miss => tracing::debug!(
                target: "oxide::cache",
                key = %event.key,
                "cache miss"
            ),
            CacheOp::Set { bytes } => tracing::debug!(
                target: "oxide::cache",
                key = %event.key,
                bytes = *bytes,
                "cache set"
            ),
            CacheOp::Delete => tracing::debug!(
                target: "oxide::cache",
                key = %event.key,
                "cache delete"
            ),
            CacheOp::Invalidate { tag, soft, count } => tracing::info!(
                target: "oxide::cache",
                tag = %tag,
                soft = *soft,
                count = *count,
                "cache tag invalidated"
            ),
            CacheOp::Coalesce { followers } => tracing::debug!(
                target: "oxide::cache",
                key = %event.key,
                followers = *followers,
                "cache flight coalesced"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_stable_names() {
        let k = |op| CacheEvent { key: "k", op }.kind();
        assert_eq!(k(CacheOp::Miss), "miss");
        assert_eq!(k(CacheOp::Set { bytes: 0 }), "set");
        assert_eq!(k(CacheOp::Delete), "delete");
        assert_eq!(k(CacheOp::Coalesce { followers: 1 }), "coalesce");
        assert_eq!(
            k(CacheOp::Invalidate {
                tag: "t".into(),
                soft: true,
                count: 0
            }),
            "invalidate"
        );
        assert_eq!(
            k(CacheOp::Hit {
                layer: Layer::L0,
                stale: false,
                bytes: 1
            }),
            "hit"
        );
    }

    #[test]
    fn tracing_observer_takes_every_row() {
        let obs = TracingCacheObserver::new();
        // No subscriber installed: the rows are no-ops (the tracing
        // contract) — the pin is "takes every row without panic".
        obs.on_event(&CacheEvent {
            key: "k",
            op: CacheOp::Miss,
        });
        obs.on_event(&CacheEvent {
            key: "k",
            op: CacheOp::Hit {
                layer: Layer::L2,
                stale: true,
                bytes: 0,
            },
        });
        obs.on_event(&CacheEvent {
            key: "k",
            op: CacheOp::Coalesce { followers: 3 },
        });
    }
}

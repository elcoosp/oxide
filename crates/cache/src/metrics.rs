//! The cache plane's metrics exporter (`observer.rs`'s named consumer):
//! one `MetricsCacheObserver` aggregates every completed cache event into
//! atomic counters and renders the §20 Prometheus text exposition on
//! scrape.
//!
//! ## Why the arm exists
//!
//! The F28 seam's doc names the structured end a host attaches: "a
//! metrics exporter, a test harness, a feed". Until now the §11.2 cache
//! plane answered a scrape only through the pull channel —
//! [`crate::tiers::TieredCache::stats`] samples live counters into
//! [`crate::CacheStats`] (hits, misses, stale, coalesced, memory
//! occupancy) and the CLI renders them per invocation. What pull cannot
//! answer across samples is the exporter's whole point: the per-tier hit
//! split (did this process serve from L0 memory, or did the boot pay L1
//! disk reads), the bytes served vs stored delta, the invalidation rows a
//! deploy produced — aggregated push-side, monotone, and rendered in the
//! same text exposition the request plane's
//! [`crate::observer`]-adjacent `oxide_fw_server::metrics::Metrics`
//! speaks.
//!
//! ## The two families
//!
//! The request plane already exposes `oxide_cache_l0_*_total` (the
//! page-render memory cache in `oxide_fw_server::metrics`). This arm's
//! rows carry the `oxide_cache_plane_*` prefix — the §11.2 tiered store's
//! own family — so one scrape shows both without a label collision: the
//! page-render rows and the tiered-store rows are different caches.
//!
//! ## The contract
//!
//! - Counters are monotone and never reset: a scrape is a delta over
//!   whatever window the scraper keeps, the Prometheus posture.
//! - The exporter is infallible and synchronous — the `CacheObserver`
//!   contract (the callback runs on the cache's call path, so every arm
//!   is one relaxed atomic add; a hot L0 hit pays one branch and one
//!   add, the same cost [`crate::tiers::TieredCache::stats`]'s own
//!   counters pay).
//! - The single-flight joins ride the same rules (F33): one
//!   `oxide_cache_plane_coalesce_events_total` counter per completed
//!   flight the host records through
//!   [`crate::tiers::TieredCache::record_coalesced`], and
//!   `oxide_cache_plane_coalesced_followers_total` for the joins — the
//!   push twin of `stats().coalesced`, so the pull and push channels
//!   never disagree.
//! - No error codes: nothing here can fail (the F21 posture — the
//!   registry gains no rows for a seam without refusals).
//! - `render_prometheus` is deterministic: the fixed family order, the
//!   fixed `L0`/`L1`/`L2` label set, one `# HELP`/`# TYPE` counter pair
//!   per family, a trailing newline — two renders over the same counters
//!   are byte-equal, so a snapshot diff of two scrapes is a diff of the
//!   traffic, not of the renderer.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::Layer;
use crate::observer::{CacheEvent, CacheObserver, CacheOp};

/// One relaxed saturating add (the exporter's only counter op): a counter
/// that reaches `u64::MAX` CLAMPS there instead of wrapping — a scrape
/// that reads a saturated counter knows the value is "at or past the
/// bound", never a silently-restarted total.
fn saturating_add(counter: &AtomicU64, value: u64) {
    let _ = counter.try_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
        Some(cur.saturating_add(value))
    });
}

/// The pull mirror of one exporter's counters (the
/// [`crate::single_flight`] posture: `Clone` + `Eq`, value-shaped). A
/// host copies the snapshot before and after a window and diffs the
/// values; the tests compare snapshots, not pointers.
///
/// All fields are monotone counters since the observer was attached (the
/// Prometheus posture — a scrape is a delta over the scraper's window).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CacheMetricsSnapshot {
    /// Lookups served from L0 memory.
    pub hits_l0: u64,
    /// Lookups served from L1 disk.
    pub hits_l1: u64,
    /// Lookups served from L2 remote.
    pub hits_l2: u64,
    /// Hits served stale (inside the SWR window, past the ttl).
    pub stale_hits: u64,
    /// Lookups that exhausted every attached tier.
    pub misses: u64,
    /// Write-through completions.
    pub sets: u64,
    /// Deletes completed across every attached tier.
    pub deletes: u64,
    /// Entry body bytes served on hits.
    pub served_bytes: u64,
    /// Entry body bytes stored by write-through completions.
    pub stored_bytes: u64,
    /// Completed tag invalidations (one aggregate row per call).
    pub invalidations: u64,
    /// Distinct keys the invalidations affected (the count each call
    /// returned).
    pub invalidated_keys: u64,
    /// Completed single-flight flights the host recorded (one aggregate
    /// row per flight, F33).
    pub coalesce_events: u64,
    /// Followers that coalesced onto those flights' owners (the join
    /// count the push twin of `stats().coalesced` sums).
    pub coalesced_followers: u64,
}

/// The metrics exporter: the F28 seam's named structured consumer. Every
/// completed cache event lands in one relaxed atomic add; the scrape
/// reads the counters out as either the value mirror
/// ([`MetricsCacheObserver::snapshot`]) or the §20 Prometheus text
/// exposition ([`MetricsCacheObserver::render_prometheus`]).
///
/// Attach through the F28 builder — the `Arc<MetricsCacheObserver>`
/// coerces to the `Arc<dyn CacheObserver>` the tiers' field stores, so
/// one clone stays with the host task for scraping while the cache owns
/// the seam half:
///
/// ```
/// use std::sync::Arc;
/// use oxide_fw_cache::{MetricsCacheObserver, TieredCache};
///
/// let metrics = Arc::new(MetricsCacheObserver::new());
/// let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(metrics.clone()));
/// // … the pipeline serves through `cache`; the host scrapes `metrics`:
/// let text = metrics.render_prometheus();
/// assert!(text.contains("oxide_cache_plane_hits_total{layer=\"L0\"} 0"));
/// ```
#[derive(Debug, Default)]
pub struct MetricsCacheObserver {
    hits_l0: AtomicU64,
    hits_l1: AtomicU64,
    hits_l2: AtomicU64,
    stale_hits: AtomicU64,
    misses: AtomicU64,
    sets: AtomicU64,
    deletes: AtomicU64,
    served_bytes: AtomicU64,
    stored_bytes: AtomicU64,
    invalidations: AtomicU64,
    invalidated_keys: AtomicU64,
    coalesce_events: AtomicU64,
    coalesced_followers: AtomicU64,
}

impl MetricsCacheObserver {
    /// A fresh exporter: every counter at zero, ready to attach.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            hits_l0: AtomicU64::new(0),
            hits_l1: AtomicU64::new(0),
            hits_l2: AtomicU64::new(0),
            stale_hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            sets: AtomicU64::new(0),
            deletes: AtomicU64::new(0),
            served_bytes: AtomicU64::new(0),
            stored_bytes: AtomicU64::new(0),
            invalidations: AtomicU64::new(0),
            invalidated_keys: AtomicU64::new(0),
            coalesce_events: AtomicU64::new(0),
            coalesced_followers: AtomicU64::new(0),
        }
    }

    /// The value mirror of the counters (the diff-friendly pull shape;
    /// the module doc's contract).
    #[must_use]
    pub fn snapshot(&self) -> CacheMetricsSnapshot {
        let load = |c: &AtomicU64| c.load(Ordering::Relaxed);
        CacheMetricsSnapshot {
            hits_l0: load(&self.hits_l0),
            hits_l1: load(&self.hits_l1),
            hits_l2: load(&self.hits_l2),
            stale_hits: load(&self.stale_hits),
            misses: load(&self.misses),
            sets: load(&self.sets),
            deletes: load(&self.deletes),
            served_bytes: load(&self.served_bytes),
            stored_bytes: load(&self.stored_bytes),
            invalidations: load(&self.invalidations),
            invalidated_keys: load(&self.invalidated_keys),
            coalesce_events: load(&self.coalesce_events),
            coalesced_followers: load(&self.coalesced_followers),
        }
    }

    /// The §20 Prometheus text exposition (the request plane's
    /// `oxide_fw_server::metrics::Metrics::render` shape): one
    /// `# HELP`/`# TYPE` counter pair per family, the hits family split
    /// per serving layer over the fixed `L0`/`L1`/`L2` label set, a
    /// deterministic row order, a trailing newline. Infallible — the
    /// `String` building cannot fail — so the scrape path carries no
    /// error shape (the F21 posture: no registry rows for a seam that
    /// cannot refuse).
    #[must_use]
    pub fn render_prometheus(&self) -> String {
        let load = |c: &AtomicU64| c.load(Ordering::Relaxed);
        // Capacity: 9 families × ~100 bytes of HELP/TYPE/rows + the three
        // labeled hit rows — one allocation for a full scrape.
        let mut out = String::with_capacity(1024);

        let family = |out: &mut String, name: &str, help: &str, value: u64| {
            let _ = writeln!(out, "# HELP {name} {help}");
            let _ = writeln!(out, "# TYPE {name} counter");
            let _ = writeln!(out, "{name} {value}");
        };
        // The hits family has no bare row — it is fully labeled (the
        // fixed L0/L1/L2 serving-tier set), so only the HELP/TYPE pair
        // precedes the rows.
        let _ = writeln!(
            out,
            "# HELP oxide_cache_plane_hits_total Cache-plane lookups served, by serving tier (§11.2)"
        );
        let _ = writeln!(out, "# TYPE oxide_cache_plane_hits_total counter");
        for (layer, counter) in [
            ("L0", &self.hits_l0),
            ("L1", &self.hits_l1),
            ("L2", &self.hits_l2),
        ] {
            let _ = writeln!(
                out,
                "oxide_cache_plane_hits_total{{layer=\"{layer}\"}} {}",
                load(counter)
            );
        }
        family(
            &mut out,
            "oxide_cache_plane_stale_hits_total",
            "Cache-plane hits served stale (the SWR window, §11.3)",
            load(&self.stale_hits),
        );
        family(
            &mut out,
            "oxide_cache_plane_misses_total",
            "Cache-plane lookups that exhausted every attached tier",
            load(&self.misses),
        );
        family(
            &mut out,
            "oxide_cache_plane_sets_total",
            "Cache-plane write-through completions",
            load(&self.sets),
        );
        family(
            &mut out,
            "oxide_cache_plane_served_bytes_total",
            "Cache-plane entry body bytes served on hits",
            load(&self.served_bytes),
        );
        family(
            &mut out,
            "oxide_cache_plane_stored_bytes_total",
            "Cache-plane entry body bytes stored by write-through",
            load(&self.stored_bytes),
        );
        family(
            &mut out,
            "oxide_cache_plane_deletes_total",
            "Cache-plane deletes completed across every attached tier",
            load(&self.deletes),
        );
        family(
            &mut out,
            "oxide_cache_plane_invalidations_total",
            "Cache-plane tag invalidations completed (one row per call)",
            load(&self.invalidations),
        );
        family(
            &mut out,
            "oxide_cache_plane_invalidated_keys_total",
            "Cache-plane keys affected by tag invalidations",
            load(&self.invalidated_keys),
        );
        family(
            &mut out,
            "oxide_cache_plane_coalesce_events_total",
            "Cache-plane single-flight flights the host recorded",
            load(&self.coalesce_events),
        );
        family(
            &mut out,
            "oxide_cache_plane_coalesced_followers_total",
            "Cache-plane followers that coalesced onto flight owners",
            load(&self.coalesced_followers),
        );

        out
    }
}

impl CacheObserver for MetricsCacheObserver {
    fn on_event(&self, event: &CacheEvent<'_>) {
        // One relaxed saturating add per arm: the callback runs ON the
        // cache's call path (a hot L0 hit pays one branch + one add), the
        // observer contract's "cheap and never panics" rule. Every counter
        // saturates at `u64::MAX` — a scrape-honest row, never a wrap.
        match event.op {
            CacheOp::Hit {
                layer,
                stale,
                bytes,
            } => {
                let counter = match layer {
                    Layer::L0 => &self.hits_l0,
                    Layer::L1 => &self.hits_l1,
                    Layer::L2 => &self.hits_l2,
                };
                saturating_add(counter, 1);
                saturating_add(&self.served_bytes, bytes);
                if stale {
                    saturating_add(&self.stale_hits, 1);
                }
            }
            CacheOp::Miss => {
                saturating_add(&self.misses, 1);
            }
            CacheOp::Set { bytes } => {
                saturating_add(&self.sets, 1);
                saturating_add(&self.stored_bytes, bytes);
            }
            CacheOp::Delete => {
                saturating_add(&self.deletes, 1);
            }
            CacheOp::Invalidate { count, .. } => {
                saturating_add(&self.invalidations, 1);
                // The affected-key count is a usize; the counter is a u64.
                // `try_from` keeps the arm cast-free (a negative count is
                // not a real traffic shape — the row still counts as one
                // invalidation, with no keys fabricated).
                if let Ok(keys) = u64::try_from(count) {
                    saturating_add(&self.invalidated_keys, keys);
                }
            }
            CacheOp::Coalesce { followers } => {
                saturating_add(&self.coalesce_events, 1);
                saturating_add(&self.coalesced_followers, followers);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_exporter_renders_the_zero_scrape() {
        let metrics = MetricsCacheObserver::new();
        let snap = metrics.snapshot();
        assert_eq!(snap, CacheMetricsSnapshot::default());
        // Every family renders its zero row (the dashboard's
        // register-before-traffic shape) and the hit rows split L0/L1/L2.
        let out = metrics.render_prometheus();
        assert!(out.contains("oxide_cache_plane_hits_total{layer=\"L0\"} 0\n"));
        assert!(out.contains("oxide_cache_plane_hits_total{layer=\"L1\"} 0\n"));
        assert!(out.contains("oxide_cache_plane_hits_total{layer=\"L2\"} 0\n"));
        assert!(out.contains("oxide_cache_plane_misses_total 0\n"));
        assert_eq!(out, metrics.render_prometheus());
    }

    #[test]
    fn saturating_byte_accounting_never_wraps() {
        let metrics = MetricsCacheObserver::new();
        metrics.on_event(&CacheEvent {
            key: "k",
            op: CacheOp::Set {
                bytes: u64::MAX - 1,
            },
        });
        metrics.on_event(&CacheEvent {
            key: "k",
            op: CacheOp::Set { bytes: 100 },
        });
        let snap = metrics.snapshot();
        assert_eq!(snap.stored_bytes, u64::MAX, "saturated, not wrapped");
        assert_eq!(snap.sets, 2);
    }
}

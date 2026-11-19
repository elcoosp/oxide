//! The cache plane's posture/health pull seam: one [`CacheHealth`] value
//! describing the whole tiered stack as it stands right now, beside the
//! two pulls already in the crate — [`crate::tiers::TieredCache::stats`]
//! (the per-invocation counters) and the F30 exporter (the per-event
//! aggregates scraped through `render_prometheus`) (§11.2, §20).
//!
//! ## Why the seam exists
//!
//! The plane's existing answers are partial on purpose: `stats()` counts
//! events (hits, misses, coalesced) and the memory plane's occupancy, and
//! the F30 exporter aggregates events into monotone counters. Neither
//! answers the question an operator's probe asks at boot and on every
//! health check: **what is this cache stack** — is the disk tier attached
//! and how many bytes is it holding, is a remote tier attached (and which
//! driver), how much memory headroom is left, how much tag-index pressure
//! has accumulated, is the observability seam itself wired? Fabricating
//! those from event counters means replaying every event since attach —
//! the posture is a fact of the tiers, not of the traffic.
//!
//! ## The contract
//!
//! - One value, one pass: [`TieredCache::health`] samples every tier's
//!   live state into the value-shaped [`CacheHealth`] (`Clone` + `Eq` +
//!   `Default`, the [`crate::single_flight`] posture) — a host diffs two
//!   snapshots, a probe serializes one.
//! - Nothing is fabricated: an unattached tier reads `false` / `0` /
//!   `None`, never a zeroed stand-in for a real tier. The remote tier is
//!   an opaque `Arc<dyn CacheHandler>` — the posture names it through the
//!   [`crate::CacheHandler::handler_kind`] introspection method (the
//!   defaulted trait method: `"custom"` for host drivers that do not
//!   override, the crate's own drivers name themselves).
//! - `render_prometheus` is the §20 text exposition in the F30 exporter's
//!   shape — `# HELP`/`# TYPE` pairs, the fixed family order, a trailing
//!   newline, two renders over the same posture byte-equal — with
//!   **`gauge`** TYPEs: a scrape reads current posture, not deltas since
//!   attach, so the exporter's monotone-counter contract stays distinct
//!   (the `oxide_cache_plane_*` prefix is shared; the metric kind is what
//!   separates the two halves).
//! - No error codes: the seam is infallible (the F21 posture — the
//!   registry gains no rows for a pull that cannot refuse).

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// The whole-stack posture of one [`TieredCache`](crate::tiers::TieredCache)
/// (§11.2): tier attachment, resident occupancy, tag-index pressure, and
/// the observability seam's own attachment state.
///
/// Value-shaped by design (`Clone` + `Eq` + `Default` + serde): a probe
/// copies the snapshot, a dashboard diffs two, a test pins one — nothing
/// holds a borrow on the cache.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CacheHealth {
    /// Entries resident in the L0 memory tier right now (expired entries
    /// stay resident until the lazy delete — freshness is classified at
    /// read time, §11.3).
    pub memory_entries: u64,
    /// Byte weight the memory tier holds (the moka weighted size, ≤ the
    /// capacity after any insert completes).
    pub memory_bytes: u64,
    /// The memory tier's configured byte cap (`MemoryTier::max_bytes`).
    pub memory_capacity_bytes: u64,
    /// Whether a local disk tier (L1) is attached.
    pub disk_attached: bool,
    /// Bytes the disk tier's index accounts for (`0` when unattached).
    pub disk_bytes: u64,
    /// Whether a remote tier (L2) is attached.
    pub remote_attached: bool,
    /// The attached remote driver's self-reported
    /// [`handler_kind`](crate::CacheHandler::handler_kind) (`None` when
    /// unattached — no fabricated kind).
    pub remote_kind: Option<String>,
    /// Distinct tags the tag index maps to at least one key.
    pub tags_indexed: u64,
    /// Distinct keys the reverse tag index tracks.
    pub keys_tracked: u64,
    /// Whether the §20 observability seam has an observer attached
    /// (`with_observer`) — the posture of the seam itself.
    pub observer_attached: bool,
    /// F52: evictions the disk tier's S-08 prune completed since the
    /// tier opened — the same counter `stats().evictions` and the push
    /// seam's `Evict` rows answer, sampled into the posture. Honestly
    /// zero without a disk tier (no tier, no counted eviction); the
    /// serde default keeps pre-F52 probe payloads decoding.
    #[serde(default)]
    pub disk_evictions: u64,
}

impl CacheHealth {
    /// The §20 Prometheus text exposition in the F30 exporter's shape: one
    /// `# HELP`/`# TYPE` pair per family, the fixed family order, a
    /// trailing newline — two renders over the same posture are
    /// byte-equal, so a scraper diffs postures, not renderers.
    ///
    /// Every family is a **gauge**, not the exporter's counters: a scrape
    /// reads the stack's current posture (a boot probe re-reads the same
    /// families and overwrites its series), never a delta since attach.
    /// The unattached tiers render `0` for bytes and `0` for the attached
    /// flag — the flag is what says "this tier is not there", so no row
    /// fabricates a tier that is not.
    ///
    /// The remote driver's kind is intentionally NOT a label: the kind is
    /// a per-stack constant (`remote_kind` on the snapshot carries it),
    /// and a label would join the exposition's label set to the driver
    /// taxonomy — the numeric exposition stays stable when a host swaps
    /// `http` for `s3` underneath.
    ///
    /// Infallible — the `String` building cannot fail — so the probe path
    /// carries no error shape (the F21 posture: no registry rows for a
    /// seam that cannot refuse).
    #[must_use]
    pub fn render_prometheus(&self) -> String {
        // Ten families × ~110 bytes of HELP/TYPE/row — one allocation
        // for a full posture scrape.
        let mut out = String::with_capacity(1200);

        let family = |out: &mut String, name: &str, help: &str, value: u64| {
            let _ = writeln!(out, "# HELP {name} {help}");
            let _ = writeln!(out, "# TYPE {name} gauge");
            let _ = writeln!(out, "{name} {value}");
        };
        let flag = |out: &mut String, name: &str, help: &str, on: bool| {
            family(out, name, help, u64::from(on));
        };

        family(
            &mut out,
            "oxide_cache_plane_memory_entries",
            "Entries resident in the L0 memory tier (§11.2)",
            self.memory_entries,
        );
        family(
            &mut out,
            "oxide_cache_plane_memory_bytes",
            "Byte weight the L0 memory tier holds",
            self.memory_bytes,
        );
        family(
            &mut out,
            "oxide_cache_plane_memory_capacity_bytes",
            "Configured byte cap of the L0 memory tier",
            self.memory_capacity_bytes,
        );
        flag(
            &mut out,
            "oxide_cache_plane_disk_attached",
            "1 when a local disk tier (L1) is attached",
            self.disk_attached,
        );
        family(
            &mut out,
            "oxide_cache_plane_disk_bytes",
            "Bytes the L1 disk tier's index accounts for (0 when unattached)",
            self.disk_bytes,
        );
        flag(
            &mut out,
            "oxide_cache_plane_remote_attached",
            "1 when a remote tier (L2) is attached",
            self.remote_attached,
        );
        family(
            &mut out,
            "oxide_cache_plane_tags_indexed",
            "Distinct tags the tag index maps to at least one key (§11.3)",
            self.tags_indexed,
        );
        family(
            &mut out,
            "oxide_cache_plane_keys_tracked",
            "Distinct keys the reverse tag index tracks",
            self.keys_tracked,
        );
        flag(
            &mut out,
            "oxide_cache_plane_observer_attached",
            "1 when the §20 observability seam has an observer attached",
            self.observer_attached,
        );
        // F52: the eviction gauge APPENDS after the flags — the F33
        // order rule (new families render after the last one, so a
        // scraper's offsets for the existing families never move). A
        // gauge, not the exporter's counter: the posture reads the disk
        // tier's current eviction total, the exporter counts the events.
        family(
            &mut out,
            "oxide_cache_plane_disk_evictions",
            "Entries the L1 disk tier's S-08 budget prune evicted since it opened",
            self.disk_evictions,
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_posture_renders_the_zeroed_exposition() {
        let text = CacheHealth::default().render_prometheus();
        assert!(text.ends_with('\n'));
        // Every family present, all zeros, the attachment flags honest.
        assert!(text.contains("oxide_cache_plane_memory_entries 0"));
        assert!(text.contains("oxide_cache_plane_memory_capacity_bytes 0"));
        assert!(text.contains("oxide_cache_plane_disk_attached 0"));
        assert!(text.contains("oxide_cache_plane_remote_attached 0"));
        assert!(text.contains("oxide_cache_plane_observer_attached 0"));
        // Gauge TYPEs only — the counter families live on the exporter.
        assert!(text.contains("# TYPE oxide_cache_plane_memory_bytes gauge"));
        assert!(!text.contains(" counter\n"));
        // Byte-equal determinism.
        assert_eq!(text, CacheHealth::default().render_prometheus());
    }

    #[test]
    fn a_populated_posture_renders_the_live_rows() {
        let h = CacheHealth {
            memory_entries: 2,
            memory_bytes: 512,
            memory_capacity_bytes: 1 << 20,
            disk_attached: true,
            disk_bytes: 4096,
            remote_attached: true,
            remote_kind: Some("http".into()),
            tags_indexed: 3,
            keys_tracked: 5,
            observer_attached: true,
            disk_evictions: 0,
        };
        let text = h.render_prometheus();
        assert!(text.contains("oxide_cache_plane_memory_entries 2"));
        assert!(text.contains("oxide_cache_plane_memory_bytes 512"));
        assert!(text.contains("oxide_cache_plane_disk_attached 1"));
        assert!(text.contains("oxide_cache_plane_disk_bytes 4096"));
        assert!(text.contains("oxide_cache_plane_remote_attached 1"));
        assert!(text.contains("oxide_cache_plane_tags_indexed 3"));
        assert!(text.contains("oxide_cache_plane_keys_tracked 5"));
        assert!(text.contains("oxide_cache_plane_observer_attached 1"));
    }

    #[test]
    fn the_snapshot_round_trips_through_json() {
        let h = CacheHealth {
            remote_attached: true,
            remote_kind: Some("s3".into()),
            ..CacheHealth::default()
        };
        let json = serde_json::to_string(&h).expect("a posture serializes");
        let back: CacheHealth = serde_json::from_str(&json).expect("a posture deserializes");
        assert_eq!(back, h);
        assert!(json.contains("\"remote_kind\":\"s3\""));
    }
}

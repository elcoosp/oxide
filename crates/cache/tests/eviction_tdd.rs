//! TDD suite for the cache plane's eviction story (F51/F52). Written
//! first per the house TDD rule: every pin below failed on unresolved
//! symbols (E0599 over `CacheOp::Evict`, E0609/E0599 over the
//! `evictions`/`disk_evictions` reads) before the seam existed.
//!
//! The contract the pins hold:
//!
//! 1. (F51) The S-08 eviction is a completed operation the observability
//!    seam can finally hear: `CacheOp::Evict { layer, bytes }` names the
//!    tier that freed the entry and the size it freed, the stable kind
//!    name is `"evict"`, and every shipped consumer takes the row (the
//!    tracing observer, the exporter's counters, the feed, the ledger).
//! 2. (F51) The disk tier counts its own prunes: saturating counters,
//!    exact per-victim accounting, exposed as pull reads and through
//!    `CacheStats.evictions`. An explicit delete and a corrupt-miss
//!    self-heal are NOT evictions — the caller asked for the first, the
//!    second is §17.2's documented silent self-healing.
//! 3. (F51) The exporter's exposition grows by APPEND (the F33 order
//!    rule): the two eviction families render after the coalesce
//!    families, the fixed order before them untouched.
//! 4. (F52) The health pull carries the disk tier's eviction total —
//!    `CacheHealth::disk_evictions` — honestly zero without a disk tier,
//!    riding the probe JSON and the tenth gauge family of
//!    `render_prometheus` (appended after `observer_attached`).

use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use oxide_fw_cache::{
    CacheEvent, CacheHealth, CacheObserver, CacheOp, CacheStats, DiskTier, Entry, EventFeed,
    HotKeyGate as _, HotKeyKindGate, HotKeyObserver, Layer, MetricsCacheObserver, Policy,
    TieredCache,
};

fn entry(body: &[u8], tags: &[&str], stored_at: u64) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: tags.iter().copied().map(str::to_string).collect(),
        policy: Policy {
            ttl: Duration::from_secs(60),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at,
        etag: "\"v1\"".into(),
    }
}

/// The eviction-row recorder: every op payload it receives, tagged with
/// the event key so the pin can tell the victim from the writer.
struct Recorder(Arc<Mutex<Vec<(String, CacheOp)>>>);

impl CacheObserver for Recorder {
    fn on_event(&self, event: &CacheEvent<'_>) {
        self.0
            .lock()
            .expect("the payload log")
            .push((event.key.to_string(), event.op.clone()));
    }
}

fn recorded(log: &[(String, CacheOp)]) -> Vec<(String, CacheOp)> {
    log.to_vec()
}

// ------------------------------------------------------- F51: the eviction event

/// The disk tier counts its S-08 prune victims exactly. A zero-budget
/// store (`max_bytes` = 1) prunes every write it just completed — the
/// honest steady state where each `set` is also one eviction — so two
/// sets answer `evictions() == 2` with every byte accounted, and both
/// keys are gone (the eviction really happened, not just counted).
#[test]
fn the_disk_tier_counts_its_prune_victims() {
    let dir = tempfile::tempdir().unwrap();
    let disk = DiskTier::open_with_budget(dir.path().join("tier"), 1, 1 << 20).unwrap();
    disk.set("k1", &entry(b"payload-one", &[], 1)).unwrap();
    disk.set("k2", &entry(b"payload-two", &[], 2)).unwrap();
    assert_eq!(disk.evictions(), 2, "one prune victim per over-budget set");
    assert!(
        disk.evicted_bytes() > 0,
        "the freed compressed sizes ride the counter"
    );
    assert!(
        disk.get("k1").unwrap().is_none(),
        "the first victim is gone from the store"
    );
    assert!(
        disk.get("k2").unwrap().is_none(),
        "the second victim is gone from the store"
    );
}

/// The eviction event rides the F28 seam: a tiered cache whose disk tier
/// prunes emits one `Evict { layer: L1, bytes }` row per victim through
/// the ONE observer slot, and the row lands BEFORE the set's own row for
/// the incoming key (the eviction is part of completing the write). The
/// bytes are the disk tier's own accounting unit — the compressed blob
/// size the store freed — so they are honest and positive.
#[tokio::test]
async fn the_evict_event_rides_the_seam_with_its_freed_bytes() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let dir = tempfile::tempdir().unwrap();
    // A zero-budget DISK tier is what prunes: the tiered cache rides a
    // one-byte disk tier, so every write-through prunes past the budget —
    // and with a one-byte budget the entry the set just wrote is its own
    // victim (insert, then prune to target 0). Three sets answer THREE
    // evictions and the disk ends empty; the memory cap stays roomy so
    // the tiered cache itself never refuses.
    let disk = Arc::new(DiskTier::open_with_budget(dir.path().join("zero"), 1, 1 << 20).unwrap());
    let cache = TieredCache::new(64 * 1024 * 1024)
        .with_disk_tier(disk)
        .with_observer(Arc::new(Recorder(Arc::clone(&log))));
    for i in 0..3u64 {
        cache
            .set(
                &format!("route:/k{i}"),
                entry(b"steady state", &[], 1_000 + i),
            )
            .await
            .unwrap();
    }
    let rows = recorded(&log.lock().unwrap());
    let evicts: Vec<&(String, CacheOp)> = rows
        .iter()
        .filter(|(_, op)| matches!(op, CacheOp::Evict { .. }))
        .collect();
    assert_eq!(
        evicts.len(),
        3,
        "each over-budget set's prune emitted one row (the entry it just wrote is its own victim)"
    );
    for (key, op) in &evicts {
        assert!(
            key.starts_with("route:/k"),
            "the victim's key rides the row: {key}"
        );
        let CacheOp::Evict { layer, bytes } = op else {
            unreachable!("filtered above")
        };
        assert_eq!(*layer, Layer::L1, "the disk tier names itself");
        assert!(*bytes > 0, "the freed size is the tier's honest account");
    }
    // The eviction rows land before the set rows: the S-08 prune is part
    // of completing the write that triggered it (the victim's row is
    // delivered before the writer's own `set` row).
    let evict_first = rows
        .iter()
        .position(|(_, op)| matches!(op, CacheOp::Evict { .. }))
        .unwrap();
    let third_set = rows
        .iter()
        .position(|(key, op)| key == "route:/k2" && matches!(op, CacheOp::Set { .. }))
        .unwrap();
    assert!(
        evict_first < third_set,
        "the first eviction completed before the write that followed it"
    );
}

/// The shipped consumers take the row: the exporter counts the evictions
/// and the bytes they freed into the snapshot mirror, and the exposition
/// renders the two families — APPENDED after the coalesce families (the
/// F33 order rule), counter TYPEs, byte-stable re-renders.
#[test]
fn the_exporter_counts_evictions_and_renders_the_families() {
    let metrics = MetricsCacheObserver::new();
    metrics.on_event(&CacheEvent {
        key: "route:/old",
        op: CacheOp::Evict {
            layer: Layer::L1,
            bytes: 512,
        },
    });
    metrics.on_event(&CacheEvent {
        key: "route:/older",
        op: CacheOp::Evict {
            layer: Layer::L1,
            bytes: 256,
        },
    });
    let snap = metrics.snapshot();
    assert_eq!(snap.evictions, 2, "the counter mirror carries the rows");
    assert_eq!(snap.evicted_bytes, 768, "the freed bytes sum");
    let out = metrics.render_prometheus();
    assert!(
        out.contains("# TYPE oxide_cache_plane_evictions_total counter"),
        "the eviction family renders as a counter"
    );
    assert!(out.contains("oxide_cache_plane_evictions_total 2\n"));
    assert!(out.contains("oxide_cache_plane_evicted_bytes_total 768\n"));
    // Append order: the eviction families render after the coalesce
    // families — the F33 families' offsets stay where their consumers
    // expect them.
    let coalesce_at = out
        .find("# HELP oxide_cache_plane_coalesced_followers_total")
        .expect("the coalesce families render first");
    let evict_at = out
        .find("# HELP oxide_cache_plane_evictions_total")
        .expect("the eviction families render");
    assert!(
        coalesce_at < evict_at,
        "the new families append, never reorder"
    );
    assert_eq!(out, metrics.render_prometheus(), "byte-stable re-render");
}

/// The pull channel agrees with the push channel: `CacheStats.evictions`
/// reads the disk tier's counter (zero without a disk tier — the
/// memory-only tier honestly evicts nothing IT counts), and the count
/// matches what the exporter saw over the same traffic.
#[tokio::test]
async fn cache_stats_evictions_rides_the_pull_channel() {
    let metrics = Arc::new(MetricsCacheObserver::new());
    let dir = tempfile::tempdir().unwrap();
    // The zero-budget disk tier: every write-through prunes past the
    // budget (with a one-byte budget the just-written entry is its own
    // victim), so the steady-state prune count is one per set.
    let disk = Arc::new(DiskTier::open_with_budget(dir.path().join("zero"), 1, 1 << 20).unwrap());
    let cache = TieredCache::new(64 * 1024 * 1024)
        .with_disk_tier(disk)
        .with_observer(metrics.clone());
    for i in 0..3u64 {
        cache
            .set(
                &format!("route:/p{i}"),
                entry(b"pull channel", &[], 2_000 + i),
            )
            .await
            .unwrap();
    }
    let s = cache.stats();
    assert_eq!(
        s.evictions, 3,
        "the pull counter matches the steady-state prune count"
    );
    assert_eq!(
        metrics.snapshot().evictions,
        3,
        "the push channel counted the same rows"
    );
    // The memory-only tier answers the honest zero.
    let memory_only = TieredCache::new(1 << 20);
    let s = memory_only.stats();
    assert_eq!(s.evictions, 0, "no disk tier, no counted eviction");
}

/// An explicit delete is NOT an eviction: the caller asked for it, the
/// row it emits is `Delete`, and the disk tier's eviction counters stay
/// at zero when the budget is never crossed.
#[tokio::test]
async fn an_explicit_delete_is_not_an_eviction() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let cache = TieredCache::new(64 * 1024 * 1024)
        .with_disk(tempfile::tempdir().unwrap().path().join("tier"))
        .unwrap()
        .with_observer(Arc::new(Recorder(Arc::clone(&log))));
    cache
        .set("route:/a", entry(b"roomy budget", &[], 3_000))
        .await
        .unwrap();
    cache.delete("route:/a").await.unwrap();
    let rows = recorded(&log.lock().unwrap());
    assert!(
        rows.iter()
            .any(|(key, op)| key == "route:/a" && matches!(op, CacheOp::Delete)),
        "the explicit delete emitted its own row"
    );
    assert!(
        !rows
            .iter()
            .any(|(_, op)| matches!(op, CacheOp::Evict { .. })),
        "a roomy-budget delete never counts as an eviction"
    );
    assert_eq!(cache.stats().evictions, 0, "the pull counter stays zero");
}

/// The stable kind name is `"evict"` and the row flows through the
/// shipped consumers' kind-driven contracts: the feed retains it, the
/// ledger attributes it to its key, and the kind gates can NAME it.
#[test]
fn the_evict_kind_name_is_stable_and_kind_driven_consumers_take_it() {
    let evict = CacheEvent {
        key: "route:/hot",
        op: CacheOp::Evict {
            layer: Layer::L1,
            bytes: 64,
        },
    };
    assert_eq!(evict.kind(), "evict", "the stable row name");
    // The bounded feed retains the row (the heavy consumer's replay).
    let feed = EventFeed::bounded(8);
    feed.on_event(&evict);
    let rows = feed.drain();
    assert_eq!(rows.len(), 1, "the feed kept the eviction row");
    assert_eq!(rows[0].0, "route:/hot", "the victim's key rides the row");
    assert!(
        matches!(rows[0].1, CacheOp::Evict { .. }),
        "the retained row is the eviction payload"
    );
    // The ledger attributes the eviction to its key.
    let hot = HotKeyObserver::bounded(8);
    hot.on_event(&evict);
    assert_eq!(hot.count("route:/hot"), Some(1), "the eviction attributed");
    // A kind gate can name the eviction row explicitly.
    let gate = HotKeyKindGate::only(&["evict"]);
    assert!(gate.admits(&evict), "the gate admits the kind it names");
    assert!(
        !gate.admits(&CacheEvent {
            key: "route:/hot",
            op: CacheOp::Miss,
        }),
        "and refuses the kinds it does not"
    );
}

/// The tracing observer takes the eviction row like every other (the
/// takes-every-row posture: with no subscriber installed the row is a
/// no-op, never a panic on the cache's call path).
#[test]
fn the_tracing_observer_takes_the_evict_row() {
    oxide_fw_cache::TracingCacheObserver::new().on_event(&CacheEvent {
        key: "route:/old",
        op: CacheOp::Evict {
            layer: Layer::L1,
            bytes: 1,
        },
    });
}

/// The pull value stays wire-honest: `CacheStats` serializes with the
/// evictions field, and an OLD payload that predates the field decodes
/// with the honest zero (the serde default keeps stored snapshots
/// readable across the upgrade).
#[test]
fn stats_json_round_trips_with_the_evictions_field() {
    let s = CacheStats {
        hits: 1,
        misses: 2,
        evictions: 7,
        ..Default::default()
    };
    let v = serde_json::to_value(s).unwrap();
    assert_eq!(v["evictions"], 7, "the field rides the JSON shape");
    let back: CacheStats = serde_json::from_value(v).unwrap();
    assert_eq!(back.evictions, 7);
    // The backward-compat arm: a pre-F51 payload has no evictions key.
    let old: CacheStats = serde_json::from_value(json!({
        "hits": 3, "misses": 0, "stale_served": 0, "coalesced": 0,
        "entries_memory": 0, "bytes_memory": 0
    }))
    .expect("an old payload decodes");
    assert_eq!(old.evictions, 0, "the honest zero for pre-F51 payloads");
}

// ------------------------------------------------- F52: the health pull rides

/// The health pull carries the disk tier's eviction total: the same
/// counter the push seam and `stats()` answer, sampled into the probe's
/// value shape — honestly zero without a disk tier, never fabricated.
#[tokio::test]
async fn the_health_pull_carries_the_disk_eviction_total() {
    let dir = tempfile::tempdir().unwrap();
    // The zero-budget disk tier: every write-through prunes past the
    // budget (the just-written entry is its own victim), so two evictions
    // ride two sets — and the health pull samples the same counter.
    let disk = Arc::new(DiskTier::open_with_budget(dir.path().join("zero"), 1, 1 << 20).unwrap());
    let cache = TieredCache::new(64 * 1024 * 1024).with_disk_tier(disk);
    for i in 0..2u64 {
        cache
            .set(
                &format!("route:/h{i}"),
                entry(b"health pull", &[], 4_000 + i),
            )
            .await
            .unwrap();
    }
    let h = cache.health();
    assert_eq!(
        h.disk_evictions, 2,
        "the posture samples the disk tier's counter"
    );
    // The memory-only stack answers the honest zero.
    let memory_only = TieredCache::new(1 << 20);
    assert_eq!(
        memory_only.health().disk_evictions,
        0,
        "no disk tier, no fabricated total"
    );
}

/// The health exposition renders the eviction gauge LAST (the F52 append
/// after `observer_attached`), as a gauge (posture, not deltas — the
/// counter TYPE never leaks into the posture exposition), byte-stable
/// over two renders.
#[tokio::test]
async fn the_health_exposition_renders_the_eviction_gauge_last() {
    let dir = tempfile::tempdir().unwrap();
    let cache = TieredCache::new(64 * 1024 * 1024)
        .with_disk(dir.path().join("tier"))
        .unwrap();
    let text = cache.health().render_prometheus();
    assert!(
        text.contains("# TYPE oxide_cache_plane_disk_evictions gauge"),
        "the eviction total renders as a posture gauge"
    );
    assert!(text.contains("oxide_cache_plane_disk_evictions 0\n"));
    // Append order: the posture's flags close the exposition, the new
    // family renders after them.
    let obs_at = text
        .find("# HELP oxide_cache_plane_observer_attached")
        .expect("the flags render");
    let evict_at = text
        .find("# HELP oxide_cache_plane_disk_evictions")
        .expect("the eviction gauge renders");
    assert!(obs_at < evict_at, "the new gauge appends after the flags");
    assert!(
        !text.contains(" counter\n"),
        "the posture exposition stays gauge-only"
    );
    assert_eq!(text, cache.health().render_prometheus(), "byte-stable");
}

/// The probe JSON shape carries the field — and a payload that predates
/// F52 still decodes (the serde default keeps old probe answers and
/// stored snapshots readable across the upgrade).
#[tokio::test]
async fn health_json_gains_the_disk_evictions_field() {
    let dir = tempfile::tempdir().unwrap();
    let cache = TieredCache::new(1 << 20)
        .with_disk(dir.path().join("tier"))
        .unwrap();
    cache
        .set("route:/a", entry(b"wire", &["blog"], 5_000))
        .await
        .unwrap();
    let h = cache.health();
    let v = serde_json::to_value(&h).unwrap();
    assert_eq!(v["disk_evictions"], 0, "the field rides the JSON shape");
    let back: CacheHealth = serde_json::from_value(v).unwrap();
    assert_eq!(back.disk_evictions, 0);
    // The backward-compat arm: a pre-F52 probe payload has no field.
    let old: CacheHealth = serde_json::from_value(json!({
        "memory_entries": 0, "memory_bytes": 0, "memory_capacity_bytes": 0,
        "disk_attached": false, "disk_bytes": 0,
        "remote_attached": false, "remote_kind": null,
        "tags_indexed": 0, "keys_tracked": 0, "observer_attached": false
    }))
    .expect("an old payload decodes");
    assert_eq!(
        old.disk_evictions, 0,
        "the honest zero for pre-F52 payloads"
    );
}

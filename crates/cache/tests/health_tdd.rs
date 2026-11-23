//! TDD suite for the cache plane's posture/health pull seam (`health.rs`).
//! Written first per the house TDD rule: every pin failed on unresolved
//! symbols (E0432 over `CacheHealth`, E0599 over `health` /
//! `render_prometheus` / `handler_kind`) before the seam existed.
//!
//! The contract the pins hold:
//!
//! 1. [`CacheHealth`](oxide_fw_cache::CacheHealth) is the whole-stack
//!    posture pull beside [`CacheStats`](oxide_fw_cache::CacheStats) (the
//!    per-invocation counters) and the F30 exporter (the per-event
//!    aggregates): which tiers are attached, what each resident tier
//!    holds, the tag-index pressure, and whether the observability seam
//!    itself is attached. One value, read in one pass — a probe asks "what
//!    is this cache stack right now", not "what happened since attach".
//! 2. The L2 remote is an opaque `Arc<dyn CacheHandler>`; the posture
//!    names it through the new
//!    [`CacheHandler::handler_kind`](oxide_fw_cache::CacheHandler::handler_kind)
//!    introspection method (the defaulted trait method — `"custom"` for
//!    handlers that do not override, the crate's own drivers name
//!    themselves). Nothing is fabricated: an unattached tier reads
//!    `false` / `None`, never a zeroed stand-in.
//! 3. `TieredCache::health()` agrees with `stats()` on the memory plane
//!    (entries + bytes) — the two pulls sample the same live counters.
//! 4. `render_prometheus` is the §20 text exposition in the F30 shape —
//!    `# HELP`/`# TYPE` pairs, the fixed family order, a trailing newline,
//!    two renders over the same posture byte-equal — with `gauge` TYPEs:
//!    a scrape reads CURRENT posture, not deltas since attach.
//! 5. The snapshot round-trips through serde (the `CacheStats` posture —
//!    `oxide cache stats` serializes the pull; the posture rides the same
//!    wire).

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oxide_fw_cache::{CacheHandler, CacheObserver, Entry, HttpCacheHandler, Policy, TieredCache};
use serde_json::json;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn entry(body: &[u8], tags: &[&str]) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: tags.iter().map(std::string::ToString::to_string).collect(),
        policy: Policy {
            ttl: Duration::from_secs(60),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: now_ms(),
        etag: "\"v1\"".into(),
    }
}

/// A minimal custom handler: the default `handler_kind` contract's other
/// side — a host-built driver that does not override the introspection
/// method reads as `"custom"`, honestly unnamed.
struct CustomHandler;

#[async_trait::async_trait]
impl CacheHandler for CustomHandler {
    async fn get(&self, _key: &str) -> oxide_fw_cache::Result<Option<Entry>> {
        Ok(None)
    }
    async fn set(&self, _key: &str, _entry: Entry) -> oxide_fw_cache::Result<()> {
        Ok(())
    }
    async fn delete(&self, _key: &str) -> oxide_fw_cache::Result<()> {
        Ok(())
    }
    async fn tag_keys(&self, _tag: &str) -> oxide_fw_cache::Result<Vec<String>> {
        Ok(Vec::new())
    }
    async fn invalidate_tag(&self, _tag: &str, _soft: bool) -> oxide_fw_cache::Result<usize> {
        Ok(0)
    }
    async fn publish_invalidation(&self, _tag: &str, _soft: bool) -> oxide_fw_cache::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------- the posture pins

#[tokio::test]
async fn default_posture_reports_an_empty_memory_only_cache() {
    let cache = TieredCache::new(1 << 20);
    let h = cache.health();
    assert_eq!(h.memory_entries, 0);
    assert_eq!(h.memory_bytes, 0);
    assert_eq!(h.memory_capacity_bytes, 1 << 20);
    assert!(!h.disk_attached);
    assert_eq!(h.disk_bytes, 0);
    assert!(!h.remote_attached);
    assert_eq!(h.remote_kind, None);
    assert_eq!(h.tags_indexed, 0);
    assert_eq!(h.keys_tracked, 0);
    assert!(!h.observer_attached);
}

#[tokio::test]
async fn memory_occupancy_agrees_with_the_stats_pull() {
    let cache = TieredCache::new(1 << 20);
    cache
        .set("route:/a", entry(b"aaaaaaaa", &[]))
        .await
        .unwrap();
    cache
        .set("route:/b", entry(b"bbbbbbbbbbbb", &[]))
        .await
        .unwrap();
    let h = cache.health();
    assert_eq!(h.memory_entries, 2);
    // The two pulls sample the same live counters (the exact pin).
    let s = cache.stats();
    assert_eq!(h.memory_entries, s.entries_memory);
    assert_eq!(h.memory_bytes, s.bytes_memory);
    assert!(h.memory_bytes > 0);
}

#[tokio::test]
async fn disk_posture_names_attachment_and_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let cache = TieredCache::new(1 << 20)
        .with_disk(dir.path().join("tier"))
        .unwrap();
    let before = cache.health();
    assert!(before.disk_attached);
    assert_eq!(before.disk_bytes, 0);

    cache
        .set("route:/a", entry(b"disk-resident", &[]))
        .await
        .unwrap();
    let after = cache.health();
    assert!(after.disk_attached);
    assert!(after.disk_bytes > 0);
}

#[tokio::test]
async fn remote_posture_names_the_handler_kind() {
    let remote = TieredCache::new_memory();
    let cache = TieredCache::new(1 << 20).with_remote(remote.clone());
    let h = cache.health();
    assert!(h.remote_attached);
    assert_eq!(h.remote_kind.as_deref(), Some("tiered"));
    // The remote's own posture is untouched by the parent's probe.
    assert_eq!(remote.health().memory_entries, 0);
}

#[tokio::test]
async fn tag_index_pressure_is_reported_and_invalidation_decreases_it() {
    let cache = TieredCache::new(1 << 20);
    cache
        .set("route:/a", entry(b"a", &["blog", "v2"]))
        .await
        .unwrap();
    cache.set("route:/b", entry(b"b", &["blog"])).await.unwrap();
    cache.set("route:/c", entry(b"c", &["docs"])).await.unwrap();
    let h = cache.health();
    assert_eq!(h.tags_indexed, 3); // blog, v2, docs
    assert_eq!(h.keys_tracked, 3); // /a, /b, /c

    let affected = cache.invalidate_tag("blog", false).await.unwrap();
    assert_eq!(affected, 2);
    let after = cache.health();
    assert_eq!(after.tags_indexed, 2); // v2, docs
    assert_eq!(after.keys_tracked, 1); // /c
}

#[tokio::test]
async fn observer_attachment_is_honest_posture() {
    let bare = TieredCache::new(1 << 20);
    assert!(!bare.health().observer_attached);

    let observed: Arc<dyn CacheObserver> = Arc::new(oxide_fw_cache::EventFeed::bounded(4));
    let cache = TieredCache::new(1 << 20).with_observer(observed);
    assert!(cache.health().observer_attached);
}

// ---------------------------------------------------------- the introspection pins

#[tokio::test]
async fn handler_kind_names_every_in_crate_driver() {
    assert_eq!(
        HttpCacheHandler::new("http://127.0.0.1:1").handler_kind(),
        "http"
    );
    assert_eq!(TieredCache::new(1 << 20).handler_kind(), "tiered");
}

#[tokio::test]
async fn handler_kind_defaults_to_custom_for_host_drivers() {
    assert_eq!(CustomHandler.handler_kind(), "custom");
}

#[cfg(feature = "redis")]
#[tokio::test]
#[ignore = "requires OXIDE_TEST_REDIS_URL"]
async fn handler_kind_names_the_redis_driver() {
    use oxide_fw_cache::RedisCacheHandler;
    let url = match std::env::var("OXIDE_TEST_REDIS_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("skipped: OXIDE_TEST_REDIS_URL not set");
            return;
        }
    };
    match RedisCacheHandler::new(&url).await {
        Ok(h) => assert_eq!(h.handler_kind(), "redis"),
        Err(e) => panic!("redis handler did not connect: {e}"),
    }
}

#[cfg(feature = "s3")]
#[test]
fn handler_kind_names_the_s3_driver() {
    use oxide_fw_cache::S3CacheHandler;
    // Constructed without IO — the kind is a static fact of the type.
    let h = S3CacheHandler::new("s3://bucket/prefix").expect("construct");
    assert_eq!(h.handler_kind(), "s3");
}

// ---------------------------------------------------------------- the render pins

#[tokio::test]
async fn render_prometheus_is_the_deterministic_gauge_exposition() {
    let cache = TieredCache::new(1 << 20);
    let text = cache.health().render_prometheus();
    // §20 shape: HELP/TYPE pairs, a trailing newline, two renders
    // byte-equal, gauge TYPEs (posture, not deltas).
    assert!(text.ends_with('\n'));
    assert_eq!(text, cache.health().render_prometheus());
    assert!(text.contains("# TYPE oxide_cache_plane_memory_entries gauge"));
    assert!(text.contains("# TYPE oxide_cache_plane_memory_bytes gauge"));
    assert!(text.contains("# TYPE oxide_cache_plane_memory_capacity_bytes gauge"));
    assert!(text.contains("# TYPE oxide_cache_plane_disk_attached gauge"));
    assert!(text.contains("# TYPE oxide_cache_plane_disk_bytes gauge"));
    assert!(text.contains("# TYPE oxide_cache_plane_remote_attached gauge"));
    assert!(text.contains("# TYPE oxide_cache_plane_tags_indexed gauge"));
    assert!(text.contains("# TYPE oxide_cache_plane_keys_tracked gauge"));
    assert!(text.contains("# TYPE oxide_cache_plane_observer_attached gauge"));
    // Fixed order: the memory rows lead, the posture flags close.
    let mem = text.find("oxide_cache_plane_memory_entries").unwrap();
    let obs = text.find("oxide_cache_plane_observer_attached").unwrap();
    assert!(mem < obs);
    // No counter TYPE leaks into the posture exposition (gauge only).
    assert!(!text.contains(" counter\n"));
    // The unattached tiers read zero/zero — not fabricated rows.
    assert!(text.contains("oxide_cache_plane_disk_bytes 0"));
    assert!(text.contains("oxide_cache_plane_disk_attached 0"));
    assert!(text.contains("oxide_cache_plane_remote_attached 0"));
}

#[tokio::test]
async fn render_prometheus_reflects_live_posture() {
    let dir = tempfile::tempdir().unwrap();
    let cache = TieredCache::new(1 << 20)
        .with_disk(dir.path().join("tier"))
        .unwrap();
    cache
        .set("route:/a", entry(b"resident", &["blog"]))
        .await
        .unwrap();
    let text = cache.health().render_prometheus();
    assert!(text.contains("oxide_cache_plane_memory_entries 1"));
    assert!(text.contains("oxide_cache_plane_disk_attached 1"));
    assert!(text.contains("oxide_cache_plane_tags_indexed 1"));
    assert!(text.contains("oxide_cache_plane_keys_tracked 1"));
}

// -------------------------------------------------------------- the wire pins

#[tokio::test]
async fn health_snapshot_round_trips_through_json() {
    let remote = TieredCache::new_memory();
    let cache = TieredCache::new(1 << 20).with_remote(remote);
    cache
        .set("route:/a", entry(b"wire", &["blog"]))
        .await
        .unwrap();
    let h = cache.health();
    let v = serde_json::to_value(&h).unwrap();
    assert_eq!(
        v,
        json!({
            "memory_entries": 1,
            // The entry-weight rule (tiers.rs): body 4 + header key/value
            // 12 + 9 + 32.
            "memory_bytes": 57,
            "memory_capacity_bytes": 1 << 20,
            "disk_attached": false,
            "disk_bytes": 0,
            "remote_attached": true,
            "remote_kind": "tiered",
            "tags_indexed": 1,
            "keys_tracked": 1,
            "observer_attached": false,
            // F52: the disk tier's eviction total rides the posture (the
            // one-line pin drift the field's shape asked for — no disk
            // tier here, so the honest zero).
            "disk_evictions": 0,
        })
    );
    let back: oxide_fw_cache::CacheHealth = serde_json::from_value(v).unwrap();
    assert_eq!(back, h);
}

#[tokio::test]
async fn health_is_the_value_shaped_default_snapshot() {
    // The `SingleFlight` posture: `Clone` + `Eq` + `Default` — hosts and
    // tests diff values, never pointers; the default is the empty posture.
    let h = oxide_fw_cache::CacheHealth::default();
    assert_eq!(h, oxide_fw_cache::CacheHealth::default());
    let cloned = h.clone();
    assert_eq!(cloned, h);
}

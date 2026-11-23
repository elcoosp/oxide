//! TDD suite for the cache observability seam (`observer.rs`). Written
//! first per the house TDD rule: every pin failed on unresolved symbols
//! (E0432 over `CacheOp`/`CacheEvent`/`CacheObserver`/`TracingCacheObserver`,
//! E0599 over `TieredCache::with_observer`) before the seam existed.
//!
//! The contract the pins hold:
//!
//! 1. [`CacheOp`] is the value-shaped op record (the `LogEvent` posture —
//!    `Clone` + `Eq` so seams and tests compare records, not pointers);
//!    [`CacheEvent`] pairs it with the borrowed key so a hot-path emission
//!    allocates nothing.
//! 2. [`CacheObserver`] is the push seam: one synchronous, infallible
//!    callback per event, `Send + Sync + 'static` so it rides the
//!    `TieredCache`'s existing `Arc` sharing.
//! 3. [`TieredCache::with_observer`] attaches one (the
//!    `CaptureLayer::with_gate` builder shape); the emit sites are
//!    `set` / `get` (per served layer, fresh and stale, miss at the
//!    bottom of the fall-through) / `delete` / `invalidate_tag`
//!    (one aggregate event, not one per key).
//! 4. [`TracingCacheObserver`] is the production impl: every row leaves
//!    as one `tracing` event under the `oxide::cache` target (the
//!    `oxide::request` convention) — misses/hits/sets/deletes at debug,
//!    invalidations at info (the deploy-visible arm).
//! 5. No observer attached = today's behavior, byte for byte: the stats
//!    counters keep their contract.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oxide_fw_cache::{
    CacheEvent, CacheObserver, CacheOp, Entry, Hit, Layer, Policy, TieredCache,
    TracingCacheObserver,
};

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

/// The test's own pluggability proof: a counting observer that owns
/// captured `(key, op)` pairs — the exact shape a metrics exporter or a
/// test harness plugs in.
#[derive(Default)]
struct CountingObserver {
    events: Mutex<Vec<(String, CacheOp)>>,
}

impl CountingObserver {
    fn captured(&self) -> Vec<(String, CacheOp)> {
        self.events.lock().expect("the counting lock").clone()
    }
}

impl CacheObserver for CountingObserver {
    fn on_event(&self, event: &CacheEvent<'_>) {
        self.events
            .lock()
            .expect("the counting lock")
            .push((event.key.to_string(), event.op.clone()));
    }
}

/// Pin 1: the record shapes. `CacheOp` carries the op payload; the event
/// pairs it with the borrowed key; `kind()` is the stable, filterable row
/// name.
#[test]
fn op_and_record_shapes_pin() {
    assert_eq!(
        CacheEvent {
            key: "k",
            op: CacheOp::Hit {
                layer: Layer::L0,
                stale: false,
                bytes: 512
            },
        }
        .kind(),
        "hit"
    );
    assert_eq!(
        CacheEvent {
            key: "k",
            op: CacheOp::Miss,
        }
        .kind(),
        "miss"
    );
    assert_eq!(
        CacheEvent {
            key: "k",
            op: CacheOp::Set { bytes: 9 },
        }
        .kind(),
        "set"
    );
    assert_eq!(
        CacheEvent {
            key: "k",
            op: CacheOp::Delete,
        }
        .kind(),
        "delete"
    );
    assert_eq!(
        CacheEvent {
            key: "k",
            op: CacheOp::Invalidate {
                tag: "content:posts".into(),
                soft: true,
                count: 3
            },
        }
        .kind(),
        "invalidate"
    );
    // The value-shaped posture: records clone and compare (a metrics
    // exporter batches them; a test pins them).
    let a = CacheOp::Hit {
        layer: Layer::L1,
        stale: true,
        bytes: 0,
    };
    let b = a.clone();
    assert_eq!(a, b);
    assert_ne!(
        CacheOp::Hit {
            layer: Layer::L0,
            stale: false,
            bytes: 0
        },
        CacheOp::Hit {
            layer: Layer::L1,
            stale: false,
            bytes: 0
        }
    );
}

/// Pin 2: the production impl exists and takes every row without panic —
/// the infallible-callback contract. Its rows are only observable through
/// a subscriber; the kinds it reports are pinned here.
#[test]
fn tracing_observer_impls_the_seam() {
    let obs = TracingCacheObserver::new();
    for op in [
        CacheOp::Hit {
            layer: Layer::L0,
            stale: false,
            bytes: 128,
        },
        CacheOp::Hit {
            layer: Layer::L2,
            stale: true,
            bytes: 0,
        },
        CacheOp::Miss,
        CacheOp::Set { bytes: 42 },
        CacheOp::Delete,
        CacheOp::Invalidate {
            tag: "content:posts".into(),
            soft: false,
            count: 7,
        },
    ] {
        obs.on_event(&CacheEvent { key: "k", op });
    }
}

/// Pin 3: the wiring. `with_observer` attaches (the owned-builder shape
/// the `with_disk`/`with_remote` constructors use); the write-read-delete
/// story emits exactly the expected rows in order — one `set`, one fresh
/// `hit` at L0 with the payload size, one `miss` for the unknown key, one
/// `delete`.
#[tokio::test]
async fn with_observer_sees_the_write_read_delete_story() {
    let counter = Arc::new(CountingObserver::default());
    let cache = Arc::new(
        TieredCache::new(64 * 1024).with_observer(Arc::clone(&counter) as Arc<dyn CacheObserver>),
    );

    cache
        .set("route:/", entry(&[0u8; 512], &["content:posts"]))
        .await
        .expect("set");
    let hit = cache.get("route:/").await.expect("get");
    assert!(matches!(hit, Some(Hit::Fresh(_))));
    let miss = cache.get("route:/absent").await.expect("get");
    assert!(miss.is_none());
    cache.delete("route:/").await.expect("delete");

    assert_eq!(
        counter.captured(),
        vec![
            ("route:/".to_string(), CacheOp::Set { bytes: 512 }),
            (
                "route:/".to_string(),
                CacheOp::Hit {
                    layer: Layer::L0,
                    stale: false,
                    bytes: 512
                }
            ),
            ("route:/absent".to_string(), CacheOp::Miss),
            ("route:/".to_string(), CacheOp::Delete),
        ]
    );
}

/// Pin 4: the freshness arms. A stale hit carries `stale: true`; an
/// expired entry reads as a miss (the caller-visible outcome — the lazy
/// delete is not a `delete` event); a hard invalidate emits one aggregate
/// `invalidate` row with the affected-key count; a soft one carries
/// `soft: true`.
#[tokio::test]
async fn observer_sees_stale_expired_and_invalidate() {
    let counter = Arc::new(CountingObserver::default());
    let cache = Arc::new(
        TieredCache::new(64 * 1024).with_observer(Arc::clone(&counter) as Arc<dyn CacheObserver>),
    );

    // Stale: stored_at rewound past the ttl but inside the SWR window.
    // The SWR arm carries a 30s margin past the rewound stamp — a
    // bare-exact window (SWR 30s at 90s old) lands the boundary on the
    // get itself, and one clock tick between setup and read flips the
    // pin to expired (the flake the parallel suite hit).
    let mut e = entry(b"stale", &[]);
    e.policy.swr = Some(Duration::from_secs(60));
    e.stored_at = now_ms().saturating_sub(90_000); // ttl 60s gone, SWR covers
    cache.set("route:/stale", e).await.expect("set");
    let stale = cache.get("route:/stale").await.expect("get");
    assert!(matches!(stale, Some(Hit::Stale(_))));

    // Expired: past the whole usable window — a miss to the caller.
    let mut dead = entry(b"dead", &[]);
    dead.stored_at = now_ms().saturating_sub(600_000);
    cache.set("route:/dead", dead).await.expect("set");
    assert!(cache.get("route:/dead").await.expect("get").is_none());

    // Hard invalidate over the two live tag members.
    cache
        .set("route:/a", entry(b"a", &["content:posts"]))
        .await
        .expect("set a");
    cache
        .set("route:/b", entry(b"b", &["content:posts"]))
        .await
        .expect("set b");
    let n = cache
        .invalidate_tag("content:posts", false)
        .await
        .expect("invalidate");
    assert_eq!(n, 2);
    let soft_n = cache
        .invalidate_tag("content:posts", true)
        .await
        .expect("invalidate");
    assert_eq!(soft_n, 0, "a hard-invalidated tag has no live members");

    let events = counter.captured();
    // The stale hit, the expired read's miss, then the invalidate rows
    // (the per-key set/delete traffic of the invalidation is real traffic
    // and emits too — pinned only for the aggregate rows here).
    assert!(events.contains(&(
        "route:/stale".to_string(),
        CacheOp::Hit {
            layer: Layer::L0,
            stale: true,
            bytes: 5
        }
    )));
    assert!(events.contains(&("route:/dead".to_string(), CacheOp::Miss)));
    assert!(events.iter().any(|(k, op)| matches!(
        op,
        CacheOp::Invalidate { tag, soft, count }
            if k == "content:posts" && !soft && *count == 2 && tag == "content:posts"
    )));
    assert!(events.iter().any(|(k, op)| matches!(
        op,
        CacheOp::Invalidate { soft, .. } if k == "content:posts" && *soft
    )));
}

/// Pin 5: the layer field names the tier that SERVED, and the promotion
/// shows up as a layer change on the second read: a disk-only entry reads
/// as a L1 hit, then — promoted — as a L0 hit.
#[tokio::test]
async fn disk_hit_reports_the_disk_layer() {
    let counter = Arc::new(CountingObserver::default());
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = TieredCache::new(64 * 1024)
        .with_disk(dir.path().to_path_buf())
        .expect("disk tier opens")
        .with_observer(Arc::clone(&counter) as Arc<dyn CacheObserver>);
    let disk = cache.disk.as_ref().expect("the attached disk tier");
    disk.set("route:/disk", &entry(b"on-disk", &[]))
        .expect("disk set");

    let first = cache.get("route:/disk").await.expect("get");
    assert!(matches!(first, Some(Hit::Fresh(_))));
    let second = cache.get("route:/disk").await.expect("get");
    assert!(matches!(second, Some(Hit::Fresh(_))));

    let hits: Vec<CacheOp> = counter
        .captured()
        .into_iter()
        .filter(|(k, _)| k == "route:/disk")
        .map(|(_, op)| op)
        .collect();
    assert_eq!(
        hits,
        vec![
            CacheOp::Hit {
                layer: Layer::L1,
                stale: false,
                bytes: 7
            },
            CacheOp::Hit {
                layer: Layer::L0,
                stale: false,
                bytes: 7
            },
        ]
    );
}

/// Pin 6: no observer attached = today's contract. The stats counters
/// keep their meaning (hits count fresh and stale serves; misses count
/// the fall-through bottom) with the seam inert.
#[tokio::test]
async fn no_observer_default_keeps_stats_green() {
    let cache = TieredCache::new_memory();
    cache.set("k", entry(b"v", &[])).await.expect("set");
    assert!(cache.get("k").await.expect("get").is_some());
    assert!(cache.get("absent").await.expect("get").is_none());
    let stats = cache.stats();
    assert_eq!((stats.hits, stats.misses), (1, 1));
    assert_eq!(stats.entries_memory, 1);
}

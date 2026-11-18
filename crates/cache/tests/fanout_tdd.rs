//! TDD suite for the cache plane's observer fanout (`fanout.rs`). Written
//! first per the house TDD rule: every pin failed on unresolved symbols
//! (E0432 over `FanoutCacheObserver`) before the fanout existed.
//!
//! The contract the pins hold:
//!
//! 1. [`FanoutCacheObserver`](oxide_fw_cache::FanoutCacheObserver) is the
//!    F28 seam's composition arm — the server crate's `FanoutTraceSink`
//!    posture (F22) transposed to the cache plane: one event reaches every
//!    child observer, so the crate's three shipped consumers (tracing,
//!    metrics, feed) compose behind the ONE `with_observer` slot instead
//!    of the inline hand-roll the `coalesce` walkthrough carried before
//!    the type existed.
//! 2. Delivery is in construction order, children see the SAME event (the
//!    borrow re-shared, no per-child copy the seam would have to pay), and
//!    a child's cost stays the child's own (the fan adds one virtual call).
//! 3. An empty fan is the honest no-op (the `FanoutTraceSink` decision):
//!    it attaches, counts zero children, and records nowhere.
//! 4. `FanoutCacheObserver::shared` is the attach-and-compose convenience —
//!    the `Arc<dyn CacheObserver>` the tiers' field stores, ready for
//!    `with_observer`.
//! 5. Over real cache traffic the composed consumers agree: the exporter's
//!    counters and the feed's FIFO describe the same events, and
//!    `health().observer_attached` is honest about the single slot the
//!    fanout rides.
//! 6. Fanouts nest — a fan over fans is just children again (the trait
//!    contract, pinned once so the composition stays composable).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use oxide_fw_cache::{
    CacheEvent, CacheObserver, CacheOp, Entry, EventFeed, FanoutCacheObserver, Layer,
    MetricsCacheObserver, Policy, TieredCache,
};

fn entry(body: &[u8], tags: &[&str]) -> Entry {
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
        stored_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the wall clock runs")
            .as_millis()
            .try_into()
            .unwrap_or_default(),
        etag: "\"v1\"".into(),
    }
}

/// The payload pin's instrument: records every op payload it receives
/// into a shared log (module level — items read better beside the other
/// observers, and a mid-function item after statements reads worse).
struct Recorder(Arc<Mutex<Vec<CacheOp>>>);

impl CacheObserver for Recorder {
    fn on_event(&self, event: &CacheEvent<'_>) {
        self.0
            .lock()
            .expect("the payload log")
            .push(event.op.clone());
    }
}

/// A test observer that tags every row it receives into a shared log —
/// the order pin's instrument (the `CountingObserver::captured` shape).
struct TaggingObserver {
    tag: &'static str,
    log: Arc<Mutex<Vec<(&'static str, &'static str)>>>,
}

impl CacheObserver for TaggingObserver {
    fn on_event(&self, event: &CacheEvent<'_>) {
        self.log
            .lock()
            .expect("the order log lock")
            .push((self.tag, event.kind()));
    }
}

// --------------------------------------------------------------- the trait pin

/// The seam pin: the fanout IS a `CacheObserver`, attachable through the
/// F28 builder, and every child sees every completed event over real
/// traffic (the exporter's counters and the feed's FIFO describe the same
/// events — the composite story the `coalesce` walkthrough hand-rolled).
#[tokio::test]
async fn fanout_is_a_cache_observer_and_children_see_every_event() {
    let metrics = Arc::new(MetricsCacheObserver::new());
    let feed = Arc::new(EventFeed::bounded(64));
    let fan = Arc::new(FanoutCacheObserver::new(vec![
        metrics.clone() as Arc<dyn CacheObserver>,
        feed.clone() as Arc<dyn CacheObserver>,
    ]));
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(fan));

    cache
        .set("route:/a", entry(b"hello", &["content:posts"]))
        .await
        .expect("the memory tier accepts the entry");
    let _ = cache.get("route:/a").await.expect("the memory tier");
    let _ = cache.get("route:/missing").await.expect("the memory tier");

    // The exporter counted the traffic (1 set + 1 hit + 1 miss).
    let snap = metrics.snapshot();
    assert_eq!(snap.sets, 1, "the exporter saw the write-through");
    assert_eq!(snap.hits_l0, 1, "the exporter saw the L0 hit");
    assert_eq!(snap.misses, 1, "the exporter saw the miss");
    // The feed drained the same events in FIFO order.
    let kinds: Vec<&str> = feed
        .drain()
        .iter()
        .map(|(_, op)| match op {
            CacheOp::Set { .. } => "set",
            CacheOp::Hit { .. } => "hit",
            CacheOp::Miss => "miss",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["set", "hit", "miss"],
        "the feed saw the same events"
    );
    // The posture names the seam attached — the fan rides the one slot.
    assert!(cache.health().observer_attached);
}

// --------------------------------------------------------------- the order pin

/// Delivery is in construction order and the event is the SAME value: the
/// children's logs interleave per event, never per child's own stream.
#[tokio::test]
async fn fanout_delivers_in_construction_order() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let a = TaggingObserver {
        tag: "a",
        log: Arc::clone(&log),
    };
    let b = TaggingObserver {
        tag: "b",
        log: Arc::clone(&log),
    };
    let fan = Arc::new(FanoutCacheObserver::new(vec![
        Arc::new(a) as Arc<dyn CacheObserver>,
        Arc::new(b) as Arc<dyn CacheObserver>,
    ]));
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(fan));

    cache
        .set("route:/a", entry(b"hello", &[]))
        .await
        .expect("the memory tier accepts the entry");

    let rows = log.lock().expect("the order log lock");
    assert_eq!(
        *rows,
        vec![("a", "set"), ("b", "set")],
        "children receive every event in construction order"
    );
}

/// The fan re-shares the borrowed event — a child receiving a stale-hit
/// row sees the same layer/stale/bytes payload every other child sees
/// (the payload is not rebuilt per child).
#[tokio::test]
async fn fanout_children_see_the_same_payload() {
    let seen: Arc<Mutex<Vec<CacheOp>>> = Arc::new(Mutex::new(Vec::new()));
    let fan = Arc::new(FanoutCacheObserver::new(vec![
        Arc::new(Recorder(Arc::clone(&seen))) as Arc<dyn CacheObserver>,
        Arc::new(Recorder(Arc::clone(&seen))) as Arc<dyn CacheObserver>,
    ]));
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(fan));

    let mut stale_entry = entry(b"hello", &[]);
    // Inside the SWR window, past the ttl: ttl=60s from 120s ago, swr=300s.
    stale_entry.policy.swr = Some(Duration::from_secs(300));
    stale_entry.stored_at = stale_entry
        .stored_at
        .saturating_sub(u64::try_from(Duration::from_secs(120).as_millis()).unwrap_or_default());
    cache
        .set("route:/a", stale_entry)
        .await
        .expect("the memory tier accepts the entry");
    let _ = cache.get("route:/a").await.expect("the memory tier");

    let rows = seen.lock().expect("the payload log");
    let hits: Vec<&CacheOp> = rows
        .iter()
        .filter(|op| matches!(op, CacheOp::Hit { .. }))
        .collect();
    assert_eq!(hits.len(), 2, "both children saw the hit");
    for op in &hits {
        let CacheOp::Hit {
            layer,
            stale,
            bytes,
        } = op
        else {
            continue;
        };
        assert_eq!(*layer, Layer::L0);
        assert!(stale, "the serve was inside the stale window");
        assert_eq!(*bytes, 5, "the body bytes ride the payload");
    }
}

// --------------------------------------------------------------- the no-op pin

/// An empty fan is the honest no-op (the `FanoutTraceSink` decision): it
/// attaches, answers `children() == 0`, and records nowhere.
#[tokio::test]
async fn empty_fanout_is_the_honest_no_op() {
    let fan = Arc::new(FanoutCacheObserver::new(Vec::new()));
    assert_eq!(fan.children(), 0);
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(fan));
    cache
        .set("route:/a", entry(b"hello", &[]))
        .await
        .expect("the memory tier accepts the entry");
    // Traffic ran; nothing panicked, nothing was delivered anywhere.
    assert!(cache.health().observer_attached);
}

// --------------------------------------------------------------- the count pins

/// The child count the fan was built with (the boot's attach shape).
#[test]
fn fanout_children_count_names_the_fan() {
    let empty = FanoutCacheObserver::new(Vec::new());
    assert_eq!(empty.children(), 0);
    let two = FanoutCacheObserver::new(vec![
        Arc::new(MetricsCacheObserver::new()) as Arc<dyn CacheObserver>,
        Arc::new(EventFeed::bounded(8)) as Arc<dyn CacheObserver>,
    ]);
    assert_eq!(two.children(), 2);
}

// --------------------------------------------------------------- the convenience pin

/// `shared` is the attach-and-compose convenience: the `Arc<dyn
/// CacheObserver>` the tiers' field stores, ready for `with_observer`.
#[tokio::test]
async fn shared_returns_the_attached_half() {
    let metrics = Arc::new(MetricsCacheObserver::new());
    let fan = FanoutCacheObserver::shared(vec![metrics.clone() as Arc<dyn CacheObserver>]);
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(fan));
    cache
        .set("route:/a", entry(b"hello", &[]))
        .await
        .expect("the memory tier accepts the entry");
    assert_eq!(metrics.snapshot().sets, 1);
}

// --------------------------------------------------------------- the nesting pin

/// Fanouts nest: a fan over fans is just children again (the composition
/// stays composable — the trace plane's fan-of-fans posture).
#[tokio::test]
async fn fanouts_nest() {
    let metrics = Arc::new(MetricsCacheObserver::new());
    let feed = Arc::new(EventFeed::bounded(8));
    let inner = FanoutCacheObserver::shared(vec![metrics.clone() as Arc<dyn CacheObserver>]);
    let outer = FanoutCacheObserver::shared(vec![inner, feed.clone() as Arc<dyn CacheObserver>]);
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(outer));
    cache
        .set("route:/a", entry(b"hello", &[]))
        .await
        .expect("the memory tier accepts the entry");
    assert_eq!(metrics.snapshot().sets, 1, "the nested child saw the event");
    assert_eq!(feed.len(), 1, "the sibling child saw the event");
}

// --------------------------------------------------------------- the debug pin

/// The fan's Debug names the child count (the boot's attach shape), not
/// the children's internals.
#[test]
fn debug_names_the_child_count() {
    let two = FanoutCacheObserver::new(vec![
        Arc::new(MetricsCacheObserver::new()) as Arc<dyn CacheObserver>,
        Arc::new(EventFeed::bounded(8)) as Arc<dyn CacheObserver>,
    ]);
    let rendered = format!("{two:?}");
    assert!(rendered.starts_with("FanoutCacheObserver"), "{rendered}");
    assert!(rendered.contains("children: 2"), "{rendered}");
}

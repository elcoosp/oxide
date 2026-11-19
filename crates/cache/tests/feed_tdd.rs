//! TDD suite for the cache plane's bounded event feed (`feed.rs`). Written
//! first per the house TDD rule: every pin failed on unresolved symbols
//! (E0432 over `EventFeed`, E0599 over `try_pop`/`drain`) before the feed
//! existed.
//!
//! The contract the pins hold:
//!
//! 1. [`EventFeed`](oxide_fw_cache::EventFeed) is the F28 seam's named
//!    heavy-consumer shape — the observer module's doc prescribes it: "a
//!    heavy consumer should enqueue into its own channel and process
//!    elsewhere — the server crate's `FileTraceSink` posture is the model
//!    for lossy consumers". It implements
//!    [`CacheObserver`](oxide_fw_cache::CacheObserver) and rides the same
//!    attach point (`TieredCache::with_observer`), so a host that ships
//!    rows off-process never slows the hot path with its own queue code.
//! 2. Rows are FIFO owned `(key, op)` pairs (the
//!    `CountingObserver::captured` shape — the seam's borrow lives only
//!    for the callback, the queue owns its copies).
//! 3. The queue is bounded and drops the ARRIVING row when full (the
//!    stream posture — a stalled consumer keeps the rows already queued
//!    and loses what arrives after saturation; a retention ring would
//!    silently drop the OLD rows instead). Drops are counted
//!    (`dropped()`), never silent.
//! 4. The failure rule is the FileTraceSink sticky warn adapted: the
//!    FIRST drop leaves one `tracing::warn!` under the `oxide::cache`
//!    target (naming the capacity), then the rows drop silently while the
//!    counter keeps the score — a saturated feed must not spam the very
//!    console the operator is reading.
//! 5. A zero capacity retains nothing (the `RingLogSink` posture) — every
//!    row is a drop.
//! 6. The drain arms (`try_pop`, `drain`) empty the queue FIFO so the
//!    host's processing loop is a plain loop.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use oxide_fw_cache::{CacheEvent, CacheObserver, CacheOp, Entry, EventFeed, Policy, TieredCache};
use tracing_subscriber::prelude::*;

fn entry(body: &[u8], tags: &[&str]) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: tags.iter().map(|s| s.to_string()).collect(),
        policy: Policy {
            ttl: Duration::from_secs(60),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the wall clock runs")
            .as_millis() as u64,
        etag: "\"v1\"".into(),
    }
}

/// The console sink the sticky-warn pin reads: a `MakeWriter` over a
/// shared byte buffer (the `log_feed_gate_tdd` shape).
#[derive(Clone)]
struct ConsoleWriter(Arc<Mutex<Vec<u8>>>);

impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for ConsoleWriter {
    type Writer = ConsoleSink;

    fn make_writer(&'a self) -> Self::Writer {
        ConsoleSink(self.0.clone())
    }
}

struct ConsoleSink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for ConsoleSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("console buffer")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Count the rendered warn rows the feed's sticky rule leaves. The row
/// key is the level plus the feed's message (the fmt layer renders the
/// message inline, so the pin holds the warn arm and its wording).
fn warn_rows(buffer: &ConsoleWriter) -> usize {
    let bytes = buffer.0.lock().expect("console buffer");
    std::str::from_utf8(&bytes)
        .expect("the fmt render is utf-8")
        .lines()
        .filter(|line| line.contains("WARN") && line.contains("saturated"))
        .count()
}

/// The capture pins' registration lock: two subscriber installs racing
/// rebuild the tracing callsite cache, and a warn fired mid-rebuild is
/// dropped — the sticky-warn pins register one at a time so each drive
/// runs against a warm cache (the flake the parallel suite hit).
static CAPTURE_LOCK: Mutex<()> = Mutex::new(());

// --------------------------------------------------------------- the trait pin

/// The seam pin: the feed IS a `CacheObserver`, attachable through the F28
/// builder (the `Arc<dyn CacheObserver>` coercion the tiers' field
/// stores), and real cache traffic lands FIFO in the drain.
#[tokio::test]
async fn feed_is_a_cache_observer_over_real_traffic() {
    let feed = Arc::new(EventFeed::bounded(64));
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(feed.clone()));

    cache
        .set("route:/a", entry(b"hello", &["content:posts"]))
        .await
        .expect("the memory tier accepts the entry");
    let _ = cache.get("route:/a").await.expect("the memory tier");
    let _ = cache.get("route:/missing").await.expect("the memory tier");

    let rows = feed.drain();
    let kinds: Vec<&str> = rows
        .iter()
        .map(|(_, op)| match op {
            CacheOp::Hit { .. } => "hit",
            CacheOp::Miss => "miss",
            CacheOp::Set { .. } => "set",
            CacheOp::Delete => "delete",
            CacheOp::Invalidate { .. } => "invalidate",
            CacheOp::Coalesce { .. } => "coalesce",
        })
        .collect();
    assert_eq!(kinds, vec!["set", "hit", "miss"], "FIFO over the traffic");
    assert!(feed.is_empty(), "drain empties the queue");
    assert_eq!(feed.dropped(), 0, "under capacity nothing drops");
}

// -------------------------------------------------------------- the order pins

/// The queue is FIFO owned pairs: the borrow lives only for the callback,
/// the feed owns its copies, and `try_pop` hands them back in arrival
/// order.
#[test]
fn feed_is_fifo_and_pop_empties_one_row_at_a_time() {
    let feed = EventFeed::bounded(8);
    let observer: &dyn CacheObserver = &feed;
    observer.on_event(&CacheEvent {
        key: "route:/a",
        op: CacheOp::Set { bytes: 4 },
    });
    observer.on_event(&CacheEvent {
        key: "route:/b",
        op: CacheOp::Miss,
    });
    observer.on_event(&CacheEvent {
        key: "route:/c",
        op: CacheOp::Delete,
    });

    assert_eq!(feed.len(), 3);
    let first = feed.try_pop().expect("the first row");
    assert_eq!(first.0, "route:/a");
    assert_eq!(first.1, CacheOp::Set { bytes: 4 });
    let second = feed.try_pop().expect("the second row");
    assert_eq!(second.0, "route:/b");
    let third = feed.try_pop().expect("the third row");
    assert_eq!(third.1, CacheOp::Delete);
    assert!(feed.try_pop().is_none(), "the queue is empty");
    assert_eq!(feed.len(), 0);
}

/// `drain` is the batch arm: everything queued comes back in one call, in
/// arrival order, and the queue ends empty.
#[test]
fn drain_returns_everything_in_order() {
    let feed = EventFeed::bounded(8);
    let observer: &dyn CacheObserver = &feed;
    for i in 0u32..5 {
        observer.on_event(&CacheEvent {
            key: "route:/x",
            op: CacheOp::Set {
                bytes: u64::from(i),
            },
        });
    }
    let rows = feed.drain();
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0].1, CacheOp::Set { bytes: 0 });
    assert_eq!(rows[4].1, CacheOp::Set { bytes: 4 });
    assert!(feed.is_empty());
    assert!(feed.drain().is_empty(), "the second drain is empty");
}

// ----------------------------------------------------------- the bounded pins

/// The stream posture: saturation drops the ARRIVING row — the rows
/// already queued survive for the consumer, the counter keeps the score.
#[test]
fn saturation_drops_the_arriving_row() {
    let feed = EventFeed::bounded(3);
    let observer: &dyn CacheObserver = &feed;
    for i in 0u32..3 {
        observer.on_event(&CacheEvent {
            key: "route:/kept",
            op: CacheOp::Set {
                bytes: u64::from(i),
            },
        });
    }
    assert_eq!(feed.len(), 3);
    observer.on_event(&CacheEvent {
        key: "route:/dropped",
        op: CacheOp::Miss,
    });
    assert_eq!(feed.len(), 3, "the queued rows survive");
    assert_eq!(feed.dropped(), 1, "the arriving row is the drop");
    let rows = feed.drain();
    assert!(
        rows.iter().all(|(key, _)| key == "route:/kept"),
        "the kept rows are the queued ones, not the dropped arrival"
    );
}

/// The `RingLogSink` posture: a zero capacity retains nothing — every row
/// is a drop, none are silent.
#[test]
fn zero_capacity_records_nothing() {
    let feed = EventFeed::bounded(0);
    let observer: &dyn CacheObserver = &feed;
    observer.on_event(&CacheEvent {
        key: "route:/a",
        op: CacheOp::Set { bytes: 4 },
    });
    assert!(feed.is_empty());
    assert_eq!(feed.dropped(), 1);
    assert_eq!(feed.capacity(), 0);
}

/// The counters are honest across a sustained overshoot: N arrivals past
/// capacity land as N drops, and `dropped()` never counts a drained row.
#[test]
fn sustained_overshoot_counts_every_drop() {
    let feed = EventFeed::bounded(2);
    let observer: &dyn CacheObserver = &feed;
    for i in 0u32..10 {
        observer.on_event(&CacheEvent {
            key: "route:/x",
            op: CacheOp::Set {
                bytes: u64::from(i),
            },
        });
    }
    assert_eq!(feed.len(), 2);
    assert_eq!(feed.dropped(), 8);
    let rows = feed.drain();
    assert_eq!(rows.len(), 2);
    // The kept rows are the first two arrivals (FIFO), the drops are the
    // eight that arrived after saturation.
    assert_eq!(rows[0].1, CacheOp::Set { bytes: 0 });
    assert_eq!(rows[1].1, CacheOp::Set { bytes: 1 });
    assert_eq!(feed.dropped(), 8, "draining never moves the drop counter");
}

// ------------------------------------------------------------ the sticky pin

/// The FileTraceSink posture adapted: the FIRST drop leaves one warn under
/// the `oxide::cache` target naming the capacity; the rows after it drop
/// silently (counted, never rendered) — a saturated feed must not spam
/// the console.
#[test]
fn first_drop_warns_once_then_the_rows_drop_silently() {
    let _capture = CAPTURE_LOCK.lock().expect("the capture lock");
    let buffer = ConsoleWriter(Arc::new(Mutex::new(Vec::new())));
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(buffer.clone())
        .with_target(false)
        .without_time();
    let feed = EventFeed::bounded(1);
    let observer: &dyn CacheObserver = &feed;

    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        // The first row fits (no warn); the second drops (warn #1); the
        // third and fourth drop silently (no warn #2/#3).
        observer.on_event(&CacheEvent {
            key: "route:/a",
            op: CacheOp::Set { bytes: 4 },
        });
        observer.on_event(&CacheEvent {
            key: "route:/b",
            op: CacheOp::Miss,
        });
        observer.on_event(&CacheEvent {
            key: "route:/c",
            op: CacheOp::Miss,
        });
        observer.on_event(&CacheEvent {
            key: "route:/d",
            op: CacheOp::Miss,
        });
    });

    assert_eq!(warn_rows(&buffer), 1, "exactly one sticky warn");
    assert_eq!(feed.dropped(), 3);
}

/// The zero-capacity feed warns on its first drop too — the rule keys on
/// the drop, not on the capacity.
#[test]
fn zero_capacity_warns_once() {
    let _capture = CAPTURE_LOCK.lock().expect("the capture lock");
    let buffer = ConsoleWriter(Arc::new(Mutex::new(Vec::new())));
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(buffer.clone())
        .with_target(false)
        .without_time();
    let feed = EventFeed::bounded(0);
    let observer: &dyn CacheObserver = &feed;

    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        for _ in 0..3 {
            observer.on_event(&CacheEvent {
                key: "route:/a",
                op: CacheOp::Set { bytes: 4 },
            });
        }
    });

    assert_eq!(warn_rows(&buffer), 1, "exactly one sticky warn");
    assert_eq!(feed.dropped(), 3);
}

// --------------------------------------------------------- the concurrency pin

/// The seam runs the callback on the cache's call path — two tasks
/// producing through one shared feed while the consumer drains must
/// neither deadlock nor lose rows past the honest bound.
#[tokio::test]
async fn concurrent_producers_and_a_draining_consumer_stay_honest() {
    let feed = Arc::new(EventFeed::bounded(1024));
    let observer: Arc<dyn CacheObserver> = feed.clone();

    let producer_a = tokio::task::spawn_blocking({
        let observer = Arc::clone(&observer);
        move || {
            for _ in 0..500 {
                observer.on_event(&CacheEvent {
                    key: "route:/a",
                    op: CacheOp::Miss,
                });
            }
        }
    });
    let producer_b = tokio::task::spawn_blocking({
        let observer = Arc::clone(&observer);
        move || {
            for _ in 0..500 {
                observer.on_event(&CacheEvent {
                    key: "route:/b",
                    op: CacheOp::Miss,
                });
            }
        }
    });

    let mut popped = 0usize;
    while !producer_a.is_finished() || !producer_b.is_finished() || !feed.is_empty() {
        while feed.try_pop().is_some() {
            popped += 1;
        }
        tokio::task::yield_now().await;
    }
    producer_a.await.expect("producer a");
    producer_b.await.expect("producer b");
    while feed.try_pop().is_some() {
        popped += 1;
    }

    // 1000 arrivals into a 1024 queue: none dropped, all popped.
    assert_eq!(popped, 1000);
    assert_eq!(feed.dropped(), 0);
    assert!(feed.is_empty());
}

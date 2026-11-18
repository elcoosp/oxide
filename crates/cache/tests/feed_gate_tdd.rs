//! TDD suite for the cache plane's event-feed admission gate (`feed.rs`,
//! F35). Written first per the house TDD rule: every pin failed on
//! unresolved symbols (E0432 over `FeedGate`/`AdmitAll`/`KindGate`,
//! E0599 over `with_gate`) before the gate existed.
//!
//! The contract the pins hold (the request plane's F26 filter-surface
//! shape transposed to the cache feed):
//!
//! 1. [`FeedGate`](oxide_fw_cache::FeedGate) is the feed's record-side
//!    admission predicate — the capture-side narrowing: the fanout and
//!    the exporters widen who hears an event, the gate narrows what the
//!    feed retains. It sees the borrowed event BEFORE the owned row is
//!    built, so a rejected row allocates nothing.
//! 2. [`AdmitAll`](oxide_fw_cache::AdmitAll) is the default spelled as a
//!    type: `with_gate(Arc::new(AdmitAll))` matches the ungated feed row
//!    for row (the F24 posture: the default shape never moves).
//! 3. [`KindGate`](oxide_fw_cache::KindGate) is the domain gate: it
//!    admits the row kinds named at construction (`event.kind()`'s
//!    stable names) and rejects the rest — a deploy-watch consumer
//!    (`invalidate` + `coalesce` only) stops paying the enqueue for the
//!    hot-hit stream it would have drained and discarded.
//! 4. Gate rejections are ADMISSION, not saturation: they never count in
//!    `dropped()`, never fire the sticky warn — the warn is the bound's
//!    signal, and the bound is untouched by a configured filter.
//! 5. The bound still applies behind the gate: admitted rows saturate a
//!    tiny queue exactly as before (the warn-once rule holds — the gate
//!    and the bound compose).
//! 6. A host implements `FeedGate` for its own predicate (the seam's
//!    custom-impl contract — a key-prefix gate pins it).
//! 7. Over real cache traffic the gated feed holds ONLY the admitted
//!    kinds, and the gated feed attaches through the unmodified seam.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use oxide_fw_cache::{
    AdmitAll, CacheEvent, CacheObserver, CacheOp, Entry, EventFeed, FeedGate, KindGate, Layer,
    Policy, TieredCache,
};
use tracing_subscriber::prelude::*;

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

/// The console sink the warn pins read: a `MakeWriter` over a shared
/// byte buffer (the `feed_tdd` shape).
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

/// Count the rendered warn rows the bound's sticky rule leaves.
fn warn_rows(buffer: &ConsoleWriter) -> usize {
    let bytes = buffer.0.lock().expect("console buffer");
    std::str::from_utf8(&bytes)
        .expect("the fmt render is utf-8")
        .lines()
        .filter(|line| line.contains("WARN") && line.contains("saturated"))
        .count()
}

// --------------------------------------------------------------- the trait pins

/// `with_gate(Arc::new(AdmitAll))` matches the ungated feed row for row:
/// the same events through both, the same FIFO drain out (the F24
/// posture — the default shape never moves).
#[test]
fn admit_all_matches_the_ungated_feed_row_for_row() {
    let ungated = EventFeed::bounded(64);
    let gated = EventFeed::bounded(64).with_gate(Arc::new(AdmitAll));
    let ungated_obs: &dyn CacheObserver = &ungated;
    let gated_obs: &dyn CacheObserver = &gated;

    for observer in [ungated_obs, gated_obs] {
        observer.on_event(&CacheEvent {
            key: "route:/a",
            op: CacheOp::Set { bytes: 4 },
        });
        observer.on_event(&CacheEvent {
            key: "route:/b",
            op: CacheOp::Miss,
        });
        observer.on_event(&CacheEvent {
            key: "route:/a",
            op: CacheOp::Set { bytes: 4 },
        });
    }

    assert_eq!(ungated.drain(), gated.drain(), "the default shape holds");
    assert_eq!(gated.dropped(), 0);
}

/// `KindGate::only` admits exactly the named kinds — the deploy-watch
/// gate: invalidation and coalesce rows join, the hot rows never do.
#[test]
fn kind_gate_admits_only_the_named_kinds() {
    let feed =
        EventFeed::bounded(64).with_gate(Arc::new(KindGate::only(&["invalidate", "coalesce"])));
    let observer: &dyn CacheObserver = &feed;

    // The hot stream the gate rejects: hits, misses, sets, deletes.
    observer.on_event(&CacheEvent {
        key: "route:/a",
        op: CacheOp::Hit {
            layer: Layer::L0,
            stale: false,
            bytes: 4,
        },
    });
    observer.on_event(&CacheEvent {
        key: "route:/a",
        op: CacheOp::Miss,
    });
    observer.on_event(&CacheEvent {
        key: "route:/a",
        op: CacheOp::Set { bytes: 4 },
    });
    observer.on_event(&CacheEvent {
        key: "route:/a",
        op: CacheOp::Delete,
    });
    assert_eq!(feed.len(), 0, "the hot rows never joined the queue");
    assert_eq!(feed.dropped(), 0);

    // The deploy-visible rows the gate admits.
    observer.on_event(&CacheEvent {
        key: "content:posts",
        op: CacheOp::Invalidate {
            tag: "content:posts".into(),
            soft: true,
            count: 2,
        },
    });
    observer.on_event(&CacheEvent {
        key: "route:/a",
        op: CacheOp::Coalesce { followers: 3 },
    });
    let rows = feed.drain();
    let kinds: Vec<&str> = rows
        .iter()
        .map(|(key, op)| {
            CacheEvent {
                key,
                op: op.clone(),
            }
            .kind()
        })
        .collect();
    assert_eq!(kinds, vec!["invalidate", "coalesce"]);
}

// --------------------------------------------------------------- the admission pins

/// Gate rejections are admission, not saturation: `dropped()` stays zero
/// and the sticky warn never fires for a configured filter.
#[test]
fn rejections_are_not_drops_and_never_warn() {
    let buffer = ConsoleWriter(Arc::new(Mutex::new(Vec::new())));
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(buffer.clone())
        .with_target(false)
        .without_time();
    let feed = EventFeed::bounded(64).with_gate(Arc::new(KindGate::only(&["invalidate"])));
    let observer: &dyn CacheObserver = &feed;

    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        for _ in 0..128 {
            observer.on_event(&CacheEvent {
                key: "route:/a",
                op: CacheOp::Miss,
            });
        }
    });

    assert_eq!(feed.len(), 0);
    assert_eq!(feed.dropped(), 0, "a rejected row is not a saturation drop");
    assert_eq!(warn_rows(&buffer), 0, "the warn is the bound's signal");
}

/// The bound still applies behind the gate: admitted rows saturate a tiny
/// queue exactly as before — arriving row dropped, `dropped()` counting,
/// the warn-once rule holding (the gate and the bound compose).
#[test]
fn the_bound_composes_behind_the_gate() {
    let buffer = ConsoleWriter(Arc::new(Mutex::new(Vec::new())));
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(buffer.clone())
        .with_target(false)
        .without_time();
    let feed = EventFeed::bounded(2).with_gate(Arc::new(KindGate::only(&["invalidate"])));
    let observer: &dyn CacheObserver = &feed;

    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        // A rejected row first: the gate narrows before the bound sees it.
        observer.on_event(&CacheEvent {
            key: "route:/a",
            op: CacheOp::Miss,
        });
        // Three admitted rows into a two-slot queue: the third is the drop.
        for count in [0usize, 1, 2] {
            observer.on_event(&CacheEvent {
                key: "content:posts",
                op: CacheOp::Invalidate {
                    tag: "content:posts".into(),
                    soft: true,
                    count,
                },
            });
        }
    });

    assert_eq!(feed.len(), 2, "the bound holds behind the gate");
    assert_eq!(feed.dropped(), 1, "the arriving row was the drop");
    assert_eq!(warn_rows(&buffer), 1, "the sticky warn fired exactly once");
}

/// A zero capacity still retains nothing behind an admitting gate — every
/// admitted row is a drop (the `RingLogSink` posture, unchanged).
#[test]
fn zero_capacity_with_admit_all_retains_nothing() {
    let feed = EventFeed::bounded(0).with_gate(Arc::new(AdmitAll));
    let observer: &dyn CacheObserver = &feed;
    observer.on_event(&CacheEvent {
        key: "route:/a",
        op: CacheOp::Miss,
    });
    assert!(feed.is_empty());
    assert_eq!(feed.dropped(), 1);
}

/// `gated` is the shared-shape sugar: the F35 narrowing over the
/// attach-and-drain constructor, one expression.
#[test]
fn gated_is_the_shared_shape_with_the_narrowing() {
    let feed = EventFeed::gated(8, Arc::new(KindGate::only(&["miss"])));
    let observer: Arc<dyn CacheObserver> = feed.clone();
    observer.on_event(&CacheEvent {
        key: "route:/a",
        op: CacheOp::Set { bytes: 4 },
    });
    observer.on_event(&CacheEvent {
        key: "route:/a",
        op: CacheOp::Miss,
    });
    // Attach-and-drain: the typed half reads the queue behind the gate.
    assert_eq!(feed.len(), 1);
    let rows = feed.drain();
    assert_eq!(rows[0].0, "route:/a");
}

// --------------------------------------------------------------- the custom-impl pin

/// The seam's custom-impl contract: a host implements `FeedGate` for its
/// own predicate — a key-prefix gate pins it.
#[test]
fn a_host_gate_predicate_is_honored() {
    /// The host's own admission rule: only rows keyed under `/blog/`.
    struct BlogOnly;

    impl FeedGate for BlogOnly {
        fn admits(&self, event: &CacheEvent<'_>) -> bool {
            event.key.starts_with("/blog/")
        }
    }

    let feed = EventFeed::bounded(64).with_gate(Arc::new(BlogOnly));
    let observer: &dyn CacheObserver = &feed;
    observer.on_event(&CacheEvent {
        key: "/blog/a",
        op: CacheOp::Miss,
    });
    observer.on_event(&CacheEvent {
        key: "/docs/a",
        op: CacheOp::Miss,
    });
    observer.on_event(&CacheEvent {
        key: "/blog/b",
        op: CacheOp::Miss,
    });
    let rows = feed.drain();
    let keys: Vec<&str> = rows.iter().map(|(key, _)| key.as_str()).collect();
    assert_eq!(keys, vec!["/blog/a", "/blog/b"]);
}

// --------------------------------------------------------------- the real-traffic pins

/// Over real cache traffic the gated feed holds ONLY the admitted kinds —
/// the deploy-watch shape end to end (hits/misses/sets flowed, the queue
/// keeps the invalidation and coalesce rows).
#[tokio::test]
async fn gated_feed_over_real_traffic_keeps_only_admitted_kinds() {
    let feed = Arc::new(
        EventFeed::bounded(64).with_gate(Arc::new(KindGate::only(&["invalidate", "coalesce"]))),
    );
    let cache = Arc::new(
        TieredCache::new(64 * 1024 * 1024).with_observer(feed.clone() as Arc<dyn CacheObserver>),
    );

    for i in 0..8 {
        cache
            .set(
                &format!("route:/a/{i}"),
                entry(b"hello", &["content:posts"]),
            )
            .await
            .expect("the memory tier accepts the entry");
        let _ = cache
            .get(&format!("route:/a/{i}"))
            .await
            .expect("the memory tier");
    }
    cache
        .invalidate_tag("content:posts", true)
        .await
        .expect("the memory tier invalidates");
    cache.record_coalesced("route:/a/0", 2);

    let rows = feed.drain();
    assert!(
        !rows.is_empty(),
        "the invalidation sweep reaches the gated feed"
    );
    for (key, op) in &rows {
        let kind = CacheEvent {
            key,
            op: op.clone(),
        }
        .kind();
        assert!(
            kind == "invalidate" || kind == "coalesce",
            "a rejected kind leaked: {kind} ({key})"
        );
    }
    assert!(rows.iter().any(|(key, _)| key == "content:posts"));
}

/// The attach shape: the gated feed coerces to the `Arc<dyn CacheObserver>`
/// the tiers' field stores (the `with_observer` point), unmodified.
#[tokio::test]
async fn gated_feed_attaches_through_the_seam() {
    let feed = Arc::new(EventFeed::bounded(16).with_gate(Arc::new(KindGate::only(&["set"]))));
    let observer: Arc<dyn CacheObserver> = feed.clone();
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(observer));
    cache
        .set("route:/a", entry(b"hello", &[]))
        .await
        .expect("the memory tier accepts the entry");
    let _ = cache.get("route:/a").await.expect("the memory tier");

    let rows = feed.drain();
    assert_eq!(
        rows.len(),
        1,
        "the hit row was rejected, the set row admitted"
    );
    assert_eq!(rows[0].0, "route:/a");
}

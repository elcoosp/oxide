//! TDD suite for the cache plane's hot-key admission gate (`hot_keys.rs`,
//! F39). Written first per the house TDD rule: every pin failed on
//! unresolved symbols (E0432 over `HotKeyGate`/`AdmitAllKeys`/
//! `HotKeyKindGate`, E0599 over `with_gate`) before the gate existed.
//!
//! The contract the pins hold (the F35 filter-surface shape transposed
//! from the feed to the attribution ledger):
//!
//! 1. [`HotKeyGate`](oxide_fw_cache::HotKeyGate) is the ledger's
//!    admission predicate — the attribution-side narrowing: the fanout
//!    and the exporters widen who hears an event, the gate narrows which
//!    keys the ledger tracks. It sees the borrowed event BEFORE the
//!    ledger lock, so a rejected row allocates nothing.
//! 2. [`AdmitAllKeys`](oxide_fw_cache::AdmitAllKeys) is the default
//!    spelled as a type: `with_gate(Arc::new(AdmitAllKeys))` matches the
//!    ungated ledger row for row (the F24 posture: the default shape
//!    never moves).
//! 3. [`HotKeyKindGate`](oxide_fw_cache::HotKeyKindGate) is the domain
//!    gate: it attributes the row kinds named at construction
//!    (`event.kind()`'s stable names) and refuses the rest — the
//!    deploy-watch ledger (`invalidate` + `coalesce` only) tracks the
//!    keys a deploy touched, not the hot-hit stream it would have
//!    counted and discarded.
//! 4. Gate rejections are ADMISSION, not saturation: they never count in
//!    `unattributed()`, never fire the sticky warn — the warn is the
//!    bound's signal, and the bound is untouched by a configured filter.
//! 5. The bound still applies behind the gate: admitted keys refuse past
//!    capacity exactly as before (the warn-once rule holds — the gate
//!    and the bound compose).
//! 6. A host implements `HotKeyGate` for its own predicate (the seam's
//!    custom-impl contract — a key-prefix gate pins it).
//! 7. The Debug shape names the gate (`gated`), the same posture the
//!    `EventFeed` debug carries.
//! 8. Over real cache traffic the gated ledger attributes ONLY the
//!    admitted kinds, and the gated ledger attaches through the
//!    unmodified seam.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use oxide_fw_cache::{
    AdmitAllKeys, CacheEvent, CacheObserver, CacheOp, Entry, HotKeyGate, HotKeyKindGate,
    HotKeyObserver, Layer, Policy, TieredCache,
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

/// Drive one event straight into the observer (the `hot_keys_tdd` shape
/// — the pin names the payload, not the tier mechanics).
fn push(obs: &HotKeyObserver, key: &str, op: CacheOp) {
    obs.on_event(&CacheEvent { key, op });
}

/// The console sink the warn pins read: a `MakeWriter` over a shared
/// byte buffer (the `feed_gate_tdd` shape).
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

/// The capture pins' registration lock: two subscriber installs racing
/// rebuild the tracing callsite cache, and a warn fired mid-rebuild is
/// dropped — the sticky-warn pins register one at a time so each drive
/// runs against a warm cache (the flake the parallel suite hit).
static CAPTURE_LOCK: Mutex<()> = Mutex::new(());

// --------------------------------------------------------------- the trait pins

/// `with_gate(Arc::new(AdmitAllKeys))` matches the ungated ledger row for
/// row: the same events through both, the same top window out (the F24
/// posture — the default shape never moves).
#[test]
fn admit_all_keys_matches_the_ungated_ledger_row_for_row() {
    let ungated = HotKeyObserver::bounded(64);
    let gated = HotKeyObserver::bounded(64).with_gate(Arc::new(AdmitAllKeys));
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

    assert_eq!(ungated.top(10), gated.top(10), "the default shape holds");
    assert_eq!(gated.unattributed(), 0);
}

/// `HotKeyKindGate::only` attributes exactly the named kinds — the
/// deploy-watch ledger: invalidation and coalesce rows track their keys,
/// the hot rows never do.
#[test]
fn hot_key_kind_gate_admits_only_the_named_kinds() {
    let hot = HotKeyObserver::bounded(64)
        .with_gate(Arc::new(HotKeyKindGate::only(&["invalidate", "coalesce"])));
    let observer: &dyn CacheObserver = &hot;

    // The hot stream the gate refuses: hits, misses, sets, deletes.
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
    assert!(hot.is_empty(), "the hot rows never joined the ledger");
    assert_eq!(hot.unattributed(), 0);

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
    let rows = hot.top(10);
    let keys: Vec<&str> = rows.iter().map(|row| row.key.as_str()).collect();
    // Count desc, key asc on ties: both rows count one, so the sort
    // decides (the F38 deterministic-order doctrine).
    assert_eq!(keys, vec!["content:posts", "route:/a"]);
}

// --------------------------------------------------------------- the admission pins

/// Gate rejections are admission, not saturation: `unattributed()` stays
/// zero and the sticky warn never fires for a configured filter.
#[test]
fn rejections_are_not_refusals_and_never_warn() {
    let _capture = CAPTURE_LOCK.lock().expect("the capture lock");
    let buffer = ConsoleWriter(Arc::new(Mutex::new(Vec::new())));
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(buffer.clone())
        .with_target(false)
        .without_time();
    let hot =
        HotKeyObserver::bounded(64).with_gate(Arc::new(HotKeyKindGate::only(&["invalidate"])));
    let observer: &dyn CacheObserver = &hot;

    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        for _ in 0..128 {
            observer.on_event(&CacheEvent {
                key: "route:/a",
                op: CacheOp::Miss,
            });
        }
    });

    assert!(hot.is_empty());
    assert_eq!(
        hot.unattributed(),
        0,
        "a rejected row is not a saturation refusal"
    );
    assert_eq!(warn_rows(&buffer), 0, "the warn is the bound's signal");
}

/// The bound still applies behind the gate: admitted keys refuse past a
/// tiny capacity exactly as before — unattributed counting, the
/// warn-once rule holding (the gate and the bound compose).
#[test]
fn the_bound_composes_behind_the_gate() {
    let _capture = CAPTURE_LOCK.lock().expect("the capture lock");
    let buffer = ConsoleWriter(Arc::new(Mutex::new(Vec::new())));
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(buffer.clone())
        .with_target(false)
        .without_time();
    let hot = HotKeyObserver::bounded(1).with_gate(Arc::new(HotKeyKindGate::only(&["invalidate"])));
    let observer: &dyn CacheObserver = &hot;

    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        // A rejected row first: the gate narrows before the bound sees it.
        observer.on_event(&CacheEvent {
            key: "route:/a",
            op: CacheOp::Miss,
        });
        // Two admitted keys into a one-key ledger: the second is the
        // refusal.
        for count in [0usize, 1] {
            observer.on_event(&CacheEvent {
                key: &format!("content:posts/{count}"),
                op: CacheOp::Invalidate {
                    tag: "content:posts".into(),
                    soft: true,
                    count,
                },
            });
        }
    });

    assert_eq!(hot.len(), 1, "the bound holds behind the gate");
    assert_eq!(hot.unattributed(), 1, "the arriving key was the refusal");
    assert_eq!(warn_rows(&buffer), 1, "the sticky warn fired exactly once");
}

/// A zero capacity still retains nothing behind an admitting gate —
/// every admitted row is a refusal (the `EventFeed` zero-bound posture,
/// unchanged).
#[test]
fn zero_capacity_with_admit_all_keys_refuses_every_admitted_row() {
    let hot = HotKeyObserver::bounded(0).with_gate(Arc::new(AdmitAllKeys));
    let observer: &dyn CacheObserver = &hot;
    observer.on_event(&CacheEvent {
        key: "route:/a",
        op: CacheOp::Miss,
    });
    assert!(hot.is_empty());
    assert_eq!(hot.unattributed(), 1);
}

/// `gated` is the shared-shape sugar: the F39 narrowing over the
/// attach-and-read constructor, one expression.
#[test]
fn gated_is_the_shared_shape_with_the_narrowing() {
    let hot = HotKeyObserver::gated(8, Arc::new(HotKeyKindGate::only(&["miss"])));
    let observer: Arc<dyn CacheObserver> = hot.clone();
    observer.on_event(&CacheEvent {
        key: "route:/a",
        op: CacheOp::Set { bytes: 4 },
    });
    observer.on_event(&CacheEvent {
        key: "route:/a",
        op: CacheOp::Miss,
    });
    // Attach-and-read: the typed half reads the ledger behind the gate.
    assert_eq!(hot.len(), 1);
    assert_eq!(hot.count("route:/a"), Some(1));
}

// --------------------------------------------------------------- the custom-impl pin

/// The seam's custom-impl contract: a host implements `HotKeyGate` for
/// its own predicate — a key-prefix gate pins it.
#[test]
fn a_host_gate_predicate_is_honored() {
    /// The host's own admission rule: only rows keyed under `/blog/`.
    struct BlogOnly;

    impl HotKeyGate for BlogOnly {
        fn admits(&self, event: &CacheEvent<'_>) -> bool {
            event.key.starts_with("/blog/")
        }
    }

    let hot = HotKeyObserver::bounded(64).with_gate(Arc::new(BlogOnly));
    push(&hot, "/blog/a", CacheOp::Miss);
    push(&hot, "/docs/a", CacheOp::Miss);
    push(&hot, "/blog/b", CacheOp::Miss);
    let rows = hot.top(10);
    let keys: Vec<&str> = rows.iter().map(|row| row.key.as_str()).collect();
    assert_eq!(keys, vec!["/blog/a", "/blog/b"]);
}

// --------------------------------------------------------------- the debug pin

/// The Debug shape names the gate (the `EventFeed` debug posture: name
/// the attach shape, not the map's internals).
#[test]
fn debug_names_the_gated_shape() {
    let ungated = HotKeyObserver::bounded(8);
    let rendered = format!("{ungated:?}");
    assert!(
        rendered.contains("gated: false"),
        "the ungated ledger names its shape: {rendered}"
    );

    let gated = HotKeyObserver::bounded(8).with_gate(Arc::new(HotKeyKindGate::only(&["hit"])));
    let rendered = format!("{gated:?}");
    assert!(
        rendered.contains("gated: true"),
        "the gated ledger names its shape: {rendered}"
    );
}

// --------------------------------------------------------------- the real-traffic pins

/// Over real cache traffic the gated ledger attributes ONLY the admitted
/// kinds — the deploy-watch shape end to end (hits/misses/sets flowed,
/// the ledger keeps the invalidation and coalesce keys).
#[tokio::test]
async fn gated_ledger_over_real_traffic_attributes_only_admitted_kinds() {
    let hot = Arc::new(
        HotKeyObserver::bounded(64)
            .with_gate(Arc::new(HotKeyKindGate::only(&["invalidate", "coalesce"]))),
    );
    let cache = Arc::new(
        TieredCache::new(64 * 1024 * 1024).with_observer(hot.clone() as Arc<dyn CacheObserver>),
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

    let rows = hot.top(64);
    assert!(
        !rows.is_empty(),
        "the invalidation sweep reaches the gated ledger"
    );
    for row in &rows {
        assert!(
            row.key == "content:posts" || row.key == "route:/a/0",
            "a refused key leaked: {}",
            row.key
        );
    }
    assert_eq!(hot.unattributed(), 0, "refused rows are not refusals");
}

/// The attach shape: the gated ledger coerces to the
/// `Arc<dyn CacheObserver>` the tiers' field stores (the `with_observer`
/// point), unmodified.
#[tokio::test]
async fn gated_ledger_attaches_through_the_seam() {
    let hot =
        Arc::new(HotKeyObserver::bounded(16).with_gate(Arc::new(HotKeyKindGate::only(&["set"]))));
    let observer: Arc<dyn CacheObserver> = hot.clone();
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(observer));
    cache
        .set("route:/a", entry(b"hello", &[]))
        .await
        .expect("the memory tier accepts the entry");
    let _ = cache.get("route:/a").await.expect("the memory tier");

    assert_eq!(
        hot.count("route:/a"),
        Some(1),
        "the hit row was refused, the set row attributed"
    );
}

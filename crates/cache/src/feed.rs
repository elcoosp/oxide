//! The cache plane's bounded event feed (`observer.rs`'s named
//! heavy-consumer shape): a `CacheObserver` that ENQUEUES the rows it
//! receives into a bounded FIFO queue and lets the host's own task drain
//! them elsewhere, instead of running its work inline on the cache's
//! call path.
//!
//! ## Why the feed exists
//!
//! The F28 seam's contract says what a heavy consumer must do: "a slow
//! observer slows the cache. A heavy consumer should enqueue into its own
//! channel and process elsewhere — the server crate's `FileTraceSink`
//! posture (one warn on failure, then degrade) is the model for lossy
//! consumers." Every consumer in the crate so far is inline: the tracing
//! observer renders synchronously, the metrics exporter pays two atomic
//! adds. A consumer that ships rows off-process (a JSONL export, a
//! dashboard batcher, a network hop) cannot run inline without stalling
//! hot L0 hits behind its own backpressure — until now it had to
//! hand-roll the queue, the bound, the drop rule and the warn. This
//! module is that hand-roll, done once, with the posture pinned.
//!
//! ## The contract
//!
//! - Rows are owned `(key, op)` pairs in FIFO arrival order (the seam's
//!   borrow lives only for the callback; the queue owns its copies — the
//!   `CountingObserver::captured` shape).
//! - The queue is bounded; when it is full the ARRIVING row is dropped
//!   and counted. The stream posture, NOT the ring's: a retention ring
//!   evicts the oldest rows silently (right for a window you poll, wrong
//!   for a stream you drain — a stalled consumer keeps what it can still
//!   receive and loses what arrives after saturation).
//! - Drops are never silent: the FIRST drop leaves one `tracing::warn!`
//!   under the `oxide::cache` target naming the capacity (the
//!   `FileTraceSink` sticky rule — one warn, then the rows drop), and the
//!   counter keeps the score for the honest read afterwards
//!   ([`EventFeed::dropped`]). A saturated feed must not spam the console
//!   the operator is already staring at.
//! - A zero capacity retains nothing — every row is a drop (the
//!   `RingLogSink` posture), and the first drop still warns.
//! - The observer half stays synchronous and infallible: `on_event` is
//!   one mutex-protected push (the `std::sync::Mutex` posture the
//!   single-flight module documents — no `.await` held across it), so it
//!   is safe on the cache's call path; the heavy work lives in the
//!   consumer's own task behind [`EventFeed::try_pop`] and
//!   [`EventFeed::drain`].
//!
//! ## The admission gate (F35)
//!
//! The queue retains EVERY row — right for a pipeline consumer, wrong for
//! a narrow watcher: a deploy-watch consumer (`invalidate` + `coalesce`
//! rows only) pays the enqueue for the whole hot-hit stream it would
//! have drained and discarded, and a hot stream saturates its bound
//! before the deploy rows it actually wants arrive. The gate is the
//! record-side narrowing (the server crate's F26 filter-surface shape —
//! `LogGate` narrowed the capture, this narrows the enqueue):
//! [`FeedGate`] sees the borrowed event BEFORE the owned row is built, so
//! a rejected row allocates nothing, never joins the queue, never counts
//! in `dropped()` and never fires the warn — admission is the feed's
//! configured shape, not a degradation. The bound applies behind the
//! gate unchanged: admitted rows saturate exactly as before. The default
//! is the gate spelled as a type ([`AdmitAll`]) — the ungated feed row
//! for row.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::observer::{CacheEvent, CacheObserver, CacheOp};

/// One owned feed row: the key the op ran against plus the op payload
/// (the [`CacheEvent`](crate::CacheEvent) pair, copied out of the borrow
/// the callback received).
pub type FeedRow = (String, CacheOp);

/// The feed's record-side admission predicate (F35, the F26 filter-surface
/// shape transposed): the fanout and the exporters widen who hears an
/// event, the gate narrows what the feed retains.
///
/// The gate sees the BORROWED event — [`on_event`](CacheObserver::on_event)
/// consults it before the owned row is built, so a rejected row allocates
/// nothing and never joins the queue. A rejection is admission, not
/// saturation: it never counts in [`EventFeed::dropped`] and never fires
/// the sticky warn (the warn is the bound's signal).
///
/// `Send + Sync + 'static` so the gate rides the `Arc`-shared feed across
/// the cache's tasks. Implementations must be cheap and never panic — the
/// predicate runs ON the cache's call path, the same contract the
/// [`CacheObserver`](CacheObserver) callback carries.
pub trait FeedGate: Send + Sync + 'static {
    /// Whether the event joins the queue. The `event` borrow lives only
    /// for the call.
    fn admits(&self, event: &CacheEvent<'_>) -> bool;
}

/// The default gate spelled as a type: every row joins the queue. The
/// ungated feed (`bounded`) is this predicate — `with_gate(Arc::new(
/// AdmitAll))` matches it row for row (the F24 posture: the default
/// shape never moves; the type exists so a host can NAME the default).
#[derive(Debug, Default, Clone, Copy)]
pub struct AdmitAll;

impl FeedGate for AdmitAll {
    fn admits(&self, _event: &CacheEvent<'_>) -> bool {
        true
    }
}

/// The domain gate: admits the row kinds named at construction and
/// rejects the rest, keyed on [`CacheEvent::kind`]'s stable names
/// (`"hit"`, `"miss"`, `"set"`, `"delete"`, `"invalidate"`,
/// `"coalesce"`). The deploy-watch shape — `KindGate::only(&[
/// "invalidate", "coalesce"])` stops paying the enqueue for the hot-hit
/// stream a narrow consumer would have drained and discarded.
#[derive(Debug, Clone)]
pub struct KindGate {
    admitted: &'static [&'static str],
}

impl KindGate {
    /// A gate admitting only the named kinds. The slice is held by
    /// reference (`&'static str` names are constants), so construction
    /// allocates nothing.
    #[must_use]
    pub fn only(admitted: &'static [&'static str]) -> Self {
        Self { admitted }
    }
}

impl FeedGate for KindGate {
    fn admits(&self, event: &CacheEvent<'_>) -> bool {
        self.admitted.contains(&event.kind())
    }
}

/// The bounded, lossy FIFO feed: attach one half through the F28 seam
/// (`with_observer` — the `Arc<EventFeed>` coerces to the
/// `Arc<dyn CacheObserver>` the tiers' field stores), drain the other
/// half from the host's own task.
///
/// ```
/// use std::sync::Arc;
/// use oxide_fw_cache::{EventFeed, TieredCache};
///
/// let feed = Arc::new(EventFeed::bounded(1024));
/// let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(feed.clone()));
/// // … the pipeline serves through `cache`; the host's task drains:
/// while let Some((key, op)) = feed.try_pop() {
///     let _ = (key, op); // ship, batch, or render — off the hot path
/// }
/// ```
pub struct EventFeed {
    /// The bounded FIFO. `Mutex` over `VecDeque`: the producer side is
    /// the cache's call path (short critical sections, no await held),
    /// the consumer side is any task the host owns.
    queue: Mutex<CacheQueue>,
    /// The rows the bound dropped (never counted for drained rows, and
    /// never counted for gate rejections — admission is not saturation).
    dropped: AtomicU64,
    /// The F35 admission gate: `None` is the [`AdmitAll`] default (the
    /// ungated shape, byte-identical), `Some` narrows the intake before
    /// the owned row is built.
    gate: Option<Arc<dyn FeedGate>>,
}

impl std::fmt::Debug for EventFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The gate is the feed's attach shape (the FanoutTraceSink debug
        // posture: name the shape, not the children's internals); the
        // queue's own Debug carries its state.
        f.debug_struct("EventFeed")
            .field("queue", &self.queue)
            .field("dropped", &self.dropped.load(Ordering::Relaxed))
            .field("gated", &self.gate.is_some())
            .finish()
    }
}

/// The queue plus its one-shot warn state — one lock guards both, so a
/// drop and its sticky warn are atomic against racing producers (two
/// producers saturating together warn exactly once, not once each).
#[derive(Debug)]
struct CacheQueue {
    rows: VecDeque<FeedRow>,
    capacity: usize,
    /// The sticky warn rule: `false` until the first drop flips it.
    warned: bool,
}

impl EventFeed {
    /// A feed holding at most `capacity` rows. A zero capacity retains
    /// nothing — every row is a drop (the `RingLogSink` posture), and the
    /// sticky warn still fires once. The gate is the untyped default
    /// ([`AdmitAll`]); narrow the intake with [`EventFeed::with_gate`].
    #[must_use]
    pub fn bounded(capacity: usize) -> Self {
        Self {
            queue: Mutex::new(CacheQueue {
                rows: VecDeque::with_capacity(capacity.min(1024)),
                capacity,
                warned: false,
            }),
            dropped: AtomicU64::new(0),
            gate: None,
        }
    }

    /// Narrow the feed's intake (F35): only events the gate admits join
    /// the queue. The gate runs BEFORE the owned row is built, so a
    /// rejected row allocates nothing, never counts in [`EventFeed::dropped`]
    /// and never fires the sticky warn — admission is the feed's
    /// configured shape, not a degradation. The bound applies behind the
    /// gate unchanged; the builder consumes and returns the feed so the
    /// attach chain stays one expression.
    #[must_use = "the gate attaches through the builder chain; dropping it loses the narrowing"]
    pub fn with_gate(mut self, gate: Arc<dyn FeedGate>) -> Self {
        self.gate = Some(gate);
        self
    }

    /// The bound the feed was built with.
    ///
    /// # Panics
    ///
    /// Only if the feed queue lock is poisoned (a prior holder panicked
    /// mid-update) — the same contract the single-flight module's locks
    /// carry.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.queue.lock().expect("the feed queue lock").capacity
    }

    /// Rows currently queued (drained rows and dropped rows are not here).
    ///
    /// # Panics
    ///
    /// Only if the feed queue lock is poisoned (a prior holder panicked
    /// mid-update).
    #[must_use]
    pub fn len(&self) -> usize {
        self.queue.lock().expect("the feed queue lock").rows.len()
    }

    /// True when nothing is queued.
    ///
    /// # Panics
    ///
    /// Only if the feed queue lock is poisoned (a prior holder panicked
    /// mid-update).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue
            .lock()
            .expect("the feed queue lock")
            .rows
            .is_empty()
    }

    /// Rows the bound dropped since attach — the honest score a stalled
    /// consumer reads before deciding what its window means.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Pop the OLDEST row (FIFO). `None` when the queue is empty — the
    /// host's processing loop is a plain `while let`.
    ///
    /// # Panics
    ///
    /// Only if the feed queue lock is poisoned (a prior holder panicked
    /// mid-update).
    pub fn try_pop(&self) -> Option<FeedRow> {
        self.queue
            .lock()
            .expect("the feed queue lock")
            .rows
            .pop_front()
    }

    /// Pop every queued row in arrival order and empty the queue — the
    /// batch arm a dashboard batcher prefers over one-pop-at-a-time.
    ///
    /// # Panics
    ///
    /// Only if the feed queue lock is poisoned (a prior holder panicked
    /// mid-update).
    pub fn drain(&self) -> Vec<FeedRow> {
        let mut queue = self.queue.lock().expect("the feed queue lock");
        queue.rows.drain(..).collect()
    }
}

impl CacheObserver for EventFeed {
    fn on_event(&self, event: &CacheEvent<'_>) {
        // The F35 gate first, on the BORROWED event: a rejected row
        // allocates nothing, never joins the queue, never counts in
        // `dropped()` and never fires the warn — admission is the feed's
        // configured shape, not a degradation. `None` (the default) is
        // the `AdmitAll` shape — one `Option` branch, the ungated path
        // byte-identical.
        if let Some(gate) = &self.gate
            && !gate.admits(event)
        {
            return;
        }
        // One owned copy out of the borrow (the queue owns its rows), one
        // push under the lock. The lock is never held across an await —
        // the seam's synchronous contract — so the cache's call path pays
        // a short critical section, not the consumer's work.
        let row: FeedRow = (event.key.to_owned(), event.op.clone());
        let dropped = {
            let mut queue = self.queue.lock().expect("the feed queue lock");
            if queue.rows.len() < queue.capacity {
                queue.rows.push_back(row);
                None
            } else {
                // The stream posture: the ARRIVING row is the drop, the
                // queued rows survive. The sticky warn fires once — the
                // warn state lives under the same lock as the queue, so
                // two producers saturating together warn exactly once.
                let first_drop = !queue.warned;
                if first_drop {
                    queue.warned = true;
                }
                Some((first_drop, queue.capacity))
            }
        };
        if let Some((first_drop, capacity)) = dropped {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            if first_drop {
                tracing::warn!(
                    target: "oxide::cache",
                    capacity,
                    "cache event feed saturated; further rows drop until the consumer drains (counted in EventFeed::dropped)"
                );
            }
        }
    }
}

/// The `Arc<EventFeed>` sharing convenience: one constructor that hands
/// back the attached-and-drainable pair already split. (`with_observer`
/// takes the `Arc<dyn CacheObserver>` coercion; the host keeps the typed
/// half for draining.) A gated feed rides the same constructor —
/// `EventFeed::gated(capacity, gate)` is
/// `Arc::new(EventFeed::bounded(capacity).with_gate(gate))` with the
/// shapes spelled.
impl EventFeed {
    /// `Arc::new(EventFeed::bounded(capacity))` with the types spelled —
    /// the attach-and-drain shape the module doc shows.
    #[must_use]
    pub fn shared(capacity: usize) -> Arc<Self> {
        Arc::new(Self::bounded(capacity))
    }

    /// The gated feed's shared shape: the F35 narrowing over the
    /// attach-and-drain constructor, one expression.
    #[must_use]
    pub fn gated(capacity: usize, gate: Arc<dyn FeedGate>) -> Arc<Self> {
        Arc::new(Self::bounded(capacity).with_gate(gate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_returns_the_drainable_half() {
        let feed = EventFeed::shared(4);
        let observer: Arc<dyn CacheObserver> = feed.clone();
        observer.on_event(&CacheEvent {
            key: "k",
            op: CacheOp::Set { bytes: 4 },
        });
        assert_eq!(feed.len(), 1);
        let row = feed.try_pop().expect("the row");
        assert_eq!(row.0, "k");
        assert_eq!(feed.capacity(), 4);
    }

    #[test]
    fn oversized_capacity_does_not_preallocate_the_bound() {
        // `with_capacity(capacity.min(1024))`: a huge bound reserves a
        // sane initial allocation, the VecDeque grows as rows arrive.
        let feed = EventFeed::bounded(usize::MAX);
        assert_eq!(feed.capacity(), usize::MAX);
        assert!(feed.is_empty());
    }
}

//! The cache plane's bounded event feed (`observer.rs`'s named
//! heavy-consumer shape): a `CacheObserver` that ENQUEUES the rows it
//! receives into a bounded FIFO queue and lets the host's own task drain
//! them elsewhere, instead of running its work inline on the cache's call
//! path.
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

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::observer::{CacheEvent, CacheObserver, CacheOp};

/// One owned feed row: the key the op ran against plus the op payload
/// (the [`CacheEvent`](crate::CacheEvent) pair, copied out of the borrow
/// the callback received).
pub type FeedRow = (String, CacheOp);

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
#[derive(Debug)]
pub struct EventFeed {
    /// The bounded FIFO. `Mutex` over `VecDeque`: the producer side is
    /// the cache's call path (short critical sections, no await held),
    /// the consumer side is any task the host owns.
    queue: Mutex<CacheQueue>,
    /// The rows the bound dropped (never counted for drained rows).
    dropped: AtomicU64,
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
    /// sticky warn still fires once.
    #[must_use]
    pub fn bounded(capacity: usize) -> Self {
        Self {
            queue: Mutex::new(CacheQueue {
                rows: VecDeque::with_capacity(capacity.min(1024)),
                capacity,
                warned: false,
            }),
            dropped: AtomicU64::new(0),
        }
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
/// half for draining.)
impl EventFeed {
    /// `Arc::new(EventFeed::bounded(capacity))` with the types spelled —
    /// the attach-and-drain shape the module doc shows.
    #[must_use]
    pub fn shared(capacity: usize) -> Arc<Self> {
        Arc::new(Self::bounded(capacity))
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

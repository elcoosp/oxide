//! The per-id hot-key attribution decorator (the observability arm the
//! cache plane's `HotKeyObserver` (F38) already lands for cache events
//! and the rustfn registry story names as the missing attribution arm
//! beside the [`crate::stats::StatsRegistry`] aggregate counters): the
//! [`StatsRegistry`] answers "how many native calls succeeded vs fell
//! back" (one counter set over every id), but NO decorator answers
//! "which native fn ids dominate the registry's traffic" — the per-id
//! ledger a traffic dashboard opens with. The cache plane's answer is
//! `HotKeyObserver` (a bounded per-key event ledger, admission by first
//! sight, the honest saturation posture); this module is that arm for
//! the registry plane.
//!
//! [`HotKeyRegistry`] closes that gap as a decorator: it wraps
//! `Arc<dyn RustFnRegistry>` and IS-A [`crate::host::RustFnRegistry`]
//! itself, so `Arc<HotKeyRegistry>` upcasts to `Arc<dyn RustFnRegistry>`
//! and the dispatcher consults ONE attributed registry through the same
//! object-safe seam it already wires for the boot-time
//! [`crate::host::RustFnHost`], the dev-loop [`crate::swap::SwappableHost`],
//! the multi-host [`crate::chain::ChainedRegistry`], the call-counter
//! [`crate::stats::StatsRegistry`] and the per-call tracer
//! [`crate::tracing::TracingRegistry`] — no per-consult attribution
//! plumbing in the server crate, no fork between an attributed and a
//! plain registry.
//!
//! The contract (the "observe, never alter" rule — the same rule
//! [`crate::stats::StatsRegistry`] and [`crate::tracing::TracingRegistry`]
//! hold):
//!
//! - `call` attributes EVERY call to its id — served OR error. The
//!   ledger is per-id TRAFFIC, not a per-outcome split (the same posture
//!   the cache `HotKeyObserver` holds for cache keys: a hit, a miss, a
//!   set, a delete — each counts ONE toward its key). The inner call's
//!   result — bytes or [`crate::host::RegistryError`] — passes through
//!   UNCHANGED. The decorator observes, it never alters the wire.
//! - `has` / `ids` / `types_json` delegate to the inner registry
//!   UNCOUNTED (they are consult reads, not calls — the same carve-out
//!   [`crate::stats::StatsRegistry`] holds: attributing `has` would
//!   drown the call ledger in consult noise).
//! - The map is bounded ([`HotKeyRegistry::bounded`]); the admission
//!   rule is FIRST SIGHT (the cache `HotKeyObserver` posture): an id
//!   admits while there is room, and once resident it counts past the
//!   bound forever — the ledger never loses an id it already tracks. A
//!   NEW id past the bound is refused: its calls count toward
//!   [`HotKeyRegistry::unattributed`] (the honest saturation posture —
//!   the score is "how much of the stream the ledger covers"). The
//!   default ([`HotKeyRegistry::new`]) is unbounded (every id admits).
//! - [`HotKeyRegistry::top`] is the deterministic read: count DESC, id
//!   ASC on ties, at most `n` rows of owned [`HotKeyRow`] values. Two
//!   reads over the same state are equal, so a snapshot diff is a diff
//!   of the traffic, not of the sort (the F30 renderer doctrine the
//!   cache `HotKeyObserver` holds).
//! - [`HotKeyRegistry::reset`] clears the map and zeroes
//!   `unattributed` (the dev overlay's "refresh the window" verb).
//! - Counts saturate at `u64::MAX` (the F30 saturating rule the cache
//!   `HotKeyObserver` holds): a ledger that reaches the bound reads "at
//!   or past the bound", never a silently-restarted count.
//!
//! The ledger is `std::sync::Mutex`-protected (the cache `HotKeyObserver`
//! posture — no `.await` held across the update), so it is safe on the
//! registry's call path; the reads (`top`, `unattributed`) are the
//! host's own calls. Attach through the decorator constructor and
//! compose beside the stats and tracing decorators behind one nested
//! consult — `HotKeyRegistry::new(TracingRegistry::new(StatsRegistry::new(host)))`
//! attributes, traces AND counts through one consult surface.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use crate::host::{RegistryError, RustFnRegistry};

/// The bound sentinel for an unbounded ledger — every id admits, no id
/// is ever refused. The default constructor ([`HotKeyRegistry::new`])
/// uses this; [`HotKeyRegistry::bounded`] sets a finite cap.
const UNBOUNDED: usize = usize::MAX;

/// One row of the hot-key window — the id and its attributed call
/// count. The [`HotKeyRegistry::top`] read returns a `Vec<HotKeyRow>`
/// sorted count DESC, id ASC on ties, so two reads over the same state
/// are byte-equal (the F30 renderer doctrine the cache `HotKeyObserver`
/// holds).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct HotKeyRow {
    /// The attributed id (the §9.4 route id the call carried).
    pub id: String,
    /// The number of calls attributed to the id (saturating at
    /// `u64::MAX`).
    pub calls: u64,
}

/// The per-id hot-key attribution decorator: `Arc<dyn RustFnRegistry>`
/// behind the [`RustFnRegistry`] seam. `Arc<HotKeyRegistry>` upcasts to
/// `Arc<dyn RustFnRegistry>`, so the dispatcher consults ONE attributed
/// registry through the same object-safe surface it already wires for
/// the boot-time host, the dev-loop host, the chain, the stats decorator
/// and the tracing decorator — the consult runs through the decorator,
/// every call attributes to its id, and the inner registry's result
/// passes through unchanged.
///
/// Construct with [`HotKeyRegistry::new`] (unbounded) or
/// [`HotKeyRegistry::bounded`] (first-sight admission past `cap` ids);
/// [`HotKeyRegistry::top`] reads the deterministic window,
/// [`HotKeyRegistry::unattributed`] reads the refused-ids score,
/// [`HotKeyRegistry::reset`] clears the window, and
/// [`HotKeyRegistry::inner`] exposes the wrapped handle. The decorator
/// composes with [`crate::stats::StatsRegistry`] and
/// [`crate::tracing::TracingRegistry`] — wrap either and the consult
/// counts/traces AND attributes.
pub struct HotKeyRegistry {
    inner: Arc<dyn RustFnRegistry>,
    state: Mutex<State>,
}

/// The mutable ledger state, guarded by the registry's `Mutex`: the
/// per-id counters, the capacity (the first-sight bound), and the
/// unattributed score (the calls past the bound that the ledger could
/// not admit).
struct State {
    /// The per-id call counters. An id admitted by first sight lives
    /// here forever (until `reset`); an id refused past the bound never
    /// enters the map and counts toward `unattributed`.
    counts: HashMap<String, u64>,
    /// The first-sight bound. `UNBOUNDED` admits every id; a finite
    /// value refuses a NEW id once the map holds `cap` entries.
    cap: usize,
    /// The number of calls the ledger could not attribute (the id was
    /// new and the map was full). The honest saturation score.
    unattributed: u64,
}

impl std::fmt::Debug for HotKeyRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The inner trait object is not printable — the debug shape is
        // the observability-relevant view (the top window + the
        // unattributed score), the same shape `StatsRegistry`'s Debug
        // holds over its counters.
        let state = self
            .state
            .lock()
            .expect("the hot-key state mutex is not poisoned");
        f.debug_struct("HotKeyRegistry")
            .field("tracked", &state.counts.len())
            .field("unattributed", &state.unattributed)
            .finish_non_exhaustive()
    }
}

impl HotKeyRegistry {
    /// Wrap `inner` with an UNBOUNDED per-id ledger — every id admits,
    /// no id is ever refused, `unattributed` stays zero. The decorator
    /// takes ownership of the `Arc<dyn RustFnRegistry>` handle — the
    /// caller's clone survives, so a registry the dev task still drives
    /// can be attributed without relinquishing its handle.
    #[must_use]
    pub fn new(inner: Arc<dyn RustFnRegistry>) -> Self {
        Self {
            inner,
            state: Mutex::new(State {
                counts: HashMap::new(),
                cap: UNBOUNDED,
                unattributed: 0,
            }),
        }
    }

    /// Wrap `inner` with a BOUNDED per-id ledger — admission is FIRST
    /// SIGHT: an id admits while the map has room, and once resident it
    /// counts past the bound forever. A NEW id past the bound is
    /// refused: its calls count toward `unattributed` (the honest
    /// saturation posture). A `cap` of zero retains nothing and refuses
    /// every call (the zero-bound posture).
    #[must_use]
    pub fn bounded(inner: Arc<dyn RustFnRegistry>, cap: usize) -> Self {
        Self {
            inner,
            state: Mutex::new(State {
                counts: HashMap::new(),
                cap,
                unattributed: 0,
            }),
        }
    }

    /// The wrapped registry handle (the dev overlay's "introspect the
    /// underlying consult" seam — render the posture snapshot AND, when
    /// the inner registry is a `StatsRegistry`, the call stats from one
    /// wired handle).
    #[must_use]
    pub fn inner(&self) -> &Arc<dyn RustFnRegistry> {
        &self.inner
    }

    /// The deterministic top-`n` window: count DESC, id ASC on ties, at
    /// most `n` rows. Two reads over the same state are equal, so a
    /// snapshot diff is a diff of the traffic, not of the sort.
    ///
    /// This is an observability read, never a panic: a poisoned mutex
    /// (a panic on a racing writer thread) is recovered from — the
    /// ledger renders the last consistent state it can, the same
    /// "observe, never alter" posture the cache `HotKeyObserver`'s
    /// reads hold. A poisoned ledger never crashes the dev overlay.
    #[must_use]
    pub fn top(&self, n: usize) -> Vec<HotKeyRow> {
        let mut rows: Vec<HotKeyRow> = self
            .lock_state()
            .counts
            .iter()
            .map(|(id, calls)| HotKeyRow {
                id: id.clone(),
                calls: *calls,
            })
            .collect();
        // count DESC, id ASC on ties — `HotKeyRow`'s `Ord` is id ASC
        // then calls ASC, so reverse on calls while keeping id ASC by a
        // custom comparator (NOT the derived `Ord`, which would sort id
        // first).
        rows.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.id.cmp(&b.id)));
        rows.truncate(n);
        rows
    }

    /// The number of calls the ledger could not attribute — the ids
    /// that were new past the bound. The honest saturation score: a
    /// dashboard renders it beside the top window as "how much of the
    /// stream the ledger covers". Zero for an unbounded ledger or a
    /// bounded one that has not saturated.
    ///
    /// This is an observability read, never a panic: see [`Self::top`].
    #[must_use]
    pub fn unattributed(&self) -> u64 {
        self.lock_state().unattributed
    }

    /// The number of ids the ledger tracks (the admitted set size).
    /// The dev overlay's "is the ledger saturated?" check —
    /// `tracked() == cap` means every new id is refused.
    ///
    /// This is an observability read, never a panic: see [`Self::top`].
    #[must_use]
    pub fn tracked(&self) -> usize {
        self.lock_state().counts.len()
    }

    /// Clear the map and zero `unattributed` — the dev overlay's
    /// "refresh the window" verb (start a clean slate for a new
    /// measurement interval). In-flight calls that resolve after the
    /// reset still attribute; the reset is a window boundary, not a
    /// quiescence point.
    ///
    /// This is an observability verb, never a panic: a poisoned mutex is
    /// recovered from (see [`Self::top`]) — the reset clears the
    /// recovered state, so a fresh window starts even across a writer's
    /// panic.
    pub fn reset(&self) {
        let mut state = self.lock_state();
        state.counts.clear();
        state.unattributed = 0;
    }

    /// Lock the ledger state, recovering from a poisoned mutex instead
    /// of panicking. A poison means a writer thread panicked while
    /// holding the lock (a `saturating_add` cannot, but a future
    /// mutation might); the ledger is observability state, not
    /// synchronization — the last consistent state is a better read
    /// than a crash, so the recovery is the honest posture. Every read
    /// and write goes through this helper, so the no-panic contract is
    /// one place, not five.
    fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Attribute one call to `id`. The first-sight admission rule: an id
    /// already in the map bumps (resident forever); a new id admits
    /// while the map has room under `cap`; a new id past the bound
    /// counts toward `unattributed` and never enters the map. The
    /// counter saturates at `u64::MAX`.
    fn attribute(&self, id: &str) {
        let mut state = self.lock_state();
        if let Some(count) = state.counts.get_mut(id) {
            // Resident — bump (saturating at u64::MAX, the F30 rule).
            *count = count.saturating_add(1);
            return;
        }
        // New id. Admit while there is room; refuse past the bound.
        if state.counts.len() < state.cap {
            state.counts.insert(id.to_string(), 1);
        } else {
            state.unattributed = state.unattributed.saturating_add(1);
        }
    }
}

#[async_trait::async_trait]
impl RustFnRegistry for HotKeyRegistry {
    fn has(&self, id: &str) -> bool {
        // Delegate uncounted — `has` is a consult read the posture
        // snapshots already own; attributing it would drown the call
        // ledger in consult noise, the same carve-out `StatsRegistry`
        // holds for its counters.
        self.inner.has(id)
    }

    fn ids(&self) -> Vec<String> {
        // Delegate uncounted — the sorted id set is posture, not a call.
        self.inner.ids()
    }

    fn types_json(&self) -> Option<String> {
        // Delegate uncounted — the types payload is posture, not a call.
        self.inner.types_json()
    }

    async fn call(&self, id: &str, args_json: &[u8]) -> Result<Vec<u8>, RegistryError> {
        // Attribute EVERY call to its id — served OR error. The ledger
        // is per-id TRAFFIC, not a per-outcome split (the cache
        // `HotKeyObserver` posture: every completed event counts one).
        // The inner call's result passes through UNCHANGED.
        let result = self.inner.call(id, args_json).await;
        self.attribute(id);
        result
    }
}

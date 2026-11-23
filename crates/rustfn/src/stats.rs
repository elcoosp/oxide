//! The call-outcome observability decorator (the gap the substrate's own
//! rustdocs name): `host.rs` on [`crate::host::RegistryReport`] — "the
//! registry has no traffic counters to expose yet; the report is a
//! posture read for the overlay's 'is the host healthy?' check"; and
//! `chain.rs` on [`crate::chain::ChainedReport`] — "the chain owns no
//! traffic counters; the report is a posture read." The posture
//! snapshots answer "is the host attached?"; they do NOT answer "how
//! many native calls succeeded vs fell back, and which wire arm
//! refused?" — the question the server crate's `RustFirstDispatcher`
//! and the dev overlay both need to decide whether the native registry
//! is healthy under load.
//!
//! [`StatsRegistry`] closes that gap as a decorator: it wraps
//! `Arc<dyn RustFnRegistry>` and IS-A [`crate::host::RustFnRegistry`]
//! itself, so `Arc<StatsRegistry>` upcasts to `Arc<dyn RustFnRegistry>`
//! and the dispatcher consults ONE instrumented registry through the
//! same object-safe seam it already wires for the boot-time
//! [`crate::host::RustFnHost`], the dev-loop [`crate::swap::SwappableHost`]
//! and the multi-host [`crate::chain::ChainedRegistry`] — no per-consult
//! counter plumbing in the server crate, no fork between an
//! instrumented and a plain registry.
//!
//! The contract (the "observe, never alter" rule):
//!
//! - `call` bumps `calls_attempted` on entry, delegates to the inner
//!   registry, bumps `calls_served` on `Ok` and the matching per-kind
//!   counter on `Err` (the six [`crate::host::RegistryErrorKind`] arms).
//!   The inner call's result — bytes or [`crate::host::RegistryError`] —
//!   passes through UNCHANGED. The decorator never invents, drops or
//!   rewrites an outcome.
//! - `has` / `ids` / `types_json` delegate to the inner registry
//!   UNCOUNTED (they are consult reads, not calls — the posture
//!   snapshots already own that surface; double-counting `has` would
//!   drown the call counters in consult noise).
//! - [`StatsRegistry::stats`] is ONE snapshot over the counters, not N —
//!   the same "one consult surface" contract
//!   [`crate::host::RegistryReport`] and [`crate::chain::ChainedReport`]
//!   hold. [`CallStats`] mirrors the six [`RegistryErrorKind`] arms plus
//!   `calls_attempted` and `calls_served`, so the overlay renders the
//!   full traffic shape through ONE read.
//!
//! The counters are lock-free `AtomicU64`s (one per arm) — a call bumps
//! exactly two (attempted + the outcome) under `Relaxed` ordering, so an
//! instrumented consult never serializes on the counter. The snapshot is
//! a cheap copy of eight `u64`s, allocation-free; the dev overlay polls
//! it on its refresh tick the same way it polls the posture snapshots.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;

use crate::host::{RegistryError, RegistryErrorKind, RustFnRegistry};

/// A snapshot of a [`StatsRegistry`]'s traffic — the call-outcome
/// counters the posture snapshots do not carry. ONE read over the
/// decorator's counters, not N — the dev overlay renders the report, it
/// does not read each counter separately.
///
/// The fields mirror the six [`RegistryErrorKind`] arms plus
/// `calls_attempted` (every `call` invocation) and `calls_served`
/// (the `Ok` outcomes). The invariant the decorator holds:
/// `calls_attempted` equals `calls_served` plus
/// [`CallStats::error_count`] — every call lands in exactly one
/// outcome bucket.
///
/// The fields are a snapshot, not live counters (the decorator owns the
/// atomics; the snapshot is the cheap copy the overlay diff reads) —
/// the same shape [`crate::host::RegistryReport`] holds for posture.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CallStats {
    /// Every `call` invocation (the denominator; the sum of
    /// `calls_served` and [`CallStats::error_count`]).
    pub calls_attempted: u64,
    /// The `Ok` outcomes (the calls the native registry served).
    pub calls_served: u64,
    /// The `NotFound` outcomes — an id no member owned (the consult's
    /// JS-pool fallback takes these back).
    pub not_found: u64,
    /// The `Wire` outcomes — the args JSON did not convert to ODF.
    pub wire: u64,
    /// The `FnError` outcomes — the fn returned `Err` (status 1).
    pub fn_error: u64,
    /// The `Panic` outcomes — the fn panicked (status 2; the shim caught
    /// the unwind).
    pub panic: u64,
    /// The `Timeout` outcomes — the fn exceeded its per-call budget.
    pub timeout: u64,
    /// The `Internal` outcomes — the host itself failed (a closed
    /// semaphore lane, a dead task).
    pub internal: u64,
}

impl CallStats {
    /// Whether the decorator has seen any traffic (no `call` was
    /// attempted). The dev overlay's "is the instrumented registry
    /// wired and serving?" sentinel — `is_empty()` is true even when
    /// the inner registry owns ids, if no call has crossed the decorator
    /// yet.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.calls_attempted == 0
    }

    /// The derived total of the six error arms (`calls_attempted -
    /// calls_served`). The overlay's "error rate" numerator —
    /// `error_count() as f64 / calls_attempted as f64` is the served vs
    /// fell-back ratio the dispatcher health check reads.
    #[must_use]
    pub const fn error_count(&self) -> u64 {
        self.not_found + self.wire + self.fn_error + self.panic + self.timeout + self.internal
    }
}

/// The lock-free counter set a [`StatsRegistry`] bumps on every call —
/// one `AtomicU64` per [`CallStats`] field. The snapshot reads eight
/// `Relaxed` loads; the bump writes two `Relaxed` fetch-adds (attempted
/// and the outcome). `Relaxed` is correct because the counters are
/// observability state, not synchronization — the decorator's contract
/// is "count every outcome," and the snapshot's contract is "a cheap
/// copy of the current values," neither orders against the inner call.
#[derive(Default)]
struct Counters {
    attempted: AtomicU64,
    served: AtomicU64,
    not_found: AtomicU64,
    wire: AtomicU64,
    fn_error: AtomicU64,
    panic: AtomicU64,
    timeout: AtomicU64,
    internal: AtomicU64,
}

impl Counters {
    /// A cheap copy of the eight counters — the snapshot the overlay
    /// reads. `Relaxed` loads: the values are monotone counters, the
    /// snapshot does not order against any call.
    #[must_use]
    fn snapshot(&self) -> CallStats {
        CallStats {
            calls_attempted: self.attempted.load(Relaxed),
            calls_served: self.served.load(Relaxed),
            not_found: self.not_found.load(Relaxed),
            wire: self.wire.load(Relaxed),
            fn_error: self.fn_error.load(Relaxed),
            panic: self.panic.load(Relaxed),
            timeout: self.timeout.load(Relaxed),
            internal: self.internal.load(Relaxed),
        }
    }

    /// Zero every counter — the dev overlay's "refresh the window" verb
    /// (start a clean slate for a new measurement interval) and the
    /// test suite's "reset between pins" helper.
    fn reset(&self) {
        self.attempted.store(0, Relaxed);
        self.served.store(0, Relaxed);
        self.not_found.store(0, Relaxed);
        self.wire.store(0, Relaxed);
        self.fn_error.store(0, Relaxed);
        self.panic.store(0, Relaxed);
        self.timeout.store(0, Relaxed);
        self.internal.store(0, Relaxed);
    }

    /// Bump the counter matching one error kind. The `Ok` arm bumps
    /// `served` directly in [`StatsRegistry::call`] — this helper is the
    /// `Err` arm only.
    fn bump_error(&self, kind: RegistryErrorKind) {
        let slot = match kind {
            RegistryErrorKind::NotFound => &self.not_found,
            RegistryErrorKind::Wire => &self.wire,
            RegistryErrorKind::FnError => &self.fn_error,
            RegistryErrorKind::Panic => &self.panic,
            RegistryErrorKind::Timeout => &self.timeout,
            RegistryErrorKind::Internal => &self.internal,
        };
        slot.fetch_add(1, Relaxed);
    }
}

/// The call-outcome observability decorator: `Arc<dyn RustFnRegistry>`
/// behind the [`RustFnRegistry`] seam. `Arc<StatsRegistry>` upcasts to
/// `Arc<dyn RustFnRegistry>`, so the dispatcher consults ONE
/// instrumented registry through the same object-safe surface it
/// already wires for the boot-time [`crate::host::RustFnHost`], the
/// dev-loop [`crate::swap::SwappableHost`] and the multi-host
/// [`crate::chain::ChainedRegistry`] — the consult runs through the
/// decorator, every outcome lands in ONE counter set, and the inner
/// registry's result passes through unchanged.
///
/// Construct with [`StatsRegistry::new`]; [`StatsRegistry::stats`]
/// reads the snapshot, [`StatsRegistry::reset`] zeroes the window, and
/// [`StatsRegistry::inner`] exposes the wrapped handle so the dev
/// overlay can render the posture snapshot AND the call stats from one
/// wired handle. The decorator composes with [`crate::chain::ChainedRegistry`]
/// — wrap `Arc<ChainedRegistry>` and every outcome across every member
/// lands in one counter set covering every native crate behind the chain.
pub struct StatsRegistry {
    inner: Arc<dyn RustFnRegistry>,
    counters: Arc<Counters>,
}

impl std::fmt::Debug for StatsRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The trait object is not printable — the debug shape is the
        // observability-relevant view (the snapshot over the counters).
        f.debug_struct("StatsRegistry")
            .field("stats", &self.stats())
            .finish()
    }
}

impl StatsRegistry {
    /// Wrap `inner` with call-outcome counters. The decorator takes
    /// ownership of the `Arc<dyn RustFnRegistry>` handle — the caller's
    /// clone survives, so a registry the dev task still drives can be
    /// instrumented without relinquishing its handle.
    #[must_use]
    pub fn new(inner: Arc<dyn RustFnRegistry>) -> Self {
        Self {
            inner,
            counters: Arc::new(Counters::default()),
        }
    }

    /// The wrapped registry handle (the dev overlay's "introspect the
    /// underlying consult" seam — render the posture snapshot AND the
    /// call stats from one wired handle).
    #[must_use]
    pub fn inner(&self) -> &Arc<dyn RustFnRegistry> {
        &self.inner
    }

    /// A snapshot of the call-outcome counters — ONE read over the
    /// decorator's eight atomics, not N. The overlay renders the full
    /// traffic shape (served vs each error arm) through this one read.
    #[must_use]
    pub fn stats(&self) -> CallStats {
        self.counters.snapshot()
    }

    /// Zero every counter — the dev overlay's "refresh the window" verb
    /// (start a clean slate for a new measurement interval). In-flight
    /// calls that resolve after the reset still bump the counters; the
    /// reset is a snapshot boundary, not a quiescence point.
    pub fn reset(&self) {
        self.counters.reset();
    }
}

#[async_trait::async_trait]
impl RustFnRegistry for StatsRegistry {
    fn has(&self, id: &str) -> bool {
        // Delegate uncounted — `has` is a consult read the posture
        // snapshots already own; counting it would drown the call
        // counters in consult noise.
        self.inner.has(id)
    }

    fn ids(&self) -> Vec<String> {
        // Delegate uncounted — the sorted id set is posture, not traffic.
        self.inner.ids()
    }

    fn types_json(&self) -> Option<String> {
        // Delegate uncounted — the types payload is posture, not traffic.
        self.inner.types_json()
    }

    async fn call(&self, id: &str, args_json: &[u8]) -> Result<Vec<u8>, RegistryError> {
        // `attempted` bumps on entry, before the delegate — a call that
        // panics inside the inner registry (it cannot: the shims catch
        // unwinds and the swap lock is panic-free by construction) would
        // still be counted as attempted. The outcome bucket (served or
        // the matching error arm) bumps once the delegate resolves.
        self.counters.attempted.fetch_add(1, Relaxed);
        match self.inner.call(id, args_json).await {
            Ok(out) => {
                self.counters.served.fetch_add(1, Relaxed);
                Ok(out)
            }
            Err(e) => {
                self.counters.bump_error(e.kind);
                Err(e)
            }
        }
    }
}

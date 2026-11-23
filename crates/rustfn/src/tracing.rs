//! The per-call tracing decorator (the observability arm the cache
//! plane's `TracingCacheObserver` (F22) already lands for cache events
//! and the rustfn registry story names as the missing piece beside the
//! [`crate::stats::StatsRegistry`] call-outcome counters): the
//! [`StatsRegistry`] answers "how many calls succeeded vs fell back"
//! (a counter snapshot the dev overlay polls), but NO decorator answers
//! "which id ran, how long it took, and what outcome it returned" — the
//! per-call structured-trace surface the dev overlay and the operator's
//! `oxide::rustfn` log scrape need to attribute one slow native call
//! inside a mixed consult. The cache plane's answer is
//! `TracingCacheObserver` (one `tracing` event per cache op under
//! `oxide::cache`); this module is that arm for the registry plane.
//!
//! [`TracingRegistry`] closes that gap as a decorator: it wraps
//! `Arc<dyn RustFnRegistry>` and IS-A [`crate::host::RustFnRegistry`]
//! itself, so `Arc<TracingRegistry>` upcasts to `Arc<dyn RustFnRegistry>`
//! and the dispatcher consults ONE traced registry through the same
//! object-safe seam it already wires for the boot-time
//! [`crate::host::RustFnHost`], the dev-loop [`crate::swap::SwappableHost`],
//! the multi-host [`crate::chain::ChainedRegistry`] and the call-counter
//! [`crate::stats::StatsRegistry`] — no per-consult tracing plumbing in
//! the server crate, no fork between a traced and a plain registry.
//!
//! The contract (the "observe, never alter" rule — the same rule
//! [`crate::stats::StatsRegistry`] holds):
//!
//! - `call` opens ONE `tracing` span per native call (target
//!   `oxide::rustfn`, the id and the outcome kind as fields, the
//!   elapsed wall time as a `debug` field) and delegates to the inner
//!   registry. The inner call's result — bytes or
//!   [`crate::host::RegistryError`] — passes through UNCHANGED. The
//!   decorator never invents, drops or rewrites an outcome; it emits,
//!   it does not gate.
//! - `has` / `ids` / `types_json` delegate to the inner registry
//!   UNTRACED (they are consult reads, not calls — the same carve-out
//!   [`crate::stats::StatsRegistry`] holds: a span per `has` would
//!   drown the call spans in consult noise the same way a counter per
//!   `has` would drown the call counters).
//! - The decorator composes with [`crate::stats::StatsRegistry`]: wrap
//!   an `Arc<StatsRegistry>` and the consult STILL counts AND traces —
//!   every outcome lands in ONE `CallStats` snapshot AND ONE tracing
//!   span, the same "one consult surface" composition the stats rustdoc
//!   names for [`crate::chain::ChainedRegistry`] (wrap a chain, every
//!   outcome across every member lands in one counter set), lifted to
//!   the tracing arm.
//!
//! The span is emitted through the `tracing` facade the workspace
//! already pins (`oxide::cache` for the cache plane, `oxide::request`
//! for the request log); this module owns the `oxide::rustfn` target so
//! an operator's `RUST_LOG=oxide::rustfn=debug` scrape surfaces every
//! native call without the cache or request rows. A `tracing`
//! subscriber is the host's concern, never the crate's: the decorator
//! emits unconditionally, and a `no-subscriber` build pays only the
//! `enabled!`-guarded field-formatting cost (the `tracing` macro's own
//! fast path).

use std::sync::Arc;

use crate::host::{RegistryError, RegistryErrorKind, RustFnRegistry};

/// The `tracing` target the per-call span emits under (the same
/// `oxide::<plane>` convention the cache plane's `TracingCacheObserver`
/// holds under `oxide::cache`). An operator's `RUST_LOG=oxide::rustfn=debug`
/// scrape surfaces every native call without the cache or request rows.
pub const RUSTFN_TRACE_TARGET: &str = "oxide::rustfn";

/// The per-call tracing decorator: `Arc<dyn RustFnRegistry>` behind the
/// [`RustFnRegistry`] seam. `Arc<TracingRegistry>` upcasts to
/// `Arc<dyn RustFnRegistry>`, so the dispatcher consults ONE traced
/// registry through the same object-safe surface it already wires for
/// the boot-time [`crate::host::RustFnHost`], the dev-loop
/// [`crate::swap::SwappableHost`], the multi-host
/// [`crate::chain::ChainedRegistry`] and the call-counter
/// [`crate::stats::StatsRegistry`] — the consult runs through the
/// decorator, every call opens ONE `tracing` span, and the inner
/// registry's result passes through unchanged.
///
/// Construct with [`TracingRegistry::new`]; [`TracingRegistry::inner`]
/// exposes the wrapped handle so the dev overlay can render the
/// posture snapshot AND the call stats (when the inner registry is a
/// `StatsRegistry`) from one wired handle. The decorator composes with
/// [`crate::stats::StatsRegistry`] — wrap an `Arc<StatsRegistry>` and
/// every call counts AND traces.
pub struct TracingRegistry {
    inner: Arc<dyn RustFnRegistry>,
}

impl std::fmt::Debug for TracingRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The trait object is not printable — the debug shape names the
        // decorator and its target (the observability-relevant view, the
        // same shape `StatsRegistry`'s Debug holds over its counters).
        f.debug_struct("TracingRegistry")
            .field("target", &RUSTFN_TRACE_TARGET)
            .finish_non_exhaustive()
    }
}

impl TracingRegistry {
    /// Wrap `inner` with a per-call tracing span. The decorator takes
    /// ownership of the `Arc<dyn RustFnRegistry>` handle — the caller's
    /// clone survives, so a registry the dev task still drives can be
    /// traced without relinquishing its handle.
    #[must_use]
    pub fn new(inner: Arc<dyn RustFnRegistry>) -> Self {
        Self { inner }
    }

    /// The wrapped registry handle (the dev overlay's "introspect the
    /// underlying consult" seam — render the posture snapshot AND, when
    /// the inner registry is a `StatsRegistry`, the call stats from one
    /// wired handle).
    #[must_use]
    pub fn inner(&self) -> &Arc<dyn RustFnRegistry> {
        &self.inner
    }
}

/// The outcome label the per-call span carries — the string form of the
/// [`RegistryErrorKind`] arm the call resolved to, or `"served"` for the
/// `Ok` arm. One field per call, so an operator's log scrape groups by
/// outcome without re-decoding the error.
fn outcome_label(result: &Result<Vec<u8>, RegistryError>) -> &'static str {
    match result {
        Ok(_) => "served",
        Err(e) => match e.kind {
            RegistryErrorKind::NotFound => "not_found",
            RegistryErrorKind::Wire => "wire",
            RegistryErrorKind::FnError => "fn_error",
            RegistryErrorKind::Panic => "panic",
            RegistryErrorKind::Timeout => "timeout",
            RegistryErrorKind::Internal => "internal",
        },
    }
}

#[async_trait::async_trait]
impl RustFnRegistry for TracingRegistry {
    fn has(&self, id: &str) -> bool {
        // Delegate untraced — `has` is a consult read the posture
        // snapshots already own; a span per `has` would drown the call
        // spans in consult noise, the same carve-out `StatsRegistry`
        // holds for its counters.
        self.inner.has(id)
    }

    fn ids(&self) -> Vec<String> {
        // Delegate untraced — the sorted id set is posture, not a call.
        self.inner.ids()
    }

    fn types_json(&self) -> Option<String> {
        // Delegate untraced — the types payload is posture, not a call.
        self.inner.types_json()
    }

    async fn call(&self, id: &str, args_json: &[u8]) -> Result<Vec<u8>, RegistryError> {
        // The delegate runs first; the outcome label is derived from the
        // result, so the `debug!` row carries the resolution the cache
        // plane's `TracingCacheObserver` carries per cache op. The
        // elapsed wall time is the attribution field the operator's
        // scrape groups slow native calls by — recorded after the
        // delegate resolves, so a call that hangs inside the inner
        // registry (a timeout the host's `spawn_blocking` arm catches)
        // surfaces its full duration, not a partial one.
        let started = std::time::Instant::now();
        let result = self.inner.call(id, args_json).await;
        let elapsed = started.elapsed();
        // Saturate the micros at `u64::MAX` (a single native call's
        // micros cannot realistically overflow — `u64` micros is ~584k
        // years — but the `u128 → u64` cast is truncating, so the
        // saturating `try_from` keeps the field clippy-clean without an
        // `#[allow]` and without lying about a pathological duration).
        let elapsed_us = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        match &result {
            Ok(_) => tracing::debug!(
                target: RUSTFN_TRACE_TARGET,
                id,
                outcome = "served",
                elapsed_us,
                "native server fn served"
            ),
            Err(e) => tracing::debug!(
                target: RUSTFN_TRACE_TARGET,
                id,
                outcome = outcome_label(&result),
                error = %e,
                elapsed_us,
                "native server fn refused"
            ),
        }
        result
    }
}

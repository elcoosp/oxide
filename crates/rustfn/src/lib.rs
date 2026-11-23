//! oxide-rustfn — the V2-F08 native server functions HOST side: the
//! loader, the registry and the ODF-over-C-ABI bridge the
//! `*.server.rs` cdylibs speak (spec §V2-F08, decision D33: model A —
//! the user crate is a `cdylib` loaded with `libloading`, no Rust
//! types or allocators cross the boundary).
//!
//! This is the substrate milestone (`feat/rustfn-host-seam`): the ABI
//! ([`abi`]), the ODF bridge ([`bridge`]), the panic-safe shim
//! ([`shim`]), the library loader + id-mapped registry ([`host`]) and
//! the `.d.ts` renderer ([`types`]). The proc macros
//! (`#[server_fn]`, `#[derive(TsType)]`, `export_table!`) re-export
//! from [`oxide_macros`] so user code imports this one crate — and
//! the `TsType` trait and its derive share one name, the serde
//! pattern.
//!
//! The dev-loop hot-swap state ([`swap`]) and the multi-host consult
//! adapter ([`chain`]) ride on the substrate: `SwappableHost` is the
//! `RustFnHost` behind a short mutex the dev task drives, and
//! `ChainedRegistry` is the "consult each host in turn" seam the spec
//! names — `Arc<ChainedRegistry>` IS-A `Arc<dyn RustFnRegistry>`, so the
//! dispatcher consults ONE chain over every native crate's host.
//!
//! The call-outcome observability decorator ([`stats`]) closes the gap
//! the substrate's own rustdocs name ("the registry has no traffic
//! counters to expose yet"): `StatsRegistry` wraps
//! `Arc<dyn RustFnRegistry>` and IS-A `RustFnRegistry` itself, so
//! `Arc<StatsRegistry>` upcasts to `Arc<dyn RustFnRegistry>` and the
//! dispatcher consults ONE instrumented registry — every served call
//! and every wire-arm refusal lands in a [`stats::CallStats`] snapshot
//! the dev overlay reads through the same "one consult surface"
//! contract the posture snapshots hold.
//!
//! The per-call tracing decorator ([`tracing`]) is the observability
//! arm beside the counters: `TracingRegistry` wraps
//! `Arc<dyn RustFnRegistry>` and IS-A `RustFnRegistry` itself, so
//! `Arc<TracingRegistry>` upcasts to `Arc<dyn RustFnRegistry>` and the
//! dispatcher consults ONE traced registry — every native call opens a
//! `tracing` span under `oxide::rustfn` carrying the id and the
//! outcome, the cache plane's `TracingCacheObserver` (F22) transposed
//! to the registry plane. The decorator composes with `StatsRegistry`
//! (wrap an `Arc<StatsRegistry>` and the consult counts AND traces).
//!
//! The per-id hot-key attribution decorator ([`hot_keys`]) is the
//! attribution arm beside the counters and the spans: `HotKeyRegistry`
//! wraps `Arc<dyn RustFnRegistry>` and IS-A `RustFnRegistry` itself, so
//! `Arc<HotKeyRegistry>` upcasts to `Arc<dyn RustFnRegistry>` and the
//! dispatcher consults ONE attributed registry — every call attributes
//! to its id, the cache plane's `HotKeyObserver` (F38) transposed to
//! the registry plane. The decorator composes with `StatsRegistry` and
//! `TracingRegistry` (wrap either and the consult counts/traces AND
//! attributes).

pub mod abi;
pub mod bridge;
pub mod chain;
pub mod host;
pub mod hot_keys;
pub mod shim;
pub mod stats;
pub mod swap;
pub mod tracing;
pub mod types;

use oxide_fw_diagnostics::OxideError;

pub use crate::host::{
    RegistryError, RegistryErrorKind, RegistryReport, RustFnHost, RustFnLibrary, RustFnRegistry,
};
// V2-F08 M1: the dev-loop host IS-A `RustFnRegistry` — the consult the
// dispatcher wires behind `Arc<dyn RustFnRegistry>` answers through
// both the boot-time host and the swappable host, so the server crate
// no longer forks between them.
// V2-F08 M2: `RegistryReport` re-exports from `host` here; `swap`'s
// own re-export serves `use oxide_rustfn::swap::RegistryReport`.
pub use crate::swap::{FnBinding, RetireReport, SwapReport, SwappableHost};
// V2-F08 M3: the multi-host consult adapter — `ChainedRegistry` IS-A
// `RustFnRegistry`, so the dispatcher consults ONE chain over every
// native crate's host through the same object-safe seam the boot-time
// `RustFnHost` and the dev-loop `SwappableHost` already answer. The
// "consult each host in turn" the host and swap modules named as the
// missing piece lives here (spec §V2-F08 "Host").
pub use crate::chain::{ChainedRegistry, ChainedReport};
// V2-F08 M4: the call-outcome observability decorator — `StatsRegistry`
// IS-A `RustFnRegistry`, so the dispatcher consults ONE instrumented
// registry through the same object-safe seam it already wires for the
// boot-time host, the dev-loop host and the chain. The posture snapshots
// answer "is the host attached?"; `CallStats` answers "how many native
// calls succeeded vs fell back, and which wire arm refused?" — the gap
// the substrate's own rustdocs name ("the registry has no traffic
// counters to expose yet").
pub use crate::stats::{CallStats, StatsRegistry};
// V2-F08 M5: the per-call tracing decorator — `TracingRegistry` IS-A
// `RustFnRegistry`, so the dispatcher consults ONE traced registry
// through the same object-safe seam it already wires for the boot-time
// host, the dev-loop host, the chain and the stats decorator. The
// counters answer "how many calls succeeded vs fell back"; the tracing
// arm answers "which id ran, how long, and what outcome" — the
// per-call structured-trace surface the cache plane's
// `TracingCacheObserver` (F22) already lands for cache events,
// transposed to the registry plane. The decorator composes with
// `StatsRegistry` (wrap an `Arc<StatsRegistry>` and the consult counts
// AND traces).
pub use crate::tracing::{RUSTFN_TRACE_TARGET, TracingRegistry};
// V2-F08 M6: the per-id hot-key attribution decorator — `HotKeyRegistry`
// IS-A `RustFnRegistry`, so the dispatcher consults ONE attributed
// registry through the same object-safe seam it already wires for the
// boot-time host, the dev-loop host, the chain, the stats decorator
// and the tracing decorator. The counters answer "how many calls
// succeeded vs fell back"; the tracing arm answers "which id ran, how
// long, and what outcome"; the hot-key arm answers "which ids dominate
// the registry's traffic" — the per-id ledger a traffic dashboard opens
// with, the cache plane's `HotKeyObserver` (F38) transposed to the
// registry plane. The decorator composes with `StatsRegistry` and
// `TracingRegistry` (wrap either and the consult counts/traces AND
// attributes).
pub use crate::hot_keys::{HotKeyRegistry, HotKeyRow};
// The macro namespace and the trait namespace share one name (the
// serde pattern): `use oxide_rustfn::TsType` serves both
// `#[derive(TsType)]` and `T: TsType`.
pub use crate::types::TsType;
pub use oxide_macros::TsType;
pub use oxide_macros::{export_table, server_fn};

/// The convenience error the `#[server_fn]` bodies return (any
/// `Display` error works; this one carries a plain message).
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ServerError {
    /// The message the envelope carries (`FN-THREW` 400).
    pub message: String,
}

impl ServerError {
    /// An error with a plain message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<String> for ServerError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for ServerError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

/// The house error helper (the compiler/odf pattern): a coded
/// [`OxideError`].
fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Box::new(oxide_fw_diagnostics::Diagnostic::error(code, msg)))
}

/// The user-facing prelude (`use oxide_rustfn::prelude::*;` — the
/// spec's user-crate import shape).
pub mod prelude {
    pub use crate::ServerError;
    pub use crate::chain::ChainedRegistry;
    pub use crate::host::{
        RegistryError, RegistryReport, RustFnHost, RustFnLibrary, RustFnRegistry,
    };
    pub use crate::hot_keys::HotKeyRegistry;
    pub use crate::stats::{CallStats, StatsRegistry};
    pub use crate::swap::{FnBinding, SwappableHost};
    pub use crate::tracing::TracingRegistry;
    pub use crate::{TsType, export_table, server_fn};
}

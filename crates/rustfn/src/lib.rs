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
//! The remaining F08 halves are the CLI-owned loops documented in
//! `docs/deviations.md`: the dev cargo-build/watch half and the
//! production `dist/native/<lib>` emission ride the `rust_fns` fact
//! the R02 refusal seam already carries; the dispatcher consult lands
//! in the server crate (`RustFirstDispatcher`).

pub mod abi;
pub mod bridge;
pub mod host;
pub mod shim;
pub mod swap;
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
    pub use crate::host::{
        RegistryError, RegistryReport, RustFnHost, RustFnLibrary, RustFnRegistry,
    };
    pub use crate::swap::{FnBinding, SwappableHost};
    pub use crate::{TsType, export_table, server_fn};
}

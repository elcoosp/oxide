//! The dev-loop hot-swap state (the §V2-F08 dev-loop half:
//! `feat/server-lib-dev-loop`). The spec: "`oxide dev` … hot-swaps by
//! loading the new copy and retiring the old `Arc<Library>` after
//! in-flight calls finish."
//!
//! [`RustFnHost`] is a boot-time structure: `mount`/`register` take
//! `&mut self`, so the dispatcher that consults it freezes at startup.
//! The dev loop needs the same state to change UNDER a running server:
//! every `*.server.rs` rebuild mounts a new library and re-points the
//! §9.4 ids at it, while the calls already in flight finish against the
//! library they resolved.
//!
//! [`SwappableHost`] is that state: the host behind a short mutex whose
//! critical sections are lookup-and-clone only — a call resolves its
//! library `Arc` under the lock and runs OUTSIDE it, so a swap never
//! waits on a call and a call never waits on a swap. Retiring an orphaned
//! mount (a library no id references anymore) is refcount-honest: a
//! mount an in-flight call still holds (`Arc::strong_count > 1`) survives
//! the retire pass and is reclaimed by the next one — the spec's retire
//! rule, enforced rather than promised.
//!
//! The host serves ONE native crate's §9.4 id set (one host per crate;
//! the consult adapter consults each host in turn): the swap's bindings
//! are that crate's full post-rebuild set, so "the ids the bindings no
//! longer name are unregistered" is safe. The id bindings (`FnBinding`)
//! are re-derived per swap by the loop from the library's types side
//! export and the build secret (the ONE `server_fn_id` definition the
//! compiler crate owns) — the consult falls back to the JS pool for a
//! removed export instead of answering through a stale library.

use std::sync::Mutex;

use oxide_fw_diagnostics::OxideError;

use crate::host::{RegistryError, RegistryErrorKind, RustFnHost, RustFnLibrary};

// V2-F08 M2: re-export the introspection surface so `use
// oxide_rustfn::swap::RegistryReport` serves the dev overlay (the
// same shape the boot-time host exposes — the "one consult surface"
// contract, lifted to the report).
pub use crate::host::RegistryReport;

/// One id → export pair of a rebuilt library: the loop's swap input, so
/// the host never re-derives ids from raw types payloads (the loop owns
/// the secret and the ONE id definition).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnBinding {
    /// The §9.4 id (`server_fn_id(file, export, secret)`).
    pub id: String,
    /// The export name the entry is indexed under.
    pub fn_name: String,
}

impl FnBinding {
    /// A binding for one export of the rebuilt library.
    pub fn new(id: impl Into<String>, fn_name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            fn_name: fn_name.into(),
        }
    }
}

/// What one swap did: the ids the bindings re-pointed, the ids they added
/// (never seen before) and the ids they dropped (previously registered,
/// absent from the new bindings). Every list is sorted, so the report is
/// byte-stable for the dev overlay and the tests.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SwapReport {
    /// The generation after this swap (1 on the first).
    pub generation: u64,
    /// Previously-registered ids the bindings still name (re-pointed).
    pub rebound: Vec<String>,
    /// Ids the bindings name that the host had not seen before.
    pub added: Vec<String>,
    /// Previously-registered ids absent from the new bindings
    /// (unregistered — the JS pool consult takes them back).
    pub dropped: Vec<String>,
    /// The binding count this swap registered under.
    pub total_ids: usize,
}

/// What one retire pass did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RetireReport {
    /// Orphaned mounts reclaimed this pass.
    pub retired: usize,
    /// Mounts kept (referenced by an id, or still on loan to an
    /// in-flight call).
    pub kept: usize,
}

/// The swappable host state: a [`RustFnHost`] behind a short mutex, with
/// the dev loop's swap / retire verbs and the two-phase call the consult
/// path runs. `Arc<SwappableHost>` is the shape the dev task and the
/// future `NativeFnSource` adapter share.
///
/// Every accessor locks the swap mutex and panics on a poisoned lock —
/// the state itself is panic-free by construction (the C ABI shims catch
/// unwinds), so a poison means a panic while holding the lock, and the
/// dev loop's host is unsalvageable at that point.
pub struct SwappableHost {
    state: Mutex<Swapped>,
}

struct Swapped {
    host: RustFnHost,
    generation: u64,
}

impl Default for SwappableHost {
    fn default() -> Self {
        Self::new()
    }
}

impl SwappableHost {
    /// An empty host (no mounts, no ids, generation 0).
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(Swapped {
                host: RustFnHost::new(),
                generation: 0,
            }),
        }
    }

    /// A swappable host wrapping a pre-built [`RustFnHost`] (V2-F08 M2:
    /// the dev overlay's `with_timeout` / `with_lanes` config surface
    /// needs a way to construct the swappable host from a configured
    /// boot-time host — the `RegistryReport` introspection exposes
    /// the configured budget and lane count, so the overlay's "is the
    /// budget set?" check reads them back).
    ///
    /// The host's mounted libraries and registered ids carry through —
    /// the dev loop's first swap re-points them at the rebuilt library.
    /// The generation starts at `0` (the swap state's own count; the
    /// boot-time host's mounts are generation `0`).
    #[must_use]
    pub fn with_host(host: RustFnHost) -> Self {
        Self {
            state: Mutex::new(Swapped {
                host,
                generation: 0,
            }),
        }
    }

    /// The swap count so far (0 on a fresh host; each `swap` bumps by 1).
    ///
    /// # Panics
    ///
    /// When the swap lock is poisoned (a call panicked while holding it).
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.state.lock().expect("the swap lock").generation
    }

    /// A snapshot of the swappable host's posture for the dev overlay's
    /// observability (V2-F08 M2): the live generation, the mount count,
    /// the id count, the sorted ids, whether a types payload is mounted,
    /// the per-function timeout, and the blocking-lane count. ONE read
    /// over the swap lock, not N — the dev overlay renders the report,
    /// it does not take the lock per field.
    ///
    /// The boot-time [`crate::host::RustFnHost`] answers the same shape
    /// (its `report` reports generation `0`), so the dev overlay
    /// introspects EITHER host through [`crate::host::RegistryReport`]
    /// — the same "one consult surface" contract M1 landed for
    /// [`crate::host::RustFnRegistry::call`].
    ///
    /// # Panics
    ///
    /// When the swap lock is poisoned (a call panicked while holding it).
    #[must_use]
    pub fn report(&self) -> crate::host::RegistryReport {
        let st = self.state.lock().expect("the swap lock");
        crate::host::RegistryReport {
            generation: st.generation,
            mount_count: st.host.mount_count(),
            id_count: st.host.ids().len(),
            ids: st.host.ids(),
            has_types: st.host.types_json().is_some(),
            timeout_secs: st.host.timeout_for_consult().as_secs(),
            lanes: st.host.lanes(),
        }
    }

    /// Mount the rebuilt library and re-point every binding at it. Ids the
    /// bindings still name are re-registered under the new mount; ids the
    /// host knew that the bindings no longer name are UNREGISTERED (the
    /// consult's JS-pool fallback takes them back); ids the bindings add
    /// register fresh. The old mount stays until [`Self::retire_idle`]
    /// reclaims it — never mid-call.
    ///
    /// # Errors
    ///
    /// Only the poisoned-lock case surfaces as an error-shaped refusal;
    /// the registry itself cannot refuse here (the library arrived
    /// through the loader's gates and the bindings are host-shaped).
    pub fn swap(
        &self,
        library: RustFnLibrary,
        bindings: &[FnBinding],
    ) -> Result<SwapReport, OxideError> {
        let mut st = self.state.lock().map_err(|_| SwappableHost::poisoned())?;
        let index = st.host.mount(library);
        let incoming: std::collections::BTreeSet<&str> =
            bindings.iter().map(|b| b.id.as_str()).collect();
        let mut rebound = Vec::new();
        let mut added = Vec::new();
        for binding in bindings {
            if st.host.has(&binding.id) {
                rebound.push(binding.id.clone());
            } else {
                added.push(binding.id.clone());
            }
            st.host.register(&binding.id, index, &binding.fn_name);
        }
        let dropped: Vec<String> = st
            .host
            .ids()
            .into_iter()
            .filter(|id| !incoming.contains(id.as_str()))
            .collect();
        for id in &dropped {
            st.host.unregister(id);
        }
        rebound.sort();
        added.sort();
        st.generation += 1;
        Ok(SwapReport {
            generation: st.generation,
            rebound,
            added,
            dropped,
            total_ids: bindings.len(),
        })
    }

    /// Whether the id is natively owned right now.
    ///
    /// # Panics
    ///
    /// When the swap lock is poisoned (a call panicked while holding it).
    #[must_use]
    pub fn has(&self, id: &str) -> bool {
        self.state.lock().expect("the swap lock").host.has(id)
    }

    /// The registered ids (sorted).
    ///
    /// # Panics
    ///
    /// When the swap lock is poisoned (a call panicked while holding it).
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        self.state.lock().expect("the swap lock").host.ids()
    }

    /// Whether anything is registered.
    ///
    /// # Panics
    ///
    /// When the swap lock is poisoned (a call panicked while holding it).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state.lock().expect("the swap lock").host.is_empty()
    }

    /// The §9.4 call: resolve the library `Arc` + export name UNDER the
    /// swap lock, convert the args JSON to the ODF bytes, then run the
    /// call OUTSIDE the lock — an in-flight call holds only its library
    /// Arc, so swaps (and retires of OTHER mounts) never wait on it.
    ///
    /// # Errors
    ///
    /// [`RegistryError`] with the wire mapping (unknown id →
    /// `FN-NOT-FOUND`; args JSON → `FN-BAD-REQUEST`; the call's own
    /// statuses ride [`crate::host::RawCallError`]).
    ///
    /// # Panics
    ///
    /// When the swap lock is poisoned (a call panicked while holding it).
    pub fn call_now(&self, id: &str, args_json: &[u8]) -> Result<Vec<u8>, RegistryError> {
        let (lib, name, input) = {
            let st = self.state.lock().expect("the swap lock");
            let Some((lib, name)) = st.host.resolve_call(id) else {
                return Err(RegistryError {
                    kind: RegistryErrorKind::NotFound,
                    message: format!("no native function registered under id {id:?}"),
                });
            };
            let input = crate::host::args_to_odf(args_json).map_err(|e| RegistryError {
                kind: RegistryErrorKind::Wire,
                message: e.to_string(),
            })?;
            (lib, name, input)
        };
        lib.call_raw(&name, &input).map_err(RegistryError::from)
    }

    /// Reclaim the orphaned mounts: a library no id references anymore
    /// AND no in-flight call holds (`Arc::strong_count == 1` — only the
    /// mount list owns it) is dropped, with the surviving mounts'
    /// indices re-compacted so the id table stays consistent. A mount an
    /// id still references is NEVER retired, whatever its refcount; a
    /// mount on loan to an in-flight call survives this pass and is
    /// reclaimed by the next one (the spec's retire rule).
    ///
    /// # Panics
    ///
    /// When the swap lock is poisoned (a call panicked while holding it).
    #[must_use]
    pub fn retire_idle(&self) -> RetireReport {
        let mut st = self.state.lock().expect("the swap lock");
        let before = st.host.mount_count();
        let retired = st.host.retire_idle_mounts();
        let kept = before - retired;
        RetireReport { retired, kept }
    }

    fn poisoned() -> OxideError {
        crate::err(
            crate::host::RFN_LOAD,
            "the swappable host state is poisoned (a call panicked while holding the swap lock)",
        )
    }
}

/// The consult seam the dispatcher wires: `Arc<SwappableHost>` IS-A
/// `Arc<dyn RustFnRegistry>`, so the dev-loop host and the boot-time
/// [`crate::host::RustFnHost`] share ONE consult surface. The
/// dispatcher no longer forks between a boot-time registry and a
/// dev-loop adapter — the same `has` / `call` / `types_json` the
/// server crate's `RustFirstDispatcher` consults answers through both.
///
/// `has` and `types_json` consult the live swap state through the
/// short mutex. `call` resolves the library `Arc` + entry name AND
/// clones the host's permit semaphore + timeout UNDER the swap lock,
/// converts the args JSON to the ODF bytes the boundary carries, then
/// runs the call OUTSIDE the lock through the host's shared permit +
/// `spawn_blocking` + timeout trio — an in-flight call holds only its
/// library `Arc` and a permit, so a swap never waits on it and a call
/// never waits on a swap (the same two-phase shape [`Self::call_now`]
/// runs, lifted onto the runtime).
///
/// # Panics
///
/// The lock accessors panic on a poisoned swap lock — the same policy
/// [`SwappableHost`]'s other accessors hold (the state is panic-free
/// by construction; a poison means the dev loop's host is unsalvageable).
#[async_trait::async_trait]
impl crate::host::RustFnRegistry for SwappableHost {
    fn has(&self, id: &str) -> bool {
        SwappableHost::has(self, id)
    }

    fn types_json(&self) -> Option<String> {
        let st = self.state.lock().expect("the swap lock");
        st.host.types_json()
    }

    async fn call(
        &self,
        id: &str,
        args_json: &[u8],
    ) -> Result<Vec<u8>, crate::host::RegistryError> {
        use crate::host::{RegistryError, RegistryErrorKind};

        // Resolve the library `Arc` + entry name AND clone the host's
        // permit semaphore + budget UNDER the swap lock, convert the
        // args JSON to the ODF bytes the boundary carries — then run
        // the call OUTSIDE the lock through the shared trio. A call in
        // flight holds only its library `Arc` and a permit, so swaps
        // (and retires of OTHER mounts) never wait on it, and the swap
        // lock is never held across an await.
        let (lib, name, input, permits, timeout) = {
            let st = self.state.lock().expect("the swap lock");
            let Some((lib, name)) = st.host.resolve_call(id) else {
                return Err(RegistryError {
                    kind: RegistryErrorKind::NotFound,
                    message: format!("no native function registered under id {id:?}"),
                });
            };
            let input = crate::host::args_to_odf(args_json).map_err(|e| RegistryError {
                kind: RegistryErrorKind::Wire,
                message: e.to_string(),
            })?;
            (
                lib,
                name,
                input,
                st.host.permits_for_consult(),
                st.host.timeout_for_consult(),
            )
        };
        crate::host::run_resolved_call(permits, timeout, lib, name, input).await
    }
}

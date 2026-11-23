//! The multi-host consult adapter (spec §V2-F08 "Host": "one host per
//! crate; the consult adapter consults each host in turn"). The substrate
//! landed the boot-time [`crate::host::RustFnHost`] and the dev-loop
//! [`crate::swap::SwappableHost`], each owning ONE native crate's §9.4 id
//! set; the consult that walks the members in order — the seam the host
//! and swap modules both NAME as "the consult adapter" but leave unbuilt
//! — lives here.
//!
//! [`ChainedRegistry`] holds `Vec<Arc<dyn RustFnRegistry>>` and IS-A
//! [`crate::host::RustFnRegistry`] itself: `Arc<ChainedRegistry>` upcasts
//! to `Arc<dyn RustFnRegistry>`, so the dispatcher consults ONE chain
//! through the same object-safe seam it already wires for the boot-time
//! and dev-loop hosts — no fork per native crate, no per-host match in
//! the server crate's `RustFirstDispatcher`.
//!
//! The consult contract (the spec's "each host in turn", made
//! deterministic):
//!
//! - `has(id)` is true when ANY member owns the id.
//! - `call(id, args)` walks the members in PUSH order and delegates to
//!   the FIRST that `has(id)` — the order the dispatcher relies on, so a
//!   cross-crate id collision (two crates that happen to share an id)
//!   resolves one way, every time. An id no member owns refuses with the
//!   `FN-NOT-FOUND` wire mapping.
//! - `ids()` is the SORTED, deduplicated union of every member's
//!   registered ids (the [`crate::host::RustFnRegistry::ids`] seam the
//!   chain's report renders through).
//! - `types_json()` MERGES every member's types payload: the `fns`
//!   arrays concatenate and the `decls` maps union (first contribution
//!   wins — the same dedup [`crate::types::types_json`] already holds), so
//!   the `.d.ts` the host renders covers every native crate behind one
//!   consult. A member with no payload contributes nothing; a malformed
//!   payload is skipped (logged `RFN-TYPES`, never panics) so one bad
//!   crate does not blank the whole declaration file.
//!
//! [`ChainedReport`] is the chain's observability snapshot — the same
//! "one consult surface" contract [`crate::host::RegistryReport`] holds
//! for a single host, lifted to the chain: the member count, the sorted
//! union of ids, the union id count, and whether ANY member carries a
//! types payload. The dev overlay renders the chain's posture through
//! ONE read, not N.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::host::{RegistryError, RegistryErrorKind, RustFnRegistry};
use crate::types::{TYPES_JSON, TypesPayload};

/// A snapshot of the chain's posture for the dev overlay's observability:
/// the member count, the sorted union of every member's registered ids,
/// the union id count, and whether ANY member carries a types payload.
///
/// ONE consult over the chain, not N — the dev overlay renders the
/// report, it does not call `has` per id. The same shape answers for a
/// one-member chain and a many-member chain, so the overlay introspects
/// EITHER through [`ChainedReport`] — the same "one consult surface"
/// contract [`crate::host::RegistryReport`] holds for a single host.
///
/// The fields are a snapshot, not live counters (the chain owns no
/// traffic counters; the report is a posture read for the overlay's "is
/// the native registry wired?" check).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChainedReport {
    /// The member count (one per native crate the chain consults).
    pub member_count: usize,
    /// The union id count across every member (deduplicated — an id two
    /// members both own counts once).
    pub total_ids: usize,
    /// The sorted, deduplicated union of every member's registered ids
    /// (sorted so two reports over the same state are byte-equal for the
    /// dev overlay's diff).
    pub ids: Vec<String>,
    /// Whether ANY member carries a types payload (the same consult
    /// `RustFnRegistry::types_json` answers for the chain).
    pub has_types: bool,
}

impl ChainedReport {
    /// Whether the chain owns any id (no member registered anything).
    /// The dev overlay's "is anything native-mounted?" sentinel —
    /// `is_empty()` is true even when `member_count` is non-zero, if no
    /// member registered an id.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total_ids == 0
    }
}

/// The multi-host consult adapter: `Vec<Arc<dyn RustFnRegistry>>` behind
/// the `RustFnRegistry` seam. `Arc<ChainedRegistry>` upcasts to
/// `Arc<dyn RustFnRegistry>`, so the dispatcher consults ONE chain
/// through the same object-safe surface it already wires for the
/// boot-time [`crate::host::RustFnHost`] and the dev-loop
/// [`crate::swap::SwappableHost`] — the per-crate "consult each host in
/// turn" the spec names, made deterministic by the push order.
///
/// Construct with [`ChainedRegistry::new`] (empty) or
/// [`ChainedRegistry::from_members`]; [`ChainedRegistry::push`] appends a
/// member. The consult walks the members in the order they were pushed.
pub struct ChainedRegistry {
    members: Vec<Arc<dyn RustFnRegistry>>,
}

impl Default for ChainedRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ChainedRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The trait objects are not printable — the debug shape is the
        // consult-relevant view (the member count and the union id set).
        f.debug_struct("ChainedRegistry")
            .field("member_count", &self.members.len())
            .field("ids", &self.ids())
            .finish()
    }
}

impl ChainedRegistry {
    /// An empty chain (no members). The consult refuses every id with
    /// `FN-NOT-FOUND` until a member is pushed.
    #[must_use]
    pub fn new() -> Self {
        Self {
            members: Vec::new(),
        }
    }

    /// A chain over `members`, consulted in the order given. The chain
    /// takes ownership of the `Arc<dyn RustFnRegistry>` handles — the
    /// caller's clone survives, so a host the dev loop still drives can
    /// be a member of the chain without relinquishing its handle.
    #[must_use]
    pub fn from_members(members: Vec<Arc<dyn RustFnRegistry>>) -> Self {
        Self { members }
    }

    /// Append a member at the end of the consult order. The first member
    /// that `has(id)` wins the consult, so push order is the priority.
    pub fn push(&mut self, member: Arc<dyn RustFnRegistry>) {
        self.members.push(member);
    }

    /// The member count (one per native crate the chain consults).
    #[must_use]
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether the chain has any member.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// The members in consult order (the slice the dispatcher could walk
    /// directly, though the `RustFnRegistry` impl already does).
    #[must_use]
    pub fn members(&self) -> &[Arc<dyn RustFnRegistry>] {
        &self.members
    }

    /// A snapshot of the chain's posture for the dev overlay's
    /// observability: the member count, the sorted union of every
    /// member's registered ids, the union id count, and whether ANY
    /// member carries a types payload. ONE consult over the chain, not
    /// N — the overlay renders the report, it does not call `has` per id.
    #[must_use]
    pub fn report(&self) -> ChainedReport {
        let mut ids: BTreeSet<String> = BTreeSet::new();
        let mut has_types = false;
        for member in &self.members {
            ids.extend(member.ids());
            if member.types_json().is_some() {
                has_types = true;
            }
        }
        let ids_vec: Vec<String> = ids.into_iter().collect();
        let total_ids = ids_vec.len();
        ChainedReport {
            member_count: self.members.len(),
            total_ids,
            ids: ids_vec,
            has_types,
        }
    }
}

#[async_trait::async_trait]
impl RustFnRegistry for ChainedRegistry {
    fn has(&self, id: &str) -> bool {
        self.members.iter().any(|m| m.has(id))
    }

    fn ids(&self) -> Vec<String> {
        // The sorted, deduplicated union — a BTreeSet gives both for free
        // (the host's `ids` is already sorted, so the union preserves the
        // order; the set deduplicates a cross-crate id collision).
        let mut ids: BTreeSet<String> = BTreeSet::new();
        for member in &self.members {
            ids.extend(member.ids());
        }
        ids.into_iter().collect()
    }

    fn types_json(&self) -> Option<String> {
        merge_types_payloads(self.members.iter().filter_map(|m| m.types_json()))
    }

    async fn call(&self, id: &str, args_json: &[u8]) -> Result<Vec<u8>, RegistryError> {
        // Walk the members in push order; the FIRST that `has(id)` wins
        // the consult (the spec's "each host in turn", made deterministic
        // by the push order). A member that does not own the id is never
        // consulted for it, so a cross-crate id collision resolves one
        // way every time.
        for member in &self.members {
            if member.has(id) {
                return member.call(id, args_json).await;
            }
        }
        Err(RegistryError {
            kind: RegistryErrorKind::NotFound,
            message: format!(
                "no native function registered under id {id:?} (consulted {} members)",
                self.members.len()
            ),
        })
    }
}

/// Merge the members' `types_json` payloads into one declaration file
/// payload: the `fns` arrays concatenate and the `decls` maps union
/// (first contribution wins — the same dedup [`crate::types::types_json`]
/// already holds). A payload that does not parse as a
/// [`TypesPayload`] is skipped (logged `RFN-TYPES`, never panics) so one
/// bad crate does not blank the whole `.d.ts`. Returns `None` when no
/// member contributed a parseable payload.
fn merge_types_payloads(payloads: impl Iterator<Item = String>) -> Option<String> {
    let mut fns = Vec::new();
    let mut decls: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let mut contributed = 0usize;
    for payload in payloads {
        match serde_json::from_str::<TypesPayload>(&payload) {
            Ok(member) => {
                fns.extend(member.fns);
                for (name, block) in member.decls {
                    decls.entry(name).or_insert(block);
                }
                contributed += 1;
            }
            Err(e) => {
                // A malformed payload is a corrupt crate the loader
                // somehow mounted — the merge drops it (logged, never
                // panics) and serves the rest, so one bad crate never
                // blanks the whole `.d.ts`. The house warn convention
                // (the panic record's shape): the code names the refusal.
                tracing::warn!(
                    code = TYPES_JSON,
                    error = %e,
                    "a chained member's types payload did not parse; the merge skips it and keeps the rest"
                );
            }
        }
    }
    if contributed == 0 {
        return None;
    }
    let merged = TypesPayload { fns, decls };
    Some(serde_json::to_string(&merged).inspect_err(|e| {
        tracing::error!(
            code = crate::bridge::RFN_WIRE,
            error = %e,
            "the merged types payload failed to serialize; the chain's types_json carries an EMPTY payload"
        );
    }).unwrap_or_default())
}

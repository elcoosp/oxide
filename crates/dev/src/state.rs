//! The V2-E01 **phase-4 state-preservation substrate** (spec §V2-E01).
//!
//! The boundary planner ([`crate::boundary::plan_classified`]) already
//! answers "which modules swap in place?"; the acceptance it could not
//! answer yet is the spec's third HMR proof: *type into an input, edit the
//! component's text, the input value survives*. That needs a verdict the
//! client can act on — is this update **state-preserving** (snapshot and
//! restore the DOM state around the swap) or **state-clearing** (the page
//! is going away; say so with the reason)?
//!
//! The verdict is a property of the plan, so it is computed where the plan
//! is computed and it RIDES THE WIRE: the §13.3 v1 `update` frame carries a
//! `preserve` field (`{"policy":"keep"}` or
//! `{"policy":"clear","reason":...}`). The field is additive and
//! backward-compatible — a frame from a pre-substrate build decodes with
//! the default verdict ([`Preservation::Keep`], what those servers always
//! meant), and an old client ignores the unknown field. The client half
//! (snapshot/restore, the fallback-reload stash) lives in
//! `packages/core/src/client/hmr.ts`.
//!
//! The decision itself is the injectable [`StatePolicy`] seam: the default
//! ([`DefaultStatePolicy`]) maps `Update` → keep and `FullReload` → clear
//! with the planner's reason, and a custom policy can flip any class (the
//! same injectable pattern the F18 `Probe` seam set). [`update_frame`] is
//! the single construction seam the fan-out uses, so every published
//! `update` frame carries a verdict and the `hmr_state_preservation` span
//! makes the per-tick decision visible under `RUST_LOG=oxide_fw_dev=debug`.

use crate::boundary::{ClassifiedPlan, Plan};
use crate::{HmrMessage, UpdateEntry};
use serde::{Deserialize, Serialize};

/// The §13.3 v1 `preserve` verdict (E01 phase 4): what the client may
/// assume about DOM state across this update.
///
/// The wire spelling is internally tagged on `policy` —
/// `{"policy":"keep"}` or `{"policy":"clear","reason":"..."}` — so the TS
/// client narrows on one discriminant and the reason travels with the
/// clear verdict (a stale-tab reload can then log WHY the state died).
///
/// The default is [`Preservation::Keep`]: every pre-substrate `update`
/// frame was boundary-scoped and never cleared client state, so a frame
/// without the field decodes to the verdict those servers always meant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "kebab-case")]
pub enum Preservation {
    /// The update is boundary-scoped: the client snapshots DOM state
    /// (form fields, focus) before applying the entries and restores it
    /// after — including across a fallback reload, where the snapshot
    /// rides `sessionStorage` instead.
    Keep,
    /// The update clears client state. Carries the planner's reload
    /// reason (`<module> has no HMR boundary` / `<module> is not
    /// imported`) so the loss is explainable, not mysterious.
    Clear {
        /// Why the state dies — the [`Plan::FullReload`] reason.
        reason: String,
    },
}

impl Default for Preservation {
    /// Every pre-substrate update was boundary-scoped, so `Keep` is the
    /// faithful default for frames that predate the field.
    fn default() -> Self {
        Preservation::Keep
    }
}

impl Preservation {
    /// `true` when the verdict allows a snapshot/restore cycle.
    #[must_use]
    pub fn is_keep(&self) -> bool {
        matches!(self, Preservation::Keep)
    }
}

/// The injectable state-policy seam (E01 phase 4): decides the
/// [`Preservation`] verdict for a fan-out's [`ClassifiedPlan`].
///
/// Implementations must be pure and cheap — the fan-out calls this per
/// published frame, inside the watcher's debounce window, and the verdict
/// is deterministic wire contract (the same plan must always produce the
/// same verdict, or two tabs disagree about whether their input survives).
pub trait StatePolicy: Send + Sync {
    /// The verdict for `plan`.
    fn preservation(&self, plan: &ClassifiedPlan) -> Preservation;
}

/// The spec's default policy (E01 phase 4): a boundary-scoped update
/// ([`Plan::Update`]) keeps client state — the accepting module swaps in
/// place — and a [`Plan::FullReload`] clears it, carrying the planner's
/// reason verbatim so the client's log line matches the overlay's.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultStatePolicy;

impl StatePolicy for DefaultStatePolicy {
    fn preservation(&self, plan: &ClassifiedPlan) -> Preservation {
        match &plan.plan {
            Plan::Update(_) => Preservation::Keep,
            Plan::FullReload(reason) => Preservation::Clear {
                reason: reason.clone(),
            },
        }
    }
}

/// The frame-construction seam (E01 phase 4): stamp the policy's verdict
/// for `plan` onto the v1 `update` frame carrying `updates`.
///
/// Every fan-out publish site goes through this function, so a frame is
/// never published without a verdict and the decision is observable in one
/// place (`hmr_state_preservation` debug span + the stamped field).
pub fn update_frame(
    plan: &ClassifiedPlan,
    policy: &dyn StatePolicy,
    updates: Vec<UpdateEntry>,
) -> HmrMessage {
    let _span = tracing::debug_span!("hmr_state_preservation", kind = plan.kind.as_str()).entered();
    let preserve = policy.preservation(plan);
    match &preserve {
        Preservation::Keep => {
            tracing::debug!("hmr_state_preservation: keep — the client snapshots around the swap");
        }
        Preservation::Clear { reason } => {
            tracing::debug!(
                reason = %reason,
                "hmr_state_preservation: clear — the client drops state with the reason"
            );
        }
    }
    HmrMessage::Update { updates, preserve }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_spelling_is_internally_tagged_on_policy() {
        let keep = serde_json::to_string(&Preservation::Keep).unwrap();
        assert_eq!(keep, r#"{"policy":"keep"}"#);
        let clear = serde_json::to_string(&Preservation::Clear {
            reason: "config changed".into(),
        })
        .unwrap();
        assert_eq!(clear, r#"{"policy":"clear","reason":"config changed"}"#);
    }
}

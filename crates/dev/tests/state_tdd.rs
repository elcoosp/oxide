//! TDD pins for the V2-E01 phase-4 state-preservation substrate (spec
//! §V2-E01: "type into an input, edit the component's text, the input value
//! survives").
//!
//! Written FIRST (red): failed to compile before `dev::state` existed and
//! the v1 `Update` frame carried no `preserve` verdict.
//!
//! The substrate is the server half of state-preserving HMR: the fan-out's
//! [`ClassifiedPlan`] is already the boundary verdict, and the new seam
//! turns that verdict into a wire-visible `preserve` field the client can
//! act on (snapshot/restore around the swap; stash across a fallback
//! reload). The client half lives in `packages/core/src/client/hmr.ts`.

use oxide_fw_dev::boundary::{ClassifiedPlan, Plan, UpdateKind};
use oxide_fw_dev::state::{DefaultStatePolicy, Preservation, StatePolicy, update_frame};
use oxide_fw_dev::{HmrMessage, UpdateEntry};

/// A `js` boundary update keeps client state: the accepting module swaps in
/// place (React Refresh / adapter runtime), so the input value survives.
#[test]
fn default_policy_keeps_state_on_a_boundary_update() {
    let plan = ClassifiedPlan {
        kind: UpdateKind::Js,
        plan: Plan::Update(vec!["src/routes/index/page.tsx".into()]),
    };
    assert_eq!(
        DefaultStatePolicy.preservation(&plan),
        Preservation::Keep,
        "a boundary-scoped update is state-preserving"
    );
}

/// A full reload clears client state, and the verdict carries the planner's
/// reason so the client (and the overlay) can say WHY the input was lost.
#[test]
fn default_policy_clears_state_on_a_full_reload_with_the_planners_reason() {
    let reason = "src/lib/util.ts is not imported";
    let plan = ClassifiedPlan {
        kind: UpdateKind::Js,
        plan: Plan::FullReload(reason.into()),
    };
    match DefaultStatePolicy.preservation(&plan) {
        Preservation::Clear { reason: cleared } => assert_eq!(cleared, reason),
        other => panic!("expected Clear with the planner's reason, got {other:?}"),
    }
}

/// The wire contract: an `update` frame stamped `keep` serializes the
/// internally-tagged verdict and round-trips through the v1 gate.
#[test]
fn update_frame_carries_the_preserve_verdict_through_the_wire() {
    let entry = UpdateEntry::stamped(
        UpdateKind::Js,
        "src/routes/index/page.tsx",
        "/src/routes/index/page.tsx",
        None,
    );
    let plan = ClassifiedPlan {
        kind: UpdateKind::Js,
        plan: Plan::Update(vec!["src/routes/index/page.tsx".into()]),
    };
    let frame = update_frame(&plan, &DefaultStatePolicy, vec![entry]);
    let json = frame.to_json();
    assert!(json.contains("\"type\":\"update\""), "{json}");
    assert!(
        json.contains("\"preserve\":{\"policy\":\"keep\"}"),
        "{json}"
    );
    match HmrMessage::from_json(&json) {
        Some(HmrMessage::Update { preserve, .. }) => {
            assert_eq!(preserve, Preservation::Keep, "round-tripped verdict");
        }
        other => panic!("expected an Update frame, got {other:?}"),
    }
}

/// A `Clear` verdict rides with the reason so a stale-tab reload can log
/// why the state was dropped.
#[test]
fn a_clear_verdict_serializes_its_reason() {
    let entry = UpdateEntry::stamped(UpdateKind::Js, "src/a.tsx", "/src/a.tsx", None);
    let plan = ClassifiedPlan {
        kind: UpdateKind::Js,
        plan: Plan::FullReload("src/a.tsx is not imported".into()),
    };
    let json = update_frame(&plan, &DefaultStatePolicy, vec![entry]).to_json();
    assert!(
        json.contains(
            "\"preserve\":{\"policy\":\"clear\",\"reason\":\"src/a.tsx is not imported\"}"
        ),
        "{json}"
    );
}

/// Backward compatibility: a v1 frame from a build that predates the
/// substrate (no `preserve` field) still decodes — as `Keep`, the verdict
/// those servers always meant (a boundary-scoped update never cleared
/// state on the client).
#[test]
fn a_pre_substrate_frame_decodes_to_keep() {
    let legacy = r#"{"v":1,"type":"update","updates":[{"kind":"js","path":"src/a.tsx","acceptedPath":"/src/a.tsx","timestamp":1,"hash":""}]}"#;
    match HmrMessage::try_from_json(legacy) {
        Ok(HmrMessage::Update { preserve, .. }) => {
            assert_eq!(
                preserve,
                Preservation::Keep,
                "absent field defaults to Keep"
            );
        }
        other => panic!("expected a decodable Update frame, got {other:?}"),
    }
}

/// The policy is injectable: a custom policy (here, a never-preserve one)
/// flips a boundary update to `Clear` — the seam the dev server's future
/// state-aware modes hang on, and the pin that proves the trait is load
/// bearing rather than decorative.
#[test]
fn the_policy_seam_is_injectable() {
    struct NeverPreserve;
    impl StatePolicy for NeverPreserve {
        fn preservation(&self, _plan: &ClassifiedPlan) -> Preservation {
            Preservation::Clear {
                reason: "policy: never preserve".into(),
            }
        }
    }
    let plan = ClassifiedPlan {
        kind: UpdateKind::Css,
        plan: Plan::Update(vec!["src/styles/app.css".into()]),
    };
    assert_eq!(
        NeverPreserve.preservation(&plan),
        Preservation::Clear {
            reason: "policy: never preserve".into()
        }
    );
    let json = update_frame(&plan, &NeverPreserve, vec![]).to_json();
    assert!(json.contains("\"policy\":\"clear\""), "{json}");
}

/// The default verdict is `Keep` so an explicitly constructed frame (the
/// pre-substrate shape) means what it always meant.
#[test]
fn preservation_defaults_to_keep() {
    assert_eq!(Preservation::default(), Preservation::Keep);
    assert!(Preservation::Keep.is_keep());
    assert!(!Preservation::Clear { reason: "x".into() }.is_keep());
}

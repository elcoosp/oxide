//! The state-preservation walkthrough (V2-E01 phase 4): watch the planner
//! verdicts turn into `preserve` verdicts on the wire — the seam that makes
//! "type into an input, edit the component's text, the input value
//! survives" a server-side contract instead of an adapter accident.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dev --example hmr_state
//! ```

use oxide_fw_dev::boundary::{ClassifiedPlan, Plan};
use oxide_fw_dev::state::{DefaultStatePolicy, StatePolicy, update_frame};
use oxide_fw_dev::{UpdateEntry, UpdateKind};

fn main() {
    let policy = DefaultStatePolicy;

    // 1. The happy path: a util edit bubbles to two component boundaries —
    //    the plan is boundary-scoped, so the verdict is `keep` and the
    //    client snapshots DOM state around the swap.
    let boundary_plan = ClassifiedPlan {
        kind: UpdateKind::Js,
        plan: Plan::Update(vec!["src/comp/a.tsx".into(), "src/comp/b.tsx".into()]),
    };
    let entries = vec![UpdateEntry::stamped(
        UpdateKind::Js,
        "src/lib/util.ts",
        "/src/lib/util.ts",
        None,
    )];
    let frame = update_frame(&boundary_plan, &policy, entries);
    println!("boundary update frame: {}", frame.to_json());

    // 2. The reload fallback: an orphan util has no boundary — the verdict
    //    carries the planner's reason, so a reload that drops a half-typed
    //    form is explainable instead of mysterious.
    let reload_plan = ClassifiedPlan {
        kind: UpdateKind::Js,
        plan: Plan::FullReload("src/lib/util.ts is not imported".into()),
    };
    let verdict = policy.preservation(&reload_plan);
    match &verdict {
        oxide_fw_dev::Preservation::Clear { reason } => {
            println!("clear verdict: policy=clear reason={reason:?}");
        }
        other => println!("unexpected: {other:?}"),
    }

    // 3. The verdict is injectable: a custom policy can clear any class
    //    (here, a paranoid policy that never preserves) — the same seam
    //    pattern the F18 `Probe` trait set.
    struct NeverPreserve;
    impl StatePolicy for NeverPreserve {
        fn preservation(&self, _plan: &ClassifiedPlan) -> oxide_fw_dev::Preservation {
            oxide_fw_dev::Preservation::Clear {
                reason: "policy: never preserve".into(),
            }
        }
    }
    let paranoid = update_frame(&boundary_plan, &NeverPreserve, vec![]);
    println!("never-preserve frame: {}", paranoid.to_json());

    // 4. Round trip: the pre-substrate shape (no `preserve` field) still
    //    decodes — as `keep`, the verdict those servers always meant.
    let legacy =
        r#"{"v":1,"type":"update","updates":[{"kind":"js","path":"src/a.tsx","timestamp":0}]}"#;
    match oxide_fw_dev::HmrMessage::try_from_json(legacy) {
        Ok(oxide_fw_dev::HmrMessage::Update { preserve, .. }) => {
            println!("pre-substrate frame decodes with preserve={preserve:?}");
        }
        other => println!("unexpected: {other:?}"),
    }
}

//! The unified-locals walkthrough (§V2-A05 step 4, the adoption the CLI
//! pipeline landed): ONE [`RequestLocals`] composition the prelude
//! builds — session from the §16.6 codec, locale from the i18n resolver —
//! that the middleware stage FOLDS the chain's answer into and every
//! consumer reads through the same `to_value()` snapshot. The story:
//!
//! 1. the prelude seeds the session slot and the matcher lands the
//!    resolved locale;
//! 2. a middleware staging USER keys merges later-wins; a middleware
//!    staging a RESERVED key (`locale`, `experiments`) is REPORTED
//!    (DSP-101) and dropped — the pipeline-owned slots win;
//! 3. a middleware staging `session` REFINES the slot (the B-05
//!    write-back persists the refinement);
//! 4. the experiments stage installs the resolved assignments
//!    (`set_experiments`);
//! 5. `to_value()` is the ONE merge point — the loader, the action, the
//!    endpoint and the `/__data` envelope all read the identical value.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dispatch --example unified_locals
//! ```

use oxide_fw_dispatch::locals::{DSP_RESERVED_KEY_COLLISION, RequestLocals};

fn main() {
    println!("§V2-A05 step 4 — the unified RequestLocals the pipeline owns\n");

    // 1. The prelude: the §16.6 session decode lands in the slot; the
    //    matcher's i18n resolver lands the locale beside it.
    let mut locals = RequestLocals::default();
    locals.session = Some(serde_json::json!({ "user": "ada" }));
    locals.locale = "en".to_string();
    println!("1. prelude seed    -> session={{user:\"ada\"}} locale=\"en\"");

    // 2. The middleware chain's answer folds into the slot: user keys
    //    merge later-wins; a staged RESERVED key is reported and dropped.
    let staged = serde_json::json!({
        "section": "blog",
        "locale": "xx-YY",
        "experiments": { "hero": "staged" },
    });
    let collisions = locals.apply_middleware_locals(staged);
    println!(
        "2. chain fold      -> user keys merged; DSP-101 ({DSP_RESERVED_KEY_COLLISION}) reports {collisions:?}"
    );

    // 3. A middleware refining `session` REFINES the slot — the B-05
    //    write-back diffs the slot against the prelude-seeded value and
    //    persists the change.
    locals.session = Some(serde_json::json!({ "user": "ada", "role": "editor" }));
    println!("3. session refine  -> slot carries the staged refinement");

    // 4. The experiments stage installs the resolved assignments.
    locals.set_experiments(
        [("hero".to_string(), "treatment".to_string())]
            .into_iter()
            .collect(),
    );
    println!("4. experiments     -> resolved assignments win over any staged key");

    // 5. ONE merge point: the value every consumer reads.
    let snapshot = locals.to_value();
    println!("5. to_value()      -> {snapshot}");
    assert_eq!(
        snapshot,
        serde_json::json!({
            "section": "blog",
            "session": { "user": "ada", "role": "editor" },
            "locale": "en",
            "experiments": { "hero": "treatment" },
        }),
        "the pipeline-owned slots win; the user map carries the rest"
    );
    println!(
        "\nthe loader, the action, the endpoint and the /__data envelope all read this ONE value"
    );
}

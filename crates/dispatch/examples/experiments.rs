//! The §V2-F04 cache-safe personalization walkthrough: A/B experiments
//! evaluated natively become an L0-key dimension, so a personalized route
//! stays cacheable per cohort instead of falling back to `dynamic`.
//!
//! The story: two visitors hit the same route — one with the `x-bucket:
//! a` cohort header, one with `x-bucket: b`. The engine assigns each a
//! variant deterministically, the vary suffix the stage appends splits
//! their L0 keys (no cross-cohort contamination), and the exposure of
//! every assignment is one `oxide_experiment_assignment{experiment,
//! variant}` counter. A third identity outside the traffic band gets no
//! assignment — and with no experiments configured at all the suffix is
//! empty, so every L0 key stays byte-identical to the pre-F04 baseline.
//!
//! The CLI wiring (the `stage_experiments` call-site) rides the server
//! boot (`crates/cli/src/dispatch/stages/experiments.rs`); this example
//! walks the engine pieces the stage composes.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dispatch --example experiments
//! ```

use oxide_fw_dispatch::experiments::{self, Experiment, Resolved, Sticky};

/// The hero experiment the walkthrough configures: header-sticky (the
/// edge/CDN cohort spelling), 100% traffic, two variants, every route.
fn hero() -> Experiment {
    Experiment {
        id: "hero".to_string(),
        traffic: 100,
        sticky: Sticky::Header("x-bucket".to_string()),
        variants: vec!["control".to_string(), "treatment".to_string()],
        routes: Vec::new(),
    }
}

/// The §15.4 probes as the dispatch stage reads them off a request: the
/// path, the `Cookie` header, a header lookup, the peer IP and the mint.
fn resolve_for(
    exps: &[Experiment],
    path: &str,
    bucket: Option<&str>,
    cookie: Option<&str>,
) -> Resolved {
    experiments::resolve(
        exps,
        path,
        cookie,
        &|name| (name == "x-bucket").then(|| bucket.unwrap_or_default().to_string()),
        "",
        &|| "minted-identity-token".to_string(),
    )
}

fn main() {
    let exps = vec![hero()];

    // Two cohorts: each resolves deterministically from its sticky value,
    // and re-resolving never flips a variant (the sticky contract).
    let a = resolve_for(&exps, "/", Some("a"), None);
    let a_again = resolve_for(&exps, "/", Some("a"), None);
    let c = resolve_for(&exps, "/", Some("c"), None);
    println!(
        "cohort a -> assigned {:?}, vary suffix {:?}",
        a.assigned, a.vary_suffix
    );
    println!(
        "cohort a again -> {:?} (sticky: {})",
        a_again.assigned,
        if a_again == a { "stable" } else { "FLIPPED" }
    );
    println!(
        "cohort c -> assigned {:?}, vary suffix {:?}",
        c.assigned, c.vary_suffix
    );
    assert_eq!(a, a_again, "the same identity never flips variants");
    assert_ne!(
        a.vary_suffix, c.vary_suffix,
        "the cohorts sit in different buckets"
    );

    // The L0 key dimension the stage appends: only when the suffix is
    // non-empty (routes outside every experiment keep the pre-F04 keys).
    println!(
        "l0 vary for cohort a: {:?}",
        format!("accept-encoding|exp:{}", a.vary_suffix)
    );

    // The exposure metric the stage emits per assignment (the server
    // exposition renders one counter line per pair).
    for (id, variant) in &a.assigned {
        println!("oxide_experiment_assignment{{experiment=\"{id}\",variant=\"{variant}\"}} +1");
    }

    // A traffic gate at 10%: most identities fall outside the band and
    // get NO assignment (the suffix stays empty — no key dimension).
    let gated = vec![Experiment {
        traffic: 10,
        ..hero()
    }];
    let mut outside = 0;
    for i in 0..100 {
        let sticky = format!("visitor-{i}");
        let r = resolve_for(&gated, "/", Some(&sticky), None);
        if r.assigned.is_empty() {
            outside += 1;
        }
    }
    println!("10% traffic: {outside}/100 synthetic identities fell outside the band");
    assert!(
        outside > 70,
        "a 10% band leaves the clear majority unassigned (got {outside}/100)"
    );

    // No experiments configured: the stage short-circuits and the suffix
    // is empty — the acceptance ("keys equal the pre-F04 baseline").
    let none = resolve_for(&[], "/", Some("a"), None);
    assert_eq!(none.vary_suffix, "", "no experiments -> no key dimension");
    println!("no experiments configured -> empty vary suffix, keys stay the pre-F04 bytes");
}

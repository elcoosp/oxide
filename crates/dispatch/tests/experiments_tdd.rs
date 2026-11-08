//! Integration tests for the `oxide-fw-dispatch` experiments assignment
//! engine (spec v0.2, V2-F04 step 1 — the V8-free cache-safe
//! personalization substrate).
//!
//! §V2-F04: A/B experiments evaluated natively become L0 variant-key
//! dimensions instead of forcing a route dynamic. The pipeline owns the
//! `experiments` locals slot (V2-A05); this engine decides the assignment
//! deterministically (blake3 over `id || 0x00 || sticky_value`), mints the
//! missing sticky cookie, and computes the vary suffix the L0 key wiring
//! appends (`|exp:` + suffix, only when non-empty — routes outside every
//! experiment keep byte-identical keys, the acceptance contract).
//!
//! Deliberate delta from the spec pseudocode (recorded in
//! `docs/deviations.md`): `Resolved.set_cookie` is a `Vec<String>`, not an
//! `Option<String>` — the spec's single slot silently drops every minted
//! cookie except the last when two cookie-sticky experiments both need a
//! fresh identity, and two `Set-Cookie` values can never legally share one
//! header.

use std::cell::Cell;
use std::collections::BTreeMap;

use oxide_fw_dispatch::experiments::{Experiment, Resolved, Sticky, assign, resolve};
use oxide_fw_dispatch::locals::RequestLocals;

/// Build an experiment with the spec's config shape (§V2-F04 item 5).
fn exp(id: &str, traffic: u8, sticky: Sticky, variants: &[&str], routes: &[&str]) -> Experiment {
    Experiment {
        id: id.to_string(),
        traffic,
        sticky,
        variants: variants.iter().map(|v| (*v).to_string()).collect(),
        routes: routes.iter().map(|r| (*r).to_string()).collect(),
    }
}

/// No headers, no cookie header, fixed ip, minting a numbered identity.
fn plain_env(
    mints: &Cell<u32>,
) -> (
    impl Fn(&str) -> Option<String> + '_,
    impl Fn() -> String + '_,
) {
    (
        |_name: &str| None,
        || {
            let n = mints.get() + 1;
            mints.set(n);
            format!("identity-{n}")
        },
    )
}

#[test]
fn assignment_is_deterministic() {
    let e = exp("hero", 100, Sticky::IpHash, &["a", "b"], &[]);
    let first = assign(&e, "203.0.113.7");
    for _ in 0..50 {
        assert_eq!(
            assign(&e, "203.0.113.7"),
            first,
            "same sticky value, same variant"
        );
    }
    assert!(first.is_some(), "traffic 100 assigns everyone");
}

#[test]
fn traffic_zero_assigns_nobody() {
    let e = exp("hero", 0, Sticky::IpHash, &["a", "b"], &[]);
    for id in ["1", "42", "203.0.113.9", "user-777"] {
        assert_eq!(assign(&e, id), None, "traffic 0 never assigns {id}");
    }
}

#[test]
fn traffic_100_assigns_everyone() {
    let e = exp("hero", 100, Sticky::IpHash, &["a", "b"], &[]);
    for i in 0..200 {
        let id = format!("user-{i}");
        assert!(assign(&e, &id).is_some(), "traffic 100 assigns {id}");
    }
}

#[test]
fn distribution_within_5_percent_over_10000_ids() {
    let e = exp("hero", 100, Sticky::IpHash, &["control", "treatment"], &[]);
    let mut counts = [0usize; 2];
    for i in 0..10_000 {
        let slot = assign(&e, &format!("identity-{i}")).expect("traffic 100 assigns");
        counts[slot] += 1;
    }
    // Each variant's share of 10 000 must sit within 5 % of the fair split.
    for (slot, count) in counts.iter().enumerate() {
        let deviation = count.abs_diff(5_000);
        assert!(
            deviation <= 250,
            "variant {slot} deviates {deviation} from fair share (counts {counts:?})"
        );
    }
    assert_eq!(counts[0] + counts[1], 10_000);
}

#[test]
fn cookie_minted_when_absent_and_stable_when_present() {
    let e = exp(
        "hero",
        100,
        Sticky::Cookie("oxide_hero".into()),
        &["a", "b"],
        &[],
    );
    let mints = Cell::new(0u32);
    let (header, mint) = plain_env(&mints);

    let first = resolve(
        std::slice::from_ref(&e),
        "/",
        None,
        &header,
        "10.0.0.1",
        &mint,
    );
    assert_eq!(mints.get(), 1, "exactly one identity minted");
    // The cookie carries the minted IDENTITY (not the variant — the variant
    // is derived from the identity), so the echo-back below is stable.
    let identity = "identity-1";
    assert_eq!(
        first.set_cookie,
        vec![format!(
            "oxide_hero={identity}; Path=/; Max-Age=2592000; SameSite=Lax; HttpOnly"
        )],
        "the minted identity is set as the experiment cookie"
    );
    let expected_variant =
        e.variants[assign(&e, identity).expect("the minted identity assigns")].clone();
    assert_eq!(
        first.assigned.get("hero"),
        Some(&expected_variant),
        "the assignment is derived from the minted identity"
    );

    // The same identity (echoed back through the cookie header) resolves to
    // the same variant without minting a second one.
    let cookie = format!("other=1; oxide_hero={identity}");
    let second = resolve(
        std::slice::from_ref(&e),
        "/",
        Some(&cookie),
        &header,
        "10.0.0.1",
        &mint,
    );
    assert_eq!(mints.get(), 1, "an existing cookie is never re-minted");
    assert_eq!(second.assigned.get("hero"), Some(&expected_variant));
    assert_eq!(
        second.set_cookie,
        Vec::<String>::new(),
        "no new Set-Cookie when present"
    );
}

#[test]
fn vary_suffix_empty_without_experiments() {
    let mints = Cell::new(0u32);
    let (header, mint) = plain_env(&mints);
    let resolved = resolve(&[], "/", None, &header, "10.0.0.1", &mint);
    let Resolved {
        assigned,
        set_cookie,
        vary_suffix,
    } = resolved;
    assert!(assigned.is_empty());
    assert_eq!(set_cookie, Vec::<String>::new());
    assert_eq!(vary_suffix, "", "no experiments — the L0 key stays pre-F04");
}

#[test]
fn vary_suffix_lists_assignments_id_sorted() {
    // Two experiments; the map (and therefore the suffix) is keyed by the
    // experiment id so the suffix is byte-stable regardless of config order.
    let exps = [
        exp("zeta", 100, Sticky::IpHash, &["a", "b"], &[]),
        exp("alpha", 100, Sticky::IpHash, &["x", "y"], &[]),
    ];
    let mints = Cell::new(0u32);
    let (header, mint) = plain_env(&mints);
    let resolved = resolve(&exps, "/", None, &header, "10.0.0.1", &mint);
    // The same sticky value hashes the same slot per experiment; the suffix
    // is the id-sorted `id=variant` pair list (BTreeMap order).
    let alpha = assign(&exps[1], "10.0.0.1").expect("alpha assigns");
    let zeta = assign(&exps[0], "10.0.0.1").expect("zeta assigns");
    let expected = format!(
        "alpha={};zeta={}",
        exps[1].variants[alpha], exps[0].variants[zeta]
    );
    assert_eq!(
        resolved.vary_suffix, expected,
        "id-sorted pair list, `;`-joined (no spaces — the suffix feeds an L0 key)"
    );
}

#[test]
fn routes_prefix_filters_experiments() {
    let e = exp(
        "hero",
        100,
        Sticky::IpHash,
        &["a", "b"],
        &["/pricing", "/blog"],
    );
    let mints = Cell::new(0u32);
    let (header, mint) = plain_env(&mints);
    // Prefix match: the route list is URL-pattern prefixes (§V2-F04).
    let inside = resolve(
        std::slice::from_ref(&e),
        "/pricing/enterprise",
        None,
        &header,
        "10.0.0.1",
        &mint,
    );
    assert!(inside.assigned.contains_key("hero"), "prefix match assigns");
    let outside = resolve(
        std::slice::from_ref(&e),
        "/",
        None,
        &header,
        "10.0.0.1",
        &mint,
    );
    assert!(
        outside.assigned.is_empty() && outside.vary_suffix.is_empty(),
        "routes outside every experiment are never assigned (key bytes unchanged)"
    );
}

#[test]
fn header_sticky_reads_the_named_header() {
    let e = exp(
        "hero",
        100,
        Sticky::Header("x-bucket".into()),
        &["a", "b"],
        &[],
    );
    let header = |name: &str| (name == "x-bucket").then(|| "edge-cohort".to_string());
    let resolved = resolve(
        std::slice::from_ref(&e),
        "/",
        None,
        &header,
        "10.0.0.1",
        &|| "never".into(),
    );
    let expected = assign(&e, "edge-cohort").expect("the header value assigns");
    assert_eq!(
        resolved.assigned.get("hero").map(String::as_str),
        Some(e.variants[expected].as_str()),
        "header-sticky assignment hashes the header value"
    );
}

#[test]
fn ip_hash_sticky_uses_the_client_ip() {
    let e = exp("hero", 100, Sticky::IpHash, &["a", "b"], &[]);
    let expected = assign(&e, "198.51.100.4").expect("direct assign");
    let mints = Cell::new(0u32);
    let (header, mint) = plain_env(&mints);
    let resolved = resolve(
        std::slice::from_ref(&e),
        "/",
        None,
        &header,
        "198.51.100.4",
        &mint,
    );
    assert_eq!(
        resolved.assigned.get("hero").map(String::as_str),
        Some(if expected == 0 { "a" } else { "b" }),
        "ip-hash sticky hashes the client ip"
    );
}

#[test]
fn empty_sticky_value_assigns_nobody() {
    let e = exp(
        "hero",
        100,
        Sticky::Cookie("oxide_hero".into()),
        &["a", "b"],
        &[],
    );
    assert_eq!(assign(&e, ""), None, "an empty sticky value never assigns");
}

#[test]
fn two_cookie_experiments_each_get_their_own_minted_cookie() {
    // The V2-F04 deviation pin: two cookie-sticky experiments without a
    // cookie each mint their own identity — both survive, in config order.
    let exps = [
        exp(
            "one",
            100,
            Sticky::Cookie("oxide_one".into()),
            &["a", "b"],
            &[],
        ),
        exp(
            "two",
            100,
            Sticky::Cookie("oxide_two".into()),
            &["a", "b"],
            &[],
        ),
    ];
    let mints = Cell::new(0u32);
    let (header, mint) = plain_env(&mints);
    let resolved = resolve(&exps, "/", None, &header, "10.0.0.1", &mint);
    assert_eq!(mints.get(), 2, "each cookie experiment mints independently");
    assert_eq!(
        resolved.set_cookie.len(),
        2,
        "one Set-Cookie per minted identity"
    );
    assert!(resolved.set_cookie[0].starts_with("oxide_one="));
    assert!(resolved.set_cookie[1].starts_with("oxide_two="));
}

#[test]
fn resolved_assignments_reach_request_locals() {
    let mut locals = RequestLocals::default();
    let mut assigned = BTreeMap::new();
    assigned.insert("hero".to_string(), "treatment".to_string());
    assigned.insert("cta".to_string(), "variant-2".to_string());
    locals.set_experiments(assigned);

    assert_eq!(
        locals.to_value(),
        serde_json::json!({
            "experiments": { "hero": "treatment", "cta": "variant-2" },
        }),
        "V2-F04 wiring contract: Resolved.assigned lands in the pipeline-owned slot"
    );
}

#[test]
fn reserved_experiments_slot_wins_over_middleware() {
    // DSP-101 still guards the slot once F04 feeds it: a middleware staging
    // `experiments` is reported and dropped, the resolved assignments win.
    let mut locals = RequestLocals::default();
    let mut assigned = BTreeMap::new();
    assigned.insert("hero".to_string(), "control".to_string());
    locals.set_experiments(assigned);

    let collisions = locals.apply_middleware_locals(serde_json::json!({
        "experiments": { "hero": "spoofed" },
        "section": "blog",
    }));
    assert_eq!(collisions, vec!["experiments".to_string()]);
    assert_eq!(
        locals.to_value(),
        serde_json::json!({
            "section": "blog",
            "experiments": { "hero": "control" },
        }),
        "the pipeline-owned assignment wins over the staged payload"
    );
}

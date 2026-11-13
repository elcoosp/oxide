//! TDD pins for the two-generation serving pools (spec v0.2 §V2-F10 design
//! steps 1-3: "`GenerationPools { current, previous }` — the old build's
//! data plane keeps serving for the skew window while the new one takes
//! traffic").
//!
//! The contracts pinned here:
//!
//! - [`GenerationPools`] is the SERVING side of the F10 seam: it stores a
//!   handle per generation (the generic `P` — at the CLI layer the spec
//!   instantiates it over `Arc<dyn Pool>`, the worker pool a dist root
//!   owns) and routes a request's build id to the handle that must serve
//!   it. HTML and header-less requests always route CURRENT (`html_
//!   always_current`), an accepted demoted id routes ITS OWN pool
//!   (`previous_pool_serves_old_build_header`), an expired or unknown id
//!   routes NOWHERE — the caller answers the §V2-A04 409 through
//!   [`stage_check`], which drives the same structure with identical
//!   verdicts (the [`AcceptedGenerations`] seam).
//! - Each demotion carries its OWN demotion instant (the window runs from
//!   the demotion — two deploys ten minutes apart expire ten minutes
//!   apart) and its own handle; expiry (`expire`) drops the expired
//!   HANDLES — "at window end it drops the previous pool and unlinks
//!   nothing" — while the current pool never expires.
//! - The structure is bounded: at capacity the OLDEST demoted pool evicts
//!   first (the A04 "bounded by config, not deploy frequency" rule); the
//!   brownout early-drop (`evict_oldest`, the F12 hook) returns the
//!   evicted handle so the caller can shut the pool down gracefully.
//! - Re-promoting a demoted id is the rollback path: it installs the
//!   given handle as current and drops the stale demoted entry;
//!   promoting the current id is a no-op that keeps the live handle.

use std::sync::Arc;
use std::time::{Duration, Instant};

use oxide_fw_dispatch::build::{Gen, GenerationSet, STALE_HEADER, STALE_STATUS};
use oxide_fw_dispatch::flow::Flow;
use oxide_fw_dispatch::pools::GenerationPools;
use oxide_fw_dispatch::skew::{AcceptedGenerations, stage_check};

const WINDOW: Duration = Duration::from_mins(15);
const A: &str = "1111111111111111";
const B: &str = "2222222222222222";
const C: &str = "3333333333333333";
const D: &str = "4444444444444444";

/// The fake serving handle: the tests care about IDENTITY (which pool a
/// request routed to), not about what the pool does.
#[derive(Debug)]
struct FakePool(u8);

fn pools(current: u8, capacity: usize) -> (GenerationPools<Arc<FakePool>>, Arc<FakePool>) {
    let handle = Arc::new(FakePool(current));
    (
        GenerationPools::new(A.to_string(), Arc::clone(&handle), WINDOW, capacity),
        handle,
    )
}

/// The id the handle serves — the fake's identity probe.
fn serves(handle: &Arc<FakePool>) -> u8 {
    handle.0
}

#[test]
fn html_and_headerless_requests_always_route_current() {
    let t0 = Instant::now();
    let (mut pools, handle) = pools(1, 4);
    pools.promote(B.to_string(), Arc::new(FakePool(2)), t0);
    // HTML never carries a build id (§V2-F10 step 2: "everything else,
    // including all HTML, goes to current") — and neither does a
    // header-less data-plane request (curl, tests, old clients).
    let (html, resolved) = pools.route(None, t0).expect("html routes");
    assert_eq!(
        resolved,
        Gen::Current,
        "html navigations always take the current pool"
    );
    assert_eq!(
        serves(html),
        2,
        "html is served by the CURRENT build's pool"
    );
    let (no_header, resolved) = pools.route(None, t0).expect("header-less routes");
    assert_eq!(resolved, Gen::Current);
    assert!(Arc::ptr_eq(no_header, &handle) || serves(no_header) == 2);
}

#[test]
fn an_old_build_header_routes_to_its_own_pool() {
    // The spec's `previous_pool_serves_old_build_header` with two fake
    // pools returning their build id: the demoted generation's OWN handle
    // answers, not the current one.
    let t0 = Instant::now();
    let (mut pools, first) = pools(1, 4);
    let second = Arc::new(FakePool(2));
    pools.promote(B.to_string(), Arc::clone(&second), t0);

    let (pool, resolved) = pools.route(Some(A), t0).expect("the demoted id routes");
    assert_eq!(resolved, Gen::Previous);
    assert!(
        Arc::ptr_eq(pool, &first),
        "the OLD id is served by the OLD pool"
    );
    assert_eq!(serves(pool), 1);

    let (pool, resolved) = pools.route(Some(B), t0).expect("the current id routes");
    assert_eq!(resolved, Gen::Current);
    assert!(
        Arc::ptr_eq(pool, &second),
        "the NEW id is served by the NEW pool"
    );
}

#[test]
fn an_expired_or_unknown_id_routes_nowhere() {
    // The spec's `expired_generation_returns_409`: routing yields None and
    // the data-plane stage answers the 409 for the same id.
    let t0 = Instant::now();
    let (mut pools, _first) = pools(1, 4);
    pools.promote(B.to_string(), Arc::new(FakePool(2)), t0);

    assert!(
        pools.route(Some("9999999999999999"), t0).is_none(),
        "unknown id routes nowhere"
    );
    let after = t0 + WINDOW + Duration::from_millis(1);
    assert!(
        pools.route(Some(A), after).is_none(),
        "the expired id routes nowhere past ITS window"
    );
    // The stage answers the §V2-A04 409 for the same expired id.
    let flow = stage_check(
        "/__data",
        &[(
            oxide_fw_dispatch::build::BUILD_HEADER.to_string(),
            A.to_string(),
        )],
        &pools,
        after,
    );
    match flow {
        Flow::Respond(resp) => {
            assert_eq!(resp.status, STALE_STATUS);
            assert!(
                resp.headers
                    .iter()
                    .any(|(k, v)| k == STALE_HEADER && v == "true")
            );
        }
        Flow::Continue => panic!("an expired id must be refused on the data plane"),
    }
    // ...while the current build keeps routing past the old one's window.
    assert!(pools.route(Some(B), after).is_some());
}

#[test]
fn the_stage_drives_set_and_pools_with_identical_verdicts() {
    // The verdict-parity pin generalized: the 1-deep GenerationSet, the
    // bounded GenerationPool and the serving pools agree on every
    // §V2-A04 stage verdict.
    fn verdicts(gens: &impl AcceptedGenerations, now: Instant) -> (bool, bool, bool, bool) {
        let headers = |v: &str| {
            vec![(
                oxide_fw_dispatch::build::BUILD_HEADER.to_string(),
                v.to_string(),
            )]
        };
        let stale = |f: &Flow| {
            matches!(f, Flow::Respond(r) if r.status == STALE_STATUS
            && r.headers.iter().any(|(k, val)| k == STALE_HEADER && val == "true"))
        };
        let continue_ = |f: &Flow| matches!(f, Flow::Continue);
        (
            continue_(&stage_check("/__data", &headers(A), gens, now)),
            stale(&stage_check(
                "/__data",
                &headers("9999999999999999"),
                gens,
                now,
            )),
            continue_(&stage_check("/__data", &[], gens, now)),
            continue_(&stage_check("/", &headers("9999999999999999"), gens, now)),
        )
    }

    let t0 = Instant::now();
    let mut set = GenerationSet::new(A.to_string(), WINDOW);
    set.promote(B.to_string(), t0);
    let mut pools = GenerationPools::new(A.to_string(), Arc::new(FakePool(1)), WINDOW, 4);
    pools.promote(B.to_string(), Arc::new(FakePool(2)), t0);
    let now = t0 + Duration::from_secs(60);

    assert_eq!(
        verdicts(&set, now),
        verdicts(&pools, now),
        "the serving pools and the 1-deep set agree on every §V2-A04 verdict"
    );
}

#[test]
fn expiry_drops_the_old_pool_and_keeps_the_current_one() {
    // "At window end it drops the previous pool and unlinks nothing": the
    // expired HANDLES are dropped (the Arc's refcount hits zero — nothing
    // is reclaimed on disk, the content-hashed chunks stay valid).
    let t0 = Instant::now();
    let (mut pools, first) = pools(1, 4);
    let second = Arc::new(FakePool(2));
    pools.promote(B.to_string(), Arc::clone(&second), t0);
    let first_only = Arc::downgrade(&first);
    drop(first);

    let after = t0 + WINDOW + Duration::from_millis(1);
    assert_eq!(pools.expire(after), 1, "the expired demotion is reclaimed");
    assert!(
        first_only.upgrade().is_none(),
        "the expired pool's handle was dropped"
    );
    assert!(pools.route(Some(A), after).is_none());
    assert!(
        pools.route(Some(B), after).is_some(),
        "the current pool never expires"
    );
    assert_eq!(pools.len(), 0);
    // A second expire is a no-op.
    assert_eq!(pools.expire(after), 0);
}

#[test]
fn a_bounded_structure_evicts_the_oldest_pool_first() {
    let t0 = Instant::now();
    let (mut pools, first) = pools(1, 2);
    let second = Arc::new(FakePool(2));
    let third = Arc::new(FakePool(3));
    let fourth = Arc::new(FakePool(4));
    pools.promote(B.to_string(), Arc::clone(&second), t0);
    pools.promote(
        C.to_string(),
        Arc::clone(&third),
        t0 + Duration::from_secs(1),
    );
    pools.promote(
        D.to_string(),
        Arc::clone(&fourth),
        t0 + Duration::from_secs(2),
    );
    // Capacity 2: the third deploy evicts the OLDEST demotion (A); B and
    // C survive their windows, D is current.
    assert!(pools.route(Some(A), t0 + Duration::from_secs(3)).is_none());
    assert!(pools.route(Some(B), t0 + Duration::from_secs(3)).is_some());
    assert!(pools.route(Some(C), t0 + Duration::from_secs(3)).is_some());
    assert!(pools.route(Some(D), t0 + Duration::from_secs(3)).is_some());
    // ...and promote() DROPPED the evicted handle (the spec bounds memory
    // at two generations — the evicted pool does not linger).
    assert!(
        Arc::strong_count(&first) == 1,
        "only the test's own reference remains"
    );
}

#[test]
fn the_brownout_eviction_returns_the_handle_for_shutdown() {
    // The F12 hook: "when the pool queue wait exceeds the brownout
    // threshold the previous generation is dropped early" — evict_oldest
    // hands the handle back so the caller can drain and shut it down
    // gracefully instead of leaking the drop.
    let t0 = Instant::now();
    let (mut pools, first) = pools(1, 4);
    let second = Arc::new(FakePool(2));
    pools.promote(B.to_string(), Arc::clone(&second), t0);

    let evicted = pools
        .evict_oldest()
        .expect("the oldest demoted pool comes back");
    assert_eq!(evicted.0, A, "the OLDEST demotion evicts first");
    assert!(Arc::ptr_eq(&evicted.1, &first));
    assert!(
        pools.route(Some(A), t0).is_none(),
        "the evicted id routes nowhere"
    );
    // The current build is untouched by the brownout eviction.
    assert!(pools.route(Some(B), t0).is_some());
    // Drained: the next eviction finds nothing.
    assert!(pools.evict_oldest().is_none());
    assert!(pools.is_empty());
}

#[test]
fn re_promoting_a_demoted_id_is_the_rollback_path() {
    // §V2-F10 step 4 / the F11 rollback: `oxide start --generation
    // previous` re-promotes the old id — the caller boots a FRESH pool
    // over the old dist root and hands it in; the stale demoted entry is
    // dropped, not duplicated.
    let t0 = Instant::now();
    let (mut pools, first) = pools(1, 4);
    let second = Arc::new(FakePool(2));
    pools.promote(B.to_string(), Arc::clone(&second), t0);

    let rollback = Arc::new(FakePool(1));
    pools.promote(
        A.to_string(),
        Arc::clone(&rollback),
        t0 + Duration::from_secs(2),
    );
    let (pool, resolved) = pools
        .route(Some(A), t0 + Duration::from_secs(3))
        .expect("the rolled-back id routes");
    assert_eq!(
        resolved,
        Gen::Current,
        "the rolled-back build is current again"
    );
    assert!(
        Arc::ptr_eq(pool, &rollback),
        "the FRESH handle serves — not the stale entry"
    );
    assert_eq!(
        pools.len(),
        1,
        "A appears exactly once (the stale demoted entry is gone)"
    );
    assert!(pools.route(Some(B), t0 + Duration::from_secs(3)).is_some());
    let _ = first;
}

#[test]
fn promoting_the_current_id_is_a_no_op_and_keeps_the_live_handle() {
    let t0 = Instant::now();
    let (mut pools, handle) = pools(1, 4);
    pools.promote(
        A.to_string(),
        Arc::new(FakePool(9)),
        t0 + Duration::from_secs(1),
    );
    assert_eq!(pools.current(), A);
    assert_eq!(
        pools.len(),
        0,
        "an idempotent re-deploy never churns the pools"
    );
    let (pool, _) = pools
        .route(Some(A), t0 + Duration::from_secs(2))
        .expect("routes");
    assert!(
        Arc::ptr_eq(pool, &handle),
        "the LIVE handle stays — a no-op never swaps pools"
    );
}

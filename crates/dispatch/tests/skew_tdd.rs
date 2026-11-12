//! TDD pins for the skew-protection pool (spec v0.2 §V2-F10: "tabs still
//! on the old build keep working for a window" — the bounded
//! [`GenerationPool`] the data-plane stage consumes through the
//! [`AcceptedGenerations`] seam).
//!
//! The contracts pinned here:
//!
//! - [`AcceptedGenerations`] is the seam `stage_check` consumes: the
//!   existing 1-deep [`GenerationSet`] and the bounded F10
//!   [`GenerationPool`] drive the SAME stage with the SAME verdicts
//!   (the data-plane contract of §V2-A04 — 409 + `x-oxide-build-stale`
//!   on an unaccepted id, absent header allowed, HTML navigations
//!   always allowed).
//! - [`GenerationPool::promote`] demotes the current build into the pool
//!   with its own demotion instant; a demoted id is `Previous` while its
//!   window runs and unknown once it expires (lazily evicted).
//! - The pool is bounded: the OLDEST demotion evicts first when the
//!   capacity is reached — the "how old may a replayable response be"
//!   question stays bounded by config (the A04 rule, generalized).
//! - `promote` of a demoted id (the rollback path) moves it back to
//!   current; `promote` of the current id is a no-op (an idempotent
//!   re-deploy never churns the pool).
//! - `expire` drops the expired demotions and reports how many went.

use std::time::{Duration, Instant};

use oxide_fw_dispatch::build::{Gen, GenerationSet};
use oxide_fw_dispatch::skew::{AcceptedGenerations, GenerationPool};

const WINDOW: Duration = Duration::from_mins(15);
const A: &str = "1111111111111111";
const B: &str = "2222222222222222";
const C: &str = "3333333333333333";
const D: &str = "4444444444444444";

#[test]
fn pool_accepts_current_and_demoted_within_their_windows() {
    let t0 = Instant::now();
    let mut pool = GenerationPool::new(A.to_string(), WINDOW, 4);
    assert_eq!(pool.accepts(A, t0), Some(Gen::Current));

    // Deploy B: A is demoted at t0, accepted for the window.
    pool.promote(B.to_string(), t0);
    assert_eq!(pool.current(), B);
    assert_eq!(
        pool.accepts(A, t0 + Duration::from_secs(60)),
        Some(Gen::Previous)
    );
    assert_eq!(
        pool.accepts(B, t0 + Duration::from_secs(60)),
        Some(Gen::Current)
    );

    // Deploy C: A and B are both demoted (the 1-deep GenerationSet could
    // not hold both — the pool is the F10 growth).
    pool.promote(C.to_string(), t0 + Duration::from_secs(120));
    assert_eq!(
        pool.accepts(B, t0 + Duration::from_secs(180)),
        Some(Gen::Previous)
    );
    assert_eq!(
        pool.accepts(A, t0 + Duration::from_secs(180)),
        Some(Gen::Previous)
    );
}

#[test]
fn an_expired_demotion_is_unknown_and_evicted() {
    let t0 = Instant::now();
    let mut pool = GenerationPool::new(A.to_string(), WINDOW, 4);
    pool.promote(B.to_string(), t0);
    // The demotion is A (marked at t0): inside the window it is Previous.
    assert_eq!(
        pool.accepts(A, t0 + Duration::from_secs(60)),
        Some(Gen::Previous)
    );
    // One millisecond past ITS window: unknown on read (accepts is a
    // pure lookup), reclaimed by the housekeeping pass.
    let after = t0 + WINDOW + Duration::from_millis(1);
    assert_eq!(
        pool.accepts(A, after),
        None,
        "the expired demotion is refused"
    );
    assert_eq!(
        pool.expire(after),
        1,
        "the expired demotion is reclaimed by expire()"
    );
    assert_eq!(
        pool.len(),
        0,
        "the pool holds no expired entries after expire"
    );
    // The current build (B, promoted at t0) never expires.
    assert_eq!(pool.accepts(B, after), Some(Gen::Current));
}

#[test]
fn a_bounded_pool_evicts_the_oldest_demotion_first() {
    let t0 = Instant::now();
    let mut pool = GenerationPool::new(A.to_string(), WINDOW, 2);
    pool.promote(B.to_string(), t0);
    pool.promote(C.to_string(), t0 + Duration::from_secs(1));
    pool.promote(D.to_string(), t0 + Duration::from_secs(2));
    // Capacity 2: A (the oldest) is gone; B and C survive their windows.
    assert_eq!(pool.accepts(A, t0 + Duration::from_secs(3)), None);
    assert_eq!(
        pool.accepts(B, t0 + Duration::from_secs(3)),
        Some(Gen::Previous)
    );
    assert_eq!(
        pool.accepts(C, t0 + Duration::from_secs(3)),
        Some(Gen::Previous)
    );
    assert_eq!(
        pool.accepts(D, t0 + Duration::from_secs(3)),
        Some(Gen::Current)
    );
}

#[test]
fn promoting_a_demoted_id_moves_it_back_to_current() {
    let t0 = Instant::now();
    let mut pool = GenerationPool::new(A.to_string(), WINDOW, 4);
    pool.promote(B.to_string(), t0);
    assert_eq!(
        pool.accepts(B, t0 + Duration::from_secs(1)),
        Some(Gen::Current)
    );
    // The rollback: re-promote A, which was sitting demoted in the pool.
    pool.promote(A.to_string(), t0 + Duration::from_secs(2));
    assert_eq!(pool.current(), A);
    assert_eq!(
        pool.accepts(A, t0 + Duration::from_secs(3)),
        Some(Gen::Current),
        "the rolled-back build is current again"
    );
    assert_eq!(
        pool.accepts(B, t0 + Duration::from_secs(3)),
        Some(Gen::Previous)
    );
    // And A appears in the pool exactly once.
    assert_eq!(pool.len(), 1);
}

#[test]
fn promoting_the_current_id_is_a_no_op() {
    let t0 = Instant::now();
    let mut pool = GenerationPool::new(A.to_string(), WINDOW, 4);
    pool.promote(A.to_string(), t0 + Duration::from_secs(1));
    assert_eq!(pool.current(), A);
    assert_eq!(
        pool.len(),
        0,
        "an idempotent re-deploy never churns the pool"
    );
}

#[test]
fn capacity_zero_keeps_only_the_current_build() {
    let t0 = Instant::now();
    let mut pool = GenerationPool::new(A.to_string(), WINDOW, 0);
    pool.promote(B.to_string(), t0);
    assert_eq!(pool.current(), B);
    assert_eq!(pool.accepts(A, t0 + Duration::from_secs(1)), None);
    assert!(pool.is_empty());
}

#[test]
fn expire_reports_how_many_demotions_went() {
    let t0 = Instant::now();
    let mut pool = GenerationPool::new(A.to_string(), WINDOW, 4);
    pool.promote(B.to_string(), t0);
    pool.promote(C.to_string(), t0 + Duration::from_secs(1));
    // Nothing expired yet.
    assert_eq!(pool.expire(t0 + Duration::from_secs(2)), 0);
    // Both demotions expire past their windows.
    let after = t0 + WINDOW + Duration::from_secs(2);
    assert_eq!(pool.expire(after), 2);
    assert_eq!(pool.expire(after), 0, "a second expire is a no-op");
}

// ---------------------------------------------------------------------------
// the seam: one stage, two generation sources, identical verdicts
// ---------------------------------------------------------------------------

/// The §V2-A04 verdicts the stage must produce for a data-plane path.
fn verdicts(gens: &impl AcceptedGenerations, now: Instant) -> (bool, bool, bool, bool) {
    use oxide_fw_dispatch::build::{BUILD_HEADER, STALE_HEADER, STALE_STATUS};
    use oxide_fw_dispatch::flow::Flow;
    use oxide_fw_dispatch::skew::stage_check;

    let headers = |v: &str| vec![(BUILD_HEADER.to_string(), v.to_string())];
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

#[test]
fn the_stage_drives_set_and_pool_with_identical_verdicts() {
    let t0 = Instant::now();
    let mut set = GenerationSet::new(A.to_string(), WINDOW);
    set.promote(B.to_string(), t0);
    let mut pool = GenerationPool::new(A.to_string(), WINDOW, 4);
    pool.promote(B.to_string(), t0);
    let now = t0 + Duration::from_secs(60);
    assert_eq!(
        verdicts(&set, now),
        verdicts(&pool, now),
        "the 1-deep set and the bounded pool agree on every §V2-A04 verdict"
    );
}

#[test]
fn the_pool_answers_the_four_data_plane_paths() {
    let t0 = Instant::now();
    let mut pool = GenerationPool::new(A.to_string(), WINDOW, 4);
    pool.promote(B.to_string(), t0);
    let now = t0 + Duration::from_secs(60);
    let (accepted_current, stale_unknown, allowed_absent, html_allowed) = verdicts(&pool, now);
    assert!(accepted_current, "the current id continues");
    assert!(
        stale_unknown,
        "an unaccepted id answers 409 + x-oxide-build-stale"
    );
    assert!(
        allowed_absent,
        "a header-less request continues (curl, tests)"
    );
    assert!(html_allowed, "an HTML navigation never checks the build id");
}

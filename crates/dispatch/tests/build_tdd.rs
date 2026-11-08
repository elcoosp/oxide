//! TDD pins for the V2-A04 build id + generations substrate
//! (`oxide_fw_dispatch::build`, spec v0.2 §V2-A04 step 3).
//!
//! One canonical build id, checked on every data-plane request:
//! a request that advertises `x-oxide-build` the running process no longer
//! accepts answers `409` + `x-oxide-build-stale: true` with an empty body;
//! a request without the header (curl, tests, old clients) is allowed; a
//! data-plane response always advertises the current id. The stage runs
//! ONLY for the data plane (`/__data`, `/_oxide/fn/*`, and — since V2-F05 —
//! `/__oxide/live`, whose build id may also travel in the `build` query
//! parameter because `EventSource` cannot set headers; `/__oxide/stream/*`
//! joins when that route lands (F07).
//!
//! Written FIRST (red), per the spec's agent contract V2.0 item 3. The
//! pipeline wiring (calling `stage_build_check` from the dispatch closure
//! and `advertise_build` on data-plane responses) is deferred to the
//! A01-phase-3 CLI adoption; see `docs/deviations.md`.

use std::time::{Duration, Instant};

use oxide_fw_dispatch::build::{
    BUILD_HEADER, Gen, GenerationSet, STALE_HEADER, advertise_build, is_data_plane_path,
    stage_build_check,
};
use oxide_fw_dispatch::flow::Flow;

fn gens(current: &str, window: Duration) -> GenerationSet {
    GenerationSet::new(current.to_string(), window)
}

fn now() -> Instant {
    Instant::now()
}

// --------------------------------------------------------------- GenerationSet

/// The spec's own pin (§V2-A04 `current_accepted`).
#[test]
fn current_accepted() {
    let g = gens("a", Duration::from_secs(60));
    assert_eq!(g.accepts("a", now()), Some(Gen::Current));
}

/// The spec's own pin (§V2-A04 `unknown_rejected`): None means unknown or
/// expired — the caller answers 409.
#[test]
fn unknown_rejected() {
    let g = gens("a", Duration::from_secs(60));
    assert_eq!(g.accepts("zz", now()), None);
}

/// The spec's own pin (§V2-A04 `previous_accepted_inside_window_only`).
#[test]
fn previous_accepted_inside_window_only() {
    let t0 = now();
    let mut g = gens("a", Duration::from_secs(60));
    g.promote("b".into(), t0);
    assert_eq!(
        g.accepts("a", t0 + Duration::from_secs(30)),
        Some(Gen::Previous)
    );
    assert_eq!(g.accepts("a", t0 + Duration::from_secs(61)), None);
}

#[test]
fn promote_demotes_current_to_previous() {
    let t0 = now();
    let mut g = gens("a", Duration::from_secs(60));
    assert_eq!(g.current(), "a");
    g.promote("b".into(), t0);
    assert_eq!(g.current(), "b");
    // The demoted generation is now the accepted "previous".
    assert_eq!(
        g.accepts("a", t0 + Duration::from_secs(1)),
        Some(Gen::Previous)
    );
    // A second promotion replaces the previous slot (window is 1-deep):
    // generation A falls out of acceptance immediately.
    g.promote("c".into(), t0);
    assert_eq!(g.accepts("a", t0), None);
    assert_eq!(g.accepts("b", t0), Some(Gen::Previous));
}

#[test]
fn two_generations_never_share_a_window() {
    // The window applies per demoted generation, from the promote instant;
    // the boundary itself is inclusive (<= window, matching the spec's own
    // 60s-window pin that accepts at 60s and rejects at 61s).
    let t0 = now();
    let mut g = gens("a", Duration::from_secs(10));
    g.promote("b".into(), t0);
    assert_eq!(
        g.accepts("a", t0 + Duration::from_secs(9)),
        Some(Gen::Previous)
    );
    assert_eq!(
        g.accepts("a", t0 + Duration::from_secs(10)),
        Some(Gen::Previous)
    );
    assert_eq!(g.accepts("a", t0 + Duration::from_secs(11)), None);
}

// --------------------------------------------------------------- data plane

#[test]
fn data_plane_paths() {
    assert!(is_data_plane_path("/__data"));
    assert!(is_data_plane_path("/__data?route=/")); // query tolerated
    assert!(is_data_plane_path("/_oxide/fn/thumb"));
    assert!(is_data_plane_path("/_oxide/fn/"));
    assert!(!is_data_plane_path("/"));
    assert!(!is_data_plane_path("/about"));
    assert!(!is_data_plane_path("/_oxide/fn")); // exact fn prefix only
    // V2-F05: the live route joins the data plane (the spec's "later
    // /__oxide/live" is now landed — the A04 pins rejected it until the
    // route's WP flipped the predicate deliberately).
    assert!(is_data_plane_path("/__oxide/live"));
    assert!(is_data_plane_path("/__oxide/live?route=routes/index")); // query tolerated
    // F07's stream route is still future: it must not enforce the header.
    assert!(!is_data_plane_path("/__oxide/stream/abc"));
    assert!(!is_data_plane_path("/__oxide/livex")); // no prefix bleed
    assert!(!is_data_plane_path("/__datax")); // no prefix bleed
}

// --------------------------------------------------------------- stage check

#[test]
fn stage_ignores_non_data_plane_paths() {
    let g = gens("new", Duration::from_secs(60));
    let stale = vec![(BUILD_HEADER.to_string(), "old".to_string())];
    // Even a build id the process does not accept is fine outside the data
    // plane: HTML navigations are re-rendered, not replayed.
    assert!(matches!(
        stage_build_check("/about", &stale, &g, now()),
        Flow::Continue
    ));
}

#[test]
fn stage_allows_missing_header() {
    // curl, tests, old clients: absent header means allowed (spec §V2-A04).
    let g = gens("new", Duration::from_secs(60));
    assert!(matches!(
        stage_build_check("/__data", &[], &g, now()),
        Flow::Continue
    ));
}

#[test]
fn stage_allows_current_build_header() {
    let g = gens("new", Duration::from_secs(60));
    let hs = vec![(BUILD_HEADER.to_string(), "new".to_string())];
    assert!(matches!(
        stage_build_check("/__data", &hs, &g, now()),
        Flow::Continue
    ));
}

#[test]
fn stage_header_lookup_is_case_insensitive() {
    // HTTP header names are case-insensitive (B-06); the lookup must be too.
    let g = gens("new", Duration::from_secs(60));
    let hs = vec![("X-OXIDE-BUILD".to_string(), "new".to_string())];
    assert!(matches!(
        stage_build_check("/__data", &hs, &g, now()),
        Flow::Continue
    ));
    // A rejected id spelled with different casing on the NAME still rejects.
    let stale = vec![("X-Oxide-Build".to_string(), "old".to_string())];
    assert!(matches!(
        stage_build_check("/__data", &stale, &g, now()),
        Flow::Respond(_)
    ));
}

#[test]
fn stage_rejects_stale_build_with_409_and_stale_header() {
    let g = gens("new", Duration::from_secs(60));
    let stale = vec![(BUILD_HEADER.to_string(), "old".to_string())];
    let flow = stage_build_check("/__data", &stale, &g, now());
    match flow {
        Flow::Respond(resp) => {
            assert_eq!(resp.status, 409);
            assert!(
                resp.headers
                    .iter()
                    .any(|(k, v)| k == STALE_HEADER && v == "true"),
                "stale marker header required: {:?}",
                resp.headers
            );
            assert!(resp.body.is_empty(), "empty body per spec");
            assert!(resp.stream.is_none());
        }
        Flow::Continue => panic!("stale data-plane request must short-circuit"),
    }
}

#[test]
fn stage_rejects_expired_previous_build() {
    let t0 = now();
    let mut g = gens("a", Duration::from_secs(60));
    g.promote("b".into(), t0);
    // The previous generation inside its window is accepted…
    let old = vec![(BUILD_HEADER.to_string(), "a".to_string())];
    assert!(matches!(
        stage_build_check("/__data", &old, &g, t0 + Duration::from_secs(30)),
        Flow::Continue
    ));
    // …and rejected once the window passes (the caller answers 409).
    assert!(matches!(
        stage_build_check("/__data", &old, &g, t0 + Duration::from_secs(61)),
        Flow::Respond(_)
    ));
}

#[test]
fn stage_rejection_only_short_circuits_data_plane() {
    // Non-data-plane + stale header combination stays Continue (the data
    // plane is the only plane with replayable bytes).
    let g = gens("new", Duration::from_secs(60));
    let stale = vec![(BUILD_HEADER.to_string(), "old".to_string())];
    assert!(matches!(
        stage_build_check("/_oxide/fn/thumb", &stale, &g, now()),
        Flow::Respond(_)
    ));
}

// ------------------------------------------------- live query build (V2-F05)

#[test]
fn live_route_read_build_from_the_query_parameter() {
    // EventSource cannot set headers (spec §V2-F05 step 2): the build id
    // travels as `?build=<id>` on the live route. An unaccepted one answers
    // 409 exactly like a stale header.
    let g = gens("new", Duration::from_secs(60));
    let flow = stage_build_check("/__oxide/live?route=routes/index&build=old", &[], &g, now());
    match flow {
        Flow::Respond(resp) => {
            assert_eq!(resp.status, 409);
            assert!(
                resp.headers
                    .iter()
                    .any(|(k, v)| k == STALE_HEADER && v == "true")
            );
        }
        Flow::Continue => panic!("a stale query build id must short-circuit the live route"),
    }
}

#[test]
fn live_route_accepts_the_current_build_in_the_query() {
    let g = gens("new", Duration::from_secs(60));
    assert!(matches!(
        stage_build_check("/__oxide/live?route=r&build=new", &[], &g, now()),
        Flow::Continue
    ));
    // Inside the demotion window the previous id is accepted too.
    let t0 = now();
    let mut g = gens("b", Duration::from_secs(60));
    g.promote("c".into(), t0);
    assert!(matches!(
        stage_build_check(
            "/__oxide/live?build=b",
            &[],
            &g,
            t0 + Duration::from_secs(10)
        ),
        Flow::Continue
    ));
}

#[test]
fn live_route_without_build_param_allows_through() {
    // Absent header AND absent query param (curl, tests): allowed — the
    // spec's "absent header allowed" contract carries over to the query.
    let g = gens("new", Duration::from_secs(60));
    assert!(matches!(
        stage_build_check("/__oxide/live?route=routes/index", &[], &g, now()),
        Flow::Continue
    ));
    assert!(matches!(
        stage_build_check("/__oxide/live", &[], &g, now()),
        Flow::Continue
    ));
}

#[test]
fn header_wins_over_the_live_query_param() {
    // A client that CAN set headers (the router's /__data fetch) sends
    // both only by mistake; the header is the canonical spelling.
    let g = gens("new", Duration::from_secs(60));
    let hs = vec![(BUILD_HEADER.to_string(), "new".to_string())];
    assert!(matches!(
        stage_build_check("/__oxide/live?build=old", &hs, &g, now()),
        Flow::Continue
    ));
}

#[test]
fn query_build_fallback_applies_to_the_live_route_only() {
    // /__data has no `build` query contract: the param is ignored there,
    // a stale id must travel in the header.
    let g = gens("new", Duration::from_secs(60));
    assert!(matches!(
        stage_build_check("/__data?build=old", &[], &g, now()),
        Flow::Continue
    ));
}

#[test]
fn live_query_rejects_expired_previous_build() {
    let t0 = now();
    let mut g = gens("a", Duration::from_secs(60));
    g.promote("b".into(), t0);
    assert!(matches!(
        stage_build_check(
            "/__oxide/live?build=a",
            &[],
            &g,
            t0 + Duration::from_secs(61)
        ),
        Flow::Respond(_)
    ));
}

// --------------------------------------------------------------- advertise

#[test]
fn advertise_build_sets_the_current_id() {
    let mut resp = oxide_fw_server::OxideResponse::new(200, String::new());
    advertise_build(&mut resp, "deadbeef00000000");
    assert!(
        resp.headers
            .iter()
            .any(|(k, v)| k == BUILD_HEADER && v == "deadbeef00000000"),
        "{:?}",
        resp.headers
    );
}

#[test]
fn advertise_build_is_idempotent() {
    // Every data-plane response carries the header; promoting a generation
    // must never stack duplicates on a response that already advertised.
    let mut resp = oxide_fw_server::OxideResponse::new(200, String::new());
    advertise_build(&mut resp, "first");
    advertise_build(&mut resp, "second");
    let count = resp
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case(BUILD_HEADER))
        .count();
    assert_eq!(count, 1, "no duplicates: {:?}", resp.headers);
    // First writer wins — re-advertising does not rewrite the value.
    let val = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(BUILD_HEADER))
        .map(|(_, v)| v.as_str());
    assert_eq!(val, Some("first"));
}

#[test]
fn advertise_build_leaves_existing_header_alone() {
    // A handler that already set the header (e.g. a proxy hop) is preserved.
    let mut resp = oxide_fw_server::OxideResponse::new(200, String::new());
    resp.headers
        .push(("X-Oxide-Build".to_string(), "proxied".to_string()));
    advertise_build(&mut resp, "mine");
    let val = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(BUILD_HEADER))
        .map(|(_, v)| v.as_str());
    assert_eq!(val, Some("proxied"));
}

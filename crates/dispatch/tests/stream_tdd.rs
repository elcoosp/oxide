//! TDD pins for the V2-F07 data-plane seam (`oxide_fw_dispatch::build`,
//! spec v0.2 §V2-F07 wiring step 2): the `GET /__oxide/stream/<id>`
//! reattach route joins the data plane the A04 build check runs on — the
//! same header, the same 409 contract — and the route's id segment parses
//! out of the request path.
//!
//! `build.rs` reserved this flip at A04 time: "`/__oxide/stream/*` joins
//! when that route lands (F07) — until then it must not enforce the
//! header, because no client would send it."
//!
//! Written FIRST (red): these pins failed on unresolved-symbol errors
//! before `stream_resume_id` and the data-plane flip existed. The route
//! handler itself (store lookup, Range parse, response) lives in
//! `oxide_fw_server::stream_store`; the wiring is the A01-phase-3 CLI
//! adoption — see `docs/deviations.md`.

use std::time::{Duration, Instant};

use oxide_fw_dispatch::build::{
    BUILD_HEADER, GenerationSet, STALE_HEADER, STALE_STATUS, is_data_plane_path, stage_build_check,
    stream_resume_id,
};
use oxide_fw_dispatch::flow::Flow;

// ------------------------------------------------------------- the data plane

/// The spec's own pin (§V2-F07 wiring step 2: the reattach route is a
/// data-plane route — it replays response bytes, so it enforces the A04
/// build contract).
#[test]
fn stream_resume_path_joins_the_data_plane() {
    assert!(
        is_data_plane_path("/__oxide/stream/deadbeef00000001"),
        "the reattach route replays bytes: it is data plane"
    );
    assert!(
        is_data_plane_path("/__oxide/stream/"),
        "the bare prefix too"
    );
    // The pre-F07 seams hold: the other data-plane members and the
    // everything-else rule are unchanged.
    assert!(is_data_plane_path("/__data"));
    assert!(is_data_plane_path("/_oxide/fn/get_user"));
    assert!(is_data_plane_path("/__oxide/live"));
    assert!(
        !is_data_plane_path("/__oxide/streams/x"),
        "only the exact /__oxide/stream/ prefix joins"
    );
    assert!(
        !is_data_plane_path("/"),
        "HTML navigations are re-rendered, not replayed"
    );
}

#[test]
fn stream_resume_id_parses_the_id_segment() {
    assert_eq!(
        stream_resume_id("/__oxide/stream/deadbeef00000001"),
        Some("deadbeef00000001"),
        "the id is the path segment after the prefix"
    );
    // The A04 seam's tolerance: query and fragment spellings strip.
    assert_eq!(
        stream_resume_id("/__oxide/stream/abc?x=1"),
        Some("abc"),
        "a query is tolerated (stripped)"
    );
    assert_eq!(
        stream_resume_id("/__oxide/stream/abc#frag"),
        Some("abc"),
        "a fragment is tolerated (stripped)"
    );
    assert_eq!(
        stream_resume_id("/__oxide/stream/"),
        None,
        "no id segment: not a reattach request"
    );
    assert_eq!(
        stream_resume_id("/__oxide/stream"),
        None,
        "the prefix without the trailing slash is a different path"
    );
    assert_eq!(
        stream_resume_id("/__data"),
        None,
        "non-stream paths are not reattach requests"
    );
    assert_eq!(
        stream_resume_id("/__oxide/stream/../etc"),
        None,
        "a traversal segment is not a stream id"
    );
}

// ------------------------------------------------------------ the build check

/// The A04 contract applies unchanged on the reattach route: a stale
/// `x-oxide-build` answers `409` + `x-oxide-build-stale: true`.
#[test]
fn build_check_applies_to_the_resume_route() {
    let gens = GenerationSet::new("gen2".to_string(), Duration::from_secs(60));
    let headers = vec![(BUILD_HEADER.to_string(), "gen1".to_string())];
    match stage_build_check(
        "/__oxide/stream/deadbeef00000001",
        &headers,
        &gens,
        Instant::now(),
    ) {
        Flow::Respond(resp) => {
            assert_eq!(resp.status, STALE_STATUS, "the stale status");
            assert_eq!(resp.header(STALE_HEADER), Some("true"), "the stale marker");
        }
        Flow::Continue => panic!("a stale build id on the reattach route must 409"),
    }
}

#[test]
fn absent_build_header_allows_the_resume_route() {
    let gens = GenerationSet::new("gen1".to_string(), Duration::from_secs(60));
    assert!(
        matches!(
            stage_build_check(
                "/__oxide/stream/deadbeef00000001",
                &[],
                &gens,
                Instant::now()
            ),
            Flow::Continue
        ),
        "curl, tests and old clients send no header"
    );
}

#[test]
fn current_build_header_continues_on_the_resume_route() {
    let gens = GenerationSet::new("gen1".to_string(), Duration::from_secs(60));
    let headers = vec![(BUILD_HEADER.to_string(), "gen1".to_string())];
    assert!(matches!(
        stage_build_check(
            "/__oxide/stream/deadbeef00000001",
            &headers,
            &gens,
            Instant::now()
        ),
        Flow::Continue
    ));
}

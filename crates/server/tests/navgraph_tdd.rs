//! TDD pins for the V2-F13 learned-prefetch server substrate
//! (`server::navgraph` + the `/_oxide/nav` ops-plane endpoint + the
//! `x-oxide-nav-from` observation arm on the data plane).
//!
//! The spec contract (§V2-F13):
//! - `NavGraph::observe(from, to)` accumulates aggregate edge counts —
//!   route ids, never URLs with params; no user identifiers.
//! - `decay` halves every weight once per half-life (spec default 7 days).
//! - `top(from, k)` ranks by weight, hides sparse edges behind the
//!   k-anonymity floor (spec default 20) and returns probabilities that
//!   sum to ≤ 1.
//! - `GET /_oxide/nav?from=<route>` answers the top edges as
//!   `{ from, edges: [{ to, probability }] }`; a missing/empty `from` is a
//!   400; the endpoint is a no-op (404) when no graph is attached.
//! - The client router stamps `x-oxide-nav-from`/`x-oxide-nav-to` route ids
//!   on its `/__data` navigations; the pipeline observes the edge and falls
//!   through to the normal data dispatch (the request is never answered by
//!   the observation).

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use oxide_fw_server::{
    NAV_ENDPOINT_PATH, NAV_FROM_HEADER, NAV_TO_HEADER, NavGraph, NavGraphSettings, OxideRequest,
    OxideResponse, Pipeline, nav_observation, nav_response_from_query,
};

// ------------------------------------------------------------------ helpers

/// A minimal GET request against `url` with extra headers (the
/// `status_sidecars_tdd.rs` shape).
fn get_with(url: &str, headers: &[(&str, &str)]) -> OxideRequest {
    OxideRequest {
        method: "GET".into(),
        url: url.into(),
        headers: Arc::new(
            headers
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect::<Vec<_>>(),
        ),
        header_index: OnceLock::default(),
        body: Bytes::new(),
        request_id: "req-f13".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

/// A minimal `/__data` GET with the two nav route-id headers stamped.
fn data_req(from: &str, to: &str) -> OxideRequest {
    get_with(
        &format!("/__data?route={to}"),
        &[(NAV_FROM_HEADER, from), (NAV_TO_HEADER, to)],
    )
}

/// The dispatch delegate every endpoint pin uses — it must never run for
/// the ops-plane arms under test.
fn refused_dispatch()
-> impl Fn(OxideRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
{
    |_r: OxideRequest| {
        Box::pin(async { OxideResponse::new(500, "dispatch must not run") })
            as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
    }
}

/// The dispatch the observation pin uses — a data request falls THROUGH to
/// the route dispatch (which 404s here: no route table in the harness).
fn data_dispatch()
-> impl Fn(OxideRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
{
    |_r: OxideRequest| {
        Box::pin(async { OxideResponse::new(404, "no route table") })
            as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
    }
}

/// A graph with the k-anonymity floor lowered to 1 — for the ranking /
/// normalization / decay mechanics tests where the counts are intentionally
/// small. The default-floor behavior has its own dedicated pins below.
fn small_graph() -> NavGraph {
    NavGraph::with_settings(NavGraphSettings {
        k_anon: 1.0,
        ..NavGraphSettings::default()
    })
}

// ------------------------------------------------------------- the graph

#[test]
fn observe_accumulates_edges_and_top_ranks_by_weight() {
    let g = small_graph();
    // /blog/[slug] -> /about is observed five times, the reverse once.
    for _ in 0..5 {
        g.observe("/blog/[slug]", "/about");
    }
    g.observe("/about", "/blog/[slug]");
    let top = g.top("/blog/[slug]", 3);
    assert_eq!(top.len(), 1, "one distinct edge from /blog/[slug]");
    assert_eq!(top[0].to, "/about");
    assert!(
        (top[0].probability - 1.0).abs() < 1e-9,
        "single edge => 1.0"
    );
}

#[test]
fn top_orders_descending_and_probabilities_normalize() {
    let g = small_graph();
    for _ in 0..6 {
        g.observe("/a", "/b");
    }
    for _ in 0..3 {
        g.observe("/a", "/c");
    }
    g.observe("/a", "/d");
    let top = g.top("/a", 10);
    assert_eq!(top[0].to, "/b");
    assert_eq!(top[1].to, "/c");
    assert_eq!(top[2].to, "/d");
    let sum: f64 = top.iter().map(|e| e.probability).sum();
    assert!(sum <= 1.0 + 1e-6, "probabilities sum to <= 1 (got {sum})");
    // 6/10, 3/10, 1/10 — the spec's probability normalization.
    assert!((top[0].probability - 0.6).abs() < 1e-6);
    assert!((top[1].probability - 0.3).abs() < 1e-6);
    assert!((top[2].probability - 0.1).abs() < 1e-6);
}

#[test]
fn top_k_caps_the_result() {
    let g = small_graph();
    for i in 0..5u32 {
        for _ in 0..30 - i {
            g.observe("/a", &format!("/r{i}"));
        }
    }
    let top = g.top("/a", 2);
    assert_eq!(top.len(), 2, "k caps the returned edges");
    assert_eq!(top[0].to, "/r0");
    assert_eq!(top[1].to, "/r1");
}

#[test]
fn k_anonymity_hides_sparse_edges() {
    // The spec default floor is 20 observations: one stray edge (a
    // genuinely rare transition) must not surface.
    let g = NavGraph::default();
    for _ in 0..100 {
        g.observe("/a", "/popular");
    }
    g.observe("/a", "/rare");
    let top = g.top("/a", 10);
    assert_eq!(top.len(), 1, "the sparse edge is hidden");
    assert_eq!(top[0].to, "/popular");
}

#[test]
fn k_anonymity_floor_is_configurable() {
    let g = NavGraph::with_settings(NavGraphSettings {
        k_anon: 3.0,
        ..NavGraphSettings::default()
    });
    for _ in 0..4 {
        g.observe("/a", "/b");
    }
    let top = g.top("/a", 10);
    assert_eq!(top.len(), 1, "a 3.0 floor admits a 4-observation edge");
}

#[test]
fn decay_halves_weights_after_one_half_life() {
    let g = NavGraph::default();
    for _ in 0..100 {
        g.observe("/a", "/b");
    }
    let half_life = NavGraphSettings::default().half_life_s;
    // Advance the clock one half-life past the last observe and decay.
    let now = Instant::now() + Duration::from_secs_f64(half_life);
    g.decay(now);
    let top = g.top("/a", 10);
    assert_eq!(top.len(), 1);
    assert!(
        (top[0].probability - 1.0).abs() < 1e-6,
        "a single edge keeps probability 1.0 across decay"
    );
    // The WEIGHT halved even though the probability renormalizes: the
    // observable is that after more half-lives the edge drops below the
    // k-anonymity floor and disappears (100 -> 50 -> 25 -> 12.5 < 20).
    let now2 = now + Duration::from_secs_f64(half_life);
    g.decay(now2);
    let top2 = g.top("/a", 10);
    assert_eq!(top2.len(), 1, "25 observations still clear the 20 floor");
    let now3 = now2 + Duration::from_secs_f64(half_life);
    g.decay(now3);
    let top3 = g.top("/a", 10);
    assert!(
        top3.is_empty(),
        "12.5 observations after three half-lives: below the 20 floor"
    );
}

#[test]
fn observe_skips_unsanitized_route_ids_instead_of_folding_them() {
    let g = small_graph();
    // A full URL with a query, an empty value and a value with a space are
    // SKIPPED — the graph never grows a junk magnet edge (the vitals
    // beacon folds garbage to `_other`; a nav edge must not: `_other`
    // would accumulate an artificial popular target).
    g.observe("/a?tracking=1", "/b");
    g.observe("/a", "");
    g.observe("/a", "/with space");
    g.observe("", "/b");
    assert!(g.is_empty(), "no junk edge survives sanitization");
}

#[test]
fn edge_cap_bounds_the_graph() {
    let g = NavGraph::with_settings(NavGraphSettings {
        max_edges: 16,
        k_anon: 1.0,
        ..NavGraphSettings::default()
    });
    for i in 0..64 {
        g.observe("/a", &format!("/r{i}"));
    }
    assert_eq!(g.len(), 16, "the graph stops growing at max_edges");
}

#[test]
fn persist_and_load_round_trip_preserves_weights() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("nav-graph.json");
    let g = small_graph();
    for _ in 0..5 {
        g.observe("/a", "/b");
    }
    g.persist(&path).expect("persist");

    // `load` builds a DEFAULT-floor graph; the persisted weight (5) sits
    // below the 20 floor, so the pin reads the edge count and probes the
    // weight through a lowered-floor graph loaded from the same file.
    let g2 = NavGraph::load(&path);
    assert_eq!(g2.len(), 1, "the edge survives the round trip");
    let g3 = NavGraph::with_settings(NavGraphSettings {
        k_anon: 1.0,
        ..NavGraphSettings::default()
    });
    g3.merge_from(&path);
    let top = g3.top("/a", 3);
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].to, "/b");
}

#[test]
fn load_ignores_malformed_or_absent_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("nav-graph.json");
    std::fs::write(&path, "not json at all").expect("write junk");
    let g = NavGraph::load(&path);
    assert!(g.is_empty(), "malformed content is ignored, not fatal");
    let missing = NavGraph::load(&tmp.path().join("absent.json"));
    assert!(missing.is_empty(), "a missing file is a fresh graph");
}

#[test]
fn load_rejects_unexpected_shapes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Wrong version / wrong shape: ignored (the static_status pattern —
    // malformed content is never a serve-start failure).
    let p1 = tmp.path().join("v.json");
    std::fs::write(&p1, r#"{"version": 99, "edges": {}}"#).expect("write");
    assert!(NavGraph::load(&p1).is_empty());
    let p2 = tmp.path().join("shape.json");
    std::fs::write(&p2, r#"{"version": 1, "edges": {"a": [1,2]}}"#).expect("write");
    assert!(
        NavGraph::load(&p2).is_empty(),
        "array-shaped edges are ignored"
    );
}

#[tokio::test]
async fn spawn_flusher_persists_a_start_snapshot() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("nav-graph.json");
    let g = Arc::new(NavGraph::with_settings(NavGraphSettings {
        flush_secs: 3600,
        ..NavGraphSettings::default()
    }));
    g.observe("/a", "/b");
    // The flusher snapshots once at spawn (a restart never loses the
    // graph it just loaded), then every `flush_secs`.
    let handle = NavGraph::spawn_flusher(&g, path.clone());
    for _ in 0..40 {
        if path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    handle.abort();
    assert!(path.exists(), "the flusher wrote the snapshot");
    let g2 = NavGraph::load(&path);
    assert_eq!(g2.len(), 1);
}

// ------------------------------------------------- the observation + endpoint

#[test]
fn nav_observation_parses_both_route_id_headers() {
    let req = data_req("/blog/[slug]", "/about");
    let (from, to) = nav_observation(&req).expect("both headers present");
    assert_eq!(from, "/blog/[slug]");
    assert_eq!(to, "/about");
}

#[test]
fn nav_observation_requires_both_headers_and_the_data_path() {
    let req = get_with("/__data?route=/b", &[(NAV_FROM_HEADER, "/a")]);
    assert!(
        nav_observation(&req).is_none(),
        "a missing to-header is not an observation"
    );
    // A non-data path never observes (the headers only ride /__data).
    let other = get_with(
        "/about",
        &[(NAV_FROM_HEADER, "/a"), (NAV_TO_HEADER, "/about")],
    );
    assert!(nav_observation(&other).is_none(), "only /__data observes");
    // Garbage route ids are skipped, not folded.
    let junk = get_with(
        "/__data?route=/x",
        &[(NAV_FROM_HEADER, "/a?query=1"), (NAV_TO_HEADER, "/b")],
    );
    assert!(
        nav_observation(&junk).is_none(),
        "unsanitized ids are skipped"
    );
    // A non-GET /__data (POST to the data plane) does not observe.
    let mut post = data_req("/a", "/b");
    post.method = "POST".into();
    assert!(nav_observation(&post).is_none());
}

#[tokio::test]
async fn pipeline_observes_data_plane_nav_and_falls_through() {
    // The observation arm must never ANSWER the request: the /__data
    // fetch proceeds to the dispatch delegate (which 404s here — there is
    // no route table), and the edge still lands in the graph.
    let g = Arc::new(small_graph());
    let p = Pipeline::default().with_nav_graph(Arc::clone(&g));
    let req = data_req("/blog/[slug]", "/about");
    let resp = p.serve_request(req, &data_dispatch()).await;
    assert_eq!(resp.status, 404, "the data dispatch ran (fall-through)");
    let top = g.top("/blog/[slug]", 3);
    assert_eq!(top.len(), 1, "the edge was observed on the way through");
    assert_eq!(top[0].to, "/about");
}

#[test]
fn pipeline_without_a_graph_observes_nothing() {
    // The default pipeline (nav None) must not panic and must not
    // intercept /__data at all — the pre-F13 shape.
    let p = Pipeline::default();
    let req = data_req("/a", "/b");
    let resp = futures::executor::block_on(p.serve_request(req, &data_dispatch()));
    assert_eq!(resp.status, 404);
}

#[tokio::test]
async fn nav_endpoint_answers_top_edges_as_json() {
    // Default floor (20): both edges carry enough weight to surface —
    // this pin doubles as the floor-passes-real-traffic proof.
    let g = Arc::new(NavGraph::new());
    for _ in 0..80 {
        g.observe("/a", "/b");
    }
    for _ in 0..20 {
        g.observe("/a", "/c");
    }
    let p = Pipeline::default().with_nav_graph(Arc::clone(&g));
    let req = get_with(&format!("{NAV_ENDPOINT_PATH}?from=%2Fa"), &[]);
    let resp = p.serve_request(req, &refused_dispatch()).await;
    assert_eq!(resp.status, 200, "the endpoint answers 200");
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("json body");
    assert_eq!(body["from"], "/a");
    let edges = body["edges"].as_array().expect("edges array");
    assert_eq!(edges.len(), 2);
    assert_eq!(edges[0]["to"], "/b");
    assert!((edges[0]["probability"].as_f64().unwrap() - 0.8).abs() < 1e-6);
    assert!(
        edges[0]["probability"].as_f64().unwrap() >= 0.0,
        "probabilities are non-negative"
    );
}

#[tokio::test]
async fn nav_endpoint_refuses_an_empty_from_with_400() {
    let g = Arc::new(NavGraph::new());
    let p = Pipeline::default().with_nav_graph(g);
    for url in [
        NAV_ENDPOINT_PATH.to_string(),
        format!("{NAV_ENDPOINT_PATH}?from="),
    ] {
        let req = get_with(&url, &[]);
        let resp = p.serve_request(req, &refused_dispatch()).await;
        assert_eq!(resp.status, 400, "a missing/empty `from` is a 400");
    }
}

#[tokio::test]
async fn nav_endpoint_is_disabled_without_a_graph() {
    // The image-endpoint contract: `None` disables the endpoint (404), so
    // a project without learned prefetch serves pre-F13 responses.
    let p = Pipeline::default();
    let req = get_with(&format!("{NAV_ENDPOINT_PATH}?from=%2Fa"), &[]);
    let resp = p.serve_request(req, &refused_dispatch()).await;
    assert_eq!(resp.status, 404, "no graph attached => the endpoint is off");
}

#[tokio::test]
async fn nav_endpoint_refuses_non_get_methods() {
    let g = Arc::new(NavGraph::new());
    let p = Pipeline::default().with_nav_graph(g);
    let mut req = get_with(&format!("{NAV_ENDPOINT_PATH}?from=%2Fa"), &[]);
    req.method = "POST".into();
    let resp = p.serve_request(req, &refused_dispatch()).await;
    assert_eq!(resp.status, 405, "POST is not the nav endpoint's verb");
    assert!(
        resp.header("allow").is_some_and(|v| v.contains("GET")),
        "the 405 carries the allow header like the other method guards"
    );
}

#[test]
fn nav_response_from_query_is_the_pure_seam() {
    let g = NavGraph::new();
    for _ in 0..80 {
        g.observe("/a", "/b");
    }
    let resp = nav_response_from_query(&g, "from=%2Fa&limit=1");
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("json");
    assert_eq!(
        body["edges"].as_array().unwrap().len(),
        1,
        "limit respected"
    );
    let bad = nav_response_from_query(&g, "");
    assert_eq!(bad.status, 400);
    // A non-numeric limit is ignored (the default applies).
    let odd = nav_response_from_query(&g, "from=%2Fa&limit=many");
    assert_eq!(odd.status, 200);
}

#[test]
fn constants_carry_the_spec_header_spellings() {
    assert_eq!(NAV_FROM_HEADER, "x-oxide-nav-from");
    assert_eq!(NAV_TO_HEADER, "x-oxide-nav-to");
    assert_eq!(NAV_ENDPOINT_PATH, "/_oxide/nav");
}

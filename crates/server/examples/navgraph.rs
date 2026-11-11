//! Walkthrough: the V2-F13 learned-prefetch contract, end to end — the
//! consumer `oxide-fw-server` never had for this seam.
//!
//! The server keeps an aggregate route-transition graph behind the typed
//! `prefetch.learned` config switch: the pipeline's native prefix observes
//! the client router's `x-oxide-nav-from`/`x-oxide-nav-to` headers on
//! `/__data` navigations, and `GET /_oxide/nav?from=<route>` answers the
//! top edges as probabilities (k-anonymity floor 20 observations, weights
//! decayed with a 7-day half-life, an hourly snapshot to
//! `.oxide/nav-graph.json` across restarts).
//!
//! Run: `cargo run -p oxide-fw-server --example navgraph`

use bytes::Bytes;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use oxide_fw_server::{
    NAV_ENDPOINT_PATH, NAV_FROM_HEADER, NAV_TO_HEADER, NavGraph, NavGraphSettings, OxideRequest,
    OxideResponse, Pipeline, nav_response_from_query,
};

fn main() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let snapshot = tmp.path().join("nav-graph.json");

    // 1. Observe a realistic transition pattern (what the data plane sees
    //    on /__data requests carrying the router's route-id headers).
    //    Floor lowered so the toy counts surface — production keeps 20.
    let g = Arc::new(NavGraph::with_settings(NavGraphSettings {
        k_anon: 5.0,
        flush_secs: 3600,
        ..NavGraphSettings::default()
    }));
    for _ in 0..12 {
        g.observe("/blog/[slug]", "/about");
    }
    for _ in 0..6 {
        g.observe("/blog/[slug]", "/tags");
    }
    // A garbage edge is SKIPPED, not folded: no junk magnet target.
    g.observe("/blog/[slug]", "/x?tracking=1");
    println!(
        "observe: 12x /blog/[slug]->/about, 6x ->/tags, 1 junk skipped => {} edges",
        g.len()
    );

    // 2. The endpoint: the pure renderer the pipeline arm calls.
    let resp = nav_response_from_query(&g, "from=%2Fblog%2F%5Bslug%5D");
    println!(
        "GET {NAV_ENDPOINT_PATH}?from=/blog/[slug] -> {} {}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );

    // 3. The pipeline arms: an observation request falls through (the
    //    404 below is the data dispatch answering, not the nav arm), and
    //    the graph-less pipeline 404s the endpoint.
    let pipeline = Pipeline::default().with_nav_graph(Arc::clone(&g));
    let nav_req = OxideRequest {
        method: "GET".into(),
        url: "/__data?route=%2Fabout".into(),
        headers: Arc::new(vec![
            (NAV_FROM_HEADER.to_string(), "/blog/[slug]".to_string()),
            (NAV_TO_HEADER.to_string(), "/about".to_string()),
        ]),
        header_index: OnceLock::default(),
        body: Bytes::default(),
        request_id: "ex-f13".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    };
    let dispatch =
        |_r: OxideRequest| -> Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>> {
            Box::pin(async { OxideResponse::new(404, "route dispatch ran") })
        };
    let resp = futures::executor::block_on(pipeline.serve_request(nav_req, &dispatch));
    println!(
        "GET /__data (nav headers) -> {} (fall-through; the edge is in the graph)",
        resp.status
    );
    let off = Pipeline::default();
    let endpoint_req = OxideRequest {
        method: "GET".into(),
        url: format!("{NAV_ENDPOINT_PATH}?from=%2Fblog%2F%5Bslug%5D"),
        headers: Arc::new(vec![]),
        header_index: OnceLock::default(),
        body: Bytes::default(),
        request_id: "ex-f13".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    };
    let resp = futures::executor::block_on(off.serve_request(endpoint_req, &dispatch));
    println!(
        "GET /_oxide/nav without a graph -> {status} (not configured)",
        status = resp.status
    );

    // 4. Persist + load: the hourly snapshot survives a restart.
    g.persist(&snapshot).expect("persist");
    let reloaded = NavGraph::load(&snapshot);
    println!(
        "persist/load: snapshot {} bytes, {} edge(s) reloaded",
        std::fs::metadata(&snapshot).map_or(0, |m| m.len()),
        reloaded.len()
    );
}

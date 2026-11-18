//! The server crate's cross-module integration pins (the placeholder
//! `mod placeholder {}` stub, filled): the dev-plane surfaces ride ONE
//! pipeline — the trace feed records every completed request while the
//! log feed serves the window the capture layer recorded into, both
//! behind the same dev-token guard, and the endpoint map the docs
//! generator folds stays the single inventory of those surfaces.
//!
//! The per-module TDD files (`trace_feed_tdd`, `trace_query_tdd`,
//! `trace_export_tdd`, `log_feed_tdd`, `trace_file_tdd`) pin each seam's
//! contract in depth; these pins hold the CROSS-module shapes — one
//! pipeline, two windows, one guard — and the map/inventory coherence.

use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use oxide_fw_server::{
    ENDPOINT_MAP, LogsEndpoint, OxideRequest, OxideResponse, Pipeline, RingLogSink, RingTraceSink,
    TraceSink, TracesEndpoint,
};

/// A minimal GET request (the `trace_feed_tdd.rs` shape). `token` rides
/// the query — the D41 spelling the dev overlay uses.
fn get_with(url: &str) -> OxideRequest {
    OxideRequest {
        method: "GET".into(),
        url: url.into(),
        headers: Arc::new(Vec::new()),
        header_index: OnceLock::default(),
        body: Bytes::new(),
        request_id: "req-int".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

/// The dispatch delegate — a trivial 200 render the round trips ride.
fn ok_dispatch()
-> impl Fn(OxideRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
{
    |_r: OxideRequest| {
        Box::pin(async { OxideResponse::new(200, "ok") })
            as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
    }
}

/// One pipeline, two windows: the trace sink records the completed
/// dispatches, the log endpoint serves the capture ring's rows, and one
/// dev token guards both dev-plane arms.
#[tokio::test(flavor = "current_thread")]
async fn one_pipeline_serves_both_dev_plane_windows() {
    let traces = Arc::new(RingTraceSink::new(8));
    let logs = Arc::new(RingLogSink::new(8));
    let mut p = Pipeline::default()
        .with_trace_sink(traces.clone() as Arc<dyn TraceSink>)
        .with_trace_feed(Arc::new(TracesEndpoint::new(traces.clone())))
        .with_log_feed(Arc::new(LogsEndpoint::new(
            logs.clone() as Arc<dyn oxide_fw_server::LogSink>
        )));
    p.dev_token = Some("tok".into());

    // Two dispatches: each records one trace row (the §20 consumption).
    for path in ["/index", "/submit"] {
        let status =
            futures::executor::block_on(p.serve_request(get_with(path), &ok_dispatch())).status;
        assert_eq!(status, 200);
    }
    assert_eq!(traces.snapshot().len(), 2, "every dispatch records a row");

    // Both dev-plane arms answer behind the SAME token, on the SAME
    // pipeline.
    let feed = futures::executor::block_on(
        p.serve_request(get_with("/__oxide/traces?token=tok"), &ok_dispatch()),
    );
    assert_eq!(feed.status, 200, "the trace feed answers with the token");
    let logs_resp = futures::executor::block_on(
        p.serve_request(get_with("/__oxide/logs?token=tok"), &ok_dispatch()),
    );
    assert_eq!(logs_resp.status, 200, "the log feed answers with the token");

    // One guard, two rows: the token-less shape is refused on both.
    let refused_trace =
        futures::executor::block_on(p.serve_request(get_with("/__oxide/traces"), &ok_dispatch()))
            .status;
    let refused_logs =
        futures::executor::block_on(p.serve_request(get_with("/__oxide/logs"), &ok_dispatch()))
            .status;
    assert_eq!(refused_trace, 403);
    assert_eq!(refused_logs, 403);
}

/// The map stays the one inventory: every dev-plane path the pipeline
/// routes (the traces and logs rows) appears in `ENDPOINT_MAP`, so the
/// docs generator's page can never forget a routed surface.
#[test]
fn every_routed_dev_plane_path_is_in_the_map() {
    for path in ["/__oxide/traces", "/__oxide/logs"] {
        assert!(
            ENDPOINT_MAP.iter().any(|r| r.path == path),
            "{path} is routed but not in the map"
        );
    }
}

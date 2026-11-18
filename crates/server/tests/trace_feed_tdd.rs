//! TDD pins for the V2-F21 dev-plane trace feed (the server half):
//! `GET /__oxide/traces` — the §V2.J map's new dev-plane row, beside the
//! open-in-editor and component-cost surfaces (`/__oxide/open`,
//! `/__oxide/cost`, `/__oxide/traces` — dev plane, dev-token guarded, D41).
//!
//! §20 leaves the parsed W3C trace context *available but unconsumed* —
//! `OxideRequest.trace` rides every request and nothing reads it. The
//! feed is the consumption seam:
//!
//! - [`TraceSink`] records one [`RequestTrace`] per completed pipeline
//!   request (the native and dispatch arms both);
//! - [`RingTraceSink`] is the bounded production impl — a capacity-capped
//!   ring, newest row first on read, the oldest evicted at capacity;
//! - `TracesEndpoint` answers `GET /__oxide/traces` with the ring's rows
//!   as camelCase JSON (the §14 config-key convention);
//! - the endpoint only exists when attached (`trace_sink`/`trace_feed`
//!   are `None` in the default pipeline — production never attaches, the
//!   unattached shape falls through to the normal dispatch, the
//!   inspector's posture).

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use oxide_fw_server::trace_context::TraceContext;
use oxide_fw_server::{
    OxideRequest, OxideResponse, Pipeline, RequestTrace, RingTraceSink, TRACE_FEED_ENDPOINT_PATH,
    TraceSink, TracesEndpoint,
};

// ------------------------------------------------------------------ helpers

/// A minimal GET request against `url` (the `cost_endpoint_tdd.rs` shape).
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
        request_id: "req-f21".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

/// The same request carrying a parsed trace context (the shape the
/// pipeline's §20 extraction produces for a `traceparent` header).
fn traced_get_with(url: &str) -> OxideRequest {
    let mut req = get_with(url, &[("traceparent", TRACED_PARENT)]);
    req.trace = TraceContext::parse_traceparent(TRACED_PARENT);
    req
}

const TRACED_PARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

/// The dispatch delegate the fall-through pins use — the endpoint arm
/// must answer the request itself, never delegate.
fn refused_dispatch()
-> impl Fn(OxideRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
{
    |_r: OxideRequest| {
        Box::pin(async { OxideResponse::new(500, "dispatch must not run") })
            as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
    }
}

/// The dispatch delegate the record pins use — a completed 200 the
/// pipeline's dispatch arm must land in the sink.
fn ok_dispatch()
-> impl Fn(OxideRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
{
    |_r: OxideRequest| {
        Box::pin(async { OxideResponse::new(200, "ok") })
            as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
    }
}

// ------------------------------------------------------------- the contracts

#[test]
fn path_const_is_the_dev_plane_spelling() {
    assert_eq!(TRACE_FEED_ENDPOINT_PATH, "/__oxide/traces");
}

#[test]
fn ring_records_and_snapshots_newest_first() {
    let sink = RingTraceSink::new(8);
    for i in 0..3 {
        sink.record(RequestTrace::of(
            &format!("req-{i}"),
            "GET",
            &format!("/r{i}"),
            None,
            200,
            Duration::from_millis(i),
        ));
    }
    let rows = sink.snapshot();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].path, "/r2", "newest first");
    assert_eq!(rows[1].path, "/r1");
    assert_eq!(rows[2].path, "/r0", "oldest last");
}

#[test]
fn ring_evicts_the_oldest_at_capacity() {
    let sink = RingTraceSink::new(2);
    for i in 0..3 {
        sink.record(RequestTrace::of(
            &format!("req-{i}"),
            "GET",
            &format!("/r{i}"),
            None,
            200,
            Duration::from_millis(i),
        ));
    }
    let rows = sink.snapshot();
    assert_eq!(rows.len(), 2, "capacity bounds the ring");
    assert_eq!(rows[0].path, "/r2", "the newest survives");
    assert_eq!(rows[1].path, "/r1", "the oldest was evicted");
}

#[test]
fn zero_capacity_ring_records_nothing() {
    let sink = RingTraceSink::new(0);
    sink.record(RequestTrace::of(
        "req-0",
        "GET",
        "/r",
        None,
        200,
        Duration::ZERO,
    ));
    assert!(sink.snapshot().is_empty(), "a zero window keeps no rows");
}

#[test]
fn request_trace_maps_the_context_one_to_one() {
    let tc = TraceContext::parse_traceparent(TRACED_PARENT).expect("valid traceparent");
    let row = RequestTrace::of(
        "req-f21",
        "GET",
        "/index",
        Some(&tc),
        200,
        Duration::from_micros(1250),
    );
    assert_eq!(row.request_id, "req-f21");
    assert_eq!(row.method, "GET");
    assert_eq!(row.path, "/index");
    assert_eq!(row.status, 200);
    assert!((row.duration_ms - 1.25).abs() < 1e-9, "{}", row.duration_ms);
    assert_eq!(
        row.trace_id.as_deref(),
        Some("0af7651916cd43dd8448eb211c80319c")
    );
    assert_eq!(row.parent_id.as_deref(), Some("b7ad6b7169203331"));
    assert!(row.sampled);
    assert_eq!(row.tracestate, None);
}

#[test]
fn request_trace_without_context_keeps_the_none_fields() {
    let row = RequestTrace::of(
        "req-f21",
        "POST",
        "/submit",
        None,
        404,
        Duration::from_millis(2),
    );
    assert_eq!(row.trace_id, None);
    assert_eq!(row.parent_id, None);
    assert!(!row.sampled);
    assert_eq!(row.tracestate, None);
    assert_eq!(row.status, 404);
}

#[test]
fn trace_sink_seam_is_object_safe() {
    // The dev boot attaches it as `Arc<dyn TraceSink>` (the object safety
    // the `Pipeline::trace_sink` field and `TracesEndpoint::new` demand).
    let sink: Arc<dyn TraceSink> = Arc::new(RingTraceSink::new(4));
    assert!(sink.snapshot().is_empty());
}

// ------------------------------------------------------------- the endpoint

fn endpoint() -> TracesEndpoint {
    TracesEndpoint::new(Arc::new(RingTraceSink::new(8)) as Arc<dyn TraceSink>)
}

#[test]
fn endpoint_refuses_non_get_with_405_and_allow() {
    let resp = endpoint().serve("POST");
    assert_eq!(resp.status, 405);
    assert_eq!(resp.header("allow"), Some("GET"));
}

#[test]
fn endpoint_over_an_empty_ring_answers_an_empty_array() {
    let resp = endpoint().serve("GET");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("content-type"), Some("application/json"));
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("json");
    assert!(body["traces"].is_array(), "{body}");
    assert_eq!(body["traces"].as_array().map(Vec::len), Some(0));
}

#[test]
fn endpoint_answers_get_with_the_rows_json() {
    let sink = Arc::new(RingTraceSink::new(8));
    let tc = TraceContext::parse_traceparent(TRACED_PARENT).expect("valid traceparent");
    sink.record(RequestTrace::of(
        "req-a",
        "GET",
        "/index",
        Some(&tc),
        200,
        Duration::from_micros(1250),
    ));
    sink.record(RequestTrace::of(
        "req-b",
        "POST",
        "/submit",
        None,
        404,
        Duration::from_millis(3),
    ));
    let ep = TracesEndpoint::new(sink as Arc<dyn TraceSink>);
    let resp = ep.serve("GET");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("content-type"), Some("application/json"));
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("json");
    let rows = body["traces"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2);
    // Newest first, camelCase on the wire (the §14 convention).
    assert_eq!(rows[0]["requestId"], "req-b");
    assert_eq!(rows[0]["method"], "POST");
    assert_eq!(rows[0]["path"], "/submit");
    assert_eq!(rows[0]["status"], 404);
    assert!(rows[0]["traceId"].is_null(), "{body}");
    assert!(
        !rows[0]["sampled"].as_bool().expect("sampled bool"),
        "the untraced row is unsampled"
    );
    assert_eq!(rows[1]["requestId"], "req-a");
    assert_eq!(
        rows[1]["traceId"], "0af7651916cd43dd8448eb211c80319c",
        "the W3C trace id rides the row"
    );
    assert_eq!(rows[1]["parentId"], "b7ad6b7169203331");
    assert!(
        rows[1]["sampled"].as_bool().expect("sampled bool"),
        "the sampled bit rides the row"
    );
    assert!(rows[1]["durationMs"].as_f64().is_some(), "{body}");
}

// ------------------------------------------------------------- the pipeline arm

#[tokio::test]
async fn pipeline_without_the_feed_falls_through() {
    // The default pipeline never attaches the feed: the request falls
    // through to the normal dispatch (production byte-identical).
    let p = Pipeline::default();
    assert!(p.trace_sink.is_none(), "no sink by default");
    assert!(p.trace_feed.is_none(), "no feed endpoint by default");
    let resp = p
        .serve_request(get_with("/__oxide/traces", &[]), &refused_dispatch())
        .await;
    assert_eq!(resp.status, 500, "the dispatch delegate ran (fall-through)");
}

#[tokio::test]
async fn pipeline_answers_200_with_the_token() {
    let mut p = Pipeline::default().with_trace_feed(Arc::new(endpoint()));
    p.dev_token = Some("tok".into());
    let resp = p
        .serve_request(
            get_with("/__oxide/traces?token=tok", &[]),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("json");
    assert!(body["traces"].is_array(), "{body}");
}

#[tokio::test]
async fn pipeline_refuses_a_wrong_or_missing_token_with_403() {
    let mut p = Pipeline::default().with_trace_feed(Arc::new(endpoint()));
    p.dev_token = Some("tok".into());
    let missing = p
        .serve_request(get_with("/__oxide/traces", &[]), &refused_dispatch())
        .await;
    assert_eq!(missing.status, 403);
    let wrong = p
        .serve_request(
            get_with("/__oxide/traces", &[("x-oxide-fw-dev-token", "nope")]),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(wrong.status, 403);
}

#[tokio::test]
async fn pipeline_records_the_feed_request_for_the_next_query() {
    // One instance, two attach points (the WaterfallRecorder posture):
    // the pipeline records into the sink the feed endpoint serves. The
    // snapshot is a point-in-time read taken BEFORE the query's own row
    // exists (the row lands on completion — the endpoint never
    // fabricates a record for a request whose status is not yet known),
    // so a query answers the pre-query state and its own row rides the
    // NEXT query's answer.
    let sink = Arc::new(RingTraceSink::new(8));
    let mut p = Pipeline::default()
        .with_trace_sink(sink.clone() as Arc<dyn TraceSink>)
        .with_trace_feed(Arc::new(TracesEndpoint::new(sink.clone())));
    p.dev_token = Some("tok".into());
    let first = p
        .serve_request(
            get_with("/__oxide/traces?token=tok", &[]),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(first.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&first.body).expect("json");
    assert_eq!(
        body["traces"].as_array().map(Vec::len),
        Some(0),
        "the pre-query state is the empty ring: {body}"
    );
    let second = p
        .serve_request(
            get_with("/__oxide/traces?token=tok", &[]),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(second.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&second.body).expect("json");
    let rows = body["traces"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1, "the first feed request landed: {body}");
    assert_eq!(rows[0]["path"], "/__oxide/traces");
    assert_eq!(rows[0]["status"], 200);
    assert_eq!(rows[0]["requestId"], "req-f21");
}

#[tokio::test]
async fn serve_request_records_completed_dispatches_into_the_sink() {
    let sink = Arc::new(RingTraceSink::new(8));
    let p = Pipeline::default().with_trace_sink(sink.clone() as Arc<dyn TraceSink>);
    let resp = p
        .serve_request(traced_get_with("/index"), &ok_dispatch())
        .await;
    assert_eq!(resp.status, 200);
    let rows = sink.snapshot();
    assert_eq!(rows.len(), 1, "the completed dispatch is recorded");
    assert_eq!(rows[0].path, "/index");
    assert_eq!(rows[0].method, "GET");
    assert_eq!(rows[0].status, 200);
    assert_eq!(rows[0].request_id, "req-f21");
    // The §20 consumption: the parsed trace context rides the row.
    assert_eq!(
        rows[0].trace_id.as_deref(),
        Some("0af7651916cd43dd8448eb211c80319c")
    );
    assert_eq!(rows[0].parent_id.as_deref(), Some("b7ad6b7169203331"));
    assert!(rows[0].sampled);
}

#[tokio::test]
async fn serve_request_records_untraced_rows_with_none_trace_fields() {
    let sink = Arc::new(RingTraceSink::new(8));
    let p = Pipeline::default().with_trace_sink(sink.clone() as Arc<dyn TraceSink>);
    let resp = p
        .serve_request(get_with("/index", &[]), &ok_dispatch())
        .await;
    assert_eq!(resp.status, 200);
    let rows = sink.snapshot();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].trace_id, None, "an untraced request says so");
    assert!(!rows[0].sampled);
}

#[tokio::test]
async fn serve_request_without_a_sink_records_nothing_and_is_cheap() {
    // The default pipeline's record point is a None check — no
    // allocation, no row, the pre-F21 byte shape.
    let p = Pipeline::default();
    let resp = p
        .serve_request(get_with("/index", &[]), &ok_dispatch())
        .await;
    assert_eq!(resp.status, 200);
}

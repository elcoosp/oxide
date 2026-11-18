//! TDD pins for the V2-F21 trace response half (§20's propagation): the
//! W3C `traceresponse` header a traced dispatch response carries.
//!
//! §20: "W3C trace context propagated into `fetch` op calls and user
//! code." The request half is the parse (`OxideRequest.trace`); this is
//! the RESPONSE half — a request that arrived with a parseable
//! `traceparent` leaves with a `traceresponse` header (the W3C Trace
//! Context Level 2 draft's shape: `00-<32hex trace-id>-<16hex
//! response-id>-<2hex flags>`), so the caller sees the server's response
//! span id and joins its trace. The emission rides the dispatch arm only
//! (the framework-rendered responses — where §20's "user code" answers),
//! never overwrites a header the dispatch already set, and an untraced
//! request stays byte-identical (no header, no cost).

use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use oxide_fw_server::trace_context::TraceContext;
use oxide_fw_server::{OxideRequest, OxideResponse, Pipeline};

/// A parseable `traceparent` (the sampled shape).
const TRACED_PARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

/// A per-request id in the pipeline's SEC-02 shape (`req-` + 32 hex) —
/// the response span id derives from its first 16 hex.
const REQUEST_ID: &str = "req-11aa22bb33cc44dd55ee66ff77008899";

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
        request_id: REQUEST_ID.into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

fn traced_get_with(url: &str) -> OxideRequest {
    let mut req = get_with(url, &[("traceparent", TRACED_PARENT)]);
    req.trace = TraceContext::parse_traceparent(TRACED_PARENT);
    req
}

/// The dispatch delegate — a trivial 200 the emission rides.
fn ok_dispatch()
-> impl Fn(OxideRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
{
    |_r: OxideRequest| {
        Box::pin(async { OxideResponse::new(200, "ok") })
            as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
    }
}

// ------------------------------------------------------- the format half

#[test]
fn to_traceresponse_roundtrips_the_draft_shape() {
    let tc = TraceContext::parse_traceparent(TRACED_PARENT).expect("valid traceparent");
    let value = tc
        .to_traceresponse("b7ad6b7169203332")
        .expect("a valid response id emits");
    assert_eq!(
        value, "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203332-01",
        "the L2 draft shape: 00-<trace-id>-<response-id>-<flags>"
    );
    // The emitted value parses back through the same grammar: same trace
    // id, the response id as the parent span, the sampled bit preserved.
    let back = TraceContext::parse_traceparent(&value).expect("the shape is parseable");
    assert_eq!(back.trace_id, tc.trace_id);
    assert_eq!(back.parent_id, "b7ad6b7169203332");
    assert!(back.sampled);
}

#[test]
fn to_traceresponse_preserves_the_unsampled_flag() {
    let tc =
        TraceContext::parse_traceparent("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-00")
            .expect("valid traceparent");
    let value = tc.to_traceresponse("b7ad6b7169203332").expect("emits");
    assert!(value.ends_with("-00"), "the unsampled flag rides: {value}");
}

#[test]
fn to_traceresponse_rejects_a_bad_response_id() {
    let tc = TraceContext::parse_traceparent(TRACED_PARENT).expect("valid traceparent");
    assert!(
        tc.to_traceresponse("xyz").is_none(),
        "a non-hex response id refuses"
    );
    assert!(
        tc.to_traceresponse("b7ad6b716920333").is_none(),
        "a 15-char response id refuses (16 hex required)"
    );
}

#[test]
fn response_span_id_derives_from_the_request_id() {
    // The pipeline's SEC-02 per-request id (`req-` + 32 CSPRNG hex):
    // the response span id is its first 16 hex — deterministic per
    // request, unpredictable across requests.
    assert_eq!(
        TraceContext::response_span_id(REQUEST_ID),
        Some("11aa22bb33cc44dd")
    );
    // A short or malformed id derives nothing — the emission degrades to
    // no header, never a fabricated span id.
    assert_eq!(TraceContext::response_span_id("req-f21"), None);
    assert_eq!(TraceContext::response_span_id("not-an-id"), None);
}

// ------------------------------------------------------- the wire half

#[tokio::test]
async fn traced_dispatch_response_carries_the_traceresponse_header() {
    let p = Pipeline::default();
    let resp = p
        .serve_request(traced_get_with("/index"), &ok_dispatch())
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.header("traceresponse"),
        Some("00-0af7651916cd43dd8448eb211c80319c-11aa22bb33cc44dd-01"),
        "the response half rides the dispatch arm"
    );
}

#[tokio::test]
async fn untraced_response_has_no_traceresponse_header() {
    // No traceparent in, no traceresponse out — the untraced shape is
    // byte-identical to the pre-F21 pipeline.
    let p = Pipeline::default();
    let resp = p
        .serve_request(get_with("/index", &[]), &ok_dispatch())
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("traceresponse"), None);
}

#[tokio::test]
async fn an_existing_traceresponse_is_never_overwritten() {
    // The dispatch already set one (a user handler or an upstream
    // propagation): the pipeline's emission never contradicts it.
    let existing = "00-0af7651916cd43dd8448eb211c80319c-ffffffffffffffff-01";
    let p = Pipeline::default();
    let resp = p
        .serve_request(traced_get_with("/index"), &|_r: OxideRequest| {
            Box::pin(
                async move { OxideResponse::new(200, "ok").with_header("traceresponse", existing) },
            )
                as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
        })
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("traceresponse"), Some(existing));
}

#[tokio::test]
async fn a_malformed_traceparent_never_emits() {
    // The parse refuses (a no-trace request, not an error) — no context,
    // no header, the request answers normally.
    let mut req = get_with("/index", &[("traceparent", "00-abc-def-01")]);
    req.trace = TraceContext::parse_traceparent("00-abc-def-01");
    assert!(req.trace.is_none());
    let p = Pipeline::default();
    let resp = p.serve_request(req, &ok_dispatch()).await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("traceresponse"), None);
}

//! Walkthrough: the V2-F21 dev-plane trace feed, end to end — the
//! consumer `oxide-fw-server` never had for this seam.
//!
//! §20 parses the W3C trace context on every request and rides
//! `OxideRequest.trace`, but until this seam nothing consumed it. The
//! feed is the consumption: a [`TraceSink`] records one [`RequestTrace`]
//! per completed pipeline request, [`RingTraceSink`] bounds the window
//! (oldest evicted, snapshot newest-first), and `GET /__oxide/traces`
//! (the §V2.J map's dev-plane row beside `/__oxide/open` and
//! `/__oxide/cost`, dev-token guarded, D41) serves the rows as camelCase
//! JSON.
//!
//! This example walks the shapes against an in-process ring (no server,
//! no build data needed): the record shapes, the eviction, the endpoint
//! contracts and the pipeline's one-instance-two-readers attach.
//!
//! Run: `cargo run -p oxide-fw-server --example trace_feed`

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use oxide_fw_server::trace_context::TraceContext;
use oxide_fw_server::{
    OxideRequest, OxideResponse, Pipeline, RequestTrace, RingTraceSink, TRACE_FEED_CAPACITY,
    TRACE_FEED_ENDPOINT_PATH, TraceSink, TracesEndpoint,
};

const TRACED_PARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

/// A minimal GET request helper (the `navgraph_tdd.rs` shape).
fn get(url: &str) -> OxideRequest {
    OxideRequest {
        method: "GET".into(),
        url: url.into(),
        headers: Arc::new(Vec::new()),
        header_index: OnceLock::default(),
        body: Bytes::new(),
        request_id: "ex-f21".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

/// The dispatch delegate — a trivial 200 render the record pins ride.
fn ok_dispatch()
-> impl Fn(OxideRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
{
    |_r: OxideRequest| {
        Box::pin(async { OxideResponse::new(200, "ok") })
            as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
    }
}

fn main() {
    println!("== the ring: the record shapes ==");
    let sink = Arc::new(RingTraceSink::new(4));
    // An untraced request records honestly: the trace halves stay None.
    sink.record(RequestTrace::of(
        "req-1",
        "GET",
        "/index",
        None,
        200,
        Duration::from_micros(1250),
    ));
    // A traced request (the pipeline's parsed W3C context) rides the row.
    let tc = TraceContext::parse_traceparent(TRACED_PARENT).expect("valid traceparent");
    sink.record(RequestTrace::of(
        "req-2",
        "POST",
        "/submit",
        Some(&tc),
        404,
        Duration::from_millis(3),
    ));
    for row in sink.snapshot() {
        println!(
            "  {} {} {} -> {} in {:.2}ms trace_id={:?} sampled={}",
            row.request_id,
            row.method,
            row.path,
            row.status,
            row.duration_ms,
            row.trace_id,
            row.sampled
        );
    }

    println!("== the eviction: capacity bounds the window ==");
    let tiny = Arc::new(RingTraceSink::new(2)) as Arc<dyn TraceSink>;
    for i in 0..4 {
        tiny.record(RequestTrace::of(
            &format!("req-{i}"),
            "GET",
            &format!("/r{i}"),
            None,
            200,
            Duration::ZERO,
        ));
    }
    println!(
        "  recorded 4 into a 2-window; snapshot paths: {:?}",
        tiny.snapshot()
            .iter()
            .map(|r| r.path.as_str())
            .collect::<Vec<_>>()
    );

    println!("== the endpoint: the wire contract ==");
    let ep = TracesEndpoint::new(sink.clone());
    println!("  path const: {TRACE_FEED_ENDPOINT_PATH} (default window {TRACE_FEED_CAPACITY})");
    let refused = ep.serve("POST");
    println!(
        "  non-GET -> {} (allow: {:?})",
        refused.status,
        refused.header("allow")
    );
    let served = ep.serve("GET");
    println!("  GET -> {}", served.status);
    println!(
        "  body: {}",
        String::from_utf8_lossy(&served.body)
            .lines()
            .next()
            .unwrap_or("")
    );

    println!("== the pipeline: one sink, two attach points ==");
    // The default pipeline never attaches: the endpoint falls through and
    // the record point is a None check (production byte-identical).
    let bare = Pipeline::default();
    let fell =
        futures::executor::block_on(bare.serve_request(get("/__oxide/traces"), &ok_dispatch()));
    println!(
        "  unattached feed request -> {} (fell through to dispatch)",
        fell.status
    );
    // The dev boot attaches ONE ring to both halves (the WaterfallRecorder
    // posture): the pipeline records into it, the endpoint serves it.
    let mut p = Pipeline::default()
        .with_trace_sink(sink.clone() as Arc<dyn TraceSink>)
        .with_trace_feed(Arc::new(TracesEndpoint::new(sink.clone())));
    p.dev_token = Some("tok".into());
    let guarded =
        futures::executor::block_on(p.serve_request(get("/__oxide/traces"), &ok_dispatch()));
    println!(
        "  feed without a token -> {} (the D41 guard)",
        guarded.status
    );
    // A dispatch round trip: the completed request lands in the window.
    futures::executor::block_on(p.serve_request(get("/index"), &ok_dispatch()));
    let answered = futures::executor::block_on(
        p.serve_request(get("/__oxide/traces?token=tok"), &ok_dispatch()),
    );
    println!(
        "  feed with the token -> {} (the snapshot precedes its own row)",
        answered.status
    );

    println!("== the response half: the traceresponse header ==");
    // §20's propagation: a traced dispatch response carries the W3C L2
    // draft's `traceresponse` header — the caller sees the server's
    // response span id (derived from the SEC-02 per-request id). An
    // untraced request stays byte-identical (no header).
    let traced = {
        let mut req = get("/index");
        // A realistic SEC-02 per-request id (`req-` + 32 CSPRNG hex): the
        // response span id derives from its first 16 hex. A malformed id
        // degrades honestly to no header (the `response_span_id` contract).
        req.request_id = "req-11aa22bb33cc44dd55ee66ff77008899".into();
        req.headers = Arc::new(vec![("traceparent".to_string(), TRACED_PARENT.to_string())]);
        req.trace = TraceContext::parse_traceparent(TRACED_PARENT);
        req
    };
    let rendered = futures::executor::block_on(p.serve_request(traced, &ok_dispatch()));
    println!(
        "  traced response traceresponse: {:?}",
        rendered.header("traceresponse")
    );

    println!("== the window ==");
    println!(
        "  window now: {:?}",
        sink.snapshot()
            .iter()
            .map(|r| r.path.as_str())
            .collect::<Vec<_>>()
    );
}

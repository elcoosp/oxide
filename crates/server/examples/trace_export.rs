//! Walkthrough: the trace rows' `tracing` export (V2-F22) — the export
//! arm the F21 feed never had.
//!
//! The F21 feed records one [`RequestTrace`] per completed pipeline
//! request into the attached sink and serves the window at
//! `GET /__oxide/traces` — but the rows never leave the process unless
//! the consumer polls the endpoint. The export seam closes that:
//! [`TracingTraceSink`] emits one `tracing::info!` event per recorded
//! row (`snake_case` fields, the absent W3C halves rendered empty), and
//! [`FanoutTraceSink`] fans one record into every child sink so the
//! pipeline's single `trace_sink` slot serves both the window (the ring)
//! and the export (the subscriber's stream) — the events correlate with
//! the §20 `oxide.request` span through the same `request_id` the
//! `Server-Timing` header reports.
//!
//! The CLI's dev boot attaches the fanout only when `OXIDE_TRACE_LOG`
//! opts in (`OXIDE_TRACE_LOG=1`); production never attaches either half
//! — the F21 posture holds, and the default console output is
//! byte-identical.
//!
//! This example installs the console subscriber, serves a few requests
//! through the pipeline, and prints what the subscriber sees — the
//! events appear in the console output above the walkthrough lines.
//!
//! Run: `cargo run -p oxide-fw-server --example trace_export`

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use oxide_fw_server::trace_context::TraceContext;
use oxide_fw_server::{
    FanoutTraceSink, OxideRequest, OxideResponse, Pipeline, RequestTrace, RingTraceSink, TraceSink,
    TracesEndpoint, TracingTraceSink,
};

const TRACED_PARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

/// A minimal GET request helper (the `trace_feed.rs` example's shape).
fn get(url: &str, traced: bool) -> OxideRequest {
    let headers: Vec<(String, String)> = if traced {
        vec![("traceparent".to_string(), TRACED_PARENT.to_string())]
    } else {
        Vec::new()
    };
    OxideRequest {
        method: "GET".into(),
        url: url.into(),
        headers: Arc::new(headers),
        header_index: OnceLock::default(),
        body: Bytes::new(),
        request_id: "req-ex-f22".into(),
        trace: traced
            .then(|| TraceContext::parse_traceparent(TRACED_PARENT))
            .flatten(),
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
    // The subscriber the CLI installs at boot (`crates/cli/src/
    // observability.rs`): console formatter over an env filter. The
    // export events ride whatever subscriber is installed — console,
    // `OXIDE_LOG=json`, or the OTLP layer.
    let _guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_target(false)
            .with_max_level(tracing::Level::INFO)
            .finish(),
    );

    println!("== the export arm: one row, one event ==");
    let export = TracingTraceSink::new();
    let trace = TraceContext::parse_traceparent(TRACED_PARENT).expect("valid traceparent");
    // A traced row exports with its W3C halves; the event prints ABOVE
    // this line (the subscriber renders it inline).
    export.record(RequestTrace::of(
        "req-ex-f22",
        "GET",
        "/index",
        Some(&trace),
        200,
        Duration::from_micros(1250),
    ));
    println!("  ^ the traced row's event (request_id/method/path/status/duration_ms/…)");
    // An untraced row exports too — the None halves render as the empty
    // string (the feed's honesty rule, on the event stream as on the row).
    export.record(RequestTrace::of(
        "req-ex-f22",
        "POST",
        "/submit",
        None,
        404,
        Duration::from_millis(3),
    ));
    println!("  ^ the untraced row's event (trace_id/parent_id render empty)");
    // The export arm is not a window: rows are emitted, never retained.
    println!(
        "  snapshot after 2 records: {} rows (the export arm keeps none)",
        export.snapshot().len()
    );

    println!("== the dev-boot fanout: one slot, two halves ==");
    // The pipeline has ONE trace_sink slot; the dev boot attaches the
    // fanout — the ring stays the endpoint's window, the export rides
    // beside it. OXIDE_TRACE_LOG gates the export child in the real
    // boot; here the fanout is built unconditionally to show the shape.
    let ring = Arc::new(RingTraceSink::new(8)) as Arc<dyn TraceSink>;
    let fanout = Arc::new(FanoutTraceSink::new(vec![
        ring.clone(),
        Arc::new(TracingTraceSink::new()) as Arc<dyn TraceSink>,
    ]));
    println!(
        "  fanout width: {} children (ring + tracing)",
        fanout.children()
    );

    println!("== the pipeline: records fan into the window and the stream ==");
    let mut p = Pipeline::default()
        .with_trace_sink(fanout)
        .with_trace_feed(Arc::new(TracesEndpoint::new(ring.clone())));
    p.dev_token = Some("tok".into());
    // A traced dispatch: the row lands in the ring AND the subscriber
    // prints the event (it appears above this line).
    let traced_status =
        futures::executor::block_on(p.serve_request(get("/index", true), &ok_dispatch())).status;
    println!("  traced dispatch -> {traced_status} (one ring row, one console event)");
    // An untraced dispatch: the same two halves, empty W3C fields.
    let untraced_status =
        futures::executor::block_on(p.serve_request(get("/submit", false), &ok_dispatch())).status;
    println!("  untraced dispatch -> {untraced_status} (empty trace halves on both)");

    println!("== the window: the endpoint stays ring-backed ==");
    let served = TracesEndpoint::new(ring.clone()).serve("GET");
    let body: serde_json::Value = serde_json::from_slice(&served.body).unwrap_or_default();
    println!(
        "  GET /__oxide/traces -> {} with {} rows (camelCase rows, newest first)",
        served.status,
        body["traces"].as_array().map_or(0, Vec::len)
    );
    println!(
        "  paths: {:?}",
        ring.snapshot()
            .iter()
            .map(|r| r.path.as_str())
            .collect::<Vec<_>>()
    );
    println!(
        "  (a fanout-backed endpoint renders the ring's JSON exactly — the export child adds no rows)"
    );
}

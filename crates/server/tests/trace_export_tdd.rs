//! TDD pins for the trace-rows-into-tracing export seam (V2-F22): the
//! §20 feed's rows ride the standard `tracing` stream.
//!
//! The F21 feed records one [`RequestTrace`] per completed pipeline
//! request into the attached sink, and the dev-plane endpoint serves the
//! window — but the rows never leave the process unless the consumer
//! polls `GET /__oxide/traces`. The export seam closes that:
//!
//! - [`TracingTraceSink`] is the export arm: one `tracing::info!` event
//!   per recorded row (the CLI's subscriber rides it — the console
//!   formatter, `OXIDE_LOG=json`, and with `OTEL_EXPORTER_OTLP_ENDPOINT`
//!   the OTLP span exporter's log stream all render it). It is not a
//!   window: [`TraceSink::snapshot`] stays honestly empty.
//! - [`FanoutTraceSink`] is the record fan: one record reaches every
//!   child sink (the dev boot attaches ring + tracing behind it), and the
//!   snapshot is each child's newest-first rows in construction order —
//!   no cross-child re-ordering (rows carry no global sequence; the
//!   window child is listed first).
//!
//! The events pin the field shape: `snake_case` fields mirroring the row's
//! Rust field names (`request_id`, `method`, `path`, `status`,
//! `duration_ms`, `sampled`, `trace_id`, `parent_id`, `tracestate`), the
//! absent W3C halves rendered as the empty string (the row's `None`
//! halves stay recorded — the feed's own honesty rule), under the
//! module-path target `oxide_fw_server::trace_export`.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use oxide_fw_server::trace_context::TraceContext;
use oxide_fw_server::{
    FanoutTraceSink, OxideRequest, OxideResponse, Pipeline, RequestTrace, RingTraceSink, TraceSink,
    TracesEndpoint, TracingTraceSink,
};

// ------------------------------------------------------------------ helpers

const TRACED_PARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

/// One recorded row (the fixture shape the pipeline records — a traced
/// GET answering 200).
fn traced_row(request_id: &str, path: &str) -> RequestTrace {
    let trace = TraceContext::parse_traceparent(TRACED_PARENT);
    RequestTrace::of(
        request_id,
        "GET",
        path,
        trace.as_ref(),
        200,
        Duration::from_micros(1250),
    )
}

/// One captured event: the field (name, rendered value) pairs in record
/// order plus the event's target.
#[derive(Clone, Debug)]
struct CapturedEvent {
    target: String,
    fields: Vec<(String, String)>,
}

impl CapturedEvent {
    fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// The capture layer: records every event's fields verbatim (the pins
/// assert the exact rendered shape a subscriber sees).
#[derive(Default)]
struct CaptureLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

/// The events the EXPORT arm emitted (target-scoped — the pipeline and
/// the endpoint emit their own `oxide::request` / trace-feed events and
/// a capture subscribed to everything sees those too).
fn export_events(events: &Mutex<Vec<CapturedEvent>>) -> Vec<CapturedEvent> {
    events
        .lock()
        .expect("trace export capture poisoned")
        .iter()
        .filter(|e| e.target == "oxide_fw_server::trace_export")
        .cloned()
        .collect()
}

impl<S: tracing::Subscriber> tracing_subscriber::layer::Layer<S> for CaptureLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut fields: Vec<(String, String)> = Vec::new();
        event.record(&mut FieldVisitor(&mut fields));
        let captured = CapturedEvent {
            target: event.metadata().target().to_string(),
            fields,
        };
        self.events
            .lock()
            .expect("trace export capture poisoned")
            .push(captured);
    }
}

/// The field visitor: every value lands as its rendered string.
struct FieldVisitor<'a>(&'a mut Vec<(String, String)>);

impl tracing::field::Visit for FieldVisitor<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.push((field.name().to_string(), value.to_string()));
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.0.push((field.name().to_string(), value.to_string()));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0.push((field.name().to_string(), value.to_string()));
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.0.push((field.name().to_string(), value.to_string()));
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.0.push((field.name().to_string(), value.to_string()));
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0
            .push((field.name().to_string(), format!("{value:?}")));
    }
}

/// Install a capture subscriber as the thread-local default; the events
/// the sink emits inside this scope land in the returned handle.
fn capture() -> (
    Arc<Mutex<Vec<CapturedEvent>>>,
    tracing::subscriber::DefaultGuard,
) {
    use tracing_subscriber::layer::SubscriberExt as _;
    let layer = CaptureLayer::default();
    let events = layer.events.clone();
    let guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));
    (events, guard)
}

/// A minimal GET request (the `trace_feed_tdd.rs` shape).
fn get(url: &str, traced: bool) -> OxideRequest {
    let trace = if traced {
        TraceContext::parse_traceparent(TRACED_PARENT)
    } else {
        None
    };
    let headers: &[(&str, &str)] = if traced {
        &[("traceparent", TRACED_PARENT)]
    } else {
        &[]
    };
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
        request_id: "req-f22".into(),
        trace,
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

// ------------------------------------------------- the TracingTraceSink pins

#[test]
fn tracing_sink_records_one_event_per_row_with_the_row_fields() {
    let (events, _guard) = capture();
    let sink = TracingTraceSink::new();
    sink.record(traced_row("req-f22", "/index"));
    let events = events.lock().expect("capture poisoned");
    assert_eq!(events.len(), 1, "one row, one event: {events:?}");
    let event = &events[0];
    assert_eq!(event.target, "oxide_fw_server::trace_export");
    assert_eq!(event.field("request_id"), Some("req-f22"));
    assert_eq!(event.field("method"), Some("GET"));
    assert_eq!(event.field("path"), Some("/index"));
    assert_eq!(event.field("status"), Some("200"));
    assert_eq!(event.field("duration_ms"), Some("1.25"));
    assert_eq!(
        event.field("trace_id"),
        Some("0af7651916cd43dd8448eb211c80319c"),
        "the W3C trace id rides the event: {:?}",
        event.fields
    );
    assert_eq!(event.field("parent_id"), Some("b7ad6b7169203331"));
    assert_eq!(event.field("sampled"), Some("true"));
    assert_eq!(event.field("tracestate"), Some(""));
}

#[test]
fn tracing_sink_records_the_untraced_row_with_empty_trace_halves() {
    let (events, _guard) = capture();
    let sink = TracingTraceSink::new();
    sink.record(RequestTrace::of(
        "req-f22",
        "POST",
        "/submit",
        None,
        404,
        Duration::from_millis(3),
    ));
    let events = events.lock().expect("capture poisoned");
    assert_eq!(events.len(), 1, "an untraced row still exports: {events:?}");
    let event = &events[0];
    assert_eq!(event.field("status"), Some("404"));
    assert_eq!(event.field("sampled"), Some("false"));
    assert_eq!(
        event.field("trace_id"),
        Some(""),
        "the None halves stay recorded"
    );
    assert_eq!(event.field("parent_id"), Some(""));
    assert_eq!(event.field("tracestate"), Some(""));
}

#[test]
fn tracing_sink_snapshot_is_honestly_empty() {
    // The export arm is not a window: rows are emitted, never retained —
    // a consumer wanting the window polls the ring-backed endpoint.
    let sink = TracingTraceSink::new();
    for i in 0..3 {
        sink.record(traced_row(&format!("req-{i}"), &format!("/r{i}")));
    }
    assert!(sink.snapshot().is_empty(), "the export sink keeps no rows");
}

#[test]
fn tracing_sink_debug_names_the_seam() {
    let sink = TracingTraceSink::new();
    let debug = format!("{sink:?}");
    assert!(
        debug.contains("TracingTraceSink"),
        "the Debug names the sink: {debug}"
    );
}

// --------------------------------------------------- the FanoutTraceSink pins

#[test]
fn fanout_records_reach_every_child() {
    let (events, _guard) = capture();
    let ring = Arc::new(RingTraceSink::new(8)) as Arc<dyn TraceSink>;
    let export = Arc::new(TracingTraceSink::new()) as Arc<dyn TraceSink>;
    let fanout = FanoutTraceSink::new(vec![ring.clone(), export]);
    fanout.record(traced_row("req-f22", "/index"));
    assert_eq!(ring.snapshot().len(), 1, "the window child recorded");
    assert_eq!(
        events.lock().expect("capture poisoned").len(),
        1,
        "the export child emitted"
    );
}

#[test]
fn fanout_snapshot_is_each_child_newest_first_in_construction_order() {
    let ring_a = Arc::new(RingTraceSink::new(8)) as Arc<dyn TraceSink>;
    let ring_b = Arc::new(RingTraceSink::new(8)) as Arc<dyn TraceSink>;
    ring_a.record(traced_row("req-a0", "/a0"));
    ring_a.record(traced_row("req-a1", "/a1"));
    ring_b.record(traced_row("req-b0", "/b0"));
    let fanout = FanoutTraceSink::new(vec![ring_a.clone(), ring_b.clone()]);
    let rows = fanout.snapshot();
    let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
    // Each child's own newest-first order; no cross-child re-ordering —
    // rows carry no global sequence, so construction order is the honest
    // merge (the window child is listed first).
    assert_eq!(paths, vec!["/a1", "/a0", "/b0"], "{paths:?}");
}

#[test]
fn fanout_snapshot_over_the_tracing_child_is_the_window_childs_rows() {
    // The dev-boot shape: fanout(ring, tracing) — the tracing child is
    // not a window, so the fanout's snapshot answers the ring's rows
    // exactly (an endpoint backed by the fanout renders the same JSON as
    // today's ring-backed one).
    let (events, _guard) = capture();
    let ring = Arc::new(RingTraceSink::new(8)) as Arc<dyn TraceSink>;
    let export = Arc::new(TracingTraceSink::new()) as Arc<dyn TraceSink>;
    ring.record(traced_row("req-f22", "/index"));
    let fanout = FanoutTraceSink::new(vec![ring.clone(), export]);
    let ep = TracesEndpoint::new(Arc::new(fanout));
    let resp = ep.serve("GET");
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("json");
    let rows = body["traces"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1, "the ring's row rides the fanout: {body}");
    assert_eq!(rows[0]["path"], "/index");
    assert_eq!(rows[0]["requestId"], "req-f22");
    assert!(
        export_events(&events).is_empty(),
        "a snapshot exports nothing"
    );
}

#[test]
fn fanout_with_no_children_is_the_honest_noop() {
    let fanout = FanoutTraceSink::new(Vec::new());
    fanout.record(traced_row("req-f22", "/index"));
    assert!(fanout.snapshot().is_empty(), "the no-op fan keeps no rows");
}

#[test]
fn fanout_is_object_safe_like_the_ring() {
    // The dev boot attaches it as `Arc<dyn TraceSink>` (the
    // `Pipeline::trace_sink` field's shape).
    let fanout: Arc<dyn TraceSink> =
        Arc::new(FanoutTraceSink::new(vec![Arc::new(RingTraceSink::new(4))]));
    assert!(
        fanout.snapshot().is_empty(),
        "a fresh fan over a fresh ring answers nothing"
    );
}

#[test]
fn fanout_debug_names_the_child_count() {
    let fanout = FanoutTraceSink::new(vec![
        Arc::new(RingTraceSink::new(4)),
        Arc::new(RingTraceSink::new(4)),
    ]);
    let debug = format!("{fanout:?}");
    assert!(
        debug.contains("FanoutTraceSink") && debug.contains("children"),
        "the Debug names the fan and its width: {debug}"
    );
}

// ------------------------------------------------- the pipeline record point

#[tokio::test]
async fn pipeline_records_through_a_fanout_into_the_window_and_the_export() {
    // The dev-boot shape end to end: the pipeline's record point fans the
    // row into the ring (the endpoint's window) and the tracing sink
    // (the subscriber's event stream). The default pipeline stays
    // unattached — only this test's builder attaches.
    let (events, _guard) = capture();
    let ring = Arc::new(RingTraceSink::new(8));
    let fanout = Arc::new(FanoutTraceSink::new(vec![
        ring.clone() as Arc<dyn TraceSink>,
        Arc::new(TracingTraceSink::new()) as Arc<dyn TraceSink>,
    ]));
    let mut p = Pipeline::default()
        .with_trace_sink(fanout)
        .with_trace_feed(Arc::new(TracesEndpoint::new(
            ring.clone() as Arc<dyn TraceSink>
        )));
    p.dev_token = Some("tok".into());
    let resp = p
        .serve_request(get("/index?token=tok", true), &ok_dispatch())
        .await;
    assert_eq!(resp.status, 200);
    let rows = ring.snapshot();
    assert_eq!(rows.len(), 1, "the window recorded the dispatch");
    assert_eq!(rows[0].path, "/index");
    assert_eq!(
        rows[0].trace_id.as_deref(),
        Some("0af7651916cd43dd8448eb211c80319c")
    );
    let events = export_events(&events);
    assert_eq!(events.len(), 1, "the export emitted the row: {events:?}");
    assert_eq!(events[0].field("path"), Some("/index"));
    assert_eq!(events[0].field("status"), Some("200"));
}

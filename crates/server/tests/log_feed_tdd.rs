//! TDD pins for the dev-plane log feed (V2-F24): `GET /__oxide/logs` —
//! the §V2.J map's dev-plane row beside `/__oxide/traces` (dev-token
//! guarded, D41).
//!
//! The trace feed answers "which requests completed and how" — one
//! [`RequestTrace`](oxide_fw_server::RequestTrace) row per completed
//! pipeline request. It never records what the framework *said*: the
//! `tracing` events the pipeline and the engine emit (the §20 request
//! log, the export arm's `request completed` events, the endpoint and
//! governor notices) leave through whatever subscriber the host
//! installed — a terminal, a JSON file, an OTLP collector. A developer
//! with only the dev server (no terminal attached — the inspector's own
//! posture) has no window on those events. The log feed is that window:
//!
//! - [`LogEvent`](oxide_fw_server::LogEvent) is the record shape: the
//!   capture wall-clock (epoch ms), the level, the target, the message
//!   and the structured fields.
//! - [`LogSink`](oxide_fw_server::LogSink) is the record seam (the
//!   [`TraceSink`](oxide_fw_server::TraceSink) shape: record + newest-
//!   first snapshot) and [`RingLogSink`](oxide_fw_server::RingLogSink)
//!   is the bounded production impl — oldest evicted at capacity, the
//!   snapshot newest-first, a zero capacity records nothing.
//! - [`CaptureLayer`](oxide_fw_server::CaptureLayer) is the capture arm:
//!   a `tracing_subscriber::Layer` that records every admitted event
//!   into the attached sink. The feed never records its own module's
//!   events (the serving event included) — no feedback loop.
//! - [`LogsEndpoint`](oxide_fw_server::LogsEndpoint) answers
//!   `GET /__oxide/logs` with the window as camelCase JSON
//!   (`tsMs`/`level`/`target`/`message`/`fields`). A non-GET verb is the
//!   documented `405` + `allow: GET` (the nav/open/traces contract).
//! - [`LogQuery`](oxide_fw_server::LogQuery) is the filter seam:
//!   `?level=warn,error` (comma-separated exact names,
//!   case-insensitive), `?target=oxide_fw` (a target prefix),
//!   `?q=` (a message substring), `?limit=50` (positive, capped at the
//!   window). The filters AND together, unknown parameters are ignored
//!   (the D41 `token` rides the same query), and an invalid value
//!   degrades to no filter — never an error (the traces/nav posture).
//!
//! The endpoint is the WIRE CONTRACT only: the sink behind it is the
//! injected seam. The default pipeline leaves the endpoint absent — the
//! request falls through to the normal dispatch and production is
//! byte-identical; the dev boot attaches one [`RingLogSink`] to both the
//! capture layer and the endpoint (the one-instance-two-readers
//! posture the trace feed rides).

use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use oxide_fw_server::{
    ENDPOINT_MAP, EndpointPlane, LOG_FEED_CAPACITY, LOG_FEED_ENDPOINT_PATH, LogEvent, LogSink,
    LogsEndpoint, OxideRequest, OxideResponse, Pipeline, RingLogSink,
};

// ------------------------------------------------------------------ helpers

/// One log row, recorded oldest first (the fixture shape the capture
/// layer produces — a fixed wall clock keeps the assertions honest).
fn row(level: &str, target: &str, message: &str) -> LogEvent {
    LogEvent {
        ts_ms: 1_700_000_000_000,
        level: level.to_string(),
        target: target.to_string(),
        message: message.to_string(),
        fields: Vec::new(),
    }
}

/// A sink with a mixed window, recorded oldest first — the snapshot
/// reads newest first.
fn mixed_window() -> Arc<RingLogSink> {
    let sink = Arc::new(RingLogSink::new(16));
    sink.record(row(
        "info",
        "oxide_fw_server::trace_feed",
        "trace feed served",
    ));
    sink.record(row("warn", "oxide_fw_assets::image_jobs", "lane saturated"));
    sink.record(row(
        "info",
        "oxide_fw_server::trace_export",
        "request completed",
    ));
    sink.record(row("debug", "oxide_fw_router::matcher", "match walked"));
    sink.record(row("error", "oxide_fw_dev::watcher", "watch root vanished"));
    sink
}

/// A minimal GET request (the `trace_feed_tdd.rs` shape).
fn get_with(url: &str) -> OxideRequest {
    OxideRequest {
        method: "GET".into(),
        url: url.into(),
        headers: Arc::new(Vec::new()),
        header_index: OnceLock::default(),
        body: Bytes::new(),
        request_id: "req-f24".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

/// The dispatch delegate — a trivial 200 render the fall-through pins ride.
fn ok_dispatch()
-> impl Fn(OxideRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
{
    |_r: OxideRequest| {
        Box::pin(async { OxideResponse::new(200, "ok") })
            as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
    }
}

/// The `logs` array a log-feed response carries.
fn log_rows(resp: &OxideResponse) -> Vec<serde_json::Value> {
    serde_json::from_slice::<serde_json::Value>(&resp.body)
        .ok()
        .and_then(|v| v["logs"].as_array().cloned())
        .unwrap_or_default()
}

// ------------------------------------------------------------------ the map row

/// The §V2.J map carries the row: dev plane, beside the trace feed, the
/// docs generator folds it (the drift gate regenerates the page).
#[test]
fn endpoint_map_has_the_log_feed_row() {
    let row = ENDPOINT_MAP
        .iter()
        .find(|r| r.path == "/__oxide/logs")
        .expect("the log feed row is in the map");
    assert_eq!(row.method, "GET");
    assert_eq!(row.plane, EndpointPlane::Dev);
    assert!(row.work_packages.contains("F24"));
    assert!(row.purpose.contains("dev-token guarded"));
    assert_eq!(ENDPOINT_MAP.len(), 15, "the spec's map has 15 rows");
}

// ------------------------------------------------------------- the constants

#[test]
fn path_const_is_the_dev_plane_spelling() {
    assert_eq!(LOG_FEED_ENDPOINT_PATH, "/__oxide/logs");
}

#[test]
fn default_capacity_is_the_documented_window() {
    assert_eq!(LOG_FEED_CAPACITY, 256);
}

// ----------------------------------------------------------------- the ring

#[test]
fn ring_snapshot_is_newest_first_and_capacity_bounded() {
    let sink = Arc::new(RingLogSink::new(3));
    for i in 0..5 {
        sink.record(row("info", "t", &format!("row-{i}")));
    }
    let rows = sink.snapshot();
    assert_eq!(rows.len(), 3, "capacity bounds the window");
    assert_eq!(rows[0].message, "row-4", "newest first");
    assert_eq!(rows[1].message, "row-3");
    assert_eq!(rows[2].message, "row-2", "the oldest evicted");
}

#[test]
fn zero_capacity_ring_is_the_honest_empty_window() {
    let sink = Arc::new(RingLogSink::new(0));
    sink.record(row("info", "t", "kept? no"));
    assert!(
        sink.snapshot().is_empty(),
        "a zero capacity records nothing"
    );
}

#[test]
fn ring_debug_names_the_seam() {
    assert!(format!("{:?}", RingLogSink::new(4)).contains("RingLogSink"));
}

// ---------------------------------------------------------- the capture arm

/// Install the capture layer over the given sink as the thread-local
/// default; events emitted inside the scope land in the sink.
fn capture(sink: &Arc<RingLogSink>) -> tracing::subscriber::DefaultGuard {
    use tracing_subscriber::layer::SubscriberExt as _;
    tracing::subscriber::set_default(
        tracing_subscriber::registry().with(oxide_fw_server::CaptureLayer::new(sink.clone())),
    )
}

#[test]
fn capture_layer_records_level_target_message_and_fields() {
    let sink = Arc::new(RingLogSink::new(8));
    let _guard = capture(&sink);
    tracing::info!(target: "oxide_fw_server::test_arm", rows = 3, "test event fired");
    let rows = sink.snapshot();
    assert_eq!(rows.len(), 1, "one event, one row: {rows:?}");
    let row = &rows[0];
    assert_eq!(row.level, "info");
    assert_eq!(row.target, "oxide_fw_server::test_arm");
    assert_eq!(row.message, "test event fired");
    assert!(row.ts_ms > 0, "the capture wall clock rides the row");
    assert!(
        row.fields.contains(&("rows".to_string(), "3".to_string())),
        "the structured fields ride the row: {:?}",
        row.fields
    );
}

#[test]
fn capture_layer_never_records_its_own_module() {
    let sink = Arc::new(RingLogSink::new(8));
    let _guard = capture(&sink);
    tracing::info!(target: "oxide_fw_server::log_feed", rows = 1, "log feed served");
    assert!(
        sink.snapshot().is_empty(),
        "the feed never records its own events — no feedback loop"
    );
}

#[test]
fn capture_layer_records_every_level_the_subscriber_admits() {
    let sink = Arc::new(RingLogSink::new(8));
    let _guard = capture(&sink);
    tracing::warn!("warned");
    tracing::error!("failed");
    let rows = sink.snapshot();
    assert_eq!(rows.len(), 2, "warn + error land: {rows:?}");
    assert_eq!(rows[0].level, "error", "newest first");
    assert_eq!(rows[1].level, "warn");
}

// ----------------------------------------------------------- the endpoint

#[test]
fn endpoint_rejects_non_get_with_allow() {
    let ep = LogsEndpoint::new(Arc::new(RingLogSink::new(4)));
    let refused = ep.serve("POST");
    assert_eq!(refused.status, 405);
    assert_eq!(refused.header("allow"), Some("GET"));
}

#[test]
fn endpoint_serves_the_window_newest_first_camel_case() {
    let sink = mixed_window();
    let ep = LogsEndpoint::new(sink.clone() as Arc<dyn LogSink>);
    let resp = ep.serve("GET");
    assert_eq!(resp.status, 200);
    let rows = log_rows(&resp);
    assert_eq!(rows.len(), 5, "the whole window renders");
    assert_eq!(rows[0]["message"], "watch root vanished", "newest first");
    let first = &rows[0];
    assert!(first.get("tsMs").is_some(), "camelCase wire: {first}");
    assert!(first.get("level").is_some());
    assert!(first.get("target").is_some());
    assert!(first.get("message").is_some());
    assert!(first.get("fields").is_some());
}

#[test]
fn endpoint_fields_render_as_an_object() {
    let sink = Arc::new(RingLogSink::new(4));
    sink.record(LogEvent {
        ts_ms: 1,
        level: "info".into(),
        target: "t".into(),
        message: "m".into(),
        fields: vec![
            ("rows".to_string(), "3".to_string()),
            ("path".to_string(), "/index".to_string()),
        ],
    });
    let ep = LogsEndpoint::new(sink);
    let rows = log_rows(&ep.serve("GET"));
    assert_eq!(rows[0]["fields"]["rows"], "3");
    assert_eq!(rows[0]["fields"]["path"], "/index");
}

#[test]
fn serve_is_the_whole_window_byte_identical_to_no_query() {
    let sink = mixed_window();
    let ep = LogsEndpoint::new(sink.clone() as Arc<dyn LogSink>);
    let plain = ep.serve("GET");
    let no_query = ep.serve_query("GET", None);
    assert_eq!(plain.status, no_query.status);
    assert_eq!(
        plain.body, no_query.body,
        "serve stays the whole-window shape"
    );
}

// ------------------------------------------------------------- the filters

#[test]
fn level_filter_matches_exact_and_comma_lists_case_insensitive() {
    let sink = mixed_window();
    let ep = LogsEndpoint::new(sink.clone() as Arc<dyn LogSink>);
    let exact = log_rows(&ep.serve_query("GET", Some("level=warn")));
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0]["message"], "lane saturated");
    let list = log_rows(&ep.serve_query("GET", Some("level=warn,error")));
    assert_eq!(list.len(), 2);
    let shouting = log_rows(&ep.serve_query("GET", Some("level=WARN,ERROR")));
    assert_eq!(shouting.len(), 2, "the names are case-insensitive");
}

#[test]
fn level_filter_degrades_to_no_filter_on_an_unparsable_list() {
    let sink = mixed_window();
    let ep = LogsEndpoint::new(sink.clone() as Arc<dyn LogSink>);
    let degraded = log_rows(&ep.serve_query("GET", Some("level=warn,bogus")));
    assert_eq!(degraded.len(), 5, "an unparsable spelling filters nothing");
}

#[test]
fn target_filter_is_a_prefix() {
    let sink = mixed_window();
    let ep = LogsEndpoint::new(sink.clone() as Arc<dyn LogSink>);
    let server = log_rows(&ep.serve_query("GET", Some("target=oxide_fw_server")));
    assert_eq!(server.len(), 2, "the two server-crate events");
    let exact = log_rows(&ep.serve_query("GET", Some("target=oxide_fw_server::trace_feed")));
    assert_eq!(exact.len(), 1, "the longer prefix narrows");
}

#[test]
fn q_filter_is_a_message_substring() {
    let sink = mixed_window();
    let ep = LogsEndpoint::new(sink.clone() as Arc<dyn LogSink>);
    let hits = log_rows(&ep.serve_query("GET", Some("q=request completed")));
    assert_eq!(hits.len(), 1);
}

#[test]
fn limit_is_positive_and_capped_at_the_window() {
    let sink = mixed_window();
    let ep = LogsEndpoint::new(sink.clone() as Arc<dyn LogSink>);
    let one = log_rows(&ep.serve_query("GET", Some("limit=1")));
    assert_eq!(one.len(), 1, "the newest row alone");
    assert_eq!(one[0]["message"], "watch root vanished");
    let capped = log_rows(&ep.serve_query("GET", Some("limit=4096")));
    assert_eq!(
        capped.len(),
        5,
        "a bigger number cannot out-render the window"
    );
    let zero = log_rows(&ep.serve_query("GET", Some("limit=0")));
    assert_eq!(zero.len(), 5, "a zero limit is no limit");
}

#[test]
fn filters_and_together_and_unknown_params_are_ignored() {
    let sink = mixed_window();
    let ep = LogsEndpoint::new(sink.clone() as Arc<dyn LogSink>);
    let both = log_rows(&ep.serve_query("GET", Some("level=warn,error&target=oxide_fw_assets")));
    assert_eq!(both.len(), 1, "the filters AND");
    let with_unknown =
        log_rows(&ep.serve_query("GET", Some("token=secret&level=error&q=vanished")));
    assert_eq!(with_unknown.len(), 1, "unknown parameters are ignored");
}

// ------------------------------------------------------------ the pipeline

#[test]
fn unattached_pipeline_falls_through() {
    let bare = Pipeline::default();
    let fell =
        futures::executor::block_on(bare.serve_request(get_with("/__oxide/logs"), &ok_dispatch()));
    assert_eq!(fell.status, 200, "the request falls through to dispatch");
}

#[test]
fn attached_pipeline_serves_behind_the_dev_token() {
    let sink = Arc::new(RingLogSink::new(8));
    let mut p = Pipeline::default().with_log_feed(Arc::new(LogsEndpoint::new(sink)));
    p.dev_token = Some("tok".into());
    let guarded =
        futures::executor::block_on(p.serve_request(get_with("/__oxide/logs"), &ok_dispatch()));
    assert_eq!(guarded.status, 403, "the D41 guard");
    let answered = futures::executor::block_on(
        p.serve_request(get_with("/__oxide/logs?token=tok"), &ok_dispatch()),
    );
    assert_eq!(answered.status, 200);
    let rows = log_rows(&answered);
    assert!(rows.is_empty(), "the honest empty window before any event");
}

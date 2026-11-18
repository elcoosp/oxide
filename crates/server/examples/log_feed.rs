//! Walkthrough: the V2-F24 dev-plane log feed, end to end — the
//! consumer `oxide-fw-server` never had for this seam.
//!
//! The trace feed records one row per completed request, but the
//! framework's `tracing` events (the §20 request log, the export arm's
//! `request completed` events, the endpoint and governor notices) leave
//! through whatever subscriber the host installed. The log feed is the
//! window on those events: a [`CaptureLayer`] records every admitted
//! event into a [`LogSink`], [`RingLogSink`] bounds the window (oldest
//! evicted, snapshot newest-first), and `GET /__oxide/logs` (the §V2.J
//! map's dev-plane row beside `/__oxide/traces`, dev-token guarded, D41)
//! serves the rows as camelCase JSON.
//!
//! This example walks the shapes against an in-process ring (no server,
//! no build data needed): the capture shapes, the eviction, the
//! endpoint contracts, the filtered window and the pipeline's
//! one-instance-two-readers attach.
//!
//! Run: `cargo run -p oxide-fw-server --example log_feed`

use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use oxide_fw_server::{
    CaptureLayer, LOG_FEED_CAPACITY, LOG_FEED_ENDPOINT_PATH, LogSink, LogsEndpoint, OxideRequest,
    OxideResponse, Pipeline, RingLogSink,
};

/// A minimal GET request helper (the `trace_feed_tdd.rs` shape).
fn get(url: &str) -> OxideRequest {
    OxideRequest {
        method: "GET".into(),
        url: url.into(),
        headers: Arc::new(Vec::new()),
        header_index: OnceLock::default(),
        body: Bytes::new(),
        request_id: "ex-f24".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

/// The dispatch delegate — a trivial 200 render the walk pins ride.
fn ok_dispatch()
-> impl Fn(OxideRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
{
    |_r: OxideRequest| {
        Box::pin(async { OxideResponse::new(200, "ok") })
            as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
    }
}

/// The row count a log-feed response carries (the `logs` array).
fn log_rows_len(resp: &OxideResponse) -> usize {
    serde_json::from_slice::<serde_json::Value>(&resp.body)
        .ok()
        .and_then(|v| v["logs"].as_array().map(Vec::len))
        .unwrap_or(0)
}

/// The capture walk: events emitted under the layer land in the ring —
/// level, target, message and the structured fields; the feed's own
/// module stays unrecorded (the no-feedback rule).
fn walk_capture(sink: &Arc<RingLogSink>) {
    use tracing_subscriber::layer::SubscriberExt as _;
    let layer = CaptureLayer::new(sink.clone() as Arc<dyn LogSink>);
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        tracing::info!(target: "oxide_fw_server::demo", rows = 3, "demo event fired");
        tracing::warn!(target: "oxide_fw_server::demo", "lane saturated");
        tracing::info!(target: "oxide_fw_server::log_feed", rows = 1, "log feed served");
    });
    for row in sink.snapshot() {
        println!(
            "  [{}] {} {} {:?} {:?}",
            row.ts_ms, row.level, row.target, row.message, row.fields
        );
    }
}

/// The filtered-window walk: the window, filtered, is the answer —
/// `?level=`, `?target=`, `?q=`, `?limit=` AND together; invalid values
/// degrade to no filter; unknown parameters (the D41 `token` among
/// them) are ignored; `serve` stays the no-query whole-window shape.
fn walk_filtered_window(ep: &LogsEndpoint) {
    let only_warn = ep.serve_query("GET", Some("level=warn"));
    println!("  ?level=warn -> {} rows", log_rows_len(&only_warn));
    let targets = ep.serve_query("GET", Some("target=oxide_fw_server"));
    println!(
        "  ?target=oxide_fw_server -> {} rows (the prefix match)",
        log_rows_len(&targets)
    );
    let substring = ep.serve_query("GET", Some("q=saturated"));
    println!(
        "  ?q=saturated -> {} rows (the message substring)",
        log_rows_len(&substring)
    );
    let limited = ep.serve_query("GET", Some("limit=1"));
    println!(
        "  ?limit=1 -> {} row (the newest row alone)",
        log_rows_len(&limited)
    );
    let degraded = ep.serve_query("GET", Some("level=bogus&limit=abc"));
    println!(
        "  ?level=bogus&limit=abc -> {} rows (invalid values filter nothing)",
        log_rows_len(&degraded)
    );
    let combined = ep.serve_query("GET", Some("level=warn&target=oxide_fw_server::demo"));
    println!(
        "  ?level=warn&target=oxide_fw_server::demo -> {} row (the filters AND)",
        log_rows_len(&combined)
    );
    let refused_query = ep.serve_query("POST", Some("level=error"));
    println!(
        "  non-GET with a query -> {} (allow: {:?}; the guard precedes filtering)",
        refused_query.status,
        refused_query.header("allow")
    );
}

fn main() {
    println!("== the capture: the layer's shapes ==");
    let sink = Arc::new(RingLogSink::new(8));
    walk_capture(&sink);

    println!("== the eviction: capacity bounds the window ==");
    let tiny = Arc::new(RingLogSink::new(2));
    for i in 0..4 {
        tiny.record(oxide_fw_server::LogEvent {
            ts_ms: 1_700_000_000_000 + u64::try_from(i).unwrap_or(0),
            level: "info".into(),
            target: "demo".into(),
            message: format!("row-{i}"),
            fields: Vec::new(),
        });
    }
    println!(
        "  recorded 4 into a 2-window; snapshot messages: {:?}",
        tiny.snapshot()
            .iter()
            .map(|r| r.message.as_str())
            .collect::<Vec<_>>()
    );

    println!("== the endpoint: the wire contract ==");
    let ep = LogsEndpoint::new(sink.clone() as Arc<dyn LogSink>);
    println!("  path const: {LOG_FEED_ENDPOINT_PATH} (default window {LOG_FEED_CAPACITY})");
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

    println!("== the filtered window: the query seam ==");
    walk_filtered_window(&ep);

    println!("== the pipeline: one sink, two attach points ==");
    // The default pipeline never attaches: the endpoint falls through and
    // production is byte-identical.
    let bare = Pipeline::default();
    let fell =
        futures::executor::block_on(bare.serve_request(get("/__oxide/logs"), &ok_dispatch()));
    println!(
        "  unattached feed request -> {} (fell through to dispatch)",
        fell.status
    );
    // The dev boot attaches ONE ring to both halves (the capture layer
    // records into it, the endpoint serves it — the same
    // one-instance-two-readers posture the trace feed rides).
    let mut p = Pipeline::default().with_log_feed(Arc::new(LogsEndpoint::new(
        sink.clone() as Arc<dyn LogSink>
    )));
    p.dev_token = Some("tok".into());
    let guarded =
        futures::executor::block_on(p.serve_request(get("/__oxide/logs"), &ok_dispatch()));
    println!(
        "  feed without a token -> {} (the D41 guard)",
        guarded.status
    );
    let answered = futures::executor::block_on(
        p.serve_request(get("/__oxide/logs?token=tok"), &ok_dispatch()),
    );
    println!(
        "  feed with the token -> {} ({} rows: the capture walk's events)",
        answered.status,
        log_rows_len(&answered)
    );
}

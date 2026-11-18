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
//! no build data needed): the capture shapes, the admission gate (V2-F26
//! — the default, the narrowing floor, the widening composition), the
//! eviction, the endpoint contracts, the filtered window and the
//! pipeline's one-instance-two-readers attach.
//!
//! Run: `cargo run -p oxide-fw-server --example log_feed`

use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use oxide_fw_server::{
    AdmitAll, CaptureLayer, LOG_FEED_CAPACITY, LOG_FEED_ENDPOINT_PATH, LevelFloor, LogGate,
    LogSink, LogsEndpoint, OxideRequest, OxideResponse, Pipeline, RingLogSink,
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

/// The gate walk (V2-F26): the admission seam's three shapes — the
/// default capture (everything the subscriber admits), the narrowing
/// floor (events below the floor never join the window), and the
/// widening composition (the console layer keeps the host filter while
/// the feed's own floor admits what the console never renders — the
/// `OXIDE_LOG_FEED_LEVEL` recipe, printed with the console bytes and
/// the window rows side by side).
fn walk_gate() {
    use tracing_subscriber::layer::{Layer as _, SubscriberExt as _};

    let emit = |level: tracing::Level, message: &str| match level {
        tracing::Level::TRACE => tracing::trace!(target: "oxide_fw_server::demo", "{message}"),
        tracing::Level::DEBUG => tracing::debug!(target: "oxide_fw_server::demo", "{message}"),
        tracing::Level::INFO => tracing::info!(target: "oxide_fw_server::demo", "{message}"),
        tracing::Level::WARN => tracing::warn!(target: "oxide_fw_server::demo", "{message}"),
        tracing::Level::ERROR => tracing::error!(target: "oxide_fw_server::demo", "{message}"),
    };
    let window = |sink: &Arc<RingLogSink>| -> Vec<String> {
        sink.snapshot().iter().map(|r| r.message.clone()).collect()
    };

    // The default gate: everything admitted joins (the F24 shape —
    // `with_gate(AdmitAll)` is spelled out to show the seam; `new` is the
    // same shape).
    let sink = Arc::new(RingLogSink::new(8));
    let layer = CaptureLayer::new(sink.clone() as Arc<dyn LogSink>).with_gate(Arc::new(AdmitAll));
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        emit(tracing::Level::DEBUG, "debug event");
        emit(tracing::Level::INFO, "info event");
    });
    println!("  default (AdmitAll): {:?}", window(&sink));

    // The narrowing floor: a subscriber with NO filter admits every
    // level; the gate still keeps the window to the floor and above.
    let sink = Arc::new(RingLogSink::new(8));
    let layer = CaptureLayer::new(sink.clone() as Arc<dyn LogSink>)
        .with_gate(Arc::new(LevelFloor::new(tracing::Level::INFO)));
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        emit(tracing::Level::DEBUG, "debug event");
        emit(tracing::Level::INFO, "info event");
        emit(tracing::Level::ERROR, "error event");
    });
    println!(
        "  LevelFloor::new(INFO): {:?} (debug never joins)",
        window(&sink)
    );

    // The widening composition: the console keeps `info`, the feed
    // captures `debug` — the console bytes and the window rows side by
    // side. The gate is load-bearing: with no global filter the
    // subscriber hands every event to every layer, and the gate keeps
    // the window exactly as wide as the floor.
    let sink = Arc::new(RingLogSink::new(8));
    let gate = Arc::new(LevelFloor::new(tracing::Level::DEBUG)) as Arc<dyn LogGate>;
    let console = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let fmt = tracing_subscriber::fmt::layer()
        .with_writer(ConsoleWriter(console.clone()))
        .with_filter(tracing_subscriber::filter::EnvFilter::new("info"));
    let gated = CaptureLayer::new(sink.clone() as Arc<dyn LogSink>)
        .with_gate(gate.clone())
        .with_filter(tracing_subscriber::filter::filter_fn(move |meta| {
            gate.admits(meta)
        }));
    tracing::subscriber::with_default(tracing_subscriber::registry().with(fmt).with(gated), || {
        emit(tracing::Level::INFO, "console info event");
        emit(tracing::Level::DEBUG, "feed debug event");
    });
    let rendered = String::from_utf8(console.lock().expect("console").clone())
        .expect("console bytes are UTF-8");
    println!(
        "  widening (console info / feed debug): console {:?}, window {:?}",
        rendered.lines().any(|l| l.contains("console info event")),
        window(&sink)
    );
}

/// A minimal `MakeWriter` over a shared buffer — the console side of the
/// widening walk (the bytes one fmt render leaves).
#[derive(Clone)]
struct ConsoleWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for ConsoleWriter {
    type Writer = ConsoleSink;

    fn make_writer(&'a self) -> Self::Writer {
        ConsoleSink(self.0.clone())
    }
}

struct ConsoleSink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for ConsoleSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("console buffer")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn main() {
    println!("== the capture: the layer's shapes ==");
    let sink = Arc::new(RingLogSink::new(8));
    walk_capture(&sink);

    println!("== the gate: the admission seam (V2-F26) ==");
    walk_gate();

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

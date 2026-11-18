//! TDD pins for the log feed's own filter surface (V2-F26): the capture
//! arm's admission gate.
//!
//! The F24 capture layer records every event the subscriber admits — the
//! feed mirrors the host's filter, it does not bypass it. Two halves
//! complete that posture:
//!
//! - the RECORD-side gate ([`LogGate`], [`CaptureLayer::with_gate`]): an
//!   in-layer admission predicate. The F24 default
//!   ([`AdmitAll`]) is today's shape — every admitted event joins the
//!   window — so an un-gated install stays byte-identical. A
//!   [`LevelFloor`] narrows the window: events below the floor never
//!   join it, even when the subscriber admits them (the capture-side
//!   narrowing — the query-side [`LogQuery`] narrows the RENDER, the
//!   gate narrows the RECORD).
//! - the WIDENING recipe (the CLI's `OXIDE_LOG_FEED_LEVEL`): a per-layer
//!   filter composition — the host filter moves onto the formatter (and
//!   export) layers, the feed layer admits its own floor — so the
//!   console output stays byte-identical while the window captures
//!   events the console never renders (a dev loop with the console at
//!   `info` and the feed at `debug`). The flagship pin below proves the
//!   composition with the same primitives the boot builds.

use std::sync::{Arc, Mutex};

use oxide_fw_server::{
    AdmitAll, CaptureLayer, LevelFloor, LogEvent, LogGate, LogSink, RingLogSink,
};
use tracing_subscriber::layer::{Layer as _, SubscriberExt as _};

// ------------------------------------------------------------------ helpers

const DEMO_TARGET: &str = "oxide_fw_server::gate_demo";

/// One event emitted under a fixed demo target (the assertion shape:
/// the ring row's message names the event). `tracing::event!` needs a
/// static level path, so the level dispatches through a match.
fn emit(level: tracing::Level, message: &str) {
    match level {
        tracing::Level::TRACE => tracing::trace!(target: DEMO_TARGET, "{}", message),
        tracing::Level::DEBUG => tracing::debug!(target: DEMO_TARGET, "{}", message),
        tracing::Level::INFO => tracing::info!(target: DEMO_TARGET, "{}", message),
        tracing::Level::WARN => tracing::warn!(target: DEMO_TARGET, "{}", message),
        tracing::Level::ERROR => tracing::error!(target: DEMO_TARGET, "{}", message),
    }
}

/// The console sink the widening pins read: a `MakeWriter` over a shared
/// byte buffer (the bytes one fmt render leaves).
#[derive(Clone)]
struct ConsoleWriter(Arc<Mutex<Vec<u8>>>);

impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for ConsoleWriter {
    type Writer = ConsoleSink;

    fn make_writer(&'a self) -> Self::Writer {
        ConsoleSink(self.0.clone())
    }
}

struct ConsoleSink(Arc<Mutex<Vec<u8>>>);

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

impl ConsoleWriter {
    fn rendered(&self) -> String {
        String::from_utf8(self.0.lock().expect("console buffer").clone())
            .expect("console bytes are UTF-8")
    }
}

/// The rows a ring holds, newest first (the messages only).
fn window(sink: &Arc<RingLogSink>) -> Vec<String> {
    sink.snapshot()
        .iter()
        .map(|row| row.message.clone())
        .collect()
}

/// The gated capture layer over one ring (the `with_gate` shape).
fn gated(sink: &Arc<RingLogSink>, gate: Arc<dyn LogGate>) -> CaptureLayer {
    CaptureLayer::new(sink.clone() as Arc<dyn LogSink>).with_gate(gate)
}

// -------------------------------------------------------------------- pins

#[test]
fn default_capture_stays_the_admit_all_shape() {
    // The F24 byte-identity rule: an un-gated layer records every event
    // the subscriber admits — `CaptureLayer::new` IS the `AdmitAll` gate.
    let sink = Arc::new(RingLogSink::new(8));
    let layer = CaptureLayer::new(sink.clone() as Arc<dyn LogSink>);
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        emit(tracing::Level::DEBUG, "debug event");
        emit(tracing::Level::INFO, "info event");
    });
    let rows = window(&sink);
    assert_eq!(
        rows,
        vec!["info event".to_string(), "debug event".to_string()],
        "the default gate admits everything the subscriber admits, newest first"
    );
}

#[test]
fn with_gate_admit_all_matches_the_default_shape() {
    let sink = Arc::new(RingLogSink::new(8));
    let layer = gated(&sink, Arc::new(AdmitAll));
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        emit(tracing::Level::TRACE, "trace event");
        emit(tracing::Level::ERROR, "error event");
    });
    let rows = window(&sink);
    assert_eq!(
        rows,
        vec!["error event".to_string(), "trace event".to_string()],
        "the explicit AdmitAll gate is the un-gated default"
    );
}

#[test]
fn level_floor_drops_everything_below_the_floor() {
    // The capture-side narrowing: a registry with no filter admits every
    // level; the gate still keeps only the floor and above out of the
    // window (the RECORD narrows even when the subscriber does not).
    let sink = Arc::new(RingLogSink::new(8));
    let layer = gated(&sink, Arc::new(LevelFloor::new(tracing::Level::INFO)));
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        emit(tracing::Level::TRACE, "trace event");
        emit(tracing::Level::DEBUG, "debug event");
        emit(tracing::Level::INFO, "info event");
        emit(tracing::Level::WARN, "warn event");
        emit(tracing::Level::ERROR, "error event");
    });
    let rows = window(&sink);
    assert_eq!(
        rows,
        vec![
            "error event".to_string(),
            "warn event".to_string(),
            "info event".to_string()
        ],
        "the info floor keeps info, warn and error — newest first"
    );
}

#[test]
fn level_floor_at_error_keeps_only_error() {
    let sink = Arc::new(RingLogSink::new(8));
    let layer = gated(&sink, Arc::new(LevelFloor::new(tracing::Level::ERROR)));
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        emit(tracing::Level::WARN, "warn event");
        emit(tracing::Level::ERROR, "error event");
    });
    assert_eq!(window(&sink), vec!["error event".to_string()]);
}

#[test]
fn level_floor_parses_the_five_names_case_insensitive() {
    assert_eq!(
        LevelFloor::parse("DEBUG"),
        Some(LevelFloor::new(tracing::Level::DEBUG))
    );
    assert_eq!(
        LevelFloor::parse("warn"),
        Some(LevelFloor::new(tracing::Level::WARN))
    );
    assert_eq!(
        LevelFloor::parse(" info "),
        Some(LevelFloor::new(tracing::Level::INFO)),
        "the spelling trims"
    );
    assert_eq!(
        LevelFloor::parse("bogus"),
        None,
        "an unknown name is no floor"
    );
    assert_eq!(LevelFloor::parse(""), None, "an empty value is no floor");
}

#[test]
fn the_gate_rides_the_layer_clone() {
    // The dev boot stashes the ring and clones the layer per subscriber
    // branch — the gate must travel with it.
    let sink = Arc::new(RingLogSink::new(8));
    let layer = gated(&sink, Arc::new(LevelFloor::new(tracing::Level::WARN)));
    let cloned = layer.clone();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(cloned), || {
        emit(tracing::Level::INFO, "info event");
        emit(tracing::Level::WARN, "warn event");
    });
    assert_eq!(
        window(&sink),
        vec!["warn event".to_string()],
        "the clone keeps the floor"
    );
}

#[test]
fn the_feed_target_stays_skipped_under_a_permissive_gate() {
    // The no-feedback rule precedes the gate: the feed's own serving
    // events never join the window, even under AdmitAll.
    let sink = Arc::new(RingLogSink::new(8));
    let layer = gated(&sink, Arc::new(AdmitAll));
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        tracing::info!(target: "oxide_fw_server::log_feed", rows = 1, "log feed served");
        emit(tracing::Level::INFO, "demo event");
    });
    assert_eq!(window(&sink), vec!["demo event".to_string()]);
}

#[test]
fn the_narrowing_floor_never_touches_the_wire_contract() {
    // The gate narrows the RECORD, not the render: an endpoint over the
    // same ring serves the (already-narrowed) window unchanged — the
    // query seam still filters on top.
    let sink = Arc::new(RingLogSink::new(8));
    let layer = gated(&sink, Arc::new(LevelFloor::new(tracing::Level::ERROR)));
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        emit(tracing::Level::INFO, "info event");
        emit(tracing::Level::ERROR, "error event");
    });
    let ep = oxide_fw_server::LogsEndpoint::new(sink.clone() as Arc<dyn LogSink>);
    let served = ep.serve("GET");
    let rows: Vec<serde_json::Value> = serde_json::from_slice::<serde_json::Value>(&served.body)
        .expect("json")["logs"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(rows.len(), 1, "the window holds only the gated rows");
    assert_eq!(rows[0]["message"], "error event");
}

#[test]
fn the_widening_composition_feeds_the_window_past_the_host_filter() {
    // THE flagship pin: the CLI's OXIDE_LOG_FEED_LEVEL recipe. The host
    // filter rides the FORMATTER layer (per-layer filtering, no global
    // filter on the registry) and the capture layer admits its own
    // floor — the console stays byte-identical while the window captures
    // the events the console never renders.
    let console = ConsoleWriter(Arc::new(Mutex::new(Vec::new())));
    let sink = Arc::new(RingLogSink::new(8));
    let gate = Arc::new(LevelFloor::new(tracing::Level::DEBUG));
    let fmt = tracing_subscriber::fmt::layer()
        .with_writer(console.clone())
        .with_filter(tracing_subscriber::filter::EnvFilter::new("info"));
    let capture = gated(&sink, gate.clone()).with_filter(tracing_subscriber::filter::filter_fn(
        move |meta| gate.admits(meta),
    ));
    let subscriber = tracing_subscriber::registry().with(fmt).with(capture);
    tracing::subscriber::with_default(subscriber, || {
        emit(tracing::Level::INFO, "console info event");
        emit(tracing::Level::DEBUG, "feed debug event");
    });
    // The console rendered ONLY the info event (the host filter's shape,
    // byte-identical to a feed-less boot).
    let rendered = console.rendered();
    assert!(
        rendered.contains("console info event"),
        "the info event rides the console: {rendered:?}"
    );
    assert!(
        !rendered.contains("feed debug event"),
        "the debug event never reaches the console: {rendered:?}"
    );
    // The window captured BOTH — the debug event joined past the host
    // filter (the capability the env var buys).
    let rows = window(&sink);
    assert_eq!(
        rows,
        vec![
            "feed debug event".to_string(),
            "console info event".to_string()
        ],
        "the feed's own floor widens the window, newest first"
    );
}

#[test]
fn the_widening_composition_is_only_as_wide_as_its_gate() {
    // The composition's guardrail: with no global filter the subscriber
    // hands EVERY event to every layer — the gate is what keeps the feed
    // honest. With an info floor the window mirrors the host filter
    // exactly (the unset-env-var posture rides the F24 subscriber shape,
    // pinned by `default_capture_stays_the_admit_all_shape`; this pin is
    // the composition's own rule — a floor is never optional in it).
    let console = ConsoleWriter(Arc::new(Mutex::new(Vec::new())));
    let sink = Arc::new(RingLogSink::new(8));
    let gate = Arc::new(LevelFloor::new(tracing::Level::INFO));
    let fmt = tracing_subscriber::fmt::layer()
        .with_writer(console.clone())
        .with_filter(tracing_subscriber::filter::EnvFilter::new("info"));
    let capture = gated(&sink, gate.clone()).with_filter(tracing_subscriber::filter::filter_fn(
        move |meta| gate.admits(meta),
    ));
    let subscriber = tracing_subscriber::registry().with(fmt).with(capture);
    tracing::subscriber::with_default(subscriber, || {
        emit(tracing::Level::INFO, "console info event");
        emit(tracing::Level::DEBUG, "dropped debug event");
    });
    let rendered = console.rendered();
    assert!(rendered.contains("console info event"));
    assert!(!rendered.contains("dropped debug event"));
    assert_eq!(
        window(&sink),
        vec!["console info event".to_string()],
        "the gate keeps the window exactly as wide as the floor"
    );
}

#[test]
fn the_row_shape_is_untouched_by_the_gate() {
    // The gate drops rows before the sink; a recorded row is the same
    // camelCase wire record as ever (the F24 shape).
    let sink = Arc::new(RingLogSink::new(8));
    let layer = gated(&sink, Arc::new(LevelFloor::new(tracing::Level::INFO)));
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        tracing::info!(target: "oxide_fw_server::gate_demo", rows = 3, "fielded event");
    });
    let row: LogEvent = sink.snapshot().into_iter().next().expect("the row");
    assert_eq!(row.level, "info");
    assert_eq!(row.target, "oxide_fw_server::gate_demo");
    assert_eq!(row.message, "fielded event");
    assert_eq!(
        row.fields,
        vec![
            ("message".to_string(), "fielded event".to_string()),
            ("rows".to_string(), "3".to_string())
        ]
    );
}

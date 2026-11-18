//! TDD pins for the trace file export's injectable writer (V2-F27): the
//! sticky write-failure rule gets its pins.
//!
//! The F25 file arm appends one JSONL line per recorded row — flushed on
//! record, so the bytes are on disk before the record returns — and the
//! documented mid-session posture is sticky: one `tracing::warn!`, then
//! every subsequent row drops (a half-written line is worse than a
//! stopped export; the audit trail's tail stays parseable). The rule was
//! implemented but unpinned: forcing a mid-stream write failure
//! portably needs an injectable writer (a directory path fails at OPEN,
//! before any record), and [`TraceSink::record`] returns `()` — the
//! failure is observable only through the side effects. This target is
//! those pins: [`FileTraceSink::from_writer`] takes any
//! `std::io::Write` and the pins steer a writer that fails on demand.
//!
//! - [`FileTraceSink::from_writer`] is the seam: the same JSONL framing
//!   (one row, one line, flushed per record) over an injected byte sink.
//!   `FileTraceSink::create` stays the file-path constructor — the dev
//!   boot's shape, unchanged.
//! - The first failing write flips the sink sticky: the failed row
//!   never reaches the writer (no half line — the row is rendered, then
//!   dropped with the writer's error), one warn event names the stopped
//!   export, and every subsequent record leaves the writer untouched.
//! - The sink is an exporter, not a window: the snapshot stays honestly
//!   empty through the failure.

use std::sync::Arc;

use oxide_fw_server::trace_context::TraceContext;
use oxide_fw_server::{CaptureLayer, FileTraceSink, LogSink, RequestTrace, RingLogSink, TraceSink};

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
        std::time::Duration::from_micros(1250),
    )
}

/// The steered writer the pins inject: it accepts every byte it is
/// handed until steered to fail, and records what it accepted so the
/// pins can observe the side effects (the failure is observable only
/// through them — `record` returns `()`).
#[derive(Clone, Default)]
struct SteeredWriter {
    state: Arc<std::sync::Mutex<Steered>>,
}

#[derive(Default)]
struct Steered {
    accepted: Vec<u8>,
    /// When set, every `write`/`flush` refuses (the full-disk shape).
    failing: bool,
    write_calls: usize,
}

impl SteeredWriter {
    fn steer(&self, failing: bool) {
        self.state.lock().expect("steered").failing = failing;
    }

    fn accepted(&self) -> String {
        String::from_utf8(self.state.lock().expect("steered").accepted.clone())
            .expect("the accepted bytes are UTF-8 JSONL")
    }

    fn write_calls(&self) -> usize {
        self.state.lock().expect("steered").write_calls
    }
}

impl std::io::Write for SteeredWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut state = self.state.lock().expect("steered");
        state.write_calls += 1;
        if state.failing {
            return Err(std::io::Error::other("steered: the disk is full"));
        }
        state.accepted.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut state = self.state.lock().expect("steered");
        state.write_calls += 1;
        if state.failing {
            return Err(std::io::Error::other("steered: the disk is full"));
        }
        Ok(())
    }
}

/// Install the capture layer over a fresh ring as the thread's default
/// subscriber and run `body` under it — the warn-once pin reads the
/// captured rows back (the F24 capture arm is the observability surface
/// the pin rides).
fn under_capture(body: impl FnOnce()) -> Vec<oxide_fw_server::LogEvent> {
    use tracing_subscriber::layer::SubscriberExt as _;
    let sink = Arc::new(RingLogSink::new(64));
    let layer = CaptureLayer::new(sink.clone() as Arc<dyn LogSink>);
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), body);
    sink.snapshot()
}

// -------------------------------------------------------------------- pins

#[test]
fn writer_sink_appends_one_jsonl_line_per_record() {
    let writer = SteeredWriter::default();
    let sink = FileTraceSink::from_writer(writer.clone());
    sink.record(traced_row("req-f27a", "/index"));
    sink.record(traced_row("req-f27b", "/submit"));
    // Flush-per-record: the bytes are in the sink before the record
    // returns — an injected writer observes them with no further flush.
    let accepted = writer.accepted();
    let lines: Vec<&str> = accepted.lines().collect();
    assert_eq!(lines.len(), 2, "one row, one line: {accepted:?}");
    let first: serde_json::Value =
        serde_json::from_str(lines[0]).expect("each line is one JSON object");
    assert_eq!(first["requestId"], "req-f27a");
    assert_eq!(first["path"], "/index");
    let second: serde_json::Value =
        serde_json::from_str(lines[1]).expect("each line is one JSON object");
    assert_eq!(second["requestId"], "req-f27b");
}

#[test]
fn writer_rows_are_shape_identical_to_the_endpoint_wire() {
    // The wire identity rule (the F25 shape, now over the injected
    // writer): the file line IS the endpoint's row.
    let writer = SteeredWriter::default();
    let sink = FileTraceSink::from_writer(writer.clone());
    let row = traced_row("req-f27c", "/index");
    sink.record(row.clone());
    let ring = Arc::new(oxide_fw_server::RingTraceSink::new(4));
    ring.record(row);
    let ep = oxide_fw_server::TracesEndpoint::new(ring.clone() as Arc<dyn TraceSink>);
    let served = ep.serve("GET");
    let endpoint_row: serde_json::Value = serde_json::from_slice::<serde_json::Value>(&served.body)
        .expect("the endpoint renders JSON")["traces"][0]
        .clone();
    let accepted = writer.accepted();
    let file_row: serde_json::Value =
        serde_json::from_str(accepted.trim()).expect("the line is one JSON object");
    assert_eq!(
        file_row, endpoint_row,
        "one row, one rendering: the injected writer's line is the endpoint's row"
    );
}

#[test]
fn the_failing_row_never_reaches_the_writer() {
    // The failed record drops whole: no half line — the writer's bytes
    // end with the previous complete line.
    let writer = SteeredWriter::default();
    let sink = FileTraceSink::from_writer(writer.clone());
    sink.record(traced_row("req-f27d", "/index"));
    let before = writer.accepted();
    writer.steer(true);
    sink.record(traced_row("req-f27e", "/submit"));
    assert_eq!(writer.accepted(), before, "the failed row left no bytes");
    assert!(
        before.ends_with('\n'),
        "the accepted bytes stay complete lines"
    );
}

#[test]
fn the_failure_is_sticky_and_subsequent_rows_drop() {
    let writer = SteeredWriter::default();
    let sink = FileTraceSink::from_writer(writer.clone());
    sink.record(traced_row("req-f27f", "/index"));
    writer.steer(true);
    sink.record(traced_row("req-f27g", "/submit"));
    let calls = writer.write_calls();
    // Sticky: further records never touch the writer again (no retry
    // churn, no half lines).
    sink.record(traced_row("req-f27h", "/api"));
    sink.record(traced_row("req-f27i", "/api/x"));
    assert_eq!(
        writer.write_calls(),
        calls,
        "the sticky sink stopped writing"
    );
    assert_eq!(writer.accepted().lines().count(), 1);
}

#[test]
fn exactly_one_warn_names_the_stopped_export() {
    let writer = SteeredWriter::default();
    let sink = FileTraceSink::from_writer(writer.clone());
    sink.record(traced_row("req-f27j", "/index"));
    writer.steer(true);
    let warns = under_capture(|| {
        sink.record(traced_row("req-f27k", "/submit"));
        sink.record(traced_row("req-f27l", "/api"));
        sink.record(traced_row("req-f27m", "/api/x"));
    });
    let warn_rows: Vec<&oxide_fw_server::LogEvent> =
        warns.iter().filter(|row| row.level == "warn").collect();
    assert_eq!(
        warn_rows.len(),
        1,
        "one warn, then the rows drop silently: {warns:?}"
    );
    assert_eq!(warn_rows[0].target, "oxide_fw_server::trace_export");
    assert!(
        warn_rows[0].message.contains("trace file export stopped"),
        "the warn names the stopped export: {:?}",
        warn_rows[0].message
    );
}

#[test]
fn writer_sink_snapshot_stays_honestly_empty_through_a_failure() {
    let writer = SteeredWriter::default();
    let sink = FileTraceSink::from_writer(writer.clone());
    sink.record(traced_row("req-f27n", "/index"));
    assert!(
        sink.snapshot().is_empty(),
        "the file arm is an exporter, not a window"
    );
    writer.steer(true);
    sink.record(traced_row("req-f27n2", "/submit"));
    assert!(
        sink.snapshot().is_empty(),
        "the failure changes nothing about the exporter posture"
    );
}

#[test]
fn writer_sink_debug_names_the_seam() {
    let sink = FileTraceSink::from_writer(SteeredWriter::default());
    assert!(format!("{sink:?}").contains("FileTraceSink"));
}

#[test]
fn the_file_constructor_unchanged_shape() {
    // `create` stays the boot's constructor: the same JSONL framing over
    // the file the path opens (the F25 pins cover the file shape; this
    // pin covers the constructor's contract over the shared impl).
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("traces.jsonl");
    let sink = FileTraceSink::create(&path).expect("the export file opens");
    sink.record(traced_row("req-f27o", "/index"));
    let contents = std::fs::read_to_string(&path).expect("the row is on disk");
    let first: serde_json::Value =
        serde_json::from_str(contents.trim()).expect("the line is one JSON object");
    assert_eq!(first["requestId"], "req-f27o");
}

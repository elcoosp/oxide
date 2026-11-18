//! TDD pins for the trace rows' file export (V2-F25): the §20 feed's
//! rows persist as JSONL beside the `tracing` export arm.
//!
//! The F22 export arm rides the standard `tracing` stream — whatever
//! subscriber the host installed renders the rows (console,
//! `OXIDE_LOG=json`, the OTLP stream). But a subscriber's output is the
//! host's: a dev session whose console scrolled past, or an OTLP
//! collector that was down, has no durable record of the window. The
//! file arm is the durable half: [`FileTraceSink`] appends one JSONL
//! line per recorded row to the file the boot opened at attach time —
//! the same camelCase row shape the `GET /__oxide/traces` endpoint
//! serves (the wire identity rule: one row, one rendering, both
//! surfaces).
//!
//! - [`FileTraceSink::create`] opens (create + append) the file at
//!   attach time and fails loudly on a bad path — the dev boot aborts
//!   rather than silently losing the audit trail (the session-keys
//!   posture: a silently degraded security/audit surface is a
//!   downgrade).
//! - [`TraceSink::record`] appends + flushes per row: when the record
//!   returns, the bytes are in the file (a crash cannot lose the
//!   already-recorded rows; the dev audit trail is the point).
//! - [`TraceSink::snapshot`] stays honestly empty — the file arm is an
//!   exporter, not a window (the F22 posture; a consumer wanting the
//!   window polls the ring-backed endpoint).
//! - In a [`FanoutTraceSink`] the file child is a plain record consumer:
//!   one row reaches every child, the window child (the ring) stays
//!   first, and the export children contribute no rows to the endpoint's
//!   snapshot (the no-cross-child-re-ordering rule).

use std::sync::Arc;

use oxide_fw_server::trace_context::TraceContext;
use oxide_fw_server::{
    FanoutTraceSink, FileTraceSink, RequestTrace, RingTraceSink, TraceSink, TracesEndpoint,
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
        std::time::Duration::from_micros(1250),
    )
}

/// A fresh export file inside a per-pin temp dir.
fn export_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().join("traces.jsonl")
}

// -------------------------------------------------------------------- pins

#[test]
fn file_sink_writes_one_jsonl_line_per_record() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = export_path(&dir);
    let sink = FileTraceSink::create(&path).expect("the export file opens");
    sink.record(traced_row("req-f25a", "/index"));
    sink.record(traced_row("req-f25b", "/submit"));
    let contents = std::fs::read_to_string(&path).expect("the rows are already on disk");
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines.len(), 2, "one row, one line: {contents:?}");
    let first: serde_json::Value =
        serde_json::from_str(lines[0]).expect("each line is one JSON object");
    assert_eq!(first["requestId"], "req-f25a");
    assert_eq!(first["path"], "/index");
    let second: serde_json::Value =
        serde_json::from_str(lines[1]).expect("each line is one JSON object");
    assert_eq!(second["requestId"], "req-f25b");
}

#[test]
fn file_rows_are_shape_identical_to_the_endpoint_wire() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = export_path(&dir);
    let sink = FileTraceSink::create(&path).expect("the export file opens");
    let row = traced_row("req-f25c", "/index");
    sink.record(row.clone());
    // The endpoint's rendering of the same row (the ring-backed wire).
    let ring = Arc::new(RingTraceSink::new(4));
    ring.record(row);
    let ep = TracesEndpoint::new(ring.clone() as Arc<dyn TraceSink>);
    let served = ep.serve("GET");
    let endpoint_row: serde_json::Value = serde_json::from_slice::<serde_json::Value>(&served.body)
        .expect("the endpoint renders JSON")["traces"][0]
        .clone();
    let contents = std::fs::read_to_string(&path).expect("the row is on disk");
    let file_row: serde_json::Value =
        serde_json::from_str(contents.trim()).expect("the line is one JSON object");
    assert_eq!(
        file_row, endpoint_row,
        "one row, one rendering: the file line is the endpoint's row"
    );
}

#[test]
fn file_sink_snapshot_stays_honestly_empty() {
    let dir = tempfile::tempdir().expect("temp dir");
    let sink = FileTraceSink::create(&export_path(&dir)).expect("the export file opens");
    sink.record(traced_row("req-f25d", "/index"));
    assert!(
        sink.snapshot().is_empty(),
        "the file arm is an exporter, not a window"
    );
}

#[test]
fn file_sink_debug_names_the_seam() {
    let dir = tempfile::tempdir().expect("temp dir");
    let sink = FileTraceSink::create(&export_path(&dir)).expect("the export file opens");
    assert!(format!("{sink:?}").contains("FileTraceSink"));
}

#[test]
fn attach_fails_loudly_on_a_bad_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    // A directory as the export path: the open refuses (the boot aborts
    // rather than silently losing the audit trail).
    let missing = FileTraceSink::create(&dir.path().join("no-such-dir").join("t.jsonl"));
    assert!(missing.is_err(), "a missing parent dir fails the attach");
    let dir_path = FileTraceSink::create(dir.path());
    assert!(dir_path.is_err(), "a directory path fails the attach");
}

#[test]
fn fan_reaches_the_file_child_and_the_window_stays_first() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = export_path(&dir);
    let file = Arc::new(FileTraceSink::create(&path).expect("the export file opens"));
    let ring = Arc::new(RingTraceSink::new(4));
    let fan = FanoutTraceSink::new(vec![
        ring.clone() as Arc<dyn TraceSink>,
        file.clone() as Arc<dyn TraceSink>,
    ]);
    fan.record(traced_row("req-f25e", "/index"));
    // The window child keeps the row (the endpoint stays ring-backed).
    assert_eq!(ring.snapshot().len(), 1, "the window child records");
    // The file child persists the same row (flush-per-record: it is on
    // disk before the record returns).
    let contents = std::fs::read_to_string(&path).expect("the row is on disk");
    assert_eq!(contents.lines().count(), 1);
    // The fan's snapshot is the children's rows in construction order —
    // the window child's row, the exporter contributing none.
    assert_eq!(fan.snapshot().len(), 1, "the exporter contributes no rows");
}

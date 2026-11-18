//! V2-F22 (the server half): the trace rows' `tracing` export — the §20
//! feed's rows ride the standard observability stream.
//!
//! The F21 feed records one [`RequestTrace`] per completed pipeline
//! request into the attached sink, and `GET /__oxide/traces` serves the
//! window — but the rows never leave the process unless the consumer
//! polls the endpoint. This module is the export seam:
//!
//! - [`TracingTraceSink`] is the export arm: one `tracing::info!` event
//!   per recorded row, under the module-path target
//!   `oxide_fw_server::trace_export`, with `snake_case` fields mirroring
//!   the row's Rust field names (`request_id`, `method`, `path`,
//!   `status`, `duration_ms`, `sampled`, `trace_id`, `parent_id`,
//!   `tracestate`). Whatever subscriber the host installed renders it —
//!   the console formatter, `OXIDE_LOG=json`, and with
//!   `OTEL_EXPORTER_OTLP_ENDPOINT` the OTLP stream — so the rows
//!   correlate with the §20 `oxide.request` span through the same
//!   `request_id` the `Server-Timing` header reports. The absent W3C
//!   halves render as the empty string (the row's `None` halves stay
//!   recorded — the feed's own honesty rule, "an untraced request
//!   records the `None` halves honestly rather than being skipped").
//!   The sink is not a window: [`TraceSink::snapshot`] stays empty
//!   because rows are emitted, never retained.
//! - [`FanoutTraceSink`] is the record fan: one record reaches every
//!   child sink (the dev boot attaches ring + tracing behind one
//!   pipeline `trace_sink` slot), and the snapshot is each child's
//!   newest-first rows in construction order — no cross-child
//!   re-ordering, because rows carry no global sequence (the window
//!   child is listed first). An endpoint backed by a fanout whose
//!   window child is the ring renders exactly the ring's JSON.
//! - [`FileTraceSink`] is the durable half (V2-F25): one JSONL line per
//!   recorded row, appended + flushed on record, the same camelCase row
//!   shape the endpoint serves. The attach is loud (a bad path aborts
//!   the dev boot); a mid-session write failure is sticky — one
//!   `tracing::warn!`, then the rows drop (a half-written line is worse
//!   than a stopped export). The dev boot attaches it beside the ring
//!   and the tracing arm when `OXIDE_TRACE_FILE` opts in. V2-F27: the
//!   JSONL framing rides one constructor over any `std::io::Write`
//!   ([`FileTraceSink::from_writer`], the injectable writer the sticky
//!   rule's pins steer; `create` stays the file-path shape).
//!
//! The pipeline's record point is unchanged: still one `None` check on
//! [`crate::Pipeline::trace_sink`] — a sink that happens to fan out is
//! the attached seam's business, and the unattached pipeline stays
//! byte-identical. The CLI's dev boot attaches the fanout only when
//! `OXIDE_TRACE_LOG` (the tracing arm) or `OXIDE_TRACE_FILE` (the file
//! arm) opts in (production never attaches — the F21 posture holds).

use std::sync::Arc;

use crate::trace_feed::{RequestTrace, TraceSink};

/// The export arm: every recorded row leaves as one `tracing::info!`
/// event. Anything the host's subscriber renders, the rows render with
/// it — there is no second sink type to install.
#[derive(Debug, Default)]
pub struct TracingTraceSink;

impl TracingTraceSink {
    /// An export sink. Attach it beside the window sink through
    /// [`FanoutTraceSink`] — the pipeline has one `trace_sink` slot and
    /// the endpoint keeps serving the ring.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl TraceSink for TracingTraceSink {
    fn record(&self, trace: RequestTrace) {
        tracing::info!(
            request_id = %trace.request_id,
            method = %trace.method,
            path = %trace.path,
            status = u64::from(trace.status),
            duration_ms = trace.duration_ms,
            sampled = trace.sampled,
            trace_id = trace.trace_id.as_deref().unwrap_or(""),
            parent_id = trace.parent_id.as_deref().unwrap_or(""),
            tracestate = trace.tracestate.as_deref().unwrap_or(""),
            "request completed",
        );
    }

    fn snapshot(&self) -> Vec<RequestTrace> {
        // The export arm is not a window: rows are emitted, never
        // retained — a consumer wanting the window polls the ring-backed
        // endpoint.
        Vec::new()
    }
}

/// The record fan: one record reaches every child sink. The dev boot
/// builds the fan the pipeline records into — the ring (the endpoint's
/// window) and [`TracingTraceSink`] (the subscriber's event stream) —
/// so one `trace_sink` slot serves both halves.
///
/// The snapshot contract is honest concatenation: each child's rows in
/// that child's own newest-first order, children in construction order,
/// and no cross-child re-ordering — [`RequestTrace`] carries no global
/// sequence number, so any merge sort would be fabricated. The window
/// child (the ring) is listed first, so a fanout-backed endpoint renders
/// the same JSON as today's ring-backed one; the export children
/// contribute no rows (they are not windows).
pub struct FanoutTraceSink {
    children: Vec<Arc<dyn TraceSink>>,
}

impl std::fmt::Debug for FanoutTraceSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The children are the fan's identity (the boot's attach shape);
        // each sink's own Debug carries its internals.
        f.debug_struct("FanoutTraceSink")
            .field("children", &self.children.len())
            .finish_non_exhaustive()
    }
}

impl FanoutTraceSink {
    /// A fan over the child sinks. An empty fan is the honest no-op: it
    /// records nothing and answers an empty window.
    #[must_use]
    pub fn new(children: Vec<Arc<dyn TraceSink>>) -> Self {
        Self { children }
    }

    /// The child count the fan was built with.
    #[must_use]
    pub fn children(&self) -> usize {
        self.children.len()
    }
}

impl TraceSink for FanoutTraceSink {
    fn record(&self, trace: RequestTrace) {
        // One row reaches every child — each child gets its own clone
        // (the row is a plain record shape, `Clone` by design so the
        // seams stay value-shaped; the dev boot is ring + tracing + file
        // wide at most).
        for child in &self.children {
            child.record(trace.clone());
        }
    }

    fn snapshot(&self) -> Vec<RequestTrace> {
        let mut rows = Vec::new();
        for child in &self.children {
            rows.extend(child.snapshot());
        }
        rows
    }
}

/// V2-F25: the file arm — every recorded row appends as one JSONL line
/// to the file the boot opened at attach time. The F22 export arm rides
/// the standard `tracing` stream (whatever subscriber the host
/// installed renders the rows); the file arm is the durable half — a
/// dev session whose console scrolled past, or an OTLP collector that
/// was down, still has the audit trail.
///
/// The rows are the endpoint's wire shape, re-rendered
/// ([`crate::trace_feed::row_json`]): one row, one rendering, both
/// surfaces (the wire identity rule). One row, one line, flushed on
/// record: when the record returns, the bytes are in the file — a crash
/// cannot lose the already-recorded rows.
///
/// The attach is loud: [`FileTraceSink::create`] opens (create + append)
/// the file and surfaces the IO error — the dev boot aborts on a bad
/// path rather than silently losing the audit trail (the session-keys
/// posture: a silently degraded audit surface is a downgrade). A write
/// that fails mid-session (a full disk is the realistic shape) is
/// sticky: the sink emits one `tracing::warn!` and drops every
/// subsequent row — it never half-writes a line, and it never panics
/// the pipeline that records into it. The sink is not a window:
/// [`TraceSink::snapshot`] stays empty because rows are emitted, never
/// retained.
///
/// The JSONL framing rides ONE constructor: [`FileTraceSink::from_writer`]
/// takes any `std::io::Write` (V2-F27 — the injectable writer the
/// sticky-failure rule's pins steer; an embedded exporter can point the
/// seam at a socket or an in-memory sink), and `create` is the file-path
/// shape over the same framing.
pub struct FileTraceSink {
    writer: std::sync::Mutex<std::io::BufWriter<Box<dyn std::io::Write + Send>>>,
    failed: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for FileTraceSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The writer is the sink's internal state (a buffered writer over
        // the injected byte sink); the seam name is the identity the boot
        // attaches.
        f.debug_struct("FileTraceSink").finish_non_exhaustive()
    }
}

impl FileTraceSink {
    /// Open (create + append) the export file. The attach fails loudly —
    /// a bad path aborts the dev boot rather than silently losing the
    /// audit trail.
    ///
    /// # Errors
    /// The underlying `OpenOptions::open` error: a missing parent
    /// directory, an unwritable location, a directory path.
    pub fn create(path: &std::path::Path) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Self::from_writer(file))
    }

    /// The seam over any byte sink (V2-F27): the same one-row-one-line
    /// JSONL framing, flushed per record. The sticky write-failure rule
    /// rides the injected writer too — the first failing write flips the
    /// sink sticky (one warn, then the rows drop), which is what the
    /// `trace_file_writer_tdd` pins steer. The writer is wrapped in a
    /// `BufWriter` either way, so the syscall shape matches the file
    /// constructor (one flushed write per record).
    pub fn from_writer(writer: impl std::io::Write + Send + 'static) -> Self {
        Self {
            writer: std::sync::Mutex::new(std::io::BufWriter::new(Box::new(writer))),
            failed: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl TraceSink for FileTraceSink {
    fn record(&self, trace: RequestTrace) {
        use std::io::Write as _;
        if self.failed.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let line = crate::trace_feed::row_json(&trace).to_string();
        let mut writer = self.writer.lock().expect("trace file mutex poisoned");
        let outcome = writer
            .write_all(line.as_bytes())
            .and_then(|()| writer.write_all(b"\n"))
            .and_then(|()| writer.flush());
        if let Err(e) = outcome {
            // Sticky: one warning, then every row drops. A half-written
            // line is worse than a stopped export — the audit trail's
            // tail stays parseable.
            self.failed
                .store(true, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!(error = %e, "trace file export stopped; dropping further rows");
        }
    }

    fn snapshot(&self) -> Vec<RequestTrace> {
        // The file arm is an exporter, not a window: rows are emitted,
        // never retained — a consumer wanting the window polls the
        // ring-backed endpoint.
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_fan_is_the_honest_noop() {
        let fan = FanoutTraceSink::new(Vec::new());
        fan.record(RequestTrace::of(
            "req-f22",
            "GET",
            "/index",
            None,
            200,
            std::time::Duration::ZERO,
        ));
        assert!(fan.snapshot().is_empty(), "the honest no-op keeps no rows");
    }

    #[test]
    fn export_sink_debug_names_the_seam() {
        assert!(format!("{:?}", TracingTraceSink::new()).contains("TracingTraceSink"));
    }
}

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
//!
//! The pipeline's record point is unchanged: still one `None` check on
//! [`crate::Pipeline::trace_sink`] — a sink that happens to fan out is
//! the attached seam's business, and the unattached pipeline stays
//! byte-identical. The CLI's dev boot attaches the fanout only when
//! `OXIDE_TRACE_LOG` opts in (production never attaches — the F21
//! posture holds).

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
        // seams stay value-shaped; the dev boot is two children wide).
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

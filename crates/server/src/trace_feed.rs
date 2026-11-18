//! V2-F21 (the server half): `GET /__oxide/traces` — the dev-plane trace
//! feed the §V2.J map advertises next to the open-in-editor and
//! component-cost surfaces (`/__oxide/open, /__oxide/cost,
//! /__oxide/traces` — dev plane, dev-token guarded, D41).
//!
//! §20's W3C trace context is parsed on every request
//! ([`crate::trace_context::TraceContext::from_headers`] at the pipeline
//! entry) and rides [`crate::OxideRequest::trace`] — but until this
//! module nothing *consumed* it (the module's own doc recorded the
//! context as "available but not consumed by any live call site"). The
//! feed is the consumption seam:
//!
//! - [`TraceSink`] records one [`RequestTrace`] per completed pipeline
//!   request — the native (Raw/Static/image/server-fn) arms and the
//!   framework dispatch arm both. The record carries the request id, the
//!   method, the path, the response status, the wall-clock duration and
//!   the parsed trace context halves (`traceId`/`parentId`/`sampled`/
//!   `tracestate`); an untraced request records the `None` halves
//!   honestly rather than being skipped.
//! - [`RingTraceSink`] is the bounded production impl: a capacity-capped
//!   ring, the oldest row evicted at capacity, the snapshot newest-first.
//!   A zero capacity records nothing — the honest empty window.
//! - [`TracesEndpoint`] answers `GET /__oxide/traces` with the ring's
//!   rows as camelCase JSON (the §14 convention). A non-GET verb is the
//!   documented `405` + `allow: GET` (the nav/open contract). There is
//!   no query parameter: the window is the answer.
//!
//! The endpoint is the WIRE CONTRACT only: the sink behind it is the
//! injected seam (the dev boot attaches one [`RingTraceSink`] the
//! pipeline records into and the endpoint serves — the
//! WaterfallRecorder's one-instance-two-readers posture). The endpoint
//! only exists when attached (the inspector's posture):
//! [`crate::Pipeline::trace_sink`] and [`crate::Pipeline::trace_feed`]
//! are `None` in the default pipeline — production never attaches, the
//! record point is one `None` check, the unattached endpoint falls
//! through to the normal dispatch, and production is byte-identical.

use std::sync::Arc;

use crate::OxideResponse;
use crate::trace_context::TraceContext;

/// The §V2.J dev-plane path the endpoint serves (the map row beside the
/// open-in-editor and cost surfaces).
pub const TRACE_FEED_ENDPOINT_PATH: &str = "/__oxide/traces";

/// The default ring window the dev boot attaches (the recent-requests
/// feed the inspector/overlay reasoning pays; a dev loop with traffic
/// turns this over in seconds, a health probe cannot fill it alone).
pub const TRACE_FEED_CAPACITY: usize = 256;

/// One completed request's trace row — the feed's record shape, camelCase
/// on the wire (the §14 convention; the rows render in
/// [`TracesEndpoint::serve`]'s `json!`).
#[derive(Debug, Clone, PartialEq)]
pub struct RequestTrace {
    /// The pipeline's per-request id (SEC-02) — the `Server-Timing` /
    /// log correlation key.
    pub request_id: String,
    /// The request method.
    pub method: String,
    /// The request path (no query).
    pub path: String,
    /// The response status the pipeline completed with.
    pub status: u16,
    /// The wall-clock serve duration in milliseconds (the `serve_request`
    /// entry → response shape, the same window the `Server-Timing`
    /// `total` reports on the dispatch arm).
    pub duration_ms: f64,
    /// The W3C trace id when the request carried a parseable
    /// `traceparent`, `None` when untraced.
    pub trace_id: Option<String>,
    /// The W3C parent span id, when traced.
    pub parent_id: Option<String>,
    /// The `traceparent` sampled bit, when traced.
    pub sampled: bool,
    /// The `tracestate` header verbatim, when carried.
    pub tracestate: Option<String>,
}

impl RequestTrace {
    /// Map one completed request into a row. `trace` is the pipeline's
    /// parsed W3C context (`None` for an untraced request — the row still
    /// records, the trace halves stay `None`).
    #[must_use]
    pub fn of(
        request_id: &str,
        method: &str,
        path: &str,
        trace: Option<&TraceContext>,
        status: u16,
        elapsed: std::time::Duration,
    ) -> Self {
        Self {
            request_id: request_id.to_string(),
            method: method.to_string(),
            path: path.to_string(),
            status,
            duration_ms: elapsed.as_secs_f64() * 1000.0,
            trace_id: trace.map(|t| t.trace_id.clone()),
            parent_id: trace.map(|t| t.parent_id.clone()),
            sampled: trace.is_some_and(|t| t.sampled),
            tracestate: trace.and_then(|t| t.tracestate.clone()),
        }
    }
}

/// The record seam: the feed the pipeline's completed-request arms write
/// and the endpoint serves. The production impl is [`RingTraceSink`];
/// tests and embedded hosts inject fakes.
pub trait TraceSink: Send + Sync {
    /// Record one completed request. Bounded impls evict their oldest
    /// rows to keep the window.
    fn record(&self, trace: RequestTrace);

    /// The window's rows, newest first (the endpoint's render order).
    fn snapshot(&self) -> Vec<RequestTrace>;
}

/// The bounded production sink: a capacity-capped FIFO ring, the oldest
/// row evicted at capacity, [`TraceSink::snapshot`] newest-first. The
/// steady-state cost of a record is one push and (at capacity) one pop —
/// the `trace_sink_ring` bench group measures the shape.
pub struct RingTraceSink {
    capacity: usize,
    rows: std::sync::Mutex<std::collections::VecDeque<RequestTrace>>,
}

impl std::fmt::Debug for RingTraceSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The rows are the sink's internal state (a mutex over the
        // window); the capacity is the identity the boot attaches.
        f.debug_struct("RingTraceSink")
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl RingTraceSink {
    /// A ring holding at most `capacity` rows. A zero capacity records
    /// nothing (the honest empty window).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            rows: std::sync::Mutex::new(std::collections::VecDeque::new()),
        }
    }

    /// The window size the ring was built with.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl TraceSink for RingTraceSink {
    fn record(&self, trace: RequestTrace) {
        let mut rows = self.rows.lock().expect("trace ring mutex poisoned");
        rows.push_back(trace);
        while rows.len() > self.capacity {
            rows.pop_front();
        }
    }

    fn snapshot(&self) -> Vec<RequestTrace> {
        let rows = self.rows.lock().expect("trace ring mutex poisoned");
        rows.iter().rev().cloned().collect()
    }
}

/// One row's wire shape — the camelCase record the endpoint serves.
fn row_json(row: &RequestTrace) -> serde_json::Value {
    serde_json::json!({
        "requestId": row.request_id,
        "method": row.method,
        "path": row.path,
        "status": row.status,
        "durationMs": row.duration_ms,
        "traceId": row.trace_id,
        "parentId": row.parent_id,
        "sampled": row.sampled,
        "tracestate": row.tracestate,
    })
}

/// The endpoint state: the injected sink (the same instance the pipeline
/// records into — the dev boot attaches both halves of one
/// [`RingTraceSink`]).
pub struct TracesEndpoint {
    sink: Arc<dyn TraceSink>,
}

impl TracesEndpoint {
    /// An endpoint over one trace sink.
    #[must_use]
    pub fn new(sink: Arc<dyn TraceSink>) -> Self {
        Self { sink }
    }

    /// The total request → response function (everything but the
    /// dev-token guard, which is the pipeline arm's — the token never
    /// reaches this module).
    ///
    /// - non-`GET` → `405` + `allow: GET` (the nav/open contract).
    /// - a `GET` → `200` JSON `{"traces": [...]}`, newest row first.
    #[must_use]
    pub fn serve(&self, method: &str) -> OxideResponse {
        if !method.eq_ignore_ascii_case("GET") {
            return error_response(405, "the trace feed answers GET", Some(("allow", "GET")));
        }
        let rows = self.sink.snapshot();
        tracing::info!(rows = rows.len(), "trace feed served");
        OxideResponse::json(
            200,
            serde_json::json!({
                "traces": rows.iter().map(row_json).collect::<Vec<_>>(),
            }),
        )
    }
}

/// The endpoint's error spelling: a plain-text body with the status (the
/// nav/open rejection shape).
fn error_response(status: u16, msg: &str, extra: Option<(&str, &str)>) -> OxideResponse {
    let mut resp = OxideResponse::new(status, bytes::Bytes::from(msg.to_string()))
        .with_header("content-type", "text/plain; charset=utf-8");
    if let Some((k, v)) = extra {
        resp = resp.with_header(k, v);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_const_is_the_dev_plane_spelling() {
        assert_eq!(TRACE_FEED_ENDPOINT_PATH, "/__oxide/traces");
    }

    #[test]
    fn default_capacity_is_the_documented_window() {
        assert_eq!(TRACE_FEED_CAPACITY, 256);
    }
}

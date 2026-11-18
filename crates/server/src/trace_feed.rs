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
//! - [`TracesEndpoint`] answers `GET /__oxide/traces` with the window's
//!   rows as camelCase JSON (the §14 convention). A non-GET verb is the
//!   documented `405` + `allow: GET` (the nav/open contract). The F23
//!   query seam filters the window — `?status=5xx` (a response-class
//!   spelling) or `?status=404` (exact), `?method=POST` (case-insensitive
//!   exact), `?path=/api` (a percent-decoded, segment-honest prefix),
//!   `?limit=50` (positive, capped at [`TRACE_FEED_CAPACITY`]); the
//!   filters AND together, the newest-first order is never re-ordered,
//!   unknown parameters (`token=…` rides the same query) are ignored, and
//!   an invalid optional value degrades to no filter — never an error
//!   (the nav endpoint's optional-limit posture). [`TracesEndpoint::serve`]
//!   stays the no-query whole-window shape, byte-identical to F21's
//!   contract: the window, filtered, is the answer.
//!
//! The endpoint is the WIRE CONTRACT only: the sink behind it is the
//! injected seam (the dev boot attaches one [`RingTraceSink`] the
//! pipeline records into and the endpoint serves — the
//! `WaterfallRecorder`'s one-instance-two-readers posture). The endpoint
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

/// One row's wire shape — the camelCase record the endpoint serves. The
/// file export arm re-renders it (V2-F25): one row, one rendering, both
/// surfaces (the wire identity rule).
pub(crate) fn row_json(row: &RequestTrace) -> serde_json::Value {
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

/// One parsed `?status=` spelling: an exact status (`404`) or a response
/// class (`5xx` — case-insensitive). An unparsable spelling is no filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusFilter {
    Exact(u16),
    /// The class digit (1–5): rows whose status is `digit`00–`digit`99.
    Class(u8),
}

impl StatusFilter {
    fn parse(value: &str) -> Option<Self> {
        let bytes = value.as_bytes();
        if bytes.len() != 3 {
            return None;
        }
        let lowered = value.to_ascii_lowercase();
        if let Some(digit) = lowered
            .strip_suffix("xx")
            .and_then(|head| head.chars().next().and_then(|c| c.to_digit(10)))
            .and_then(|d| u8::try_from(d).ok())
        {
            return if (1..=5).contains(&digit) {
                Some(Self::Class(digit))
            } else {
                None
            };
        }
        if bytes.iter().all(u8::is_ascii_digit) {
            return value.parse::<u16>().ok().map(Self::Exact);
        }
        None
    }

    fn matches(self, status: u16) -> bool {
        match self {
            Self::Exact(expected) => status == expected,
            Self::Class(digit) => status / 100 == u16::from(digit),
        }
    }
}

/// The parsed `GET /__oxide/traces` query: the F23 filter seam over the
/// window. Parse never fails — every parameter is optional and an
/// unparsable value degrades to no filter (the nav endpoint's
/// optional-limit posture), so a malformed query renders the whole
/// window rather than erroring. The filters AND together.
///
/// Unknown parameters are ignored by construction (the parse reads only
/// the four filter spellings — the D41 `token` rides the same query and
/// the dev-plane arm consumes it before the endpoint sees anything).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TraceQuery {
    status: Option<StatusFilter>,
    method: Option<String>,
    path_prefix: Option<String>,
    limit: Option<usize>,
}

impl TraceQuery {
    /// Parse the raw query string (`status=5xx&limit=10`) into the
    /// filter set. The empty string (or `None`, in
    /// [`TracesEndpoint::serve_query`]) filters nothing.
    #[must_use]
    pub fn parse(query: &str) -> Self {
        let mut parsed = Self::default();
        for kv in query.split('&') {
            if let Some(value) = kv.strip_prefix("status=") {
                parsed.status = StatusFilter::parse(value);
            } else if let Some(value) = kv.strip_prefix("method=") {
                parsed.method = (!value.is_empty()).then(|| value.to_ascii_uppercase());
            } else if let Some(value) = kv.strip_prefix("path=") {
                let decoded = crate::percent_decode(value);
                parsed.path_prefix = (!decoded.is_empty()).then_some(decoded);
            } else if let Some(value) = kv.strip_prefix("limit=") {
                // Positive, capped at the default window (the nav
                // endpoint's `?limit=` shape: a bigger number cannot
                // out-render the ring, a zero or unparsable one is no
                // limit).
                parsed.limit = value
                    .parse::<usize>()
                    .ok()
                    .filter(|l| *l > 0)
                    .map(|l| l.min(TRACE_FEED_CAPACITY));
            }
        }
        parsed
    }

    /// Does one row pass every filter?
    #[must_use]
    pub fn matches(&self, row: &RequestTrace) -> bool {
        self.status.as_ref().is_none_or(|f| f.matches(row.status))
            && self
                .method
                .as_deref()
                .is_none_or(|m| row.method.eq_ignore_ascii_case(m))
            && self
                .path_prefix
                .as_deref()
                .is_none_or(|p| path_prefix_matches(&row.path, p))
    }

    /// The render cap: the parsed `?limit=`, or the default window when
    /// unlimited (the window itself never exceeds the ring's capacity,
    /// so the cap never clips an unfiltered window).
    #[must_use]
    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(TRACE_FEED_CAPACITY)
    }

    /// Was no positive `?limit=` parsed? (A zero or unparsable limit is
    /// no limit — the whole filtered window renders.)
    #[must_use]
    pub fn is_unlimited(&self) -> bool {
        self.limit.is_none()
    }
}

/// The segment-honest prefix match: `/api` matches `/api/x` but not
/// `/apiology`; `/api/` matches `/api/…`; the exact path matches itself.
fn path_prefix_matches(path: &str, prefix: &str) -> bool {
    if !path.starts_with(prefix) {
        return false;
    }
    path.len() == prefix.len() || prefix.ends_with('/') || path.as_bytes()[prefix.len()] == b'/'
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
    /// - a `GET` → `200` JSON `{"traces": [...]}`, newest row first, the
    ///   WHOLE window — the F21 contract, byte-identical to serving no
    ///   query ([`TracesEndpoint::serve_query`] with `None`).
    #[must_use]
    pub fn serve(&self, method: &str) -> OxideResponse {
        self.serve_query(method, None)
    }

    /// The F23 filtered shape: the same contract with the query applied
    /// — the window, filtered, is the answer. `query` is the raw query
    /// string (everything after the `?`; `None` is the unfiltered
    /// whole-window render above). See [`TraceQuery`] for the filter
    /// spellings and the degrade-to-no-filter posture.
    #[must_use]
    pub fn serve_query(&self, method: &str, query: Option<&str>) -> OxideResponse {
        if !method.eq_ignore_ascii_case("GET") {
            return error_response(405, "the trace feed answers GET", Some(("allow", "GET")));
        }
        let parsed = query.map_or_else(TraceQuery::default, TraceQuery::parse);
        let rows: Vec<RequestTrace> = self
            .sink
            .snapshot()
            .into_iter()
            .filter(|row| parsed.matches(row))
            .take(parsed.limit())
            .collect();
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

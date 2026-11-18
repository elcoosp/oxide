//! V2-F24 (the server half): `GET /__oxide/logs` — the dev-plane log
//! feed the §V2.J map advertises beside the trace feed (`/__oxide/open,
//! /__oxide/cost, /__oxide/traces, /__oxide/logs` — dev plane,
//! dev-token guarded, D41).
//!
//! The trace feed answers "which requests completed and how" — one
//! [`RequestTrace`](crate::trace_feed::RequestTrace) row per completed
//! pipeline request. It never records what the framework *said*: the
//! `tracing` events the pipeline and the engine emit (the §20 request
//! log under the `oxide::request` target, the export arm's `request
//! completed` events, the endpoint and governor notices) leave through
//! whatever subscriber the host installed — a terminal, a JSON file, an
//! OTLP collector. A developer with only the dev server (the
//! inspector's own posture: no terminal attached) has no window on
//! those events. The feed is that window:
//!
//! - [`LogEvent`] is the record shape: the capture wall-clock (epoch
//!   ms), the level (the lowercase `tracing` name), the target, the
//!   message and the structured fields. The fields are value-shaped
//!   `(name, rendered)` pairs — the capture renders every field value
//!   as its string, so the row stays a plain record (the
//!   [`crate::trace_feed::RequestTrace`] posture: `Clone` by design so
//!   the seams stay value-shaped).
//! - [`LogSink`] is the record seam — the
//!   [`crate::trace_feed::TraceSink`] shape (record + newest-first
//!   snapshot) — and [`RingLogSink`] is the bounded production impl: a
//!   capacity-capped ring, the oldest row evicted at capacity, the
//!   snapshot newest-first, a zero capacity records nothing.
//! - [`CaptureLayer`] is the capture arm: a
//!   `tracing_subscriber::Layer` that records every event the
//!   subscriber admits into the attached sink. The layer is a second
//!   read on the event stream — it changes no rendering, so the console
//!   output stays byte-identical (the F22 posture: capture is additive).
//!   The feed never records its own module's events (the serving event
//!   included — the target `oxide_fw_server::log_feed` is skipped), so
//!   polling the endpoint cannot fill the window with itself.
//! - [`LogsEndpoint`] answers `GET /__oxide/logs` with the window's
//!   rows as camelCase JSON (`tsMs`/`level`/`target`/`message`/
//!   `fields`). A non-GET verb is the documented `405` + `allow: GET`
//!   (the nav/open/traces contract). The [`LogQuery`] filter seam
//!   narrows the window — `?level=warn,error` (comma-separated exact
//!   names, case-insensitive), `?target=oxide_fw` (a target prefix),
//!   `?q=` (a message substring), `?limit=50` (positive, capped at
//!   [`LOG_FEED_CAPACITY`]); the filters AND together, unknown
//!   parameters are ignored, and an invalid value degrades to no filter
//!   — never an error (the F23/`traces` posture). [`LogsEndpoint::serve`]
//!   stays the no-query whole-window shape, byte-identical between
//!   calls.
//!
//! The endpoint is the WIRE CONTRACT only: the sink behind it is the
//! injected seam (the dev boot attaches one [`RingLogSink`] the capture
//! layer records into and the endpoint serves — the one-instance-two-
//! readers posture the trace feed rides). The endpoint only exists when
//! attached: [`crate::Pipeline::log_feed`] is `None` in the default
//! pipeline — production never attaches, the unattached endpoint falls
//! through to the normal dispatch, and production is byte-identical.

use std::sync::Arc;

use crate::OxideResponse;

/// The §V2.J dev-plane path the endpoint serves (the map row beside the
/// trace feed).
pub const LOG_FEED_ENDPOINT_PATH: &str = "/__oxide/logs";

/// The default ring window the dev boot attaches (the same window the
/// trace feed rides — a dev loop with traffic turns it over in seconds,
/// a quiet session keeps the recent events).
pub const LOG_FEED_CAPACITY: usize = 256;

/// The module-path target the feed itself emits under — the capture
/// layer skips it (the no-feedback rule).
const FEED_TARGET: &str = "oxide_fw_server::log_feed";

/// One captured `tracing` event — the feed's record shape, camelCase on
/// the wire (the §14 convention; the rows render in
/// [`LogsEndpoint::serve`]'s `json!`).
#[derive(Debug, Clone, PartialEq)]
pub struct LogEvent {
    /// The capture wall-clock, epoch milliseconds (the row's ordering
    /// stamp; the ring already reads newest-first, so the consumer's
    /// timeline stays reconstructible across ring turnover).
    pub ts_ms: u64,
    /// The event level, the lowercase `tracing` name (`info`, `warn`,
    /// `error`, `debug`, `trace`).
    pub level: String,
    /// The event target (the module path the event was emitted under).
    pub target: String,
    /// The event message (the `tracing` message field).
    pub message: String,
    /// The event's structured fields as `(name, rendered)` pairs in
    /// record order — every value rendered as its string, so the row
    /// stays a plain record.
    pub fields: Vec<(String, String)>,
}

/// The record seam: the feed the capture layer writes and the endpoint
/// serves. The production impl is [`RingLogSink`]; tests and embedded
/// hosts inject fakes.
pub trait LogSink: Send + Sync {
    /// Record one captured event. Bounded impls evict their oldest rows
    /// to keep the window.
    fn record(&self, event: LogEvent);

    /// The window's rows, newest first (the endpoint's render order).
    fn snapshot(&self) -> Vec<LogEvent>;
}

/// The bounded production sink: a capacity-capped FIFO ring, the oldest
/// row evicted at capacity, [`LogSink::snapshot`] newest-first. The
/// steady-state cost of a record is one push and (at capacity) one pop —
/// the `log_sink_ring` bench group measures the shape.
pub struct RingLogSink {
    capacity: usize,
    rows: std::sync::Mutex<std::collections::VecDeque<LogEvent>>,
}

impl std::fmt::Debug for RingLogSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The rows are the sink's internal state (a mutex over the
        // window); the capacity is the identity the boot attaches.
        f.debug_struct("RingLogSink")
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl RingLogSink {
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

impl LogSink for RingLogSink {
    fn record(&self, event: LogEvent) {
        let mut rows = self.rows.lock().expect("log ring mutex poisoned");
        rows.push_back(event);
        while rows.len() > self.capacity {
            rows.pop_front();
        }
    }

    fn snapshot(&self) -> Vec<LogEvent> {
        let rows = self.rows.lock().expect("log ring mutex poisoned");
        rows.iter().rev().cloned().collect()
    }
}

/// The field visitor: every value lands as its rendered string (the
/// capture's plain-record rule).
struct FieldVisitor<'a>(&'a mut Vec<(String, String)>);

impl tracing::field::Visit for FieldVisitor<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.push((field.name().to_string(), value.to_string()));
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.0.push((field.name().to_string(), value.to_string()));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0.push((field.name().to_string(), value.to_string()));
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.0.push((field.name().to_string(), value.to_string()));
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.0.push((field.name().to_string(), value.to_string()));
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0
            .push((field.name().to_string(), format!("{value:?}")));
    }
}

/// The capture arm: a `tracing_subscriber::Layer` that records every
/// event the subscriber admits into the attached sink. Install it
/// beside the formatter layers (`registry().with(CaptureLayer::new(sink))
/// .with(fmt::layer())`) — it is a second read on the event stream and
/// changes no rendering.
///
/// The layer never records the feed's own module (the target
/// [`FEED_TARGET`] is skipped): the endpoint emits one serving event per
/// poll, and a capture that recorded it would grow the window the poll
/// renders — the no-feedback rule.
#[derive(Clone)]
pub struct CaptureLayer {
    sink: Arc<dyn LogSink>,
}

impl std::fmt::Debug for CaptureLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The sink is the layer's identity (the boot's attach shape).
        f.debug_struct("CaptureLayer").finish_non_exhaustive()
    }
}

impl CaptureLayer {
    /// A capture layer over one log sink.
    #[must_use]
    pub fn new(sink: Arc<dyn LogSink>) -> Self {
        Self { sink }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::layer::Layer<S> for CaptureLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let meta = event.metadata();
        if meta.target() == FEED_TARGET {
            return;
        }
        let mut fields = Vec::new();
        event.record(&mut FieldVisitor(&mut fields));
        let ts_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        self.sink.record(LogEvent {
            ts_ms,
            level: meta.level().to_string().to_ascii_lowercase(),
            target: meta.target().to_string(),
            message: fields
                .iter()
                .find(|(name, _)| name == "message")
                .map_or_else(String::new, |(_, value)| value.clone()),
            fields,
        });
    }
}

/// The row's wire shape — the camelCase record the endpoint serves. The
/// structured fields render as an object with sorted keys (the default
/// `serde_json::Map` order — the workspace never enables
/// `preserve_order`, so the wire is deterministic bytes).
fn row_json(row: &LogEvent) -> serde_json::Value {
    let mut fields = serde_json::Map::new();
    for (name, value) in &row.fields {
        fields.insert(name.clone(), serde_json::Value::String(value.clone()));
    }
    serde_json::json!({
        "tsMs": row.ts_ms,
        "level": row.level,
        "target": row.target,
        "message": row.message,
        "fields": fields,
    })
}

/// One parsed `?level=` spelling: a comma-separated list of exact level
/// names, case-insensitive. A list naming anything but the five
/// `tracing` levels is no filter (the whole list degrades — the F23
/// posture's all-or-nothing spelling).
#[derive(Debug, Clone, PartialEq, Eq)]
struct LevelFilter {
    levels: Vec<String>,
}

impl LevelFilter {
    /// The five `tracing` level names the filter admits.
    const NAMES: [&'static str; 5] = ["trace", "debug", "info", "warn", "error"];

    fn parse(value: &str) -> Option<Self> {
        let names: Option<Vec<String>> = value
            .split(',')
            .map(|name| {
                let lowered = name.trim().to_ascii_lowercase();
                if Self::NAMES.contains(&lowered.as_str()) {
                    Some(lowered)
                } else {
                    None
                }
            })
            .collect();
        names.map(|levels| Self { levels })
    }

    fn matches(&self, level: &str) -> bool {
        self.levels.iter().any(|name| name == level)
    }
}

/// The parsed `GET /__oxide/logs` query: the filter seam over the
/// window. Parse never fails — every parameter is optional and an
/// unparsable value degrades to no filter (the nav endpoint's
/// optional-limit posture), so a malformed query renders the whole
/// window rather than erroring. The filters AND together.
///
/// Unknown parameters are ignored by construction (the parse reads only
/// the four filter spellings — the D41 `token` rides the same query and
/// the dev-plane arm consumes it before the endpoint sees anything).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogQuery {
    levels: Option<LevelFilter>,
    target_prefix: Option<String>,
    message_substring: Option<String>,
    limit: Option<usize>,
}

impl LogQuery {
    /// Parse the raw query string (`level=warn&limit=10`) into the
    /// filter set. The empty string (or `None`, in
    /// [`LogsEndpoint::serve_query`]) filters nothing.
    #[must_use]
    pub fn parse(query: &str) -> Self {
        let mut parsed = Self::default();
        for kv in query.split('&') {
            if let Some(value) = kv.strip_prefix("level=") {
                parsed.levels = LevelFilter::parse(value);
            } else if let Some(value) = kv.strip_prefix("target=") {
                parsed.target_prefix = (!value.is_empty()).then(|| value.to_string());
            } else if let Some(value) = kv.strip_prefix("q=") {
                let decoded = crate::percent_decode(value);
                parsed.message_substring = (!decoded.is_empty()).then_some(decoded);
            } else if let Some(value) = kv.strip_prefix("limit=") {
                // Positive, capped at the default window (the F23 `?limit=`
                // shape: a bigger number cannot out-render the ring, a zero
                // or unparsable one is no limit).
                parsed.limit = value
                    .parse::<usize>()
                    .ok()
                    .filter(|l| *l > 0)
                    .map(|l| l.min(LOG_FEED_CAPACITY));
            }
        }
        parsed
    }

    /// Does one row pass every filter?
    #[must_use]
    pub fn matches(&self, row: &LogEvent) -> bool {
        self.levels.as_ref().is_none_or(|f| f.matches(&row.level))
            && self
                .target_prefix
                .as_deref()
                .is_none_or(|p| row.target.starts_with(p))
            && self
                .message_substring
                .as_deref()
                .is_none_or(|q| row.message.contains(q))
    }

    /// The render cap: the parsed `?limit=`, or the default window when
    /// unlimited (the window itself never exceeds the ring's capacity,
    /// so the cap never clips an unfiltered window).
    #[must_use]
    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(LOG_FEED_CAPACITY)
    }

    /// Was no positive `?limit=` parsed? (A zero or unparsable limit is
    /// no limit — the whole filtered window renders.)
    #[must_use]
    pub fn is_unlimited(&self) -> bool {
        self.limit.is_none()
    }
}

/// The endpoint state: the injected sink (the same instance the capture
/// layer records into — the dev boot attaches both halves of one
/// [`RingLogSink`]).
pub struct LogsEndpoint {
    sink: Arc<dyn LogSink>,
}

impl LogsEndpoint {
    /// An endpoint over one log sink.
    #[must_use]
    pub fn new(sink: Arc<dyn LogSink>) -> Self {
        Self { sink }
    }

    /// The total request → response function (everything but the
    /// dev-token guard, which is the pipeline arm's — the token never
    /// reaches this module).
    ///
    /// - non-`GET` → `405` + `allow: GET` (the nav/open/traces contract).
    /// - a `GET` → `200` JSON `{"logs": [...]}`, newest row first, the
    ///   WHOLE window — byte-identical to serving no query
    ///   ([`LogsEndpoint::serve_query`] with `None`).
    #[must_use]
    pub fn serve(&self, method: &str) -> OxideResponse {
        self.serve_query(method, None)
    }

    /// The filtered shape: the same contract with the query applied —
    /// the window, filtered, is the answer. `query` is the raw query
    /// string (everything after the `?`; `None` is the unfiltered
    /// whole-window render above). See [`LogQuery`] for the filter
    /// spellings and the degrade-to-no-filter posture.
    #[must_use]
    pub fn serve_query(&self, method: &str, query: Option<&str>) -> OxideResponse {
        if !method.eq_ignore_ascii_case("GET") {
            return error_response(405, "the log feed answers GET", Some(("allow", "GET")));
        }
        let parsed = query.map_or_else(LogQuery::default, LogQuery::parse);
        let rows: Vec<LogEvent> = self
            .sink
            .snapshot()
            .into_iter()
            .filter(|row| parsed.matches(row))
            .take(parsed.limit())
            .collect();
        tracing::info!(target: FEED_TARGET, rows = rows.len(), "log feed served");
        OxideResponse::json(
            200,
            serde_json::json!({
                "logs": rows.iter().map(row_json).collect::<Vec<_>>(),
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
        assert_eq!(LOG_FEED_ENDPOINT_PATH, "/__oxide/logs");
    }

    #[test]
    fn default_capacity_is_the_documented_window() {
        assert_eq!(LOG_FEED_CAPACITY, 256);
    }

    #[test]
    fn level_filter_parses_the_five_names_case_insensitive() {
        let parsed = LevelFilter::parse("WARN,error").expect("both names valid");
        assert_eq!(parsed.levels, vec!["warn".to_string(), "error".to_string()]);
        assert!(LevelFilter::parse("warn,bogus").is_none());
        assert!(
            LevelFilter::parse("").is_none(),
            "an empty value names nothing"
        );
    }
}

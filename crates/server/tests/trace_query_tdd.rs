//! TDD pins for the trace feed's filtered window (V2-F23): the §V2.J
//! dev-plane row answers the window WITH a query, not only whole.
//!
//! F21 minted `GET /__oxide/traces` as "no query parameter — the window
//! is the answer" (256 rows, newest first). That contract held while the
//! window was the only read; a consumer that asks "which requests
//! failed recently?" or "what did the last POSTs cost?" pulls the full
//! snapshot and filters by hand. F23 is the query seam — the window
//! filtered is the answer:
//!
//! - `?status=5xx` (a response-class spelling) or `?status=404` (an
//!   exact status) — the rows' status halves;
//! - `?method=POST` — case-insensitive exact;
//! - `?path=/api` — a case-sensitive prefix, percent-decoded (the nav
//!   endpoint's `?from=` shape);
//! - `?limit=50` — a positive integer, capped at the default window
//!   ([`TRACE_FEED_CAPACITY`]); an absent, zero, or unparsable limit is
//!   the whole filtered window (the nav endpoint's optional-limit
//!   posture: optional params degrade, nothing 400s);
//! - unknown parameters (`token=…` rides the same query) are ignored —
//!   the D41 guard consumes the token before the endpoint sees anything;
//! - `TracesEndpoint::serve` stays the whole-window shape: a
//!   no-query render is byte-identical to the F21 contract.
//!
//! All filters AND together; the newest-first order is never re-ordered
//! by a filter (the window's read order is the contract).

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use oxide_fw_server::trace_context::TraceContext;
use oxide_fw_server::{
    OxideRequest, OxideResponse, Pipeline, RequestTrace, RingTraceSink, TraceQuery, TraceSink,
    TracesEndpoint,
};

// ------------------------------------------------------------------ helpers

const TRACED_PARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";

/// A minimal GET request (the `trace_feed_tdd.rs` shape).
fn get_with(url: &str, headers: &[(&str, &str)]) -> OxideRequest {
    OxideRequest {
        method: "GET".into(),
        url: url.into(),
        headers: Arc::new(
            headers
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect::<Vec<_>>(),
        ),
        header_index: OnceLock::default(),
        body: Bytes::new(),
        request_id: "req-f23".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

/// A sink with a mixed window: three GETs (one 404, one 500 on /api/x,
/// one 200), two POSTs (one 500 on /api/y, one 200). Recorded oldest
/// first — the snapshot reads newest first.
fn mixed_window() -> Arc<RingTraceSink> {
    let sink = Arc::new(RingTraceSink::new(16));
    let rows: [(&str, &str, u16); 5] = [
        ("GET", "/index", 404),
        ("GET", "/api/x", 500),
        ("POST", "/submit", 200),
        ("GET", "/", 200),
        ("POST", "/api/y", 500),
    ];
    for (i, (method, path, status)) in rows.iter().enumerate() {
        sink.record(RequestTrace::of(
            &format!("req-{i}"),
            method,
            path,
            None,
            *status,
            Duration::from_millis(u64::try_from(i).unwrap_or(0)),
        ));
    }
    sink
}

fn rows_paths(resp: &OxideResponse) -> Vec<(String, String, u16)> {
    let body: serde_json::Value =
        serde_json::from_slice(&resp.body).expect("the rows render as JSON");
    body["traces"]
        .as_array()
        .expect("traces array")
        .iter()
        .map(|r| {
            (
                r["requestId"].as_str().unwrap_or_default().to_string(),
                r["path"].as_str().unwrap_or_default().to_string(),
                u16::try_from(r["status"].as_u64().unwrap_or_default()).unwrap_or_default(),
            )
        })
        .collect()
}

/// The dispatch delegate the fall-through pins use.
fn refused_dispatch()
-> impl Fn(OxideRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
{
    |_r: OxideRequest| {
        Box::pin(async { OxideResponse::new(500, "dispatch must not run") })
            as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
    }
}

// ------------------------------------------------------------ the parse pins

#[test]
fn trace_query_parses_the_status_class_spelling() {
    let q = TraceQuery::parse("status=5xx");
    assert!(q.matches(&row_with("GET", "/x", 500)), "500 is a 5xx");
    assert!(q.matches(&row_with("GET", "/x", 503)));
    assert!(
        !q.matches(&row_with("GET", "/x", 404)),
        "a 4xx is not a 5xx"
    );
    assert!(!q.matches(&row_with("GET", "/x", 200)));
}

#[test]
fn trace_query_parses_the_exact_status_spelling() {
    let q = TraceQuery::parse("status=404");
    assert!(q.matches(&row_with("GET", "/x", 404)));
    assert!(!q.matches(&row_with("GET", "/x", 500)), "exact, not class");
    let q = TraceQuery::parse("status=600");
    assert!(
        !q.matches(&row_with("GET", "/x", 200)),
        "no such status is served"
    );
}

#[test]
fn trace_query_class_spelling_is_case_insensitive() {
    let q = TraceQuery::parse("status=5XX");
    assert!(q.matches(&row_with("GET", "/x", 500)));
    assert!(!q.matches(&row_with("GET", "/x", 404)));
}

#[test]
fn trace_query_invalid_status_degrades_to_no_filter() {
    // The optional-params posture: an unknown value is no filter at all,
    // never an error (the nav endpoint's optional-limit shape).
    for spelling in ["status=abc", "status=", "status=xx", "status=5x"] {
        let q = TraceQuery::parse(spelling);
        assert!(
            q.matches(&row_with("GET", "/x", 200)),
            "`{spelling}` filters nothing"
        );
    }
}

#[test]
fn trace_query_method_is_exact_case_insensitive() {
    let q = TraceQuery::parse("method=post");
    assert!(q.matches(&row_with("POST", "/submit", 200)));
    assert!(!q.matches(&row_with("GET", "/index", 200)));
    let q = TraceQuery::parse("method=GET");
    assert!(q.matches(&row_with("GET", "/index", 200)));
    assert!(!q.matches(&row_with("POST", "/submit", 200)));
}

#[test]
fn trace_query_path_is_a_percent_decoded_prefix() {
    let q = TraceQuery::parse("path=%2Fapi");
    assert!(q.matches(&row_with("GET", "/api/x", 200)), "prefix match");
    assert!(!q.matches(&row_with("GET", "/pages/index", 200)));
    let q = TraceQuery::parse("path=/api");
    assert!(
        q.matches(&row_with("GET", "/api/x", 200)),
        "raw spelling too"
    );
    assert!(
        !q.matches(&row_with("GET", "/apiology", 200)),
        "the prefix is segment-honest: /apiology does not start with /api/"
    );
}

#[test]
fn trace_query_limit_trims_the_newest_first_window() {
    let sink = mixed_window();
    let rows = sink.snapshot();
    assert_eq!(rows.len(), 5);
    // The endpoint applies the limit; the parse keeps it for the render.
    let q = TraceQuery::parse("limit=2");
    let kept: Vec<&RequestTrace> = rows.iter().filter(|r| q.matches(r)).take(2).collect();
    assert_eq!(kept.len(), 2);
    assert_eq!(kept[0].path, "/api/y", "newest first, then trimmed");
}

#[test]
fn trace_query_limit_is_positive_and_capped_like_the_nav_limit() {
    let q = TraceQuery::parse("limit=0");
    assert!(q.is_unlimited(), "a zero limit is no limit (the nav shape)");
    let q = TraceQuery::parse("limit=abc");
    assert!(q.is_unlimited(), "an unparsable limit is no limit");
    let q = TraceQuery::parse("limit=99999");
    assert!(
        q.limit() <= oxide_fw_server::TRACE_FEED_CAPACITY,
        "the cap is the default window, so one query cannot out-render it"
    );
}

#[test]
fn trace_query_filters_and_together() {
    let q = TraceQuery::parse("status=5xx&method=GET&path=/api");
    assert!(q.matches(&row_with("GET", "/api/x", 500)));
    assert!(
        !q.matches(&row_with("POST", "/api/y", 500)),
        "the method half excludes it"
    );
    assert!(
        !q.matches(&row_with("GET", "/index", 500)),
        "the path half excludes it"
    );
}

#[test]
fn trace_query_ignores_unknown_parameters() {
    // `token` rides the same query (the D41 guard consumes it before the
    // endpoint sees anything); any other spelling filters nothing.
    let q = TraceQuery::parse("token=tok&foo=bar&status=5xx");
    assert!(q.matches(&row_with("GET", "/x", 500)));
    assert!(
        !q.matches(&row_with("GET", "/x", 404)),
        "status still filters"
    );
}

#[test]
fn trace_query_of_an_empty_string_filters_nothing() {
    let q = TraceQuery::parse("");
    assert!(q.matches(&row_with("GET", "/x", 200)));
    assert!(q.matches(&row_with("POST", "/y", 500)));
}

// --------------------------------------------------------- the endpoint pins

#[test]
fn endpoint_serve_stays_the_whole_window_contract() {
    // The F21 contract is byte-identical: no query renders every row.
    let sink = mixed_window();
    let ep = TracesEndpoint::new(sink.clone() as Arc<dyn TraceSink>);
    let served = ep.serve("GET");
    let queried = ep.serve_query("GET", None);
    assert_eq!(served.status, queried.status);
    assert_eq!(served.body, queried.body, "None query == serve");
    assert_eq!(rows_paths(&served).len(), 5, "the whole window renders");
}

#[test]
fn endpoint_serve_query_answers_the_filtered_window() {
    let sink = mixed_window();
    let ep = TracesEndpoint::new(sink.clone() as Arc<dyn TraceSink>);
    let resp = ep.serve_query("GET", Some("status=5xx"));
    assert_eq!(resp.status, 200);
    let rows = rows_paths(&resp);
    assert_eq!(rows.len(), 2, "the two 5xx rows: {rows:?}");
    // Newest first, never re-ordered by the filter.
    assert_eq!(rows[0].0, "req-4");
    assert_eq!(rows[0].2, 500);
    assert_eq!(rows[1].0, "req-1");
}

#[test]
fn endpoint_serve_query_refuses_non_get_with_405_and_allow() {
    let sink = mixed_window();
    let ep = TracesEndpoint::new(sink.clone() as Arc<dyn TraceSink>);
    let resp = ep.serve_query("POST", Some("status=5xx"));
    assert_eq!(resp.status, 405);
    assert_eq!(resp.header("allow"), Some("GET"));
}

#[test]
fn endpoint_serve_query_limit_trims_on_the_wire() {
    let sink = mixed_window();
    let ep = TracesEndpoint::new(sink.clone() as Arc<dyn TraceSink>);
    let resp = ep.serve_query("GET", Some("limit=1"));
    let rows = rows_paths(&resp);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "req-4", "the newest row alone");
}

#[test]
fn endpoint_serve_query_degrades_invalid_params_to_the_whole_window() {
    let sink = mixed_window();
    let ep = TracesEndpoint::new(sink.clone() as Arc<dyn TraceSink>);
    let resp = ep.serve_query("GET", Some("status=bogus&limit=abc"));
    assert_eq!(rows_paths(&resp).len(), 5, "nothing filtered");
}

// --------------------------------------------------------- the pipeline pins

#[tokio::test]
async fn pipeline_answers_the_filtered_window_through_the_dev_plane_arm() {
    // The arm passes the raw query through: the D41 token guard consumes
    // `token`, the endpoint filters the rest. The point-in-time contract
    // is unchanged — a query answers the pre-query state; its own row
    // rides the NEXT query's answer.
    let sink = mixed_window();
    let mut p = Pipeline::default()
        .with_trace_sink(sink.clone() as Arc<dyn TraceSink>)
        .with_trace_feed(Arc::new(TracesEndpoint::new(
            sink.clone() as Arc<dyn TraceSink>
        )));
    p.dev_token = Some("tok".into());
    let resp = p
        .serve_request(
            get_with("/__oxide/traces?token=tok&status=500", &[]),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(resp.status, 200);
    let rows = rows_paths(&resp);
    assert_eq!(rows.len(), 2, "the two 500 rows, exact spelling: {rows:?}");
    let resp = p
        .serve_request(
            get_with("/__oxide/traces?token=tok&status=5xx", &[]),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(resp.status, 200);
    let rows = rows_paths(&resp);
    assert_eq!(
        rows.len(),
        2,
        "the two 500s — the first query's own 200 row filters out, the second's lands after: {rows:?}"
    );
}

#[tokio::test]
async fn pipeline_query_without_the_token_still_403s() {
    let sink = mixed_window();
    let mut p = Pipeline::default()
        .with_trace_sink(sink.clone() as Arc<dyn TraceSink>)
        .with_trace_feed(Arc::new(TracesEndpoint::new(
            sink.clone() as Arc<dyn TraceSink>
        )));
    p.dev_token = Some("tok".into());
    let resp = p
        .serve_request(
            get_with("/__oxide/traces?status=5xx", &[]),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(resp.status, 403, "the D41 guard precedes any filtering");
}

fn row_with(method: &str, path: &str, status: u16) -> RequestTrace {
    let trace = TraceContext::parse_traceparent(TRACED_PARENT);
    RequestTrace::of(
        "req-f23",
        method,
        path,
        trace.as_ref(),
        status,
        Duration::from_millis(2),
    )
}

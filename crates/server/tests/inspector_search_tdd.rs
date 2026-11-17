//! TDD pins for the §V2-E08 inspector search pass-through: the
//! `/__oxide` arm hands a `?q=` query to the snapshot's
//! [`InspectorSnapshot::render_html_search`], percent-decoded, with a
//! blank or absent query keeping the unfiltered render (the pre-E08 byte
//! shape).
//!
//! The trait's default search render delegates to [`render_html`] — an
//! implementor without a searchable module graph serves the same page for
//! any query, and pre-E08 implementors keep compiling unchanged.

use std::sync::{Arc, Mutex, OnceLock};

use bytes::Bytes;
use oxide_fw_server::{InspectorSnapshot, OxideRequest, OxideResponse, Pipeline};

// ------------------------------------------------------------------ helpers

/// A minimal GET request against `url` (the `cost_endpoint_tdd.rs` shape).
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
        request_id: "req-e08".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

/// The dispatch delegate the pipeline pins use — the inspector arm must
/// answer the request itself, never delegate.
fn refused_dispatch()
-> impl Fn(OxideRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
{
    |_r: OxideRequest| {
        Box::pin(async { OxideResponse::new(500, "dispatch must not run") })
            as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
    }
}

/// A snapshot that records which render the arm asked for. The search
/// render answers `searched:<q>` so the pin sees the exact decoded query;
/// the plain render answers `unfiltered`.
#[derive(Debug, Default)]
struct RecordingInspector {
    searches: Mutex<Vec<String>>,
    plain_renders: Mutex<usize>,
}

impl InspectorSnapshot for RecordingInspector {
    fn state_json(&self) -> serde_json::Value {
        serde_json::json!({ "served": "state" })
    }
    fn render_html(&self) -> String {
        if let Ok(mut n) = self.plain_renders.lock() {
            *n += 1;
        }
        "unfiltered".to_string()
    }
    fn render_html_search(&self, query: &str) -> String {
        if let Ok(mut v) = self.searches.lock() {
            v.push(query.to_string());
        }
        format!("searched:{query}")
    }
}

/// The bare default-method probe: a fake that overrides nothing but the
/// required method, to pin the trait default's delegation.
#[derive(Debug)]
struct BareInspector;

impl InspectorSnapshot for BareInspector {
    fn state_json(&self) -> serde_json::Value {
        serde_json::json!({})
    }
}

// ------------------------------------------------------------- the contracts

#[test]
fn the_trait_default_search_render_delegates_to_render_html() {
    // An implementor without a module-graph search (the pre-E08 shape)
    // serves the same page for any query — the default keeps them
    // compiling and keeps their `/__oxide?q=` answers sane.
    assert_eq!(
        BareInspector.render_html_search("anything"),
        BareInspector.render_html()
    );
}

#[tokio::test]
async fn the_arm_passes_the_decoded_q_query_to_the_snapshot() {
    let insp = Arc::new(RecordingInspector::default());
    let p = Pipeline::default().with_inspector(insp.clone());
    let resp = p
        .serve_request(
            get_with("/__oxide?q=src%2Fhero.tsx", &[]),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(resp.status, 200);
    let body = String::from_utf8(resp.body.to_vec()).expect("utf8");
    assert_eq!(body, "searched:src/hero.tsx");
    assert_eq!(
        *insp.searches.lock().unwrap(),
        vec!["src/hero.tsx".to_string()]
    );
    assert_eq!(*insp.plain_renders.lock().unwrap(), 0);
}

#[tokio::test]
async fn the_arm_without_a_query_keeps_the_unfiltered_render() {
    let insp = Arc::new(RecordingInspector::default());
    let p = Pipeline::default().with_inspector(insp.clone());
    let resp = p
        .serve_request(get_with("/__oxide", &[]), &refused_dispatch())
        .await;
    assert_eq!(resp.status, 200);
    let body = String::from_utf8(resp.body.to_vec()).expect("utf8");
    assert_eq!(body, "unfiltered");
    assert_eq!(*insp.plain_renders.lock().unwrap(), 1);
    assert!(insp.searches.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_blank_or_foreign_query_keeps_the_unfiltered_render() {
    // `?q=` (empty submit) and a query naming no `q` key both render the
    // page unfiltered — the arm never passes an empty search down.
    for url in ["/__oxide?q=", "/__oxide?other=1", "/__oxide?q=   "] {
        let insp = Arc::new(RecordingInspector::default());
        let p = Pipeline::default().with_inspector(insp.clone());
        let resp = p
            .serve_request(get_with(url, &[]), &refused_dispatch())
            .await;
        assert_eq!(resp.status, 200, "{url}");
        assert_eq!(*insp.plain_renders.lock().unwrap(), 1, "{url}");
        assert!(insp.searches.lock().unwrap().is_empty(), "{url}");
    }
}

#[tokio::test]
async fn the_state_endpoint_still_serves_the_json_unchanged() {
    // Regression pin: the JSON view ignores the query string — the search
    // is an HTML-view surface, never a wire-shape change.
    let insp = Arc::new(RecordingInspector::default());
    let p = Pipeline::default().with_inspector(insp.clone());
    let resp = p
        .serve_request(
            get_with("/__oxide/state?q=whatever", &[]),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("json");
    assert_eq!(body["served"], "state");
    assert_eq!(*insp.plain_renders.lock().unwrap(), 0);
    assert!(insp.searches.lock().unwrap().is_empty());
}

//! TDD pins for the V2-F09 search endpoint (`feature = "search"`).
//!
//! The endpoint contract (§V2-F09, `GET /_oxide/search`):
//! - `200` with `{ total, hits: [{ id, url, title, snippet, score }] }`;
//! - a missing/empty `q` is a 400; `?limit=` caps the hits, never the
//!   total; `?tag=`/`?lang=` narrow the matches;
//! - a non-GET verb is a 405 with `allow: GET`;
//! - no attached index is the documented not-configured 404;
//! - the endpoint is rate limited per peer (the spec's 20 per 10 s) —
//!   the 21st request inside the window is a 429 with `retry-after`.
//!
//! The L1 cache is exercised through the endpoint: a tag-narrowed run
//! must not be served for the unfiltered query (the cache key covers the
//! filters).
//!
//! This file is `#![cfg(feature = "search")]`: without the feature the
//! endpoint does not exist (the documented default-off posture), and the
//! pins would not compile.

#![cfg(feature = "search")]

use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use oxide_fw_search::{
    ReloadOutcome, SearchDocument, SearchEngineOptions, SearchQuery, TantivyIndex,
};
use oxide_fw_server::{OxideRequest, Pipeline, SearchEndpoint};

// ------------------------------------------------------------------ helpers

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
        request_id: "req-f09".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

fn refused_dispatch() -> impl Fn(
    OxideRequest,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = oxide_fw_server::OxideResponse> + Send>,
> {
    |_r: OxideRequest| {
        Box::pin(async { oxide_fw_server::OxideResponse::new(500, "dispatch must not run") })
            as std::pin::Pin<
                Box<dyn std::future::Future<Output = oxide_fw_server::OxideResponse> + Send>,
            >
    }
}

fn engine() -> SearchEndpoint {
    let docs = vec![
        SearchDocument {
            id: "posts/a".into(),
            url: "/posts/a".into(),
            title: "the rust guide".into(),
            body: "rust is a systems language. it ships an indexer.".into(),
            tags: vec!["guide".into()],
            lang: "en".into(),
        },
        SearchDocument {
            id: "posts/b".into(),
            url: "/posts/b".into(),
            title: "shipping notes".into(),
            body: "the word rust appears in the body here.".into(),
            tags: vec!["api".into()],
            lang: "en".into(),
        },
        SearchDocument {
            id: "posts/c".into(),
            url: "/posts/c".into(),
            title: "der deutsche artikel".into(),
            body: "dieser text spricht ueber rust im allgemeinen.".into(),
            tags: vec!["guide".into()],
            lang: "de".into(),
        },
    ];
    let idx = TantivyIndex::build_in_ram(&docs, &SearchEngineOptions::default()).expect("build");
    SearchEndpoint::new(Arc::new(idx))
}

fn pipeline_with() -> Pipeline {
    Pipeline::default().with_search(Arc::new(engine()))
}

// --------------------------------------------------------- the renderer

#[tokio::test]
async fn the_search_endpoint_answers_the_spec_json_shape() {
    let p = pipeline_with();
    let req = get_with("/_oxide/search?q=rust", &[]);
    let resp = p.serve_request(req, &refused_dispatch()).await;
    assert_eq!(resp.status, 200, "the endpoint answers 200");
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("json body");
    assert_eq!(body["total"], 3);
    let hits = body["hits"].as_array().expect("hits array");
    assert_eq!(hits.len(), 3);
    assert_eq!(
        hits[0]["id"], "posts/a",
        "the title match outranks the body matches"
    );
    assert_eq!(hits[0]["url"], "/posts/a");
    assert!(
        hits[0]["snippet"]
            .as_str()
            .expect("snippet")
            .contains("<mark>"),
        "the snippet highlights with <mark>"
    );
}

#[tokio::test]
async fn a_missing_or_empty_q_is_a_400() {
    let p = pipeline_with();
    for url in [
        "/_oxide/search",
        "/_oxide/search?q=",
        "/_oxide/search?q=%20%20",
    ] {
        let req = get_with(url, &[]);
        let resp = p.serve_request(req, &refused_dispatch()).await;
        assert_eq!(resp.status, 400, "a missing/empty `q` is a 400 for {url}");
    }
}

#[tokio::test]
async fn limit_caps_the_hits_but_not_the_total() {
    let p = pipeline_with();
    let req = get_with("/_oxide/search?q=rust&limit=1", &[]);
    let resp = p.serve_request(req, &refused_dispatch()).await;
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("json body");
    assert_eq!(body["hits"].as_array().expect("hits").len(), 1);
    assert_eq!(body["total"], 3, "the total counts every match");
}

#[tokio::test]
async fn tag_and_lang_params_narrow_the_matches() {
    let p = pipeline_with();
    let tagged = p
        .serve_request(
            get_with("/_oxide/search?q=rust&tag=guide", &[]),
            &refused_dispatch(),
        )
        .await;
    let tag_body: serde_json::Value = serde_json::from_slice(&tagged.body).expect("json body");
    assert_eq!(tag_body["total"], 2, "only the guide-tagged docs");
    let langged = p
        .serve_request(
            get_with("/_oxide/search?q=rust&lang=de", &[]),
            &refused_dispatch(),
        )
        .await;
    let lang_body: serde_json::Value = serde_json::from_slice(&langged.body).expect("json body");
    assert_eq!(lang_body["total"], 1);
    assert_eq!(lang_body["hits"][0]["id"], "posts/c");
}

#[tokio::test]
async fn the_l1_cache_never_serves_a_tagged_answer_unfiltered() {
    // Same text query, different filters: the cache key covers the
    // filters, so the tag-narrowed total must not leak into the plain
    // query's answer (and vice versa).
    let p = pipeline_with();
    let tagged = p
        .serve_request(
            get_with("/_oxide/search?q=rust&tag=guide", &[]),
            &refused_dispatch(),
        )
        .await;
    assert_eq!(tagged.status, 200);
    let plain = p
        .serve_request(get_with("/_oxide/search?q=rust", &[]), &refused_dispatch())
        .await;
    let plain_body: serde_json::Value = serde_json::from_slice(&plain.body).expect("json body");
    assert_eq!(
        plain_body["total"], 3,
        "the unfiltered query is not the cached tag answer"
    );
}

// ----------------------------------------------------------- the arm

#[tokio::test]
async fn the_search_endpoint_is_disabled_without_an_index() {
    let p = Pipeline::default();
    let req = get_with("/_oxide/search?q=rust", &[]);
    let resp = p.serve_request(req, &refused_dispatch()).await;
    assert_eq!(
        resp.status, 404,
        "no attached index: the not-configured 404"
    );
    assert!(
        std::str::from_utf8(&resp.body)
            .expect("text")
            .contains("not configured"),
        "the 404 names the contract"
    );
}

#[tokio::test]
async fn a_non_get_verb_is_a_405_with_allow() {
    let p = pipeline_with();
    let mut req = get_with("/_oxide/search?q=rust", &[]);
    req.method = "POST".into();
    let resp = p.serve_request(req, &refused_dispatch()).await;
    assert_eq!(resp.status, 405);
    assert_eq!(
        resp.headers
            .iter()
            .find(|(k, _)| k == "allow")
            .map(|(_, v)| v.as_str()),
        Some("GET"),
        "the 405 advertises the method"
    );
}

#[tokio::test]
async fn the_endpoint_is_rate_limited_per_peer() {
    let p = pipeline_with();
    let mut last = 200;
    for i in 0..25 {
        let req = get_with(&format!("/_oxide/search?q=rust&cachebust={i}"), &[]);
        let resp = p.serve_request(req, &refused_dispatch()).await;
        last = resp.status;
        // Distinct queries bypass the L1 cache; the rate limiter counts
        // every request.
        if i < 20 {
            assert_eq!(last, 200, "request {i} within the budget answers 200");
        }
    }
    assert_eq!(last, 429, "the 21st request in the window is a 429");
    assert!(
        p.search_rate_limiter.allow("other-peer", 20, 10),
        "another peer has its own bucket"
    );
}

// ------------------------------------------------- the hot-reload seam

/// The dev-loop reload contract through the endpoint (the V2-F09
/// follow-up): the same query text, answered after a swapped-in corpus,
/// must not be served from the L1. The cache key partitions on the
/// engine's `generation`, so a reload repartitions every key — a stale
/// pre-reload hit can never survive a swap.
#[tokio::test]
async fn a_swapped_index_never_serves_stale_l1_hits() {
    let dir = tempfile::tempdir().unwrap();
    let index_dir = dir.path().join("index");
    let rebuild = |docs: &[SearchDocument]| {
        if index_dir.exists() {
            std::fs::remove_dir_all(&index_dir).unwrap();
        }
        TantivyIndex::build_in_dir(docs, &SearchEngineOptions::default(), &index_dir)
            .expect("the rebuild commits");
    };
    let doc = |body: &str| SearchDocument {
        id: "posts/a".into(),
        url: "/posts/a".into(),
        title: "the reindexed guide".into(),
        body: body.into(),
        tags: vec!["guide".into()],
        lang: "en".into(),
    };
    rebuild(std::slice::from_ref(&doc("alpha is the only marker here.")));
    let hot = Arc::new(
        oxide_fw_search::SwappableSearchIndex::open(&index_dir, &SearchEngineOptions::default())
            .expect("the built index opens"),
    );
    let ep = SearchEndpoint::new(hot.clone());
    let q = SearchQuery::new("alpha");
    let v1 = ep.run(&q).expect("the query runs");
    assert_eq!(v1.hits.len(), 1);
    let cached = ep.run(&q).expect("the cached query runs");
    assert_eq!(cached, v1, "the repeat run is served from the L1");

    // The rebuild rewrites the SAME document (the query shape is
    // identical, the corpus is not): only the generation partition
    // stands between the caller and the stale snippet.
    rebuild(std::slice::from_ref(&doc(
        "alpha has been reindexed with new prose.",
    )));
    assert!(matches!(
        hot.try_reload().expect("the reload opens the rebuild"),
        ReloadOutcome::Swapped { docs: 1 }
    ));
    let v2 = ep.run(&q).expect("the post-swap query runs");
    assert_ne!(
        v2.hits[0].snippet, v1.hits[0].snippet,
        "the swap repartitioned the L1: the new corpus answers"
    );
}

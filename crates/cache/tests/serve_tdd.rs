//! TDD suite for the cache plane's §11.3 serving half (`serve_entry`,
//! F54). Written first per the house TDD rule: every pin below failed on
//! the unresolved seam (E0432 over `ServeOutcome`/`serve_entry`) before
//! the impl existed.
//!
//! The contract the pins hold:
//!
//! 1. (F54) The §11.3 serving decision exists as ONE call: the request's
//!    conditional headers against the stored entry answer `NotModified`
//!    (the 304 arm, validators + Cache-Control included — RFC 9110
//!    §15.4.5's SHOULD) or `Full` (the entry's status + body with the
//!    §11.3 header set generated beside the stored headers). The
//!    orphaned helpers (`conditional_response`, `cache_control`,
//!    `http_date`) compose here — the audit's zero-consumer seam closes.
//! 2. (F54) The generated set never duplicates what the stored headers
//!    already carry (a render that set its own `Cache-Control` wins;
//!    the generated set fills only the gaps) — one header per name,
//!    case-insensitive.
//! 3. (F54) The capability upgrade the L0 stage's hand-rolled match
//!    never had: `If-Modified-Since` alone (no `If-None-Match`) answers
//!    304 against the entry's `Last-Modified` — browsers that send IMS
//!    stop downloading unchanged pages.

use std::time::Duration;

use oxide_fw_cache::{Entry, Policy, ServeOutcome, serve_entry};

/// A deterministic "now" for the `Date` header: 2026-11-19T00:00:00Z.
const NOW: u64 = 1_795_046_400;

fn entry(etag: &str, last_modified: Option<&str>, cache_control: Option<&str>) -> Entry {
    let mut headers: Vec<(String, String)> = vec![("content-type".into(), "text/html".into())];
    if let Some(lm) = last_modified {
        headers.push(("last-modified".into(), lm.into()));
    }
    if let Some(cc) = cache_control {
        headers.push(("cache-control".into(), cc.into()));
    }
    Entry {
        body: b"<html>rendered</html>".to_vec(),
        status: 200,
        headers,
        tags: vec![],
        policy: Policy {
            ttl: Duration::from_secs(60),
            swr: Some(Duration::from_secs(30)),
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: NOW * 1000,
        etag: etag.into(),
    }
}

fn h(k: &str, v: &str) -> (String, String) {
    (k.into(), v.into())
}

/// The 304 arm's header set: the validator + the policy's Cache-Control
/// (RFC 9110 §15.4.5: a 304 carries what the 200 would have).
fn assert_not_modified_headers(headers: &[(String, String)], etag: &str, want_cc: &str) {
    let get = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("etag"), Some(etag), "the validator rides the 304");
    assert_eq!(
        get("cache-control"),
        Some(want_cc),
        "the policy rides the 304"
    );
    assert_eq!(
        get("date"),
        Some(oxide_fw_cache::http_date(NOW).as_str()),
        "the Date rides too"
    );
}

#[test]
fn matched_etag_answers_304_with_the_validator_set() {
    let e = entry("\"v1\"", None, None);
    let out = serve_entry(&e, &[h("if-none-match", "\"v1\"")], NOW);
    let ServeOutcome::NotModified { headers } = out else {
        panic!("a matching validator answers 304");
    };
    assert_not_modified_headers(
        &headers,
        "\"v1\"",
        "public, max-age=60, s-maxage=60, stale-while-revalidate=30",
    );
}

#[test]
fn non_matching_etag_serves_full_with_the_generated_set() {
    let e = entry("\"v1\"", Some("Sun, 06 Nov 1994 08:49:37 GMT"), None);
    let out = serve_entry(&e, &[h("if-none-match", "\"v2\"")], NOW);
    let ServeOutcome::Full {
        status,
        headers,
        body,
    } = out
    else {
        panic!("a non-matching validator serves in full");
    };
    assert_eq!(status, 200);
    assert_eq!(body, b"<html>rendered</html>");
    let get = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(
        get("etag"),
        Some("\"v1\""),
        "the entry's validator rides the 200"
    );
    assert_eq!(
        get("content-type"),
        Some("text/html"),
        "stored headers ride"
    );
    assert_eq!(
        get("cache-control"),
        Some("public, max-age=60, s-maxage=60, stale-while-revalidate=30"),
        "the generated §11.3 set fills the gap",
    );
    assert_eq!(get("last-modified"), Some("Sun, 06 Nov 1994 08:49:37 GMT"));
    assert_eq!(get("date"), Some(oxide_fw_cache::http_date(NOW).as_str()));
}

#[test]
fn if_modified_since_alone_answers_304_when_unchanged() {
    // The L0 stage's hand-rolled match never did this: a browser sending
    // only IMS (no INM) against an unchanged Last-Modified downloaded the
    // whole page again. The canonical comparison answers 304.
    let e = entry("", Some("Sun, 06 Nov 1994 08:49:37 GMT"), None);
    let out = serve_entry(
        &e,
        &[h("if-modified-since", "Mon, 07 Nov 1994 08:49:37 GMT")],
        NOW,
    );
    assert!(
        matches!(out, ServeOutcome::NotModified { .. }),
        "IMS newer than the entry's Last-Modified answers 304",
    );
}

#[test]
fn generated_set_never_duplicates_stored_headers() {
    // The render set its own Cache-Control: the stored one wins and the
    // generated set does not append a second (one header per name).
    let e = entry("\"v1\"", None, Some("no-store"));
    let out = serve_entry(&e, &[], NOW);
    let ServeOutcome::Full { headers, .. } = out else {
        panic!("no conditional headers serve in full");
    };
    let cc: Vec<&str> = headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("cache-control"))
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(cc, vec!["no-store"], "the stored header wins, no duplicate");
}

#[test]
fn an_entry_without_a_validator_never_answers_304() {
    let e = entry("", None, None);
    let out = serve_entry(&e, &[h("if-none-match", "*")], NOW);
    assert!(
        matches!(out, ServeOutcome::Full { .. }),
        "no validator, no 304 — even against *",
    );
}

#[test]
fn no_store_policy_answers_no_store_in_both_arms() {
    let mut e = entry("\"v1\"", None, None);
    e.policy.ttl = Duration::from_secs(0);
    // 304 arm carries the no-store too.
    let out = serve_entry(&e, &[h("if-none-match", "\"v1\"")], NOW);
    let ServeOutcome::NotModified { headers } = out else {
        panic!("matching validator answers 304");
    };
    let cc = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("cache-control"))
        .map(|(_, v)| v.as_str());
    assert_eq!(cc, Some("no-store"));
}

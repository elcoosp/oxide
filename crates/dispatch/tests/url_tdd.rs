//! Integration tests for the `oxide-fw-dispatch` public URL surface
//! (spec v0.2, V2-A02).
//!
//! These pin the pub-API contract the phase-3 dispatch stages will consume:
//! the prelude's path/data-route resolution and the §9.3 form-action pair,
//! exercised together the way the closure uses them.

use oxide_fw_dispatch::url::{
    action_from_query, data_route_of, normalize_path, parse_form_urlencoded, path_of,
};

#[test]
fn a_form_post_resolves_its_action_and_body_together() {
    // §9.3: `POST /comment?/setcookie` with a urlencoded body. The query
    // names the action, the body carries the entries the worker's
    // `action:run` op accepts.
    let url = "/comment?/setcookie";
    let body = b"name=elmo&value=42";
    let action = action_from_query(url);
    let pairs = parse_form_urlencoded(body);
    assert_eq!(action.as_deref(), Some("setcookie"));
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].0, "name");
    assert_eq!(pairs[1].1, "42");
}

#[test]
fn a_data_request_resolves_to_a_matchable_route_path() {
    // The closure composes the two helpers for the §8.4 data route: resolve
    // the target route, then fold its trailing slash exactly like a real URL
    // path before matching.
    let route = data_route_of("GET", "/__data", "/__data?route=%2Fblog%2F")
        .expect("GET /__data is a data request");
    assert_eq!(route, "/blog/");
    assert_eq!(normalize_path(&route), "/blog");
}

#[test]
fn a_page_request_never_resolves_to_a_data_route() {
    let url = "/blog/a?route=%2Fblog%2Fa";
    let path = path_of(url);
    assert_eq!(path, "/blog/a");
    assert_eq!(normalize_path(path), "/blog/a");
    assert_eq!(data_route_of("GET", path, url), None);
}

#[test]
fn invalid_percent_escapes_pass_through_unchanged() {
    // `percent_decode` (oxide-fw-server) preserves invalid escapes instead of
    // failing: `%zz` is not a hex pair, so the route path keeps it verbatim.
    assert_eq!(
        data_route_of("GET", "/__data", "/__data?route=%zz"),
        Some("%zz".into())
    );
    assert_eq!(action_from_query("/c?/a%zz"), Some("a%zz".into()));
}

#[test]
fn unicode_routes_round_trip_through_the_data_endpoint() {
    let route = data_route_of("GET", "/__data", "/__data?route=%2Fblog%2Fcaf%C3%A9")
        .expect("decodable route");
    assert_eq!(route, "/blog/café");
    assert_eq!(normalize_path(&route), "/blog/café");
}

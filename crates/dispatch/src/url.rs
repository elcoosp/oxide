//! URL/query helpers for the dispatch prelude (spec v0.2, V2-A02).
//!
//! `data_route_of`, `normalize_path` and `path_of` are verbatim extractions
//! of the inline prelude logic in the CLI's `build_dispatch` closure
//! (`crates/cli/src/lib.rs`: trailing-slash fold, the `GET /__data?route=…`
//! data-route resolution, and the request-path split). The §9.3 pair
//! `parse_form_urlencoded` / `action_from_query` moves here verbatim from the
//! same file so the phase-3 stages can adopt them without touching the CLI.
//!
//! The CLI keeps its private copies until the phase-3 integration PR; this
//! module is the single future home. Semantics are pinned by the tests below,
//! including the deliberate edge pins that document current closure behaviour
//! rather than an idealized contract.

/// Rewrite-hop budget for the dispatch prelude's rewrite guard. The guard
/// itself (§4.1: reject request chains that exceed the internal rewrite
/// budget) is wired in phase 3; the constant lives here so the CLI and the
/// stages share one number from now on.
pub const MAX_REWRITE_HOPS: u32 = 8;

/// Path without query or fragment.
#[must_use]
pub fn path_of(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or("/")
}

/// Trailing slash removed except for the root.
#[must_use]
pub fn normalize_path(raw: &str) -> String {
    if raw.len() > 1 && raw.ends_with('/') {
        raw.trim_end_matches('/').to_string()
    } else {
        raw.to_string()
    }
}

/// `Some(route)` when the request is `GET /__data?route=...` (default route is `/`).
#[must_use]
pub fn data_route_of(method: &str, path: &str, url: &str) -> Option<String> {
    if !method.eq_ignore_ascii_case("GET") || path != "/__data" {
        return None;
    }
    let q = url.split_once('?').map_or("", |(_, q)| q);
    let r = q
        .split('&')
        .find_map(|kv| {
            kv.strip_prefix("route=")
                .map(oxide_fw_server::percent_decode)
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/".to_string());
    Some(r)
}

/// §9.3: parse an `application/x-www-form-urlencoded` body into the
/// `[key, value]` entry list the worker's `action:run` op accepts. Percent
/// decoding handles `+` as space. A malformed pair is skipped, not fatal.
#[must_use]
pub fn parse_form_urlencoded(body: &[u8]) -> Vec<(String, String)> {
    let s = String::from_utf8_lossy(body);
    s.split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (
                oxide_fw_server::percent_decode(&k.replace('+', " ")),
                oxide_fw_server::percent_decode(&v.replace('+', " ")),
            )
        })
        .collect()
}

/// §9.3: the action name from a form-action query. `?/comment` → `comment`.
/// The `SvelteKit` convention: a query that begins with `/` names an action.
#[must_use]
pub fn action_from_query(url: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    let q = q.split('#').next().unwrap_or(q);
    let rest = q.strip_prefix('/')?;
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    Some(oxide_fw_server::percent_decode(rest))
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_REWRITE_HOPS, action_from_query, data_route_of, normalize_path, parse_form_urlencoded,
        path_of,
    };

    #[test]
    fn normalize_keeps_root() {
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(normalize_path("/a/"), "/a");
    }

    #[test]
    fn normalize_folds_one_trailing_slash_and_is_idempotent() {
        assert_eq!(normalize_path("/blog/"), "/blog");
        let once = normalize_path("/blog/");
        assert_eq!(normalize_path(&once), once);
    }

    #[test]
    fn normalize_leaves_plain_paths_untouched() {
        assert_eq!(normalize_path("/blog"), "/blog");
        assert_eq!(normalize_path("/a/b/c"), "/a/b/c");
    }

    #[test]
    fn normalize_pins_the_empty_result_of_a_double_slash_root() {
        // Edge pin, not an idealized contract: `raw.len() > 1` lets `"//"`
        // fall into the trim branch and collapse to the empty string. That is
        // exactly what the CLI closure does today (`raw.len() > 1 &&
        // raw.ends_with('/')`), and this substrate must not silently change
        // behaviour ahead of the phase-3 adoption. The prelude's own
        // path-split step never produces `"//"`.
        assert_eq!(normalize_path("//"), "");
    }

    #[test]
    fn path_of_strips_query_and_fragment() {
        assert_eq!(path_of("/blog/a?route=%2Fx"), "/blog/a");
        assert_eq!(path_of("/blog/a#frag"), "/blog/a");
        assert_eq!(path_of("/blog/a?x=1#frag"), "/blog/a");
    }

    #[test]
    fn path_of_keeps_paths_without_separators() {
        assert_eq!(path_of("/"), "/");
        assert_eq!(path_of("/blog/a"), "/blog/a");
    }

    #[test]
    fn path_of_pins_the_empty_input_result() {
        // `split` always yields at least one element, so the empty input
        // yields the empty path — verbatim spec behaviour.
        assert_eq!(path_of(""), "");
    }

    #[test]
    fn data_route_defaults_to_root() {
        assert_eq!(data_route_of("GET", "/__data", "/__data"), Some("/".into()));
    }

    #[test]
    fn data_route_decodes() {
        assert_eq!(
            data_route_of("GET", "/__data", "/__data?route=%2Fblog%2Fa"),
            Some("/blog/a".into())
        );
    }

    #[test]
    fn data_route_only_get() {
        assert_eq!(data_route_of("POST", "/__data", "/__data"), None);
        // The closure folds method case the same way.
        assert_eq!(data_route_of("get", "/__data", "/__data"), Some("/".into()));
    }

    #[test]
    fn data_route_requires_the_data_path() {
        // A `route=` parameter on an ordinary page request is not a data
        // request — the path must be exactly `/__data`.
        assert_eq!(
            data_route_of("GET", "/blog/a", "/blog/a?route=%2Fblog%2Fa"),
            None
        );
        assert_eq!(
            data_route_of("GET", "/__datax", "/__datax?route=%2Fa"),
            None
        );
    }

    #[test]
    fn data_route_first_param_wins() {
        assert_eq!(
            data_route_of("GET", "/__data", "/__data?a=1&route=%2Fa&route=%2Fb"),
            Some("/a".into())
        );
    }

    #[test]
    fn data_route_empty_value_falls_back_to_root() {
        assert_eq!(
            data_route_of("GET", "/__data", "/__data?route="),
            Some("/".into())
        );
    }

    #[test]
    fn data_route_prefix_must_be_exact() {
        // `xroute=` does not carry the `route=` prefix, so the default
        // applies — same as the closure's `strip_prefix("route=")` scan.
        assert_eq!(
            data_route_of("GET", "/__data", "/__data?xroute=%2Fa"),
            Some("/".into())
        );
    }

    #[test]
    fn parse_form_urlencoded_parses_pairs_in_order() {
        let body = b"name=elmo&color=red";
        let pairs = parse_form_urlencoded(body);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("name".to_string(), "elmo".to_string()));
        assert_eq!(pairs[1], ("color".to_string(), "red".to_string()));
    }

    #[test]
    fn parse_form_urlencoded_decodes_plus_as_space_and_escapes() {
        let pairs = parse_form_urlencoded(b"q=a+b%21&tag=caf%C3%A9");
        assert_eq!(pairs[0], ("q".to_string(), "a b!".to_string()));
        assert_eq!(pairs[1], ("tag".to_string(), "café".to_string()));
    }

    #[test]
    fn parse_form_urlencoded_skips_empty_and_bare_segments() {
        // A malformed pair is skipped, not fatal (§9.3): empty segments are
        // dropped and a valueless key keeps the empty-string value.
        let pairs = parse_form_urlencoded(b"&&a=1&&flag&b=2");
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[1], ("flag".to_string(), String::new()));
    }

    #[test]
    fn parse_form_urlencoded_empty_body_yields_no_pairs() {
        assert_eq!(parse_form_urlencoded(b"").len(), 0);
    }

    #[test]
    fn action_from_query_reads_the_sveltekit_convention() {
        assert_eq!(
            action_from_query("/comment?/setcookie"),
            Some("setcookie".into())
        );
    }

    #[test]
    fn action_from_query_rejects_non_action_queries() {
        // No query at all.
        assert_eq!(action_from_query("/comment"), None);
        // A plain `key=value` query does not name an action.
        assert_eq!(action_from_query("/comment?id=7"), None);
        // A bare `?/` has an empty action name.
        assert_eq!(action_from_query("/comment?/"), None);
        // Nested paths are not action names.
        assert_eq!(action_from_query("/comment?/a/b"), None);
    }

    #[test]
    fn action_from_query_strips_fragments_and_decodes() {
        assert_eq!(
            action_from_query("/comment?/set%20cookie#frag"),
            Some("set cookie".into())
        );
    }

    #[test]
    fn rewrite_hop_budget_is_pinned() {
        // The phase-3 rewrite guard consumes this constant from both sides
        // (CLI prelude and dispatch stages); the pin makes a silent change
        // visible in both crates' suites.
        assert_eq!(MAX_REWRITE_HOPS, 8);
    }
}

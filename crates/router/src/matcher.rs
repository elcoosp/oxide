//! Matcher (spec §8.2): priority static > param > optional > catch-all, percent-decoded
//! params, layout chains, and i18n-prefix behavior (§16.4).

use std::collections::{BTreeMap, HashMap};
use std::sync::{OnceLock, RwLock};

use regex::Regex;

use crate::segment::{SegKind, classify, seg_priority, split_path};
use crate::{MatchResult, RouteEntry, RouteId, RouteKind, RouteTree};

/// Uniform read-only view over owned [`RouteEntry`] and the rkyv-archived entry type,
/// so one matcher serves both the in-memory tree and the memory-mapped archive (§8.2).
pub(crate) trait EntryLike {
    fn id_str(&self) -> &str;
    fn kind(&self) -> RouteKind;
    fn seg_len(&self) -> usize;
    fn seg(&self, i: usize) -> &str;
    fn slot(&self) -> Option<&str>;

    /// Only pages without a slot marker and endpoints are URL-addressable (§8.5).
    fn matchable(&self) -> bool {
        (matches!(self.kind(), RouteKind::Page) && self.slot().is_none())
            || matches!(self.kind(), RouteKind::Endpoint)
    }
}

impl EntryLike for RouteEntry {
    fn id_str(&self) -> &str {
        &self.id.0
    }
    fn kind(&self) -> RouteKind {
        self.kind.clone()
    }
    fn seg_len(&self) -> usize {
        self.segments.len()
    }
    fn seg(&self, i: usize) -> &str {
        &self.segments[i]
    }
    fn slot(&self) -> Option<&str> {
        self.slot.as_deref()
    }
}

pub(crate) fn match_impl(tree: &RouteTree, path: &str) -> Option<MatchResult> {
    let (idx, params, score) = match_in(&tree.entries, path)?;
    let route = &tree.entries[idx];
    Some(MatchResult {
        route: route.clone(),
        layout_chain: layout_chain(&tree.entries, &route.id.0),
        params,
        score,
    })
}

/// Best match over all entries: score (higher wins) is summed per matched URL segment
/// (static=4, param=2, optional=1, catch-all=0); ties break on per-position pattern
/// priority, then on longer patterns, then on first-in-sorted-order (deterministic).
pub(crate) fn match_in<E: EntryLike>(
    entries: &[E],
    path: &str,
) -> Option<(usize, BTreeMap<String, String>, i64)> {
    let url = split_path(path);
    let mut best: Option<(usize, i64, Vec<u8>, usize, BTreeMap<String, String>)> = None;
    for (i, e) in entries.iter().enumerate() {
        if !e.matchable() {
            continue;
        }
        let mut acc: Vec<(String, String)> = Vec::new();
        if let Some(score) = match_entry(e, 0, 0, &url, &mut acc) {
            let prios: Vec<u8> = (0..e.seg_len()).map(|k| seg_priority(e.seg(k))).collect();
            let better = match &best {
                None => true,
                Some((_, b_score, b_prios, b_len, _)) => {
                    score > *b_score
                        || (score == *b_score
                            && (prios < *b_prios || (prios == *b_prios && e.seg_len() > *b_len)))
                }
            };
            if better {
                let params = acc.into_iter().collect();
                best = Some((i, score, prios, e.seg_len(), params));
            }
        }
    }
    best.map(|(i, score, _, _, params)| (i, params, score))
}

/// Recursive segment matcher with backtracking. `acc` holds captured params; it is
/// fully unwound on failure, so only successful paths leave captures behind. Score
/// accumulates on the way out, so failed branches never pollute it.
fn match_entry<E: EntryLike>(
    e: &E,
    pi: usize,
    ui: usize,
    url: &[String],
    acc: &mut Vec<(String, String)>,
) -> Option<i64> {
    if pi == e.seg_len() {
        return if ui == url.len() { Some(0) } else { None };
    }
    match classify(e.seg(pi)) {
        SegKind::Static(s) => {
            if ui < url.len() && url[ui] == s {
                match_entry(e, pi + 1, ui + 1, url, acc).map(|rest| rest + 4)
            } else {
                None
            }
        }
        SegKind::Param { name, constraint } => {
            if ui < url.len() && constraint_ok(constraint, &url[ui]) {
                acc.push((name.to_string(), url[ui].clone()));
                let r = match_entry(e, pi + 1, ui + 1, url, acc).map(|rest| rest + 2);
                if r.is_none() {
                    acc.pop();
                }
                r
            } else {
                None
            }
        }
        SegKind::CatchAll(name) => {
            // Greedy: longest capture first.
            for k in (ui + 1..=url.len()).rev() {
                acc.push((name.to_string(), url[ui..k].join("/")));
                if let Some(rest) = match_entry(e, pi + 1, k, url, acc) {
                    return Some(rest);
                }
                acc.pop();
            }
            None
        }
        SegKind::OptionalCatchAll(name) => {
            for k in (ui..=url.len()).rev() {
                acc.push((name.to_string(), url[ui..k].join("/")));
                if let Some(rest) = match_entry(e, pi + 1, k, url, acc) {
                    return Some(rest + (k - ui) as i64);
                }
                acc.pop();
            }
            None
        }
    }
}

/// Layout chain ids for a route, outermost first (§8.1 nesting).
///
/// Route ids encode the module directory (scanner: `routes/blog/[slug]/page`), so a
/// layout guards every route whose id lives at or under the layout's own directory:
/// layout id `routes/blog/[slug]/layout` → base `routes/blog/[slug]`; the root layout
/// `routes/layout` → base `` (wraps everything). A layout is in the chain of route id
/// `R` iff its base is empty or `R == base + "/…"` (i.e. `R` starts with the base at a
/// path boundary, or equals it). This works identically for owned and archived
/// entries and mirrors directory containment for scanner-produced ids.
pub(crate) fn layout_chain<E: EntryLike>(entries: &[E], route_id: &str) -> Vec<RouteId> {
    let mut chain: Vec<(usize, String)> = Vec::new();
    for e in entries {
        if !matches!(e.kind(), RouteKind::Layout) {
            continue;
        }
        let id = e.id_str();
        let base = id.strip_suffix("/layout").unwrap_or("");
        let rest = route_id.strip_prefix(base);
        if base.is_empty() || rest.is_some_and(|r| r.is_empty() || r.starts_with('/')) {
            chain.push((base.len(), id.to_string()));
        }
    }
    // Outermost (shortest base) first; ties (two layouts at the same level) break on
    // id for determinism.
    chain.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    chain.into_iter().map(|(_, id)| RouteId(id)).collect()
}

/// Compile-on-demand cache for param constraint regexes (full-match semantics).
fn regex_for(pattern: &str) -> Option<Regex> {
    static CACHE: OnceLock<RwLock<HashMap<String, Regex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| RwLock::new(HashMap::new()));
    let full = format!("^(?:{pattern})$");
    if let Ok(guard) = cache.read() {
        if let Some(re) = guard.get(&full) {
            return Some(re.clone());
        }
    }
    let re = Regex::new(&full).ok()?;
    if let Ok(mut guard) = cache.write() {
        guard.insert(full, re.clone());
    }
    Some(re)
}

fn constraint_ok(constraint: Option<&str>, value: &str) -> bool {
    match constraint {
        None => true,
        Some(pattern) => regex_for(pattern).is_some_and(|re| re.is_match(value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RouteId;

    /// Build an entry for matcher tests. The module `file` is derived from the id
    /// (mirroring the scanner: relative to the routes root) so layout-chain logic is
    /// exercised exactly as in production.
    fn entry(id: &str, kind: RouteKind, segments: &[&str]) -> RouteEntry {
        RouteEntry {
            id: RouteId(id.to_string()),
            kind,
            segments: segments.iter().map(|s| s.to_string()).collect(),
            file: format!("{id}.tsx").into(),
            render_mode: None,
            tags: Vec::new(),
            has_server_module: false,
            slot: None,
            needs_parent: false,
        }
    }

    fn page(id: &str, segments: &[&str]) -> RouteEntry {
        entry(id, RouteKind::Page, segments)
    }

    fn endpoint(id: &str, segments: &[&str]) -> RouteEntry {
        entry(id, RouteKind::Endpoint, segments)
    }

    fn layout(id: &str) -> RouteEntry {
        entry(id, RouteKind::Layout, &[])
    }

    fn tree(entries: Vec<RouteEntry>) -> RouteTree {
        let mut t = RouteTree::new();
        for e in entries {
            t.insert(e);
        }
        t
    }

    #[test]
    fn static_beats_param_beats_optional_beats_catchall() {
        let t = tree(vec![
            page("blog/:slug", &["blog", ":slug"]),
            page("*rest", &["*rest"]),
            page("*?opt", &["*?opt"]),
        ]);
        let m = t.match_path("/blog/new").unwrap();
        assert_eq!(m.route.id.0, "blog/:slug", "param beats optional catch-all");
        assert_eq!(m.score, 6, "4 for static blog + 2 for param");
        let m = t.match_path("/other").unwrap();
        assert_eq!(m.route.id.0, "*?opt", "optional catch-all beats catch-all");
        assert_eq!(m.score, 1);
        let m = t.match_path("/x/y/z").unwrap();
        assert_eq!(
            m.route.id.0, "*?opt",
            "optional catch-all keeps priority over catch-all at any depth (§8.2)"
        );
        assert_eq!(m.params.get("opt").map(String::as_str), Some("x/y/z"));
    }

    #[test]
    fn two_static_segments_beat_static_plus_param() {
        let t = tree(vec![
            page("blog/:slug", &["blog", ":slug"]),
            page("blog/new", &["blog", "new"]),
        ]);
        let m = t.match_path("/blog/new").unwrap();
        assert_eq!(
            m.route.id.0, "blog/new",
            "/blog/new must match the static route"
        );
        assert_eq!(m.score, 8);
        let m = t.match_path("/blog/other").unwrap();
        assert_eq!(m.route.id.0, "blog/:slug");
        assert_eq!(m.params.get("slug").map(String::as_str), Some("other"));
    }

    #[test]
    fn param_constraints_use_regex_full_match() {
        let t = tree(vec![page("digits/:n{\\d+}", &["digits", ":n{\\d+}"])]);
        let m = t.match_path("/digits/123").unwrap();
        assert_eq!(m.params.get("n").map(String::as_str), Some("123"));
        assert!(
            t.match_path("/digits/12a").is_none(),
            "constraint must reject non-digits"
        );
        assert!(t.match_path("/digits/x").is_none());
        assert!(t.match_path("/digits/").is_none(), "param needs a segment");
    }

    #[test]
    fn catchall_joins_multiple_segments_and_needs_one() {
        let t = tree(vec![page("docs/*rest", &["docs", "*rest"])]);
        assert!(
            t.match_path("/docs").is_none(),
            "catch-all requires >= 1 segment"
        );
        let m = t.match_path("/docs/a/b/c").unwrap();
        assert_eq!(m.params.get("rest").map(String::as_str), Some("a/b/c"));
        assert_eq!(m.score, 4, "static docs contributes 4, catch-all 0");
    }

    #[test]
    fn optional_catchall_matches_zero_or_more() {
        let t = tree(vec![page("*?path", &["*?path"])]);
        let m = t.match_path("/").unwrap();
        assert_eq!(m.params.get("path").map(String::as_str), Some(""));
        assert_eq!(m.score, 0, "zero consumed segments score 0");
        let m = t.match_path("/a/b").unwrap();
        assert_eq!(m.params.get("path").map(String::as_str), Some("a/b"));
        assert_eq!(m.score, 2, "1 per consumed segment");
    }

    #[test]
    fn trailing_slash_query_and_double_slashes_are_tolerated() {
        let t = tree(vec![page("blog/:slug", &["blog", ":slug"])]);
        let a = t.match_path("/blog/x");
        let b = t.match_path("/blog/x/");
        let c = t.match_path("/blog/x?utm=1");
        let d = t.match_path("//blog//x");
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(a, d);
        assert!(a.is_some());
    }

    #[test]
    fn params_are_percent_decoded() {
        let t = tree(vec![
            page("blog/:slug", &["blog", ":slug"]),
            page("my dir", &["my dir"]),
        ]);
        let m = t.match_path("/blog/hello%20world").unwrap();
        assert_eq!(
            m.params.get("slug").map(String::as_str),
            Some("hello world")
        );
        let m = t.match_path("/blog/a%2Fb").unwrap();
        assert_eq!(
            m.params.get("slug").map(String::as_str),
            Some("a/b"),
            "%2F decodes inside params"
        );
        assert!(
            t.match_path("/my%20dir").is_some(),
            "decoded static segments match"
        );
    }

    #[test]
    fn i18n_prefixed_static_route_wins_over_param_fallback() {
        let t = tree(vec![
            page("blog", &["blog"]),
            page("en/blog", &["en", "blog"]),
            page(":locale/about", &[":locale", "about"]),
        ]);
        // §16.4: /en/blog exists both with and without prefix; the prefixed exact
        // (static) match wins over treating `en` as a `[locale]` param.
        let m = t.match_path("/en/blog").unwrap();
        assert_eq!(m.route.id.0, "en/blog");
        let m = t.match_path("/en/about").unwrap();
        assert_eq!(m.route.id.0, ":locale/about");
        assert_eq!(m.params.get("locale").map(String::as_str), Some("en"));
        let m = t.match_path("/blog").unwrap();
        assert_eq!(m.route.id.0, "blog");
    }

    #[test]
    fn root_matches_and_unknown_is_none() {
        let t = tree(vec![page("page", &[])]);
        assert!(t.match_path("/").is_some());
        assert!(t.match_path("").is_some(), "empty path is the root");
        assert!(t.match_path("/nope").is_none());
        assert!(t.match_path("/a/b").is_none());
    }

    #[test]
    fn endpoints_match_but_slots_and_boundaries_do_not() {
        let mut t = tree(vec![
            endpoint("api/users", &["api", "users"]),
            layout("layout"),
        ]);
        let mut slot_page = page("@modal/page", &[]);
        slot_page.slot = Some("@modal".to_string());
        t.insert(slot_page);
        let m = t.match_path("/api/users").unwrap();
        assert_eq!(m.route.id.0, "api/users");
        assert!(
            t.match_path("/layout").is_none(),
            "layouts are not match targets"
        );
        assert!(
            t.match_path("/@modal/page").is_none(),
            "slot pages are not URL-addressable"
        );
    }

    #[test]
    fn layout_chain_runs_root_to_leaf() {
        let t = tree(vec![
            layout("layout"),
            layout("blog/layout"),
            layout("blog/[slug]/layout"),
            page("blog/[slug]", &["blog", ":slug"]),
        ]);
        let m = t.match_path("/blog/x").unwrap();
        let ids: Vec<&str> = m.layout_chain.iter().map(|i| i.0.as_str()).collect();
        assert_eq!(ids, vec!["layout", "blog/layout", "blog/[slug]/layout"]);
        // A page without nearby layouts still gets the outer chain.
        let t2 = tree(vec![layout("layout"), page("page", &[])]);
        let m2 = t2.match_path("/").unwrap();
        assert_eq!(m2.layout_chain, vec![RouteId("layout".to_string())]);
    }

    #[test]
    fn matcher_never_panics_on_degenerate_inputs() {
        let t = tree(vec![
            page("*rest", &["*rest"]),
            page("*?opt", &["*?opt"]),
            page(":a/:b", &[":a", ":b"]),
            page("a/*rest", &["a", "*rest"]),
        ]);
        for path in ["", "/", "///", "/%", "%", "/a/b/c/d/e", "/a%2Fb/%20"] {
            let _ = t.match_path(path);
        }
    }

    #[test]
    fn table_rows_are_sorted_triples() {
        let mut t = tree(vec![
            page("blog/:slug", &["blog", ":slug"]),
            endpoint("api/users", &["api", "users"]),
            layout("layout"),
        ]);
        t.entries[0].tags = vec!["t1".to_string()];
        let rows = t.table();
        assert_eq!(
            rows,
            vec![
                ("/".to_string(), "layout".to_string(), vec![]),
                ("/api/users".to_string(), "endpoint".to_string(), vec![]),
                (
                    "/blog/:slug".to_string(),
                    "page".to_string(),
                    vec!["t1".to_string()]
                ),
            ]
        );
    }
}

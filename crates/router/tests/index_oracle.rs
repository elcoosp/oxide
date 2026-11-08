//! V2-B02 oracle property test (spec: "random route sets … `match_ref` result
//! equals `match_in_linear` (same entry index, same params, same score)").
//!
//! The candidate-index matcher must be behaviorally identical to the original
//! linear matcher (D23: correctness by construction — the old matcher stays as
//! the oracle). Random route sets over a mixed segment alphabet, random URLs.

use std::collections::BTreeMap;

use oxide_fw_router::{RouteEntry, RouteId, RouteKind, RouteTree, match_in_linear};
use proptest::prelude::*;

/// Encoded pattern segments (the form the scanner writes into
/// `RouteEntry.segments`): statics, a plain param, a constrained param, a
/// catch-all, an optional catch-all.
const SEGMENTS: [&str; 5] = ["a", "b", ":x", ":n{[0-9]+}", "*r"];

fn entry(id: &str, segments: &[&str]) -> RouteEntry {
    RouteEntry {
        id: RouteId(id.to_string()),
        kind: RouteKind::Page,
        segments: segments.iter().map(|s| s.to_string()).collect(),
        file: format!("{id}.tsx").into(),
        render_mode: None,
        tags: Vec::new(),
        has_server_module: false,
        needs_parent: false,
        slot: None,
    }
}

/// A random route set: up to 30 routes, depth up to 4, segments from the
/// alphabet. Ids are derived from the segments plus an index suffix so ids
/// stay unique even when two routes carry the same pattern.
fn route_strategy() -> impl Strategy<Value = Vec<RouteEntry>> {
    proptest::collection::vec(
        proptest::collection::vec(proptest::sample::select(SEGMENTS.to_vec()), 0..=4),
        0..=30,
    )
    .prop_map(|patterns| {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for (i, segs) in patterns.into_iter().enumerate() {
            let id = format!("r{i}/{}", segs.join("/"));
            if seen.insert(id.clone()) {
                out.push(entry(&id, &segs));
            }
        }
        out
    })
}

/// Random URL: 0 to 5 raw segments over a small alphabet that overlaps the
/// route alphabet (hits) and includes junk (misses), plus percent-escapes.
fn url_strategy() -> impl Strategy<Value = String> {
    (
        proptest::collection::vec(
            proptest::sample::select(vec![
                "a".to_string(),
                "b".to_string(),
                "9".to_string(),
                "zz".to_string(),
                "a%20b".to_string(),
            ]),
            0..=5,
        ),
        proptest::bool::ANY,
    )
        .prop_map(|(segs, trailing)| {
            let joined = segs.join("/");
            if trailing && !joined.is_empty() {
                format!("/{joined}/")
            } else {
                format!("/{joined}")
            }
        })
}

fn params_of(
    params: &[(std::borrow::Cow<'_, str>, std::borrow::Cow<'_, str>)],
) -> BTreeMap<String, String> {
    params
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 10_000,
        max_shrink_iters: 2048,
        ..proptest::test_runner::Config::default()
    })]

    #[test]
    fn match_ref_equals_linear_oracle(routes in route_strategy(), url in url_strategy()) {
        let mut tree = RouteTree::new();
        for e in &routes {
            tree.insert(e.clone());
        }
        let oracle = match_in_linear(&routes, &url);
        let m = tree.match_ref(&url);
        prop_assert!(
            oracle.is_some() == m.is_some(),
            "presence differs for {url:?}"
        );
        if let (Some((_, o_params, o_score)), Some(m)) = (oracle, m) {
            prop_assert!(o_score == m.score, "score differs for {url:?}");
            prop_assert!(
                o_params == params_of(&m.params),
                "params differ for {url:?}"
            );
        }
    }
}

// Same oracle for the archived tree: the mmap accessor must behave exactly
// like the linear oracle on unmutated archives.
proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 2_000,
        max_shrink_iters: 1024,
        ..proptest::test_runner::Config::default()
    })]

    #[test]
    fn archived_match_ref_equals_linear_oracle(routes in route_strategy(), url in url_strategy()) {
        let tmp = tempfile::tempdir().unwrap();
        let mut tree = RouteTree::new();
        for e in &routes {
            tree.insert(e.clone());
        }
        let manifest = tmp.path().join("routes.manifest");
        tree.archive_to(&manifest).unwrap();
        let archived = RouteTree::load_archived(&manifest).unwrap();

        let live = tree.match_path(&url);
        let arch = archived.match_path(&url);
        prop_assert!(live == arch, "archived match diverges for {url:?}");
    }
}

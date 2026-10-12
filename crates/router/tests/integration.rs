//! Cross-cutting integration tests for oxide-router (spec §8, D8): full fixture route
//! trees, archive roundtrips through a live mmap, typegen artifacts, and proptests.

use std::fs;
use std::path::Path;

use oxide_router::{
    ArchivedTree, MatchResult, RateLimit, RouteEntry, RouteId, RouteKind, RouteTree, RuleOutcome,
    RulesEngine,
};
use proptest::prelude::*;

/// Write a file (creating parent dirs) inside a fixture.
fn put(root: &Path, rel: &str, contents: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, contents).unwrap();
}

/// A 3-level nested app: root layout -> blog layout -> [slug] layout -> page,
/// with a page.server.ts carrying config (§10.1 explicit render, cache tags).
fn fixture_three_levels(root: &Path) {
    put(root, "src/routes/layout.tsx", "export default null;");
    put(root, "src/routes/blog/layout.tsx", "export default null;");
    put(
        root,
        "src/routes/blog/[slug]/layout.tsx",
        "export default null;",
    );
    put(
        root,
        "src/routes/blog/[slug]/page.tsx",
        "export default null;",
    );
    put(
        root,
        "src/routes/blog/[slug]/page.server.ts",
        "export const config = { render: 'dynamic', tags: ['post:rust'] };",
    );
}

#[test]
fn scan_match_layout_chain_and_server_module_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_three_levels(tmp.path());
    let tree = RouteTree::scan_with_config(tmp.path(), None).unwrap();

    let m = tree.match_path("/blog/rust").expect("must match");
    assert_eq!(m.route.id.0, "routes/blog/[slug]/page");
    assert_eq!(m.params.get("slug").map(String::as_str), Some("rust"));
    let ids: Vec<&str> = m.layout_chain.iter().map(|i| i.0.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "routes/layout",
            "routes/blog/layout",
            "routes/blog/[slug]/layout"
        ],
        "outermost layout first, 3 levels"
    );
    assert!(m.route.has_server_module);
    assert_eq!(m.route.render_mode.as_deref(), Some("dynamic"));
    assert_eq!(m.route.tags, vec!["post:rust".to_string()]);

    // Trailing slash, query strings and percent-encoding are all tolerated.
    let m2 = tree.match_path("/blog/rust/").unwrap();
    let m3 = tree.match_path("/blog/rust?utm=1").unwrap();
    let m4 = tree.match_path("/blog/caf%C3%A9").unwrap();
    assert_eq!(m.params, m2.params);
    assert_eq!(m.params, m3.params);
    assert_eq!(m4.params.get("slug").map(String::as_str), Some("café"));
    assert!(tree.match_path("/nope").is_none());
}

#[test]
fn archive_roundtrip_matches_identically_through_mmap() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_three_levels(tmp.path());
    let tree = RouteTree::scan_with_config(tmp.path(), None).unwrap();

    let manifest = tmp.path().join(".oxide/routes.manifest");
    tree.archive_to(&manifest).unwrap();
    let archived = ArchivedTree::load_archived(&manifest).unwrap();

    assert_eq!(archived.len(), tree.entries.len());
    assert!(!archived.is_empty());
    for path in ["/", "/blog", "/blog/rust", "/blog/a/b/c", "/missing"] {
        let expected: Option<MatchResult> = tree.match_path(path);
        assert_eq!(expected, archived.match_path(path), "divergence for {path}");
    }
    // Full MatchResult equality includes the layout chain and params.
    assert_eq!(tree.match_path("/blog/x"), archived.match_path("/blog/x"));
}

#[test]
fn typegen_artifacts_from_a_scanned_tree() {
    let tmp = tempfile::tempdir().unwrap();
    fixture_three_levels(tmp.path());
    put(tmp.path(), "src/routes/docs/[...rest]/page.tsx", "p");
    let tree = RouteTree::scan_with_config(tmp.path(), None).unwrap();
    let dts_path = tree.typegen(tmp.path()).unwrap();
    let dts = fs::read_to_string(&dts_path).unwrap();
    assert!(dts.contains("`/blog/${string}`"));
    assert!(dts.contains("`/docs/${string}`"));
    let json = fs::read_to_string(tmp.path().join(".oxide/types/routes.json")).unwrap();
    assert!(json.contains("/docs/*rest"));
    // `oxide routes` table is deterministic too.
    let table = tree.table();
    assert_eq!(table.len(), tree.entries.len());
    let mut sorted = table.clone();
    sorted.sort();
    assert_eq!(table, sorted);
}

#[test]
fn rules_engine_pipeline_position_smoke() {
    // §12.1 step 4: native rules evaluated before route matching; the token-bucket
    // decision itself lives in the server crate (§17.1).
    let mut redirect = RulesEngine::compile(vec![oxide_router::NativeRule {
        source: "/old/:slug".to_string(),
        destination: Some("/new/:slug".to_string()),
        status: Some(301),
        rule_type: None,
        headers: None,
        basic_auth: None,
        rate_limit: None,
    }])
    .unwrap();
    redirect.rules.push(oxide_router::NativeRule {
        source: "/api/*".to_string(),
        destination: None,
        status: None,
        rule_type: Some("ratelimit".to_string()),
        headers: None,
        basic_auth: None,
        rate_limit: Some(RateLimit {
            key: "ip".into(),
            max: 10,
            window_secs: 60,
        }),
    });
    assert_eq!(
        redirect.evaluate("/old/docs"),
        RuleOutcome::Redirect {
            destination: "/new/docs".to_string(),
            status: 301
        }
    );
    assert_eq!(redirect.evaluate("/api/x"), RuleOutcome::RateLimited);
    assert_eq!(redirect.evaluate("/nothing"), RuleOutcome::None);
}

/// Minimal percent-decode mirroring the matcher's segment decoding (`segment.rs`),
/// so generated raw segments can be compared against decoded params (no deps).
fn decode_seg(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

proptest! {
    /// The matcher never panics on arbitrary paths built from a nasty segment alphabet.
    #[test]
    fn match_never_panics_on_arbitrary_paths(
        segs in prop::collection::vec(
            prop::sample::select(vec![
                "a", "b", "blog", "%20", "a%2Fb", "ünïcode", "", "..", "%", "new",
            ]),
            0..8,
        ),
    ) {
        let mut tree = RouteTree::new();
        for (id, segs) in [
            ("static", vec!["a".to_string()]),
            ("two-static", vec!["blog".to_string(), "new".to_string()]),
            ("param", vec!["blog".to_string(), ":slug".to_string()]),
            ("constraint", vec!["digits".to_string(), ":n{\\d+}".to_string()]),
            ("catchall", vec!["*rest".to_string()]),
            ("optional", vec!["*?opt".to_string()]),
            ("deep", vec!["a".to_string(), "*rest".to_string(), "b".to_string()]),
        ] {
            tree.insert(RouteEntry {
                id: RouteId(id.to_string()),
                kind: RouteKind::Page,
                segments: segs,
                file: format!("{id}.tsx").into(),
                render_mode: None,
                tags: Vec::new(),
                has_server_module: false,
                slot: None,
            });
        }
        let path = if segs.is_empty() { "/".to_string() } else { format!("/{}", segs.join("/")) };
        let _ = tree.match_path(&path);
        let _ = tree.match_path(&format!("{path}?q=%zz"));
    }

    /// A catch-all-only tree matches every non-empty path, with `rest` equal to the
    /// decoded, slash-joined segments.
    #[test]
    fn catchall_only_tree_matches_everything_non_empty(
        segs in prop::collection::vec(
            prop::sample::select(vec!["x", "y", "%41", "z%20z", "w/1", "héllo"]),
            1..6,
        ),
    ) {
        let mut tree = RouteTree::new();
        tree.insert(RouteEntry {
            id: RouteId("catchall".to_string()),
            kind: RouteKind::Page,
            segments: vec!["*rest".to_string()],
            file: "catchall.tsx".into(),
            render_mode: None,
            tags: Vec::new(),
            has_server_module: false,
            slot: None,
        });
        let path = format!("/{}", segs.join("/"));
        let m = tree
            .match_path(&path)
            .unwrap_or_else(|| panic!("catch-all must match {path}"));
        // `rest` is the decoded, slash-joined form of the URL segments.
        let decoded: Vec<String> = segs.iter().map(|s| decode_seg(s)).collect();
        assert_eq!(
            m.params.get("rest").map(String::as_str),
            Some(decoded.join("/").as_str())
        );
        // …but the empty path does not match a one-or-more catch-all.
        assert!(tree.match_path("/").is_none());
    }
}

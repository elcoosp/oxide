//! The assets crate's cross-module integration pins (the placeholder
//! `mod placeholder {}` stub, filled): the flows the build rides that
//! span more than one module — the prerendered HTML becoming the client
//! route-search index, the SVG sprite's referenced ids surviving the
//! build-side optimization, and a scanned collection's broken-reference
//! diagnostics.
//!
//! The crate's unit suites pin each module's pieces
//! (`content_collection_tdd.rs`/`svg_optimizer_tdd.rs`/…); these pins
//! hold the MODULES TOGETHER: the extracted route entries are the rows
//! the JSON index serializes, the ids the route-search scan collects are
//! the ones the optimizer's preservation rule refuses to lose, and the
//! collection the scanner built feeds the reference checker's targets.

use std::collections::BTreeMap;
use std::path::Path;

use oxide_fw_assets::{
    CollectionConfig, RouteSearchIndex, SchemaKind, assert_preserved,
    build_route_search_index, check_references, extract_from_html, optimize_for_build,
    referenced_svg_ids, route_search_index_to_json, scan_collection, select_optimizer,
};

/// A prerendered dist tree: two routes, one with a per-locale variant,
/// written as real HTML files the index builder reads back.
fn dist_with_two_routes(root: &Path) -> BTreeMap<String, Vec<String>> {
    let prerendered = root.join("prerendered");
    std::fs::create_dir_all(&prerendered).expect("the prerendered dir creates");
    let write = |rel: &str, title: &str, body: &str| {
        let path = prerendered.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the route dir creates");
        }
        std::fs::write(
            &path,
            format!("<!doctype html><html><head><title>{title}</title></head><body><h1>{title}</h1><p>{body}</p></body></html>"),
        )
        .expect("the prerendered page writes");
    };
    write("index.html", "Home", "the index page copy");
    write("blog/index.html", "Blog", "the blog landing copy");
    write("blog/fr/index.html", "Blog", "le blog copie");
    BTreeMap::from([
        (
            "routes/index".to_string(),
            vec!["prerendered/index.html".to_string()],
        ),
        (
            "routes/blog".to_string(),
            vec![
                "prerendered/blog/index.html".to_string(),
                "prerendered/blog/fr/index.html".to_string(),
            ],
        ),
    ])
}

/// The prerendered HTML → the client's JSON index: every page the build
/// wrote becomes one search entry (one per locale variant), the JSON
/// round-trips through serde, and the index stays byte-stable across a
/// second build of the same tree.
#[test]
fn prerendered_html_becomes_a_json_route_index() {
    let dist = tempfile::tempdir().expect("the dist tempdir");
    let prerendered = dist_with_two_routes(dist.path());

    let idx = build_route_search_index(&prerendered, dist.path());
    assert_eq!(idx.entries.len(), 3, "one entry per prerendered output");
    assert_eq!(idx.entries[0].title, "Home", "the <title> rides the entry");
    assert!(
        idx.entries.iter().any(|e| e.url.contains("fr")),
        "the per-locale variant is its own entry: {:?}",
        idx.entries.iter().map(|e| &e.url).collect::<Vec<_>>()
    );

    // The JSON the build writes is what the client parses back.
    let json = route_search_index_to_json(&idx).expect("the index serializes");
    let back: RouteSearchIndex =
        serde_json::from_str(&json).expect("the client parses the index back");
    assert_eq!(back.entries.len(), idx.entries.len());
    assert_eq!(back.entries[0].route, idx.entries[0].route);

    // A second build over the same tree renders the same bytes — the
    // determinism the client-side cache relies on.
    let idx_again = build_route_search_index(&prerendered, dist.path());
    assert_eq!(
        route_search_index_to_json(&idx).expect("first serialize"),
        route_search_index_to_json(&idx_again).expect("second serialize"),
        "the index is byte-stable across builds"
    );

    // The extractor is the same shape the build calls directly on a
    // document it never wrote (the live-render path).
    let hand = extract_from_html(
        "routes/manual",
        "/manual",
        "<html><head><title>M</title></head><body><h2>Section</h2><p>text</p></body></html>",
    );
    assert_eq!(hand.title, "M");
}

/// The SVG sprite pipeline: the ids the scan collects (`<use href="#…">`,
/// `url(#…)`, the aria spellings) are exactly the ids the optimizer's
/// preservation rule refuses to lose — through the default build shape
/// (no `svg-oxvg` backend, the identity optimizer) the sprite passes
/// through with every referenced id intact and the preservation
/// assertion green.
#[test]
fn the_svg_pipeline_preserves_the_referenced_ids() {
    let sprite = r##"<svg xmlns="http://www.w3.org/2000/svg">
  <defs>
    <linearGradient id="grad"><stop offset="0" stop-color="red"/></linearGradient>
    <symbol id="shape" viewBox="0 0 8 8"><path d="M0 0h8v8z"/></symbol>
  </defs>
  <use href="#shape"/>
  <circle fill="url(#grad)" aria-labelledby="cap" r="4"/>
  <desc id="cap">a caption</desc>
</svg>"##;

    let before = referenced_svg_ids(sprite);
    assert!(before.contains("shape"), "the use href is collected");
    assert!(before.contains("grad"), "the paint-server spelling is collected");
    assert!(before.contains("cap"), "the aria spelling is collected");

    // The default build shape: no `svg-oxvg` backend configured, the
    // identity optimizer passes the sprite through unchanged.
    let optimized = optimize_for_build(sprite, false);
    assert_preserved(sprite, &optimized).expect("nothing was lost");
    assert_eq!(
        referenced_svg_ids(&optimized),
        before,
        "the ids survive the build"
    );

    // The optimizer-selection seam answers for both build shapes — the
    // disabled arm is the identity, wired without the optional backend.
    let optimizer = select_optimizer(false).expect("the identity selects");
    assert_eq!(optimizer.id(), "identity", "the disabled arm is the identity");
}

/// A scanned collection feeds the reference checker: the entries the
/// scanner built are the known-target set `check_references` tests
/// against — a broken `[[link]]` is flagged with the target named, a
/// valid link passes silently.
#[test]
fn a_scanned_collection_flags_broken_references() {
    let dir = tempfile::tempdir().expect("the collection tempdir");
    std::fs::write(
        dir.path().join("hello.md"),
        "---\ntitle: Hello\n---\nA link to [good](world) and a [broken](missing) one.\n",
    )
    .expect("the entry writes");
    std::fs::write(
        dir.path().join("world.md"),
        "---\ntitle: World\n---\nBody copy.\n",
    )
    .expect("the target writes");

    let cfg = CollectionConfig {
        name: "docs".into(),
        dir: dir.path().to_path_buf(),
        schema: SchemaKind::Unchecked,
        indexed_fields: vec!["title".into()],
    };
    let coll = scan_collection(&cfg).expect("the collection scans");
    assert_eq!(coll.entries.len(), 2, "both entries scanned");

    let diagnostics = check_references(&coll);
    assert_eq!(
        diagnostics.len(),
        1,
        "exactly the broken link is flagged: {diagnostics:?}"
    );
    let message = format!("{}", diagnostics[0].message);
    assert!(
        message.contains("missing"),
        "the diagnostic names the broken target: {message}"
    );
}

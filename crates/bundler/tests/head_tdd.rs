//! TDD suite for the adapter-level head producer (the V2-E04 tail, spec
//! §10.2 step 4 + §V2-E04): `ClientManifest` → the modulepreload/stylesheet
//! URLs a prerendered shell should carry. The manifest already records the
//! critical-first preload order and the per-route CSS; the producer seam is
//! the missing half that turns the render `Injector`'s `preload`/`css_links`
//! fields from wired-and-ready to fed.

use oxide_fw_bundler::head::{HeadProducer, ManifestHeadProducer, RouteHeadLinks};
use oxide_fw_bundler::{ClientManifest, RouteBundle};
use std::collections::BTreeMap;

// ------------------------------------------------------------------ fixtures

/// A three-route manifest in the bundler's own key spelling: static, dynamic
/// (the `_param_` sanitized chunk form) and the `__boot__` island entry.
fn manifest_fixture() -> ClientManifest {
    let mut routes = BTreeMap::new();
    routes.insert(
        "routes/index/page".to_string(),
        RouteBundle {
            entry_chunk: "index-a1b2c3d4.js".to_string(),
            preload: vec![
                "index-a1b2c3d4.js".to_string(),
                "shared-e5f6a7b8.js".to_string(),
            ],
            css: vec!["index-c9d0e1f2.css".to_string()],
            js_bytes: 120,
            modules: Vec::new(),
            layout_chain: Vec::new(),
        },
    );
    routes.insert(
        "routes/blog/_slug_/page".to_string(),
        RouteBundle {
            entry_chunk: "blog-0a1b2c3d.js".to_string(),
            preload: vec!["blog-0a1b2c3d.js".to_string()],
            css: vec![
                "blog-4e5f6a7b.css".to_string(),
                "shared-c9d0e1f2.css".to_string(),
            ],
            js_bytes: 200,
            modules: Vec::new(),
            layout_chain: Vec::new(),
        },
    );
    routes.insert(
        "__boot__".to_string(),
        RouteBundle {
            entry_chunk: "boot-1a2b3c4d.js".to_string(),
            preload: vec!["boot-1a2b3c4d.js".to_string()],
            css: Vec::new(),
            js_bytes: 30,
            modules: Vec::new(),
            layout_chain: Vec::new(),
        },
    );
    ClientManifest {
        build_id: "fixture-build".to_string(),
        routes,
        assets: BTreeMap::new(),
        integrity: BTreeMap::new(),
    }
}

/// The seam's test vocabulary (the `ReferenceGenerator` fixture-impl
/// convention): a producer that never has links, to prove `dyn` dispatch and
/// the `None` contract work through the trait object.
struct NeverProducer;

impl HeadProducer for NeverProducer {
    fn name(&self) -> &'static str {
        "never"
    }
    fn head_for(&self, _route: &str) -> Option<RouteHeadLinks> {
        None
    }
}

// ------------------------------------------------------------------ pins

#[test]
fn head_for_resolves_the_static_route_key() {
    // The manifest keys entries as `<route>/page` (the bundler's
    // sanitized chunk name); the router's canonical id (`routes/index`)
    // must resolve through the `+ /page` shape.
    let producer = ManifestHeadProducer::new(manifest_fixture());
    let links = producer
        .head_for("routes/index")
        .expect("the static route resolves");
    assert_eq!(
        links.preload,
        vec![
            "/client/index-a1b2c3d4.js".to_string(),
            "/client/shared-e5f6a7b8.js".to_string(),
        ],
        "preload keeps the manifest's critical-first order (§10.2 step 4)"
    );
    assert_eq!(links.css, vec!["/client/index-c9d0e1f2.css".to_string()]);
}

#[test]
fn head_for_accepts_the_full_file_role_id() {
    // `routes/index/page` (the router's file-role id) is the manifest's
    // own key — the exact shape must match first.
    let producer = ManifestHeadProducer::new(manifest_fixture());
    let links = producer
        .head_for("routes/index/page")
        .expect("the file-role id resolves");
    assert_eq!(links.preload.len(), 2);
}

#[test]
fn head_for_normalizes_dynamic_segments() {
    // `[slug]` (router source form) vs `_slug_` (the manifest's sanitized
    // chunk form) — the same normalization `inject_route_css` performs.
    let producer = ManifestHeadProducer::new(manifest_fixture());
    let links = producer
        .head_for("routes/blog/[slug]")
        .expect("the dynamic route resolves");
    assert_eq!(
        links.css,
        vec![
            "/client/blog-4e5f6a7b.css".to_string(),
            "/client/shared-c9d0e1f2.css".to_string(),
        ]
    );
}

#[test]
fn head_for_returns_none_for_an_unknown_route() {
    let producer = ManifestHeadProducer::new(manifest_fixture());
    assert_eq!(producer.head_for("routes/missing"), None);
    // An empty manifest (a pre-E04 bundler, a non-client build) yields
    // `None` for everything — the wire-up then leaves the Injector's
    // fields untouched, i.e. exactly the pre-milestone bytes.
    let empty = ManifestHeadProducer::new(ClientManifest::default());
    assert_eq!(empty.head_for("routes/index"), None);
}

#[test]
fn head_for_dedupes_repeated_manifest_entries_preserving_order() {
    let mut manifest = manifest_fixture();
    let bundle = manifest.routes.get_mut("routes/index/page").unwrap();
    bundle.preload = vec![
        "index-a1b2c3d4.js".to_string(),
        "shared-e5f6a7b8.js".to_string(),
        "index-a1b2c3d4.js".to_string(),
    ];
    let producer = ManifestHeadProducer::new(manifest);
    let links = producer.head_for("routes/index").expect("resolves");
    assert_eq!(
        links.preload,
        vec![
            "/client/index-a1b2c3d4.js".to_string(),
            "/client/shared-e5f6a7b8.js".to_string(),
        ],
        "a repeated chunk is emitted once, at its first (critical) position"
    );
}

#[test]
fn boot_route_resolves_like_any_manifest_key() {
    // `__boot__` is the island entry the CLI stamps onto the boot script
    // tag; the producer treats it as an ordinary route key.
    let producer = ManifestHeadProducer::new(manifest_fixture());
    let links = producer.head_for("__boot__").expect("boot resolves");
    assert_eq!(links.preload, vec!["/client/boot-1a2b3c4d.js".to_string()]);
    assert_eq!(links.css, Vec::<String>::new());
}

#[test]
fn a_route_bundle_without_css_yields_some_with_empty_css() {
    // The rolldown backend records `preload` (entry chunk) with the CSS it
    // saw; a bundle with neither still says `Some` — the route HAS a
    // bundle, it simply carries no links. `None` is reserved for "no
    // bundle for this route".
    let mut manifest = manifest_fixture();
    let bundle = manifest.routes.get_mut("__boot__").unwrap();
    bundle.css = Vec::new();
    bundle.preload = Vec::new();
    let producer = ManifestHeadProducer::new(manifest);
    let links = producer.head_for("__boot__").expect("resolves");
    assert_eq!(links.preload, Vec::<String>::new());
    assert_eq!(links.css, Vec::<String>::new());
}

#[test]
fn every_url_spells_the_integrity_map_key() {
    // The rendered URL must strip back to exactly the file name the
    // manifest's `integrity` map (and `entry_chunk`) is keyed by — that is
    // the spelling `Injector::integrity_file_for_url` resolves to a
    // digest, so a link the producer emits is one the Injector can stamp.
    let mut manifest = manifest_fixture();
    manifest.integrity.insert(
        "index-a1b2c3d4.js".to_string(),
        "sha384-TaLTMFHfY6sTuqLLMg+Zm7Ev+NiiDsjI3P5a7fH6ztJDuNak4csE09KYeF784D3I".to_string(),
    );
    let producer = ManifestHeadProducer::new(manifest);
    let links = producer.head_for("routes/index").expect("resolves");
    for url in links.preload.iter().chain(links.css.iter()) {
        let file = url
            .strip_prefix("/client/")
            .unwrap_or_else(|| panic!("every URL is client-relative: {url}"));
        assert!(
            file.starts_with(|c: char| c.is_ascii_alphanumeric()),
            "the file name is bare (no second slash): {url}"
        );
    }
    assert!(
        links
            .preload
            .first()
            .is_some_and(|u| u == "/client/index-a1b2c3d4.js"),
        "the digest-carrying entry chunk is the first preload"
    );
}

#[test]
fn with_base_url_reroutes_the_emitted_prefix() {
    let producer = ManifestHeadProducer::new(manifest_fixture()).with_base_url("/assets/c/");
    let links = producer.head_for("routes/index").expect("resolves");
    assert_eq!(links.preload[0], "/assets/c/index-a1b2c3d4.js");
}

#[test]
fn the_seam_is_object_safe_and_named() {
    // The pipeline holds producers behind `&dyn` (the `PageRenderer` /
    // `LoaderRunner` convention) and names them for the log surfaces.
    let manifest = ManifestHeadProducer::new(manifest_fixture());
    let producers: Vec<&dyn HeadProducer> = vec![&manifest, &NeverProducer];
    assert_eq!(producers[0].name(), "manifest-head-producer");
    assert_eq!(producers[1].name(), "never");
    assert_eq!(producers[1].head_for("routes/index"), None);
}

#[test]
fn route_head_links_default_is_empty() {
    // `Default` keeps the wire-up terse: an absent producer equals an
    // empty link set, which is the pre-milestone byte shape.
    let links = RouteHeadLinks::default();
    assert_eq!(links.preload, Vec::<String>::new());
    assert_eq!(links.css, Vec::<String>::new());
}

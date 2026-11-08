//! TDD pins for the V2-F05 live-route opt-in (`RouteEntry.live`).
//!
//! Spec v0.2 §V2-F05 step 1: a route opts in with `export const config =
//! { live: true }` (or `live: { debounceMs: 100 }`) in `page.server.ts`;
//! the scanner reads it into an additive `RouteEntry.live` field next to
//! `tags`/`needs_parent`, and the field round-trips the rkyv archive the
//! matcher mmaps.
//!
//! Written FIRST (red): these pins failed on unresolved-symbol errors
//! before `RouteLive` and the scanner's `live` read existed. The endpoint
//! and hub live in `oxide-fw-server` (V2-F05 steps 2–5); the CLI wiring is
//! deferred to the A01-phase-3 adoption — see `docs/deviations.md`.

use oxide_fw_router::{ArchivedTree, RouteKind, RouteLive, RouteTree};
use std::fs;
use std::path::Path;

/// Write a file (creating parent dirs) inside a fixture.
fn put(root: &Path, rel: &str, contents: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().expect("parent")).expect("mkdirs");
    fs::write(p, contents).expect("write");
}

const PLAIN_PAGE: &str = "export default function Page() { return null }\n";
const LIVE_PAGE: &str = concat!(
    "export const config = { live: true }\n",
    "export default function Page() { return null }\n",
);
const LIVE_DEBOUNCE_PAGE: &str = concat!(
    "export const config = { live: { debounceMs: 100 } }\n",
    "export default function Page() { return null }\n",
);

#[test]
fn plain_routes_are_not_live() {
    let tmp = tempfile::tempdir().expect("tmp");
    put(tmp.path(), "src/routes/index/page.tsx", PLAIN_PAGE);
    put(
        tmp.path(),
        "src/routes/index/page.server.ts",
        "export const loader = () => ({})\n",
    );
    let tree = RouteTree::scan_with_config(tmp.path(), None).expect("scan");
    let e = tree
        .entries
        .iter()
        .find(|e| e.kind == RouteKind::Page)
        .expect("page entry");
    assert!(e.live.is_none(), "no config: route must not opt in");
}

#[test]
fn live_true_opts_the_route_in_with_the_default_debounce() {
    let tmp = tempfile::tempdir().expect("tmp");
    put(tmp.path(), "src/routes/index/page.tsx", LIVE_PAGE);
    put(
        tmp.path(),
        "src/routes/index/page.server.ts",
        "export const config = { live: true };\n",
    );
    let tree = RouteTree::scan_with_config(tmp.path(), None).expect("scan");
    let e = tree
        .entries
        .iter()
        .find(|e| e.kind == RouteKind::Page)
        .expect("page entry");
    let live = e.live.as_ref().expect("live: true must set the field");
    assert_eq!(
        live.debounce_ms, None,
        "bare `live: true` keeps the config default"
    );
}

#[test]
fn live_object_reads_the_debounce_override() {
    let tmp = tempfile::tempdir().expect("tmp");
    put(tmp.path(), "src/routes/index/page.tsx", LIVE_DEBOUNCE_PAGE);
    put(
        tmp.path(),
        "src/routes/index/page.server.ts",
        "export const config = { live: { debounceMs: 100 } };\n",
    );
    let tree = RouteTree::scan_with_config(tmp.path(), None).expect("scan");
    let e = tree
        .entries
        .iter()
        .find(|e| e.kind == RouteKind::Page)
        .expect("page entry");
    let live = e.live.as_ref().expect("live config must set the field");
    assert_eq!(
        live.debounce_ms,
        Some(100),
        "debounceMs overrides the default"
    );
}

#[test]
fn live_survives_the_rkyv_archive_round_trip() {
    let tmp = tempfile::tempdir().expect("tmp");
    put(tmp.path(), "src/routes/blog/[slug]/page.tsx", LIVE_PAGE);
    put(
        tmp.path(),
        "src/routes/blog/[slug]/page.server.ts",
        "export const config = { live: { debounceMs: 250 } };\n",
    );
    let tree = RouteTree::scan_with_config(tmp.path(), None).expect("scan");
    let manifest = tmp.path().join("routes.manifest");
    tree.archive_to(&manifest).expect("archive");
    let archived = ArchivedTree::load_archived(&manifest).expect("open");
    let m = archived.match_path("/blog/rust").expect("match");
    assert_eq!(
        m.route.live,
        Some(RouteLive {
            debounce_ms: Some(250)
        }),
        "the opt-in must survive the mmap boundary"
    );
}

#[test]
fn live_field_defaults_absent_in_pre_f05_manifests() {
    // The additive field must not force pre-F05 manifests (routes.json) to
    // spell it out: missing `live` deserializes to `None`.
    let json = r#"{
        "id": "routes/index",
        "kind": "Page",
        "segments": [],
        "file": "src/routes/index/page.tsx",
        "render_mode": "static",
        "tags": [],
        "has_server_module": false,
        "needs_parent": false,
        "slot": null
    }"#;
    let e: oxide_fw_router::RouteEntry = serde_json::from_str(json).expect("pre-F05 manifest");
    assert!(e.live.is_none(), "missing field deserializes to None");
}

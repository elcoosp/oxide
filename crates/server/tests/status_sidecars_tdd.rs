//! TDD suite for the V2-A07 prerender error-status sidecar (spec §V2-A07).
//!
//! Written BEFORE the implementation, in the E01/F16/F18/F20 substrate
//! pattern: a prerendered page whose render status is not 200 must not be
//! served as 200. The build writes `<route>/index.html.status` containing
//! the decimal status; the serve-start scan reads every `*.status` file
//! once into [`StatusSidecars`] (no per-request I/O) and the static-asset
//! responder applies the status.
//!
//! Every pin is pure and deterministic: a tempfile dist tree in, response
//! statuses out. No clock, no network, no worker.

use bytes::Bytes;
use oxide_fw_server::{OxideRequest, StaticAssets, StatusSidecars};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

// ------------------------------------------------------------------ helpers

/// A minimal GET request against `url` (the same shape server_tdd.rs uses).
fn get(url: &str) -> OxideRequest {
    OxideRequest {
        method: "GET".into(),
        url: url.into(),
        headers: Arc::new(Vec::new()),
        header_index: Default::default(),
        body: Bytes::new(),
        request_id: "req-a07".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

/// One file in the fake dist tree: `path` is relative to the root.
fn dist_file(root: &TempDir, rel: &str, content: &str) -> PathBuf {
    let p = root.path().join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).expect("create parent dir");
    }
    std::fs::write(&p, content).expect("write fixture file");
    p
}

/// The canonical fixture: a prerendered index page plus a 500 sidecar.
fn prerendered_500_fixture() -> TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    dist_file(
        &tmp,
        "prerendered/index.html",
        "<!doctype html><html><body>ERR</body></html>",
    );
    dist_file(&tmp, "prerendered/index.html.status", "500");
    tmp
}

// ------------------------------------------------------------------- pins

/// The scan maps `index.html.status` onto the `index.html` file it
/// describes — `status_for` answers for the HTML file, not the sidecar.
#[test]
fn scan_maps_a_sidecar_to_its_html_file() {
    let tmp = prerendered_500_fixture();
    let sidecars = StatusSidecars::scan(tmp.path());
    let html = tmp.path().join("prerendered/index.html");
    assert_eq!(sidecars.status_for(&html), Some(500));
    // The sidecar file itself is NOT a key.
    assert_eq!(
        sidecars.status_for(&tmp.path().join("prerendered/index.html.status")),
        None
    );
}

/// The scan is recursive: `prerendered/<locale>/<route>.html.status` maps to
/// its sibling html (the non-default-locale prerender path).
#[test]
fn scan_finds_sidecars_nested_in_subdirectories() {
    let tmp = tempfile::tempdir().expect("tempdir");
    dist_file(&tmp, "prerendered/fr/blog/index.html", "<!doctype html>");
    dist_file(&tmp, "prerendered/fr/blog/index.html.status", "404");
    dist_file(&tmp, "prerendered/about.html", "<!doctype html>");
    let sidecars = StatusSidecars::scan(tmp.path());
    assert_eq!(
        sidecars.status_for(&tmp.path().join("prerendered/fr/blog/index.html")),
        Some(404)
    );
    assert_eq!(
        sidecars.status_for(&tmp.path().join("prerendered/about.html")),
        None,
        "a page without a sidecar keeps the default 200"
    );
}

/// Malformed sidecar content is ignored (the page stays a 200) — the spec's
/// `s.trim().parse::<u16>()` gate: non-numeric, empty, out-of-range and
/// whitespace-padded-but-valid all behave as specified.
#[test]
fn scan_ignores_malformed_sidecar_content() {
    let tmp = tempfile::tempdir().expect("tempdir");
    dist_file(&tmp, "p/a.html", "x");
    dist_file(&tmp, "p/a.html.status", "not-a-number");
    dist_file(&tmp, "p/b.html", "x");
    dist_file(&tmp, "p/b.html.status", "");
    dist_file(&tmp, "p/c.html", "x");
    dist_file(&tmp, "p/c.html.status", "70000"); // out of u16 range
    dist_file(&tmp, "p/d.html", "x");
    dist_file(&tmp, "p/d.html.status", "503\n"); // trailing newline is trimmed
    let sidecars = StatusSidecars::scan(tmp.path());
    for rel in ["p/a.html", "p/b.html", "p/c.html"] {
        assert_eq!(
            sidecars.status_for(&tmp.path().join(rel)),
            None,
            "{rel}: malformed sidecar must not produce a status"
        );
    }
    assert_eq!(
        sidecars.status_for(&tmp.path().join("p/d.html")),
        Some(503),
        "whitespace-padded decimal parses"
    );
}

/// Only the exact `.status` extension is a sidecar; a file whose name merely
/// ends in a longer dotted suffix (`.status.bak`, `.status2`) is neither a
/// sidecar nor hidden from the asset index.
#[test]
fn scan_treats_only_the_status_extension_as_a_sidecar() {
    let tmp = tempfile::tempdir().expect("tempdir");
    dist_file(&tmp, "p/a.html", "x");
    dist_file(&tmp, "p/a.html.status", "500");
    dist_file(&tmp, "p/b.html", "x");
    dist_file(&tmp, "p/b.html.status.bak", "500"); // backup copy, not a sidecar
    dist_file(&tmp, "p/c.html", "x");
    dist_file(&tmp, "p/c.html.status2", "500"); // different extension
    let sidecars = StatusSidecars::scan(tmp.path());
    assert_eq!(sidecars.status_for(&tmp.path().join("p/b.html")), None);
    assert_eq!(sidecars.status_for(&tmp.path().join("p/c.html")), None);
}

/// A sidecar must never be web-facing: the asset scan does not index
/// `*.status` files (they would download as octet-stream), and `serve`
/// hard-denies them for defense in depth (the SEC-STATIC shape).
#[test]
fn serve_never_serves_the_sidecar_file_itself() {
    let tmp = prerendered_500_fixture();
    let assets = StaticAssets::scan_filtered(tmp.path(), &["server"]).expect("scan");
    assert!(
        !assets.index.contains_key("/prerendered/index.html.status"),
        "the sidecar file must not enter the asset index"
    );
    assert!(
        assets
            .serve(
                &get("/prerendered/index.html.status"),
                "/prerendered/index.html.status"
            )
            .is_none(),
        "a sidecar path is hard-denied even on an index hit"
    );
}

/// The full contract: `StaticAssets::serve` answers a prerendered error
/// page with the sidecar's status — 500 with the error body — while a page
/// without a sidecar stays 200.
#[test]
fn serve_applies_the_sidecar_status_to_the_html_response() {
    let tmp = prerendered_500_fixture();
    let assets = StaticAssets::scan_filtered(tmp.path(), &["server"]).expect("scan");
    let resp = assets
        .serve(&get("/prerendered/index.html"), "/prerendered/index.html")
        .expect("index.html is served");
    assert_eq!(resp.status, 500, "the sidecar status rides the response");
    assert!(
        String::from_utf8_lossy(&resp.body).contains("ERR"),
        "the error body is preserved"
    );
}

/// A status change between builds is honored both ways: a sidecar file is
/// removed when its page returns to 200 (the incremental-reuse path writes
/// fresh HTML but the previous build's sidecar must not survive it).
#[test]
fn serve_stays_200_when_no_sidecar_exists() {
    let tmp = tempfile::tempdir().expect("tempdir");
    dist_file(&tmp, "prerendered/index.html", "<!doctype html>");
    let assets = StaticAssets::scan_filtered(tmp.path(), &[]).expect("scan");
    let resp = assets
        .serve(&get("/prerendered/index.html"), "/prerendered/index.html")
        .expect("index.html is served");
    assert_eq!(resp.status, 200);
}

/// The contract owner: `write_status_sidecar` writes the decimal status for
/// a non-200 page and REMOVES a stale sidecar when the page returns to 200
/// — the write side of the contract lives next to the read side.
#[test]
fn write_status_sidecar_writes_and_clears() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let html = dist_file(&tmp, "prerendered/boom.html", "<!doctype html>");

    oxide_fw_server::write_status_sidecar(&html, 500).expect("write sidecar");
    let sidecar = tmp.path().join("prerendered/boom.html.status");
    assert_eq!(
        std::fs::read_to_string(&sidecar).expect("sidecar exists"),
        "500",
        "the sidecar carries the decimal status"
    );
    // Round trip through the scan: the writer's spelling is the reader's.
    let sidecars = StatusSidecars::scan(tmp.path());
    assert_eq!(sidecars.status_for(&html), Some(500));

    oxide_fw_server::write_status_sidecar(&html, 200).expect("clear sidecar");
    assert!(!sidecar.exists(), "a 200 write removes the stale sidecar");
    let sidecars = StatusSidecars::scan(tmp.path());
    assert_eq!(sidecars.status_for(&html), None);
}

/// Scanning a missing root is an empty map, not an error (the spec's
/// `read_dir` guard) — `oxide start` on a half-built dist must not refuse
/// to boot over a missing prerendered/ tree.
#[test]
fn scan_of_a_missing_root_is_empty() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sidecars = StatusSidecars::scan(&tmp.path().join("does-not-exist"));
    assert_eq!(sidecars.status_for(&PathBuf::from("whatever.html")), None);
}

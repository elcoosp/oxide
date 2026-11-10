//! Walkthrough: the V2-A07 prerender error-status sidecar contract, end to
//! end — the consumer `oxide-fw-server` never had for this seam.
//!
//! The build writes `<page>.status` next to a prerendered page whose render
//! status is not 200 (`write_status_sidecar`, called from the CLI's
//! `write_prerender` and the incremental-reuse sites). At serve start the
//! dist is scanned ONCE into `StatusSidecars`, and
//! `StaticAssets::serve` applies the sidecar status to the page — so a
//! prerendered error page is never served as a 200, even on a cold `oxide
//! start` with no L0 seed.
//!
//! Run: `cargo run -p oxide-fw-server --example status_sidecars`

use oxide_fw_server::{OxideRequest, StaticAssets, StatusSidecars, write_status_sidecar};
use std::sync::Arc;

fn main() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().to_path_buf();

    // 1. The build: a healthy page and a 500 error page (what the CLI's
    //    `write_prerender` writes for a loader short-circuit or an adapter
    //    error-status marker).
    let ok_page = dist.join("prerendered/index.html");
    std::fs::create_dir_all(ok_page.parent().unwrap()).expect("mkdir");
    std::fs::write(&ok_page, "<!doctype html><html><body>home</body></html>")
        .expect("write ok page");
    let err_page = dist.join("prerendered/boom.html");
    std::fs::write(&err_page, "<!doctype html><html><body>ERR</body></html>")
        .expect("write error page");
    write_status_sidecar(&err_page, 500).expect("write sidecar");
    println!(
        "build: {} (200) + {} (500, sidecar {})",
        ok_page.display(),
        err_page.display(),
        dist.join("prerendered/boom.html.status").display(),
    );

    // 2. Serve start: ONE scan, no per-request I/O.
    let assets = StaticAssets::scan_filtered(&dist, &["server"]).expect("scan dist");
    println!(
        "serve start: {} asset(s), {} sidecar(s)",
        assets.index.len(),
        assets.status_sidecars.len()
    );

    // 3. Requests: the error page comes back as a REAL 500; the healthy
    //    page stays a 200.
    for url in ["/prerendered/boom.html", "/prerendered/index.html"] {
        let req = OxideRequest {
            method: "GET".into(),
            url: url.into(),
            headers: Arc::new(Vec::new()),
            header_index: Default::default(),
            body: bytes::Bytes::new(),
            request_id: "example".into(),
            trace: None,
            peer: None,
            rewrite_hops: 0,
            mw_resume_after: None,
        };
        let path = url.to_string();
        let resp = assets
            .serve(&req, &path)
            .unwrap_or_else(|| panic!("{url} must be served"));
        println!("GET {url} -> {}", resp.status);
    }

    // 4. The next build flips the page back to 200: the stale sidecar is
    //    cleared, and the scan (re-run at the next serve start) agrees.
    write_status_sidecar(&err_page, 200).expect("clear sidecar");
    let rescanned = StatusSidecars::scan(&dist);
    println!(
        "after the fix: sidecar gone, rescanned status_for(boom) = {:?}",
        rescanned.status_for(&err_page)
    );
}

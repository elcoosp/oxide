//! Consumer walkthrough for the V2-F56 static plane's conditional seam:
//! the asset server's conditional re-validation composed through the
//! cache crate's canonical comparison instead of a re-rolled match (the
//! duplication the F54 serving-half doc named as its second evidence).
//!
//! Shows the pieces a contributor (or a host author) composes:
//!
//! 1. [`oxide_fw_server::StaticAssets::scan`] — the index the serve path
//!    answers from (the etag is the blake3 content hash, the mtime the
//!    `Last-Modified` referent).
//! 2. The conditional arms of [`StaticAssets::serve`]: the echoed
//!    `If-None-Match` (304 — the pre-existing pins' shape, now through
//!    the canonical comparison), the `If-Modified-Since`-only re-check
//!    (304 — the F56 upgrade, the arm the hand-rolled match never had)
//!    and the non-matching validator (the full asset, its headers now
//!    carrying the `Last-Modified` the 304 echoes).
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-server --example asset_conditional
//! ```

use bytes::Bytes;
use oxide_fw_server::{OxideRequest, StaticAssets};

fn req(method: &str, url: &str, headers: &[(&str, &str)]) -> OxideRequest {
    OxideRequest {
        method: method.into(),
        url: url.into(),
        headers: std::sync::Arc::new(
            headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        ),
        header_index: Default::default(),
        body: Bytes::new(),
        request_id: "example-asset-conditional".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A tiny fixture dir: one script, one precompressed sibling.
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("app.js"), b"console.log(1)")?;
    std::fs::write(dir.path().join("app.js.zst"), b"ZSTD-APP")?;
    let assets = StaticAssets::scan(dir.path())?;
    let entry = assets.index.get("/app.js").ok_or("the fixture indexed")?;
    let etag_h = format!("\"{}\"", entry.etag);
    let lm = oxide_fw_cache::http_date(entry.mtime);
    println!("indexed: /app.js etag={etag_h} last-modified={lm}");

    // 1. The conforming re-check echoes the served ETag: 304 with the
    //    validator set (the canonical comparison's decision).
    let resp = assets
        .serve(
            &req("GET", "/app.js", &[("if-none-match", etag_h.as_str())]),
            "/app.js",
        )
        .ok_or("the re-check answers")?;
    println!("\nif-none-match echo -> {}", resp.status);
    for (k, v) in &resp.headers {
        println!("  {k}: {v}");
    }

    // 2. The IMS-only re-check: the F56 arm.
    let resp = assets
        .serve(
            &req("GET", "/app.js", &[("if-modified-since", lm.as_str())]),
            "/app.js",
        )
        .ok_or("the IMS re-check answers")?;
    println!("\nif-modified-since (unchanged) -> {}", resp.status);

    // 3. An older IMS: the file changed after the client's copy — full.
    let older = oxide_fw_cache::http_date(entry.mtime.saturating_sub(3600));
    let resp = assets
        .serve(
            &req("GET", "/app.js", &[("if-modified-since", older.as_str())]),
            "/app.js",
        )
        .ok_or("the older IMS answers")?;
    println!("\nif-modified-since (older) -> {} in full", resp.status);
    println!(
        "  last-modified: {}",
        resp.header("last-modified").unwrap_or("")
    );

    // 4. The precedence rule: a mismatching INM decides past a matching IMS.
    let resp = assets
        .serve(
            &req(
                "GET",
                "/app.js",
                &[
                    ("if-none-match", "\"nope\""),
                    ("if-modified-since", lm.as_str()),
                ],
            ),
            "/app.js",
        )
        .ok_or("the both-validators request answers")?;
    println!(
        "both validators, INM mismatched -> {} in full (the INM decided)",
        resp.status
    );

    Ok(())
}

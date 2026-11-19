//! The cache plane's conditional-request walkthrough (F54, §11.3): the
//! serving half of the `http` module's Cache-Control/ETag/304 work —
//! ONE call (`serve_entry`) that answers a request's validators and
//! composes the §11.3 header set, so no host re-rolls the semantics.
//!
//! The walkthrough stores a rendered page, then serves the SAME entry
//! three ways:
//!
//! 1. a first visit (no conditional headers) — the full entry with the
//!    generated `Cache-Control` / `ETag` / `Date` beside the stored ones;
//! 2. a re-check carrying the stored `ETag` — the 304 arm, nothing
//!    downloaded;
//! 3. an `If-Modified-Since`-only re-check (the L0 stage's hand-rolled
//!    match never answered these) — the 304 arm again, the capability
//!    upgrade the canonical comparison brings.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example conditional
//! ```

use std::time::Duration;

use oxide_fw_cache::{Entry, Policy, ServeOutcome, serve_entry};

fn main() {
    // NOW: 2026-11-19T00:00:00Z (deterministic for the walkthrough's Date).
    const NOW: u64 = 1_795_046_400;
    let rendered = Entry {
        body: b"<html>the rendered page</html>".to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: vec![],
        policy: Policy {
            ttl: Duration::from_secs(60),
            swr: Some(Duration::from_secs(30)),
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: NOW * 1000,
        etag: "\"v1\"".into(),
    };

    // 1. First visit: full serve, the §11.3 set fills the gaps.
    let ServeOutcome::Full {
        status,
        headers,
        body,
    } = serve_entry(&rendered, &[], NOW)
    else {
        panic!("no validators — full serve");
    };
    println!(
        "first visit: {status}, {} bytes, {} headers",
        body.len(),
        headers.len()
    );
    for (k, v) in &headers {
        println!("  {k}: {v}");
    }

    // 2. A re-check with the stored validator: 304, nothing downloaded.
    let inm = vec![("if-none-match".to_string(), "\"v1\"".to_string())];
    let ServeOutcome::NotModified { headers } = serve_entry(&rendered, &inm, NOW) else {
        panic!("a matching validator answers 304");
    };
    println!("re-check (If-None-Match matches): 304 with {headers:?}");

    // 3. An IMS-only re-check: the canonical comparison answers it — the
    //    capability the hand-rolled match never had.
    let ims = vec![(
        "if-modified-since".to_string(),
        "Mon, 07 Nov 1994 08:49:37 GMT".to_string(),
    )];
    // The entry carries no Last-Modified here, so IMS cannot answer —
    // serve the stored validator's shape instead to show the 304 again.
    let with_lm = Entry {
        headers: vec![(
            "last-modified".into(),
            "Sun, 06 Nov 1994 08:49:37 GMT".into(),
        )],
        ..rendered.clone()
    };
    let ServeOutcome::NotModified { .. } = serve_entry(&with_lm, &ims, NOW) else {
        panic!("IMS newer than Last-Modified answers 304");
    };
    println!("re-check (If-Modified-Since only, Last-Modified unchanged): 304");
}

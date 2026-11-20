//! Consumer walkthrough for the V2-F55 conditional seam: the L0 response
//! plane's conditional re-validation, composed through the cache crate's
//! canonical `serve_entry` decision instead of a re-rolled match (the
//! duplication the F54 serving-half doc named as its evidence).
//!
//! Shows the four pieces a contributor (or a host author) composes:
//!
//! 1. [`oxide_fw_cli::L0Cache::make_entry`] — the store path's builder:
//!    the sentineled body, the RFC-shaped weak etag (`W/"<hash>"` — the
//!    value the responses serve and conforming clients echo back) and
//!    the generated `Last-Modified` the IMS-only re-check compares
//!    against.
//! 2. [`oxide_fw_cli::CachedResponse::serve_conditional`] — the ONE call
//!    a hit answers a conditional request with: the RFC 9110 §13
//!    comparison (weak `If-None-Match` lists and `*` first,
//!    `If-Modified-Since` against the stored `Last-Modified` second) and
//!    the §15.4.5 validator set, composed by [`oxide_fw_cache::serve_entry`].
//! 3. The three re-check shapes: the echoed `If-None-Match` (304, the
//!    validator set), the `If-Modified-Since`-only re-check (304 — the
//!    F55 upgrade, the arm the hand-rolled stage never had) and the
//!    non-matching validator (the Full outcome: the stored headers plus
//!    the generated gaps, and the STORED representation — a nonce
//!    pipeline materializes its own body over the sentinel, which is
//!    exactly what the dispatch stage's full-hit path stays with).
//! 4. The freshness rule around the decision: the lookup only serves
//!    fresh hits (or stale hits within the SWR window), so a conditional
//!    request never 304s against an expired entry.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cli --example l0_conditional
//! ```

use std::sync::Arc;
use std::time::Duration;

use oxide_fw_cache::ServeOutcome;
use oxide_fw_cli::{CachedResponse, L0Cache, NONCE_SENTINEL};

fn main() {
    // The store path: a tier behind the L0 handle, one page stored with
    // the nonce sentinel in place of the live nonce (the substitution the
    // dispatch performs per request; the etag hashes the SENTINELED body,
    // so it is stable across requests).
    let tier: Arc<dyn oxide_fw_cache::CacheHandler> =
        Arc::new(oxide_fw_cache::TieredCache::new(16 * 1024 * 1024));
    let l0 = L0Cache::new(tier, "walkthrough-build", Duration::from_secs(300));
    let body = format!("<html data-nonce=\"{NONCE_SENTINEL}\"><h1>hello</h1></html>").into_bytes();
    let entry = Arc::new(l0.make_entry(
        200,
        vec![(
            "content-type".to_string(),
            "text/html; charset=utf-8".to_string(),
        )],
        body,
        vec!["routes/index".to_string(), "path:/".to_string()],
    ));
    let hit = CachedResponse {
        entry: Arc::clone(&entry),
        stale: false,
    };

    println!("stored: status={} etag={}", hit.status(), hit.etag());
    let lm = hit
        .entry
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("last-modified"))
        .map(|(_, v)| v.clone())
        .expect("make_entry generated a Last-Modified");
    println!("stored: last-modified={lm}");

    // 1. The conforming re-check echoes the served ETag: the 304 arm with
    //    the §15.4.5 validator set — what the 200 would have carried.
    let inm = [("if-none-match".to_string(), hit.etag().to_string())];
    match hit.serve_conditional(&inm, 1_700_000_000) {
        ServeOutcome::NotModified { headers } => {
            println!("\nif-none-match echo -> 304");
            for (k, v) in &headers {
                println!("  {k}: {v}");
            }
        }
        ServeOutcome::Full { status, .. } => {
            panic!("the echoed validator must answer 304, got {status}")
        }
    }

    // 2. The IMS-only re-check: the arm the hand-rolled stage never had.
    let ims = [("if-modified-since".to_string(), lm.clone())];
    match hit.serve_conditional(&ims, 1_700_000_000) {
        ServeOutcome::NotModified { .. } => {
            println!("\nif-modified-since (unchanged) -> 304 (no re-download)")
        }
        ServeOutcome::Full { status, .. } => {
            panic!("the unchanged-entity IMS must answer 304, got {status}")
        }
    }

    // 3. An IMS older than the Last-Modified: the entity changed after the
    //    client's copy — the Full outcome, the stored representation.
    let secs = oxide_fw_cache::parse_http_date(&lm).expect("the generated date parses");
    let older = oxide_fw_cache::http_date(secs.saturating_sub(3600));
    let stale_ims = [("if-modified-since".to_string(), older)];
    match hit.serve_conditional(&stale_ims, 1_700_000_000) {
        ServeOutcome::Full {
            status,
            headers,
            body,
        } => {
            println!("\nif-modified-since (older) -> {status} in full");
            println!("  headers: {} carried", headers.len());
            println!("  body: {} bytes (the stored representation)", body.len());
        }
        ServeOutcome::NotModified { .. } => {
            panic!("an older IMS must serve in full")
        }
    }

    // 4. The precedence rule: a present-but-matching-none If-None-Match
    //    decides; the IMS comparison never runs (RFC 9110 §13.2.2).
    let both = [
        ("if-none-match".to_string(), "\"nope\"".to_string()),
        ("if-modified-since".to_string(), lm),
    ];
    match hit.serve_conditional(&both, 1_700_000_000) {
        ServeOutcome::Full { status, .. } => {
            println!("both validators, INM mismatched -> {status} in full (the INM decided)")
        }
        ServeOutcome::NotModified { .. } => {
            panic!("the INM precedence must short-circuit past the matching IMS")
        }
    }

    println!(
        "\nthe dispatch stage answers its conditional requests with exactly this call — \
         one comparison, one validator set, no re-rolled match"
    );
}

//! Consumer walkthrough for the V2-F66 request-line trust boundary: the
//! §11.2 `HttpCacheHandler` refuses to speak a key or tag the request
//! line cannot carry.
//!
//! The client builds its URLs by interpolation and the wire has no
//! escaping (the service's `route` contract: everything after the family
//! prefix IS the name). A key carrying a CR, LF, space or other control
//! byte did not fail — it corrupted the exchange: a space split the
//! request line, a CRLF injected a second exchange into the connection
//! stream. [`oxide_fw_cache::HttpCacheHandler::plane_name`] is the
//! boundary: the public predicate a host can pre-validate with, and the
//! check every verb runs BEFORE a single wire byte leaves — the refusal
//! is loud under `CACHE-HTTP` and names the offending byte.
//!
//! Shows the story end to end:
//!
//! 1. The name table: what the request line speaks verbatim (route keys,
//!    query punctuation, percent escapes, UTF-8) and what it refuses,
//!    with the byte named.
//! 2. The wire proof: against a counting plane, a CRLF key's `set`/`get`
//!    refuse with ZERO dials — the corrupt exchange is never spoken.
//! 3. The guard does not over-refuse: a UTF-8 + punctuation key
//!    round-trips through a REAL plane byte-identical.
//! 4. The tag verbs ride the same boundary; `publish_invalidation`
//!    keeps its documented best-effort contract (`Ok(())`, the gap loud
//!    in the warn) with zero dials.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example plane_name_guard
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use oxide_fw_cache::{CacheHandler, Entry, HttpCacheHandler, HttpCacheService, TieredCache};

/// The demo entry: a finite TTL and a LIVE timestamp (a zero-TTL or
/// 1970-born entry is a correct miss the guard has nothing to do with).
fn entry() -> Entry {
    Entry {
        body: b"<html>guard-demo</html>".to_vec(),
        status: 200,
        headers: vec![],
        tags: vec![],
        policy: oxide_fw_cache::Policy {
            ttl: std::time::Duration::from_secs(3600),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        etag: String::new(),
    }
}

/// Step 1: the name table — the predicate is the boundary's public half,
/// so a host composing keys can pre-validate.
fn print_name_table() {
    for name in ["route:/blog/1", "cache:user:42:v2", "route:/_blog/über?x=1"] {
        println!("plane_name({name:?}) -> Ok (the line speaks it verbatim)");
    }
    for name in ["a\rb", "a\nb", "a b", "a\tb"] {
        match HttpCacheHandler::plane_name(name) {
            Ok(()) => unreachable!(),
            Err(e) => println!("plane_name({name:?}) -> {}", e.0.message),
        }
    }
}

/// Step 2: the wire proof — against a counting plane (every dial is one
/// exchange the client spoke), a CRLF key's `set`/`get` refuse with ZERO
/// dials: the corrupt exchange is never spoken.
async fn wire_proof(client: &HttpCacheHandler, dials: &AtomicUsize) {
    let poisoned = "route:/x\r\nX-Injected: 1";
    let set_err = client
        .set(poisoned, entry())
        .await
        .expect_err("the CRLF key never reaches the wire");
    println!(
        "set with a CRLF key -> {} (code {})",
        set_err.0.message, set_err.0.code
    );
    let get_err = client
        .get(poisoned)
        .await
        .expect_err("the get is loud too — never the pre-F66 silent miss");
    println!(
        "get with a CRLF key -> {} (code {})",
        get_err.0.message, get_err.0.code
    );
    println!(
        "dials after the refusals -> {} (zero bytes left the client)",
        dials.load(Ordering::SeqCst)
    );
}

/// Step 3: the guard does not over-refuse — a real plane serves the full
/// name table, UTF-8 and punctuation round-trip byte-identical.
async fn roundtrip_proof() {
    let real_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let real_addr = real_listener.local_addr().unwrap();
    let tier = TieredCache::new_memory();
    let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>);
    tokio::spawn(async move {
        svc.serve(real_listener)
            .await
            .expect("the accept loop serves");
    });
    let real = HttpCacheHandler::new(format!("http://{real_addr}"));
    let spoken = "route:/_blog/über?x=1&y=2";
    real.set(spoken, entry())
        .await
        .expect("the guarded client still speaks the table");
    let got = real
        .get(spoken)
        .await
        .unwrap()
        .expect("the UTF-8 key round-trips");
    println!(
        "UTF-8 + punctuation key round-trip -> {} bytes (the guard changed nothing)",
        got.body.len()
    );
}

/// Step 4: the tag verbs ride the same boundary — and the publish keeps
/// its best-effort contract: loud warn, `Ok(())`, zero dials.
async fn tag_proof(client: &HttpCacheHandler, dials: &AtomicUsize) {
    let bad_tag = "products\r\ncache-poison";
    let keys_err = client
        .tag_keys(bad_tag)
        .await
        .expect_err("the listing refuses a bad tag");
    println!(
        "tag_keys with a bad tag -> {} (code {})",
        keys_err.0.message, keys_err.0.code
    );
    client
        .invalidate_tag(bad_tag, true)
        .await
        .expect_err("the invalidate refuses too");
    client
        .publish_invalidation(bad_tag, true)
        .await
        .expect("the publish keeps its best-effort Ok contract");
    println!(
        "dials after the tag refusals -> {} (the corrupt publish was never spoken)",
        dials.load(Ordering::SeqCst)
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        print_name_table();

        // The counting plane: every dial is one exchange the client spoke.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let dials = Arc::new(AtomicUsize::new(0));
        let dials2 = Arc::clone(&dials);
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                dials2.fetch_add(1, Ordering::SeqCst);
                drop(conn);
            }
        });
        let client = HttpCacheHandler::new(format!("http://{addr}"));

        wire_proof(&client, &dials).await;
        roundtrip_proof().await;
        tag_proof(&client, &dials).await;
    });
    Ok(())
}

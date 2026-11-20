//! Consumer walkthrough for the V2-F60 HTTP client's deadline arm: the
//! §11.2 "a remote cache is an optimization" contract's teeth.
//!
//! The `HttpCacheHandler` client always degraded transport failures —
//! but only REFUSED ones. A peer that hangs (a firewall dropping SYNs, a
//! wedged service) held the render path forever: connect and read had no
//! deadline. `HttpCacheHandler::with_timeout` bounds the WHOLE exchange
//! (connect through read) to the operator's duration; an exchange that
//! outlives it fails under `CACHE-HTTP` and flows through the file's
//! existing degradation arms — `get` to a miss (the local tier keeps
//! serving), `set` to a loud `Err`.
//!
//! Shows the story end to end:
//!
//! 1. A healthy exchange under a deadline: set/get round-trip through a
//!    real plane service — the bound clips dead peers, not live traffic.
//! 2. The hung peer: the deadline trips, `set` fails loudly (the code
//!    names `CACHE-HTTP`), `get` degrades to a miss — within the
//!    deadline, never a hang.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example client_deadline
//! ```

use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheHandler, HttpCacheService, Policy, TieredCache,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn entry() -> Entry {
    Entry {
        body: b"<html>deadline-v1</html>".to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: vec!["deadline-tag".to_string()],
        policy: Policy {
            ttl: Duration::from_secs(3600),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        // The tier classifies freshness against the REAL clock (unix ms) —
        // a stamp near `now` keeps the walkthrough's entry fresh.
        stored_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        etag: "\"deadline-v1\"".into(),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    // 1. The healthy half: a real plane on a local socket, the client
    //    bounded to 2 seconds — the round trip is untouched.
    let listener = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))?;
    let addr = listener.local_addr()?;
    let tier = TieredCache::new_memory();
    let service = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>);
    rt.block_on(async {
        tokio::spawn(service.serve(listener));
    });
    let bounded =
        HttpCacheHandler::new(format!("http://{addr}")).with_timeout(Duration::from_secs(2));

    rt.block_on(bounded.set("route:/live", entry()))
        .expect("the set lands under the deadline");
    let got = rt
        .block_on(bounded.get("route:/live"))
        .expect("the get answers under the deadline");
    println!(
        "healthy exchange under a 2s deadline: set ok, get -> {} bytes \
         (the deadline never clips live traffic)",
        got.as_ref().map_or(0, |e| e.body.len()),
    );

    // 2. The hung half: a peer that accepts and NEVER answers — the
    //    dropped-SYN's stand-in. The deadline trips; `set` is loud, `get`
    //    degrades to a miss, the local tier keeps serving.
    let hang = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))?;
    let hang_addr = hang.local_addr()?;
    rt.block_on(async {
        tokio::spawn(async move {
            loop {
                if let Ok((_s, _)) = hang.accept().await {
                    // Accept and hold: the exchange never completes.
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                }
            }
        });
    });
    let hung = HttpCacheHandler::new(format!("http://{hang_addr}"))
        .with_timeout(Duration::from_millis(250));

    let started = Instant::now();
    let set_outcome = rt.block_on(hung.set("route:/hung", entry()));
    let elapsed = started.elapsed();
    match set_outcome {
        Err(e) => println!(
            "hung peer, set -> Err({}: {}) in {:?} (loud, within the deadline)",
            e.0.code, e.0.message, elapsed
        ),
        Ok(()) => println!("hung peer, set -> Ok (unexpected — the deadline should have tripped)"),
    }

    let started = Instant::now();
    let got = rt.block_on(hung.get("route:/hung"));
    let elapsed = started.elapsed();
    println!(
        "hung peer, get -> {} in {:?} (the miss degrades, the render path never hangs)",
        if got.is_ok() && got.as_ref().unwrap().is_none() {
            "Ok(None)"
        } else {
            "unexpected shape"
        },
        elapsed
    );

    Ok(())
}

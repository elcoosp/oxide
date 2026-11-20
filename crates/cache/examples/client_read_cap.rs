//! Consumer walkthrough for the V2-F65 client read bound: the §11.2
//! `HttpCacheHandler` refuses to buffer an answer past its cap.
//!
//! F64 bounded the plane's shell (the service refuses an oversized body
//! BEFORE the bytes land) — but the CLIENT half still drained whatever
//! the peer sent: `read_to_end` grew the buffer without bound, so a
//! hostile or misrouted peer answering a gigantic body cost the caller
//! node its memory one header line later. The F60 deadline bounds the
//! exchange's TIME; [`oxide_fw_cache::HttpCacheHandler::with_read_limit`]
//! bounds its MEMORY: the read refuses past the cap LOUD under
//! `CACHE-HTTP`, the default stays the pre-F65 unbounded shape
//! (byte-identical until the operator opts in), and `read_limit()` pins
//! the state.
//!
//! Shows the story end to end:
//!
//! 1. A fresh client rides the unbounded default (`read_limit() == None`)
//!    and answers a legit exchange exactly as it always did.
//! 2. A capped client answers the same legit exchange identically — the
//!    bound changes no answer it should not.
//! 3. The plane serves a 2 MiB entry; the 64 KiB-capped client's `get`
//!    degrades to a miss (the over-cap answer refused, the local tier
//!    keeps serving, §11.2) while the small observability pull rides
//!    under the same cap untouched — a ceiling, not a tax.
//! 4. A terminatorless flood (headers that never complete) stops at the
//!    cap too — the parse error never even happens, the refusal does.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example client_read_cap
//! ```

use std::sync::Arc;
use std::time::Duration;

use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheHandler, HttpCacheService, PlanePeer, Policy, TieredCache,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        // The plane: a real socket shell over an in-memory tier, holding
        // one 2 MiB entry — past the demo cap, under the codec's bound.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let big = Entry {
            body: vec![b'a'; 2 * 1024 * 1024],
            status: 200,
            headers: vec![],
            tags: vec![],
            policy: Policy {
                ttl: Duration::from_secs(3600),
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
            etag: "\"read-cap-demo\"".into(),
        };
        let tier = TieredCache::new_memory();
        tier.set("route:/demo-big", big).await.unwrap();
        let svc = HttpCacheService::new(
            Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>
        )
        .with_health(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::PlaneHealth>);
        tokio::spawn(async move {
            svc.serve(listener).await.expect("the accept loop serves");
        });

        // 1. The default: the pre-F65 unbounded read rides until the
        //    operator opts in — the F60 posture for the client half.
        let unbounded = HttpCacheHandler::new(format!("http://{addr}"));
        println!(
            "default client -> read_limit={:?} (the pre-F65 unbounded shape)",
            unbounded.read_limit()
        );
        let got = unbounded
            .get("route:/demo-big")
            .await
            .unwrap()
            .expect("the unbounded client hits");
        println!(
            "unbounded GET of the 2 MiB entry -> {} bytes (the whole answer buffers)",
            got.body.len()
        );

        // 2. The capped client answers legit traffic identically.
        let capped =
            HttpCacheHandler::new(format!("http://{addr}")).with_read_limit(4 * 1024 * 1024);
        println!(
            "capped client   -> read_limit={:?} (the attached bound)",
            capped.read_limit()
        );
        let got = capped
            .get("route:/demo-big")
            .await
            .unwrap()
            .expect("the capped client hits the same entry");
        println!(
            "capped GET (4 MiB cap) -> {} bytes (the bound changes no legit answer)",
            got.body.len()
        );

        // 3. The refusal: the 64 KiB cap against the 2 MiB answer — the
        //    exchange stops at the cap, LOUD, and the get degrades to a
        //    miss (the local tier keeps serving, §11.2).
        let tight =
            HttpCacheHandler::new(format!("http://{addr}")).with_read_limit(64 * 1024);
        let got = tight.get("route:/demo-big").await.unwrap();
        println!(
            "capped GET (64 KiB cap) of the 2 MiB entry -> {:?} (the over-cap answer degrades to a miss)",
            got.is_none()
        );
        // The cap is a CEILING, not a tax: the small observability pulls
        // ride under the same client untouched — the posture answers,
        // decoded as ever.
        let posture = tight
            .peer_health()
            .await
            .expect("the small pull rides under the cap");
        println!(
            "probe under the same cap -> {} memory entries (the small answers ride)",
            posture.memory_entries
        );

        // 4. The terminatorless flood: headers that never complete — the
        //    cap stops the read before the parser ever sees a problem.
        let flood_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let flood_addr = flood_listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = flood_listener.accept().await {
                use tokio::io::AsyncWriteExt;
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\nX-Flood: ")
                    .await;
                let chunk = vec![b'A'; 64 * 1024];
                for _ in 0..64 {
                    // 4 MiB of header continuation, never the blank line.
                    if sock.write_all(&chunk).await.is_err() {
                        break;
                    }
                }
            }
        });
        let started = std::time::Instant::now();
        let client =
            HttpCacheHandler::new(format!("http://{flood_addr}")).with_read_limit(64 * 1024);
        let err = client
            .peer_metrics()
            .await
            .expect_err("the flood is refused at the cap");
        println!(
            "terminatorless flood -> refused in {:?}: {}",
            started.elapsed(),
            err.0.message
        );
    });
    Ok(())
}

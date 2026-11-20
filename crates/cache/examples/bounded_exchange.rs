//! Consumer walkthrough for the V2-F64 exchange-bounds seam: the plane's
//! socket shell refuses an oversized body BEFORE the bytes land.
//!
//! The F53 shell bounded the header block but drained the body to
//! whatever `Content-Length` claimed — the codec's own size gate ran only
//! AFTER the drain, so a hostile peer writing a gigantic length and
//! trickling bytes cost the plane node its memory one header line later.
//! [`oxide_fw_cache::ExchangeLimits`] closes it: the default cap rides
//! the codec's own bound (every legit exchange drains; every one the
//! codec would refuse stops first), `with_limits` tightens or lifts the
//! bound, and `limits()` pins the state.
//!
//! Shows the story end to end:
//!
//! 1. A legit sealed entry PUTs and serves under the default cap — the
//!    bound changes no legit answer.
//! 2. A hostile `Content-Length` past the cap answers 413 with only the
//!    headers sent — the refusal beats the drain, nothing stores.
//! 3. A tightened cap refuses a body the default would serve — the
//!    operator knob works, `limits()` carries the state.
//! 4. The explicit escape hatch (`unbounded()`) restores the pre-F64
//!    drain — the garbage body reaches the codec and answers the codec's
//!    400, the host's own documented choice.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example bounded_exchange
//! ```

use oxide_fw_cache::{ExchangeLimits, HttpCacheService, TieredCache};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// One raw exchange: request bytes in, the status line out.
async fn exchange(addr: std::net::SocketAddr, req: &[u8]) -> String {
    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    sock.write_all(req).await.unwrap();
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.unwrap();
    String::from_utf8_lossy(&buf).into_owned()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        // The default-bounded plane: the codec-bound cap rides without a
        // single line of operator code.
        let tier = TieredCache::new_memory();
        let svc = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::CacheHandler>);
        println!(
            "default limits -> max_body={} (the codec bound + the MAC + the envelope slack)",
            svc.limits().max_body
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            svc.serve(listener).await.expect("the accept loop serves");
        });

        // 1. A legit exchange under the cap: the sealed entry stores and
        //    serves exactly as it did pre-F64.
        let entry = oxide_fw_cache::Entry {
            body: b"<html>bounded-v1</html>".to_vec(),
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
            etag: "\"bounded-v1\"".into(),
        };
        let sealed =
            oxide_fw_cache::seal_entry(&oxide_fw_cache::encode_entry(&entry).unwrap());
        let head = format!(
            "PUT /entry/route:/bounded HTTP/1.1\r\nHost: t\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            sealed.len()
        );
        let mut put = head.into_bytes();
        put.extend_from_slice(&sealed);
        let resp = exchange(addr, &put).await;
        println!(
            "legit PUT -> {} (the default cap never refuses it)",
            resp.lines().next().unwrap_or("")
        );
        let resp = exchange(
            addr,
            b"GET /entry/route:/bounded HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n",
        )
        .await;
        println!("GET after the PUT -> {}", resp.lines().next().unwrap_or(""));

        // 2. The hostile shape: a length past the cap, HEADERS ONLY — the
        //    413 arrives before the first body byte would have landed.
        let flood = format!(
            "PUT /entry/route:/flood HTTP/1.1\r\nHost: t\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            ExchangeLimits::default().max_body + 16
        );
        let resp = exchange(addr, flood.as_bytes()).await;
        println!(
            "oversized PUT (headers only) -> {} — the refusal beats the drain",
            resp.lines().next().unwrap_or("")
        );

        // 3. The operator knob: a tightened cap refuses a body the
        //    default would serve; `limits()` carries the state.
        let tight_tier = TieredCache::new_memory();
        let tight =
            HttpCacheService::new(tight_tier as Arc<dyn oxide_fw_cache::CacheHandler>)
                .with_limits(ExchangeLimits { max_body: 64 });
        println!("tightened limits -> max_body={}", tight.limits().max_body);
        let tight_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tight_addr = tight_listener.local_addr().unwrap();
        tokio::spawn(async move {
            tight.serve(tight_listener).await.expect("the accept loop serves");
        });
        let resp = exchange(tight_addr, &put).await;
        println!(
            "legit PUT against the tightened cap -> {}",
            resp.lines().next().unwrap_or("")
        );

        // 4. The explicit escape hatch: the unbounded shell drains the
        //    body and the codec refuses it — the pre-F64 shape, the
        //    host's own documented choice. (Against the DEFAULT cap the
        //    same body would be a 413 before the drain; 1 MiB rides
        //    under the codec bound so the contrast stays cheap.)
        let loose_tier = TieredCache::new_memory();
        let loose =
            HttpCacheService::new(loose_tier as Arc<dyn oxide_fw_cache::CacheHandler>)
                .with_limits(ExchangeLimits::unbounded());
        let loose_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let loose_addr = loose_listener.local_addr().unwrap();
        tokio::spawn(async move {
            loose.serve(loose_listener).await.expect("the accept loop serves");
        });
        let garbage = vec![0u8; 1024 * 1024];
        let garbage_head = format!(
            "PUT /entry/route:/unbounded HTTP/1.1\r\nHost: t\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            garbage.len()
        );
        let mut garbage_req = garbage_head.into_bytes();
        garbage_req.extend_from_slice(&garbage);
        let resp = exchange(loose_addr, &garbage_req).await;
        println!(
            "garbage body against unbounded -> {} (the codec's post-drain 400, the pre-F64 shape)",
            resp.lines().next().unwrap_or("")
        );
    });
    Ok(())
}

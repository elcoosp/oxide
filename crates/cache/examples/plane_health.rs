//! Consumer walkthrough for the V2-F58 cache plane's health arm: the §11.2
//! service answers the F52 posture pull on its own wire.
//!
//! The plane itself had NO health story — the F52 pull (`TieredCache::health()`
//! → `CacheHealth`) rode the app server's scrape and the `/-/health` probe,
//! both behind the CLI's opt-ins. `HttpCacheService::with_health` attaches a
//! [`oxide_fw_cache::PlaneHealth`] provider and `GET /health` answers the
//! pull's JSON: a dedicated plane socket (or a mounted one) becomes
//! probeable. The F49 probe's contract rides verbatim: the unconfigured
//! path 404s before anything else, the surface is GET-only, the body is the
//! LIVE pull — a scrape diffs two polls, nothing freezes it.
//!
//! Shows the story end to end:
//!
//! 1. Two entries land through the plane's `PUT` arm.
//! 2. `GET /health` — the pull reads them: two residents.
//! 3. `POST /health` — 405 (the surface is GET-only).
//! 4. A service with no provider — 404 (the F40 floor, byte-preserved).
//! 5. The arm rides the real socket shell: one raw `GET /health`.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example plane_health
//! ```

use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheService, PlaneHealth, PlaneRequest, Policy, TieredCache,
};
use std::sync::Arc;
use std::time::Duration;

fn entry(body: &[u8]) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: vec![],
        policy: Policy {
            ttl: Duration::from_secs(3600),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        // The tier classifies freshness against the REAL clock (unix ms).
        stored_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        etag: "\"health-v1\"".into(),
    }
}

async fn respond(svc: &HttpCacheService, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let resp = svc
        .respond(&PlaneRequest {
            method: method.into(),
            path: path.into(),
            body: body.to_vec(),
        })
        .await;
    (resp.status, resp.body)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    // A plane rooted at the tier it serves — the natural provider shape.
    let tier = TieredCache::new_memory();
    let provider: Arc<dyn PlaneHealth> = tier.clone();
    let svc = HttpCacheService::new(tier.clone() as Arc<dyn CacheHandler>).with_health(provider);
    let bare = HttpCacheService::new(tier.clone() as Arc<dyn CacheHandler>);
    let wired = HttpCacheService::new(tier.clone() as Arc<dyn CacheHandler>).with_health({
        let p: Arc<dyn PlaneHealth> = tier.clone();
        p
    });

    rt.block_on(async {
        // 1. Two entries land through the plane.
        for (name, body) in [("route:/health-1", "one"), ("route:/health-2", "two")] {
            let sealed = oxide_fw_cache::seal_entry(
                &oxide_fw_cache::encode_entry(&entry(body.as_bytes())).unwrap(),
            );
            let (status, _) = respond(&svc, "PUT", &format!("/entry/{name}"), &sealed).await;
            println!("PUT /entry/{name} -> {status}");
        }

        // 2. The pull reads the live state: two residents.
        let (status, body) = respond(&svc, "GET", "/health", &[]).await;
        println!("GET /health -> {status} {}", String::from_utf8_lossy(&body));

        // 3. The surface is GET-only.
        let (status, _) = respond(&svc, "POST", "/health", &[]).await;
        println!("POST /health -> {status} (the surface is GET-only)");

        // 4. No provider, no arm: the F40 floor.
        let (status, _) = respond(&bare, "GET", "/health", &[]).await;
        println!("GET /health (no provider) -> {status} (the pre-F58 404)");

        // 5. The arm rides the real socket shell.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            wired.serve(listener).await.expect("the accept loop serves");
        });
        let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        sock.write_all(b"GET /health HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut buf = Vec::new();
        sock.read_to_end(&mut buf).await.unwrap();
        let head = String::from_utf8_lossy(&buf);
        println!("wire GET /health -> {}", head.lines().next().unwrap_or(""));
    });
    Ok(())
}

//! Consumer walkthrough for the V2-F57 cache plane's mount: the app server
//! hosts the §11.2 shared plane on its own socket.
//!
//! F53 minted the plane's HTTP service (`HttpCacheService`) — the client's
//! missing server half — and until now nothing in the workspace hosted it.
//! The mount is the host: `Pipeline::with_cache_plane` answers the plane's
//! three path families (`/entry/{key}`, `/tag/{tag}`, `/invalidate/{tag}`)
//! through the service's pure routing core, so ONE node serves the site AND
//! the shared plane on one port — a second node points its
//! `cache.handler = { type: 'http', url }` at the mount and the plane is
//! shared with no Redis/S3 dependency.
//!
//! Shows the story end to end (every exchange rides the full pipeline —
//! the native prefix's family match, the service's routing core, the
//! pipeline's finish/trace shape):
//!
//! 1. `PUT /entry/{key}` — a client's sealed entry lands through the mount
//!    (`204`, the trust boundary enforced where the payload writes).
//! 2. `GET /entry/{key}` — the sealed bytes come back verbatim (`200`).
//! 3. A forged (unsealed) `PUT` — `400`, and nothing stored.
//! 4. `GET /tag/{tag}` → the key list; `POST /invalidate/{tag}` → the
//!    count; the entry misses afterward — the §11.2 shared-plane contract.
//! 5. The F40 floor: with no mount attached, a plane-shaped path falls
//!    through to dispatch byte-identically.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-server --example plane_mount
//! ```

use bytes::Bytes;
use futures::future::BoxFuture;
use oxide_fw_cache::{
    CacheHandler, Entry, HttpCacheService, Policy, TieredCache, encode_entry, seal_entry,
};
use oxide_fw_server::{OxideRequest, OxideResponse, Pipeline};
use std::sync::Arc;
use std::time::Duration;

fn oxide_req(method: &str, url: &str, body: &[u8]) -> OxideRequest {
    OxideRequest {
        method: method.into(),
        url: url.into(),
        headers: Arc::new(Vec::new()),
        header_index: Default::default(),
        body: Bytes::copy_from_slice(body),
        request_id: "example-plane-mount".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn echo_dispatch() -> Arc<dyn Fn(OxideRequest) -> BoxFuture<'static, OxideResponse> + Send + Sync> {
    Arc::new(|r: OxideRequest| {
        Box::pin(async move { OxideResponse::html(200, format!("dispatch:{}", r.url)) })
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The backing store a real deployment hosts (a `TieredCache` IS a
    // `CacheHandler`), composed behind the F53 service, mounted on the
    // pipeline.
    let tier = TieredCache::new_memory();
    let plane = HttpCacheService::new(tier as Arc<dyn CacheHandler>);
    let mounted = Arc::new(Pipeline::default().with_cache_plane(Arc::new(plane)));
    let unmounted = Pipeline::default();
    let dispatch = echo_dispatch();
    let dispatch_ref: &dyn Fn(OxideRequest) -> BoxFuture<'static, OxideResponse> =
        dispatch.as_ref();

    let entry = Entry {
        body: b"<html>shared across nodes</html>".to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: vec!["mount-tag".to_string()],
        policy: Policy {
            ttl: Duration::from_secs(60),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        // The tier classifies freshness against the REAL clock (unix ms) —
        // a stamp near `now` keeps the walkthrough's entry fresh.
        stored_at: now_ms(),
        etag: "\"mount-v1\"".into(),
    };
    let sealed = seal_entry(&encode_entry(&entry)?);

    println!("node A serves the site AND the shared plane on one socket:\n");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    // 1. A second node's client PUTs the sealed entry at the mount.
    let resp = rt.block_on(mounted.serve_request(
        oxide_req("PUT", "/entry/route:/mount-1", &sealed),
        dispatch_ref,
    ));
    println!("PUT /entry/route:/mount-1 -> {} (stored)", resp.status);

    // 2. The same (or another) node reads it back through the mount.
    let resp = rt.block_on(
        mounted.serve_request(oxide_req("GET", "/entry/route:/mount-1", &[]), dispatch_ref),
    );
    println!(
        "GET /entry/route:/mount-1 -> {} ({} sealed bytes, verbatim: {})",
        resp.status,
        resp.body.len(),
        resp.body.as_ref() == sealed
    );

    // 3. A forged (unsealed) PUT never enters the store.
    let resp = rt.block_on(mounted.serve_request(
        oxide_req("PUT", "/entry/route:/forged", b"garbage-not-sealed"),
        dispatch_ref,
    ));
    let after = rt.block_on(
        mounted.serve_request(oxide_req("GET", "/entry/route:/forged", &[]), dispatch_ref),
    );
    println!(
        "PUT /entry/route:/forged -> {} (refused), GET -> {} (nothing stored)",
        resp.status, after.status
    );

    // 4. The tag list, the invalidation, and the coherent miss after.
    let resp =
        rt.block_on(mounted.serve_request(oxide_req("GET", "/tag/mount-tag", &[]), dispatch_ref));
    println!(
        "GET /tag/mount-tag -> {} {}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );
    let resp = rt.block_on(mounted.serve_request(
        oxide_req("POST", "/invalidate/mount-tag", b"{\"soft\":false}"),
        dispatch_ref,
    ));
    println!(
        "POST /invalidate/mount-tag -> {} {}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );
    let resp = rt.block_on(
        mounted.serve_request(oxide_req("GET", "/entry/route:/mount-1", &[]), dispatch_ref),
    );
    println!(
        "GET /entry/route:/mount-1 -> {} (the invalidation crossed)",
        resp.status
    );

    // 5. The F40 floor: no mount, no seam — the plane path falls through.
    let resp = rt.block_on(unmounted.serve_request(
        oxide_req("GET", "/entry/route:/fallthrough", &[]),
        dispatch_ref,
    ));
    println!(
        "\nunmounted GET /entry/route:/fallthrough -> {} {}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );
    Ok(())
}

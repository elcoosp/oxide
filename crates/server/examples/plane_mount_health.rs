//! Consumer walkthrough for the V2-F59 mounted cache plane's health arm:
//! the F57 mount × the F58 arm, composed.
//!
//! F57 mounted the §11.2 plane on the app server's socket, but the mount
//! predates F58: its path check routed only the three protocol families,
//! so the health arm — "the probe a balancer can point at the shared
//! node" — was unreachable on the mount. F59 closes that seam: a mount
//! whose service carries a provider ([`HttpCacheService::with_health`])
//! answers `GET /health` with the live posture pull's JSON on the site's
//! own socket, and a provider-less mount leaves `/health` to the
//! application (an unconfigured plane must never shadow an app route —
//! the F40 doctrine).
//!
//! Shows the story end to end (every exchange rides the full pipeline):
//!
//! 1. The healthy mount (the service over the boot's own tier, the tier
//!    doubling as the provider — one `Arc`, both seams): `GET /health` →
//!    `200` + the `CacheHealth` JSON (the balancer's surface).
//! 2. `POST /health` on the same mount → `405` — the plane's method
//!    table owns the surface (GET-only, the F58 contract).
//! 3. The provider-less mount: `GET /health` falls through to dispatch —
//!    the application's own `/health` route survives.
//! 4. No mount at all: the pre-F57 shape, byte-identical.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-server --example plane_mount_health
//! ```

use bytes::Bytes;
use futures::future::BoxFuture;
use oxide_fw_cache::{CacheHandler, HttpCacheService, TieredCache};
use oxide_fw_server::{OxideRequest, OxideResponse, Pipeline};
use std::sync::Arc;

fn oxide_req(method: &str, url: &str, body: &[u8]) -> OxideRequest {
    OxideRequest {
        method: method.into(),
        url: url.into(),
        headers: Arc::new(Vec::new()),
        header_index: Default::default(),
        body: Bytes::copy_from_slice(body),
        request_id: "example-plane-mount-health".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

fn echo_dispatch() -> Arc<dyn Fn(OxideRequest) -> BoxFuture<'static, OxideResponse> + Send + Sync> {
    Arc::new(|r: OxideRequest| {
        Box::pin(async move { OxideResponse::html(200, format!("dispatch:{}", r.url)) })
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The healthy mount, the F59 composition verbatim: the service over
    // the boot's own tier, the tier doubling as the F58 health provider.
    let tier = TieredCache::new_memory();
    let healthy = Arc::new(
        HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>)
            .with_health(Arc::clone(&tier) as Arc<dyn oxide_fw_cache::PlaneHealth>),
    );
    let healthy_mounted = Arc::new(Pipeline::default().with_cache_plane(Arc::clone(&healthy)));

    // The provider-less mount: a host that mounted the plane for its
    // entry families only. Its `/health` is NOT the plane's.
    let plain = Arc::new(HttpCacheService::new(
        Arc::clone(&tier) as Arc<dyn CacheHandler>
    ));
    let plain_mounted = Arc::new(Pipeline::default().with_cache_plane(Arc::clone(&plain)));
    let unmounted = Pipeline::default();

    let dispatch = echo_dispatch();
    let dispatch_ref: &dyn Fn(OxideRequest) -> BoxFuture<'static, OxideResponse> =
        dispatch.as_ref();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    println!("the shared node's balancer probe lives on the app server's socket:\n");

    // 1. The balancer's pull: the live posture on the mounted surface.
    let resp =
        rt.block_on(healthy_mounted.serve_request(oxide_req("GET", "/health", &[]), dispatch_ref));
    println!(
        "GET /health (provider-attached mount) -> {} {}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );

    // 2. The method table rides the plane's own: GET-only.
    let resp =
        rt.block_on(healthy_mounted.serve_request(oxide_req("POST", "/health", &[]), dispatch_ref));
    println!(
        "POST /health (provider-attached mount) -> {} (the plane's 405)",
        resp.status
    );

    // 3. The user-route contract: a provider-less mount leaves /health
    //    to the application.
    let resp =
        rt.block_on(plain_mounted.serve_request(oxide_req("GET", "/health", &[]), dispatch_ref));
    println!(
        "\nGET /health (provider-less mount) -> {} {}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );

    // 4. The pre-F57 floor: no mount, no seam.
    let resp = rt.block_on(unmounted.serve_request(oxide_req("GET", "/health", &[]), dispatch_ref));
    println!(
        "GET /health (unmounted)            -> {} {}",
        resp.status,
        String::from_utf8_lossy(&resp.body)
    );
    Ok(())
}

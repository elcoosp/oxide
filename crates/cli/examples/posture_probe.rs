//! Consumer example for the V2-F49 cache posture probe: the `/-/health`
//! production surface the F48 handoff recorded as the posture pull's
//! second tail. The F32 pull (`TieredCache::health` → `CacheHealth`) is
//! already the value shape a probe wants — the probe surface hands it
//! over the wire as JSON, and this walkthrough drives the WHOLE seam in
//! one process: the gate, the adapter, the server-side arm, and the
//! request the probe answers.
//!
//! Shows the four pieces a contributor (or a host author) composes:
//!
//! 1. [`oxide_fw_cli::posture_probe_provider`] — the boot's two-halves
//!    gate (the F40 doctrine on a surface): the opt-in resolves (the
//!    `cache_plane_health` family: the `--cache-plane-health` flag, the
//!    `OXIDE_CACHE_PLANE_HEALTH` env spelling, the `cache.healthMetrics`
//!    baseline) AND a typed tier handle must BOTH hold, or the provider
//!    attaches nothing and the probe path 404s.
//! 2. [`oxide_fw_cli::TierCachePosture`] — the CLI-side adapter behind
//!    the server crate's cache-agnostic [`oxide_fw_server::CachePosture`]
//!    trait (the F39 doctrine: the server never names the cache crate).
//! 3. [`oxide_fw_cli::cache_posture_probe_json`] — the wire body the
//!    surface answers: one posture pull rendered as the JSON a probe
//!    serializes, over a tier with real resident entries (nothing
//!    fabricated).
//! 4. The server arm: a real [`oxide_fw_server::Pipeline`] with
//!    `posture_path` set and the provider attached, driven through
//!    `serve_request` — `GET /-/health` answers 200 + `application/json`
//!    + `cache-control: no-store`, the unattached path 404s "not
//!    configured", the default (empty) path falls through to the
//!    dispatch.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cli --example posture_probe
//! ```

use std::sync::Arc;

use bytes::Bytes;
use oxide_fw_cli::{TierCachePosture, cache_posture_probe_json, posture_probe_provider};
use oxide_fw_server::{CachePosture, OxideRequest, OxideResponse, Pipeline};

fn main() {
    // A small tier with real resident entries (the write-through sets the
    // posture counts — the "nothing is fabricated" contract the cache
    // crate's health module pins).
    let tier = Arc::new(oxide_fw_cache::TieredCache::new(4 * 1024 * 1024));
    let rt = tokio::runtime::Runtime::new().expect("the example's tokio runtime");
    rt.block_on(async {
        for (i, key) in ["/routes/index", "/routes/docs", "/routes/api"]
            .into_iter()
            .enumerate()
        {
            tier.set(
                key,
                oxide_fw_cache::Entry {
                    body: format!("<html>{i}</html>").into_bytes(),
                    status: 200,
                    headers: vec![("content-type".into(), "text/html".into())],
                    tags: vec!["demo".into()],
                    policy: oxide_fw_cache::Policy {
                        ttl: std::time::Duration::from_secs(60),
                        swr: None,
                        stale_if_error: None,
                        soft_tags: true,
                    },
                    stored_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("the wall clock runs")
                        .as_millis() as u64,
                    etag: "\"v1\"".into(),
                },
            )
            .await
            .expect("the tier takes the write");
        }
    });

    // 1. The gate: BOTH halves hold, or the provider attaches nothing.
    //    The opt-in is the same family the §20 scrape arm reads — the
    //    `cache.healthMetrics` baseline here stands in for the flag/env
    //    layers the boot composes ahead of it.
    let opted_in = oxide_fw_cli::observability::cache_plane_health_with_config(Some(true));
    println!("opted in (cache.healthMetrics = true): {opted_in}");
    println!(
        "gate(no opt-in, tier)    attaches: {:?}",
        posture_probe_provider(false, Some(&tier)).is_some()
    );
    println!(
        "gate(opt-in, no tier)    attaches: {:?}",
        posture_probe_provider(true, None).is_some()
    );
    let provider = posture_probe_provider(opted_in, Some(&tier))
        .expect("the opt-in over a live tier attaches");
    println!(
        "gate(opt-in, tier)       attaches: {}",
        provider_is_attached(&provider)
    );

    // 2/3. The adapter's wire body over the live tier: the exact bytes
    //      `GET /-/health` answers. The posture carries counts and sizes,
    //      never routes or keys — the probe-friendly half of the stack's
    //      posture.
    let json = cache_posture_probe_json(&tier);
    println!("--- the wire body (TierCachePosture::posture_json) ---");
    println!("{json}");

    // 4. The server arm, driven through the real pipeline: the provider
    //    attached via the chainable builder, the path registered, three
    //    requests through `serve_request` — the wired probe answers 200
    //    JSON with `no-store`, the POST falls through to the dispatch,
    //    the unattached pipeline 404s "not configured".
    let pipeline = Arc::new(
        Pipeline::default()
            .with_cache_posture(Arc::new(TierCachePosture::new(Arc::clone(&tier))))
            .with_posture_path("/-/health"),
    );
    let dispatch = |req: OxideRequest| {
        Box::pin(async move {
            OxideResponse::html(200, format!("dispatched {} {}", req.method, req.url))
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = OxideResponse> + Send>>
    };
    rt.block_on(async {
        let get = get_request("/-/health");
        let resp = pipeline.serve_request(get, &dispatch).await;
        println!("--- the wire ---");
        println!(
            "GET /-/health          -> {} {:?} (content-type {:?})",
            resp.status,
            String::from_utf8_lossy(&resp.body),
            resp.headers
                .iter()
                .find(|(k, _)| k == "content-type")
                .map(|(_, v)| v.as_str()),
        );
        let post = post_request("/-/health");
        let resp = pipeline.serve_request(post, &dispatch).await;
        println!(
            "POST /-/health         -> {} {}",
            resp.status,
            String::from_utf8_lossy(&resp.body)
        );
        let bare = Arc::new(Pipeline::default().with_posture_path("/-/health"));
        let resp = bare
            .serve_request(get_request("/-/health"), &dispatch)
            .await;
        println!(
            "GET /-/health (no provider) -> {} {}",
            resp.status,
            String::from_utf8_lossy(&resp.body)
        );
    });
}

/// The provider is opaque behind the trait object — the honest attachment
/// check reads the trait method back (the wire body it will serve).
fn provider_is_attached(provider: &Arc<dyn CachePosture>) -> bool {
    !provider.posture_json().is_empty()
}

fn get_request(url: &str) -> OxideRequest {
    OxideRequest {
        method: "GET".into(),
        url: url.into(),
        headers: Arc::new(Vec::new()),
        header_index: Default::default(),
        body: Bytes::new(),
        request_id: "example-get".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

fn post_request(url: &str) -> OxideRequest {
    OxideRequest {
        method: "POST".into(),
        url: url.into(),
        headers: Arc::new(Vec::new()),
        header_index: Default::default(),
        body: Bytes::new(),
        request_id: "example-post".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

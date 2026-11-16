//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

pub(crate) fn build_og_endpoint() -> Option<std::sync::Arc<oxide_fw_server::OgEndpoint>> {
    Some(std::sync::Arc::new(oxide_fw_server::OgEndpoint {
        backend: std::sync::Arc::new(SvgOgBackendImpl),
    }))
}

pub(crate) fn build_image_endpoint(
    rootp: &Path,
    cfg: &oxide_fw_config::OxideConfig,
) -> Option<std::sync::Arc<oxide_fw_server::ImageEndpoint>> {
    // Asset root: the same `dist/` the static-asset store scans, so an
    // image imported at build time is reachable at the URL the bundler
    // emitted it under.
    let dist = rootp.join("dist");
    let public = rootp.join("public");
    let asset_root = if dist.is_dir() {
        dist
    } else if public.is_dir() {
        public
    } else {
        // No asset directory means no local sources to transform; the
        // endpoint would 404 every request. Leave it unset.
        return None;
    };

    // Signing: opt-in. A `OXIDE_IMAGE_SECRET` environment variable turns
    // the check on; without it the endpoint is open, which is the right
    // default for a dev server and for a project that serves its own
    // images. Production deployments that proxy third-party URLs set it.
    // SEC-08: the image-signing secret defaults to the project's build
    // secret (deterministic, `.oxide/secret`) unless `OXIDE_IMAGE_SECRET`
    // overrides it or the operator explicitly disables signing. Before this
    // the endpoint was unsigned by default.
    let signing_secret = std::env::var("OXIDE_IMAGE_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("OXIDE_IMAGE_UNSIGNED")
                .is_ok()
                .then(String::new)
        })
        .or_else(|| build_secret(rootp).ok());
    let _ = &cfg.images; // reserved: remotePatterns gating lands with the fetch path

    // §16.1: remote sources are gated by `config.images.remotePatterns`.
    // The config type carries `host` + optional `protocol`; the server's
    // `RemoteImagePattern` is the same shape, so a straight map.
    let remote_patterns: Vec<oxide_fw_server::RemoteImagePattern> = cfg
        .images
        .remote_patterns
        .iter()
        .map(|p| oxide_fw_server::RemoteImagePattern {
            host: p.host.clone(),
            protocol: p.protocol.clone(),
        })
        .collect();

    Some(std::sync::Arc::new(oxide_fw_server::ImageEndpoint {
        backend: std::sync::Arc::new(AssetsImageBackend),
        asset_root,
        signing_secret,
        remote_patterns,
        // §17.1: a project that puts its CDN on a private network adds the
        // host here. Empty by default — the SSRF check blocks the ranges
        // that matter (loopback, RFC1918, link-local).
        ssrf_allow_list: Vec::new(),
        // §16.1: a source larger than 32 MiB is almost certainly a mistake
        // or a hostile payload; the fetch refuses it.
        remote_max_bytes: 32 * 1024 * 1024,
        result_cache: std::sync::Arc::new(std::sync::Mutex::new(Default::default())),
        max_concurrency: cfg.images.max_concurrency,
        in_flight: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        max_dimension: cfg.images.max_dimension,
    }))
}

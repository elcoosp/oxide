//! V2-F63 recorded pickup walkthrough: the `cache.handler = { type: 'http' }`
//! node's own boot line — the serve loop's ONE bounded probe over the F63
//! `PlanePeer` seam, the posture rendered byte-stably, and the loud failure
//! arm (a provider-less plane is an error, never a fabricated zeroed
//! posture; the boot continues on the local tiers, §11.2).
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cli --no-default-features --example plane_boot_line
//! ```

use std::sync::Arc;

use oxide_fw_cache::{
    CacheHandler, HttpCacheHandler, HttpCacheService, MetricsCacheObserver, MetricsPlaneSource,
    PlaneHealth, PlaneMetrics, TieredCache,
};
use oxide_fw_cli::{PLANE_BOOT_PROBE_DEADLINE, remote_plane_boot_line};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        // A live plane on this process's socket — the boot's own
        // construction: the tier rooted as both the backing store and the
        // F58 health provider, the §20 exporter fanned onto it and
        // composed into the F61 metrics source.
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let metrics = Arc::new(MetricsCacheObserver::new());
        let tier = Arc::new(
            TieredCache::new(64 * 1024 * 1024)
                .with_observer(metrics.clone() as Arc<dyn oxide_fw_cache::CacheObserver>),
        );
        let served = HttpCacheService::new(Arc::clone(&tier) as Arc<dyn CacheHandler>)
            .with_health(Arc::clone(&tier) as Arc<dyn PlaneHealth>)
            .with_metrics(Arc::new(MetricsPlaneSource::new(
                metrics,
                tier.clone() as Arc<dyn PlaneHealth>,
            )) as Arc<dyn PlaneMetrics>);
        tokio::spawn(async move {
            served.serve(l).await.expect("the accept loop serves");
        });

        // The boot line: one bounded probe, the byte-stable render. This
        // is what `oxide start` prints when the config points at a plane.
        let line = remote_plane_boot_line(&format!("http://{addr}"), PLANE_BOOT_PROBE_DEADLINE)
            .await
            .expect("the live plane answers");
        println!("info: {line}");

        // The loud failure arm: a provider-less plane (no F58 health
        // provider attached) answers the F40 404 for /health. The boot
        // warns and continues — the remote cache is an optimization.
        let bare = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bare_addr = bare.local_addr().unwrap();
        let bare_svc = HttpCacheService::new(
            Arc::new(TieredCache::new(64 * 1024 * 1024)) as Arc<dyn CacheHandler>
        );
        tokio::spawn(async move {
            bare_svc.serve(bare).await.expect("the accept loop serves");
        });
        match remote_plane_boot_line(&format!("http://{bare_addr}"), PLANE_BOOT_PROBE_DEADLINE)
            .await
        {
            Ok(line) => println!("info: {line}"),
            Err(e) => println!(
                "warn [{}]: {} — continuing on the local tiers (§11.2)",
                e.0.code, e.0.message
            ),
        }

        // The F60 deadline arm's shape: the same probe over a client the
        // boot never builds a line for — a plane URL that answers nothing
        // degrades inside the bound, never hangs the boot. (The deadline
        // ride itself is pinned in the cache crate's deadline suite.)
        let _ = HttpCacheHandler::new("http://127.0.0.1:1").with_timeout(PLANE_BOOT_PROBE_DEADLINE);
        println!("the boot probe's client is throwaway — the serving client's shape is untouched");
    });
    Ok(())
}

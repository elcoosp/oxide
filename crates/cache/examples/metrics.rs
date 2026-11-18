//! The cache plane's metrics exporter walkthrough (`metrics.rs`, §11.2/§20):
//! the F28 observer seam's named structured consumer — a metrics exporter —
//! is now IN the crate. The walkthrough attaches one
//! `MetricsCacheObserver` through the F28 builder, drives real traffic
//! through the tiered cache, and shows both scrape shapes:
//!
//! 1. the value mirror (`snapshot()`) — the `Clone` + `Eq` record a host
//!    diffs across a window;
//! 2. the §20 Prometheus text exposition (`render_prometheus()`) — the
//!    `oxide_cache_plane_*` family (the §11.2 tiered store's own rows,
//!    distinct from the request plane's `oxide_cache_l0_*` page-render
//!    family), what a scraper polls and a dashboard graphs.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example metrics
//! ```

use std::sync::Arc;
use std::time::Duration;

use oxide_fw_cache::{Entry, MetricsCacheObserver, Policy, TieredCache};

fn entry(body: &[u8], tags: &[&str]) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: tags.iter().map(|s| s.to_string()).collect(),
        policy: Policy {
            ttl: Duration::from_secs(60),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the wall clock runs")
            .as_millis() as u64,
        etag: "\"v1\"".into(),
    }
}

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        // 1. Attach: the exporter rides the F28 seam (`with_observer`,
        //    the `Arc<dyn CacheObserver>` coercion); the host keeps one
        //    clone for scraping while the cache owns the seam half.
        let metrics = Arc::new(MetricsCacheObserver::new());
        let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(metrics.clone()));

        // 2. Traffic: a write-through set, a fresh L0 hit, a miss, a
        //    delete and its follow-up miss, and a tag invalidation over
        //    two keys (one aggregate row, not one per key).
        cache
            .set("route:/a", entry(b"<html>a</html>", &["content:posts"]))
            .await
            .expect("the memory tier accepts the entry");
        cache
            .set("route:/b", entry(b"<html>b</html>", &["content:posts"]))
            .await
            .expect("the memory tier accepts the entry");
        let _ = cache.get("route:/a").await.expect("the memory tier");
        let _ = cache.get("route:/missing").await.expect("the memory tier");
        cache.delete("route:/b").await.expect("the memory tier");
        let _ = cache.get("route:/b").await.expect("the memory tier");
        let affected = cache
            .invalidate_tag("content:posts", false)
            .await
            .expect("the tag index");
        println!("=== the traffic story ===");
        println!("invalidate_tag('content:posts') affected {affected} key(s)\n");

        // 3. The value mirror: `Clone` + `Eq` + `Default` — copy it
        //    before and after a window and diff the values.
        let snap = metrics.snapshot();
        println!("=== snapshot() — the value mirror ===");
        println!("{snap:#?}\n");

        // 4. The §20 text exposition: the family a scraper polls. The
        //    counters are monotone — a scrape is a delta over the
        //    scraper's own window.
        println!("=== render_prometheus() — the §20 exposition ===");
        print!("{}", metrics.render_prometheus());

        // 5. The arm is infallible end to end: no error codes, no
        //    registry rows — the counters cannot fail, the render cannot
        //    fail (the F21 posture for a seam without refusals). The
        //    arithmetic is the sweep's: the hard invalidation deletes
        //    each affected key through the public `delete` path, so the
        //    sweep's per-key traffic emits its own `Delete` rows beside
        //    the aggregate `Invalidate` row — the story above shows one
        //    explicit delete plus one swept delete (deletes_total 2)
        //    against the one-row invalidate.
    });
}

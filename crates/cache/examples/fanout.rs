//! The cache plane's observer fanout walkthrough (`fanout.rs` /
//! `observer.rs`, §20): the seam's composition arm (F34) — one event into
//! every child observer, behind the ONE `with_observer` slot.
//!
//! The walkthrough runs the exact attach a dashboard boot runs: the
//! tracing arm (the subscriber renders the rows), the metrics exporter
//! (the `oxide_cache_plane_*` exposition) and the bounded feed (the
//! heavy consumer's FIFO) ride one fan over one attach point — the
//! inline hand-roll the `coalesce` walkthrough carried before the type
//! shipped, now a crate shape. The read-back shows the three consumers
//! describing the same events: the exporter's counters, the feed's FIFO
//! and the posture's `observer_attached` flag.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example fanout
//! ```

use std::sync::Arc;
use std::time::Duration;

use oxide_fw_cache::{
    CacheObserver, Entry, EventFeed, FanoutCacheObserver, MetricsCacheObserver, Policy,
    TieredCache, TracingCacheObserver,
};

fn entry(body: &[u8], tags: &[&str]) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: tags.iter().copied().map(str::to_string).collect(),
        policy: Policy {
            ttl: Duration::from_secs(60),
            swr: Some(Duration::from_secs(120)),
            stale_if_error: None,
            soft_tags: true,
        },
        stored_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the wall clock runs")
            .as_millis()
            .try_into()
            .unwrap_or_default(),
        etag: "\"v1\"".into(),
    }
}

fn main() {
    // The three shipped consumers, one fan, one attach point.
    let metrics = Arc::new(MetricsCacheObserver::new());
    let feed = Arc::new(EventFeed::bounded(1024));
    let fan = FanoutCacheObserver::shared(vec![
        Arc::new(TracingCacheObserver::new()) as Arc<dyn CacheObserver>,
        metrics.clone() as Arc<dyn CacheObserver>,
        feed.clone() as Arc<dyn CacheObserver>,
    ]);
    let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(fan));

    // Real traffic through the composed seam: a write-through, a fresh
    // hit, a miss, a stale hit inside the SWR window, a tag invalidation
    // and one recorded flight.
    let tokio = tokio::runtime::Runtime::new().expect("the walkthrough runtime");
    tokio.block_on(async {
        cache
            .set("route:/blog/a", entry(b"<html>hello</html>", &["content:posts"]))
            .await
            .expect("the memory tier accepts the entry");
        let _ = cache.get("route:/blog/a").await.expect("the memory tier");
        let _ = cache.get("route:/missing").await.expect("the memory tier");

        let mut stale = entry(b"<html>stale</html>", &["content:posts"]);
        stale.stored_at = stale
            .stored_at
            .saturating_sub(u64::try_from(Duration::from_secs(90).as_millis()).unwrap_or_default());
        cache
            .set("route:/blog/b", stale)
            .await
            .expect("the memory tier accepts the entry");
        let _ = cache.get("route:/blog/b").await.expect("the memory tier");

        let affected = cache
            .invalidate_tag("content:posts", true)
            .await
            .expect("the memory tier invalidates");
        cache.record_coalesced("route:/blog/a", 3);

        println!("== the fan rides ONE slot: {:#?}", cache.health());
        println!("== the exporter's counters (child 2):");
        let snap = metrics.snapshot();
        println!(
            "   sets={} hits(L0)={} stale_hits={} misses={} invalidations={} invalidated_keys={} coalesce_events={} coalesced_followers={}",
            snap.sets,
            snap.hits_l0,
            snap.stale_hits,
            snap.misses,
            snap.invalidations,
            snap.invalidated_keys,
            snap.coalesce_events,
            snap.coalesced_followers,
        );
        println!("== the feed's FIFO (child 3), {} rows in arrival order:", feed.len());
        for (key, op) in feed.drain() {
            println!("   [{key}] {op:?}");
        }
        println!("== the exposition excerpt (child 2 renders):");
        for line in metrics.render_prometheus().lines() {
            if line.starts_with("oxide_cache_plane_hits_total")
                || line.starts_with("oxide_cache_plane_stale_hits_total 1")
                || line.starts_with("oxide_cache_plane_invalidations_total")
            {
                println!("   {line}");
            }
        }
        let _ = affected; // the sweep's key count, already on the invalidate row
    });
}

//! The cache plane's boot-posture walkthrough (`serve` boot shape, §20):
//! what a host assembles when it boots the plane — the F34 fan over the
//! F30 exporter and the F35-gated feed, the flight owner recording its
//! F33 join count through the publish it used to drop, and BOTH §20
//! expositions a scrape reads: the exporter's `oxide_cache_plane_*`
//! counters and the F32 posture's gauges.
//!
//! This is the wire the CLI's serve boot runs
//! (`crates/cli/src/serve/start.rs` attaches the exporter over the
//! project's `TieredCache`, and the render stage's owner site records
//! the publish count) — replayed here against one in-process cache so
//! the whole posture is visible in one run: attach, fly, record, scrape.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example boot_posture
//! ```

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use oxide_fw_cache::{
    CacheEvent, Entry, EventFeed, FanoutCacheObserver, FlightSlot, KindGate,
    MetricsCacheObserver, Policy, SingleFlight, TieredCache,
};

fn entry(body: &[u8]) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: vec![],
        policy: Policy {
            ttl: Duration::from_secs(60),
            swr: None,
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
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        // 1. The boot posture: one `with_observer` slot, three consumers
        //    behind the fan — the exporter for the scrape, the bounded
        //    feed narrowed by a deploy-watch gate (only invalidations and
        //    coalesce rows ride it; hot-path rows stay off the wire), and
        //    the tracing arm for the subscriber.
        let metrics = Arc::new(MetricsCacheObserver::new());
        let feed = Arc::new(EventFeed::gated(
            64,
            Arc::new(KindGate::only(&["invalidate", "coalesce"])),
        ));
        let cache = Arc::new(TieredCache::new(4 * 1024 * 1024).with_observer(
            FanoutCacheObserver::shared(vec![
                metrics.clone() as Arc<dyn oxide_fw_cache::CacheObserver>,
                feed.clone() as Arc<dyn oxide_fw_cache::CacheObserver>,
                Arc::new(oxide_fw_cache::TracingCacheObserver::new())
                    as Arc<dyn oxide_fw_cache::CacheObserver>,
            ]),
        ));

        // 2. One flight, two followers — the render stage's shape: the
        //    owner acquires, the staggered requests coalesce onto it.
        let flights: DashMap<String, Arc<tokio::sync::broadcast::Sender<Arc<Entry>>>> =
            DashMap::new();
        let single = SingleFlight::new();
        let owner = single.acquire(&flights, "route:/a").await;
        let mut followers = Vec::new();
        for _ in 0..2 {
            followers.push(single.acquire(&flights, "route:/a").await);
        }

        // 3. The owner completes: the publish RETURNS the join count (the
        //    F36 wire — the CLI render stage records exactly this), and
        //    the owner records the flight through the typed half.
        let FlightSlot::Owner(tx) = owner else {
            panic!("the first acquirer owns the flight");
        };
        let guard =
            oxide_fw_cache::FlightOwnerGuard::new(Arc::clone(&flights), "route:/a".into(), tx);
        let joins = guard.publish(Arc::new(entry(b"<html>a</html>")));
        let joins = u64::try_from(joins).unwrap_or(u64::MAX);
        cache.record_coalesced("route:/a", joins);

        // 4. The followers receive the owner's entry.
        for slot in followers {
            if let FlightSlot::Follower(mut rx) = slot {
                let got = rx.recv().await.expect("the owner published");
                assert_eq!(got.body, b"<html>a</html>");
            }
        }

        // 5. One invalidation row — the deploy-watch intake's other half.
        cache
            .invalidate_tag("route:/a", true)
            .await
            .expect("the soft invalidation succeeds");

        // 6. The posture: both expositions a scrape reads — the
        //    exporter's counters (the deltas since attach) and the F32
        //    gauges (the stack as it stands NOW), beside the pull
        //    channel's counter and the gated feed's rows.
        let snap = metrics.snapshot();
        println!("pull stats().coalesced          = {}", cache.stats().coalesced);
        println!("exporter coalesce_events        = {}", snap.coalesce_events);
        println!(
            "exporter coalesced_followers    = {}",
            snap.coalesced_followers
        );
        println!(
            "exporter invalidations          = {}",
            snap.invalidations
        );
        println!(
            "health observer_attached        = {}",
            cache.health().observer_attached
        );
        println!("\nthe gated feed (admission narrowed the intake):");
        for (key, op) in feed.drain() {
            let kind = CacheEvent { key: &key, op }.kind();
            println!("  feed row: {key} -> {kind}");
        }
        println!("feed dropped (saturation)       = {}", feed.dropped());
        println!("\nexporter scrape (the oxide_cache_plane_* families):");
        print!("{}", metrics.render_prometheus());
        println!("\nposture scrape (the gauges, one boot sample):");
        println!("{}", cache.health().render_prometheus());
    });
}

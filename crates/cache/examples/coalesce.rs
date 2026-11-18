//! The cache plane's coalesce parity walkthrough (`observer.rs` /
//! `single_flight.rs` / `tiers.rs`, §11.3/§20): the single-flight joins
//! join the §20 push channel (F33) — the push twin of the pull channel's
//! `stats().coalesced` counter.
//!
//! The walkthrough runs the exact shape the serving process runs: a
//! single-flight owner renders while followers coalesce onto it, the
//! owner completes and records the flight, and the row lands on every
//! in-crate consumer — the metrics exporter's
//! `oxide_cache_plane_coalesce_*` families and the bounded feed's FIFO.
//! The two consumers ride one attach point through a tiny inline fanout
//! impl — the seam's "a custom impl for anything else" contract, in
//! four lines.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example coalesce
//! ```

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use oxide_fw_cache::{CacheEvent, CacheObserver, Entry, MetricsCacheObserver, Policy, TieredCache};

/// The host's fanout: one record into every consumer (the server crate's
/// `FanoutTraceSink` posture, inline — the `CacheObserver` impl is the
/// whole seam).
struct FanoutObserver {
    a: Arc<MetricsCacheObserver>,
    b: Arc<oxide_fw_cache::EventFeed>,
}

impl CacheObserver for FanoutObserver {
    fn on_event(&self, event: &CacheEvent<'_>) {
        self.a.on_event(event);
        self.b.on_event(event);
    }
}

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
            .as_millis() as u64,
        etag: "\"v1\"".into(),
    }
}

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        // 1. Attach the consumers: the exporter stays with the host for
        //    scraping, the bounded feed stands in for the off-process
        //    pipeline — both through one inline fanout on the seam.
        let metrics = Arc::new(MetricsCacheObserver::new());
        let feed = Arc::new(oxide_fw_cache::EventFeed::bounded(64));
        let cache = Arc::new(TieredCache::new(4 * 1024 * 1024).with_observer(Arc::new(
            FanoutObserver {
                a: metrics.clone(),
                b: feed.clone(),
            },
        )));

        // 2. One flight, three followers: the owner renders, the
        //    followers await its broadcast.
        let flights: DashMap<String, Arc<tokio::sync::broadcast::Sender<Arc<Entry>>>> =
            DashMap::new();
        let flight = oxide_fw_cache::SingleFlight::new();
        let owner = flight.acquire(&flights, "route:/a").await;
        let mut followers = Vec::new();
        for _ in 0..3 {
            followers.push(flight.acquire(&flights, "route:/a").await);
        }

        // 3. The owner completes: the publish reaches the followers (the
        //    count `publish_and_cleanup` returns is the honest join
        //    count), the cache records the flight, and the row rides the
        //    push channel beside the pull counter's bump.
        let reached = flight
            .publish_and_cleanup(
                &flights,
                "route:/a",
                owner,
                Some(Arc::new(entry(b"<html>a</html>"))),
            )
            .await;
        let joins = u64::try_from(reached).unwrap_or_default();
        cache.record_coalesced("route:/a", joins);

        // 4. The followers' side: every broadcast receives the owner's
        //    entry (a missed broadcast is treated as a cache miss — the
        //    module's rule).
        for slot in followers {
            if let oxide_fw_cache::FlightSlot::Follower(mut rx) = slot {
                let got = rx.recv().await.expect("the owner published");
                assert_eq!(got.body, b"<html>a</html>");
            }
        }

        // 5. Both channels agree: the pull counter, the exporter's
        //    families, and the feed's FIFO carry the same flight.
        let snap = metrics.snapshot();
        println!("stats().coalesced            = {}", cache.stats().coalesced);
        println!("exporter coalesce_events     = {}", snap.coalesce_events);
        println!(
            "exporter coalesced_followers = {}",
            snap.coalesced_followers
        );
        for (key, op) in feed.drain() {
            let kind = CacheEvent { key: &key, op }.kind();
            println!("feed row: {key} -> {kind}");
        }
        println!(
            "\nscrape tail:\n{}",
            metrics
                .render_prometheus()
                .lines()
                .filter(|l| l.contains("coalesce"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    });
}

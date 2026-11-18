//! The cache plane's bounded event feed walkthrough (`feed.rs`, §11.2/§20):
//! the F28 seam's named heavy-consumer shape — enqueue here, drain
//! elsewhere. The walkthrough attaches an `EventFeed` through the F28
//! builder, drives traffic, drains the queue FIFO from the "host task"
//! side, then saturates the feed against a tiny bound to show the lossy
//! contract: the arriving rows drop, the sticky warn fires ONCE, the
//! counter keeps the score. The gate section (F35) narrows the intake:
//! a deploy-watch consumer admits only the rows it was built for — the
//! hot rows never enqueue at all, and a rejection is admission, not a
//! drop.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example event_feed
//! ```

use std::sync::Arc;
use std::time::Duration;

use oxide_fw_cache::{CacheOp, Entry, EventFeed, KindGate, Policy, TieredCache};

fn entry(body: &[u8], tags: &[&str]) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: tags.iter().copied().map(str::to_string).collect(),
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

fn kind(op: &CacheOp) -> &'static str {
    match op {
        CacheOp::Hit { .. } => "hit",
        CacheOp::Miss => "miss",
        CacheOp::Set { .. } => "set",
        CacheOp::Delete => "delete",
        CacheOp::Invalidate { .. } => "invalidate",
        CacheOp::Coalesce { .. } => "coalesce",
    }
}

fn main() {
    // 1. The subscriber renders the sticky warn (the walkthrough shows
    //    the drop rule, so the warn arm must be visible).
    tracing_subscriber::fmt()
        .with_env_filter("info,oxide::cache=warn")
        .with_target(false)
        .init();

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        // 2. Attach: the feed rides the F28 seam (`with_observer`); the
        //    host keeps the typed half for draining. A warm bound (1024)
        //    is the dashboard-batcher shape — plenty of room, lossy only
        //    under a real stall.
        let feed = Arc::new(EventFeed::bounded(1024));
        let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(feed.clone()));

        // 3. Traffic: the rows land FIFO while the "host task" is busy —
        //    the enqueue never blocks the cache's call path.
        cache
            .set("route:/a", entry(b"<html>a</html>", &["content:posts"]))
            .await
            .expect("the memory tier accepts the entry");
        let _ = cache.get("route:/a").await.expect("the memory tier");
        let _ = cache.get("route:/missing").await.expect("the memory tier");

        println!("=== the warm feed: drain the queue FIFO ===");
        while let Some((key, op)) = feed.try_pop() {
            println!("{key} -> {}", kind(&op));
        }
        println!("queued: {}, dropped: {}\n", feed.len(), feed.dropped());

        // 4. Saturation: a tiny bound (1) against a burst — the arriving
        //    rows drop (the stream posture: the queued rows survive for
        //    the consumer), the sticky warn fires ONCE, the counter keeps
        //    the score. The subscriber above renders exactly one warn.
        let tiny = Arc::new(EventFeed::bounded(1));
        let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(tiny.clone()));
        cache
            .set("route:/kept", entry(b"kept", &[]))
            .await
            .expect("the memory tier accepts the entry");
        for _ in 0..4 {
            let _ = cache.get("route:/missing").await.expect("the memory tier");
        }
        println!("=== the saturated feed: drops are counted, never silent ===");
        println!(
            "queued: {}, dropped: {} (one warn above, four misses after saturation)\n",
            tiny.len(),
            tiny.dropped()
        );

        // 5. The batch arm: `drain` hands the whole window over in one
        //    call — the shape a dashboard batcher prefers.
        let rows = tiny.drain();
        println!("=== drain() — the batch arm ===");
        for (key, op) in &rows {
            println!("{key} -> {}", kind(op));
        }
        assert!(tiny.is_empty(), "the batch arm empties the queue");

        // 6. The gate (F35): the deploy-watch shape — a narrow consumer
        //    admits only the rows it was built for. Hot rows (hits,
        //    misses, sets) never enqueue at all; the invalidation and
        //    coalesce rows land FIFO. A rejection is admission, not a
        //    drop: `dropped()` stays 0, the warn never fires.
        let watch = EventFeed::gated(64, Arc::new(KindGate::only(&["invalidate", "coalesce"])));
        let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(watch.clone()));
        for i in 0..3 {
            cache
                .set(
                    &format!("route:/b/{i}"),
                    entry(b"<html>b</html>", &["content:posts"]),
                )
                .await
                .expect("the memory tier accepts the entry");
            let _ = cache
                .get(&format!("route:/b/{i}"))
                .await
                .expect("the memory tier");
            let _ = cache.get("route:/missing").await.expect("the memory tier");
        }
        cache
            .invalidate_tag("content:posts", true)
            .await
            .expect("the memory tier invalidates");
        cache.record_coalesced("route:/b/0", 2);

        println!("=== the gated feed: the deploy-watch intake ===");
        println!(
            "queued: {}, dropped: {} (nine hot rows rejected by the gate, never enqueued)",
            watch.len(),
            watch.dropped()
        );
        for (key, op) in watch.drain() {
            println!("{key} -> {}", kind(&op));
        }
    });
}

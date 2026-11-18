//! The cache observability seam walkthrough (`observer.rs`, §11.2/§20):
//! the tiered cache grows a push channel beside the pull channel
//! (`stats()` already serves) — one infallible callback per completed
//! cache event, delivered while the event happens. Two arms:
//!
//! 1. the production impl (`TracingCacheObserver`) renders every row as
//!    one `tracing` event under the `oxide::cache` target — hits (with
//!    the serving layer and the stale flag), misses, sets, deletes and
//!    one aggregate row per completed tag invalidation;
//! 2. a custom observer (a counting impl below — the shape a metrics
//!    exporter or a test harness plugs in) receives the same records as
//!    value-shaped, clonable data.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example observer
//! ```

use std::sync::{Arc, Mutex};
use std::time::Duration;

use oxide_fw_cache::{CacheEvent, CacheObserver, Entry, Policy, TieredCache, TracingCacheObserver};
use tempfile::tempdir;

/// The custom arm: a counting observer — the exact shape a metrics
/// exporter, a test harness or a dashboard batcher plugs in. Records are
/// value-shaped (`Clone`), so the consumer owns its copies.
#[derive(Default)]
struct CountingObserver {
    events: Mutex<Vec<(String, String)>>,
}

impl CacheObserver for CountingObserver {
    fn on_event(&self, event: &CacheEvent<'_>) {
        self.events
            .lock()
            .expect("the counting lock")
            .push((event.key.to_string(), event.kind().to_string()));
    }
}

fn main() {
    // 1. The subscriber renders the `TracingCacheObserver`'s rows: the
    //    hot-path rows at debug, the invalidation arm at info — the
    //    filter below admits both for the walkthrough.
    tracing_subscriber::fmt()
        .with_env_filter("info,oxide::cache=debug")
        .with_target(false)
        .init();

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        // 2. The tiered cache (§11.2): L0 memory + L1 disk over a
        //    tempdir, with the tracing observer attached. The default
        //    (no `with_observer`) stays pull-only and unchanged.
        let dir = tempdir().expect("tempdir");
        let cache = TieredCache::new(64 * 1024)
            .with_disk(dir.path().to_path_buf())
            .expect("disk tier opens")
            .with_observer(Arc::new(TracingCacheObserver::new()));

        let policy = Policy {
            ttl: Duration::from_secs(60),
            ..Policy::default()
        };
        let entry = |route: &str| Entry {
            body: format!("<h1>{route}</h1>").into_bytes(),
            status: 200,
            headers: vec![("content-type".into(), "text/html".into())],
            tags: vec!["content:posts".into()],
            policy: policy.clone(),
            stored_at: u64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_millis(),
            )
            .expect("unix ms"),
            etag: "\"v1\"".into(),
        };

        // 3. The write-read-invalidate story — watch the console: every
        //    step prints its `oxide::cache` row.
        cache.set("route:/", entry("/")).await.expect("set /");
        cache
            .set("route:/blog/hello", entry("/blog/hello"))
            .await
            .expect("set /blog/hello");
        let hit = cache.get("route:/").await.expect("get /");
        println!("\nserved a fresh hit: {}", hit.is_some());
        let miss = cache.get("route:/absent").await.expect("get absent");
        println!("unknown key served: {}", miss.is_some());
        let count = cache
            .invalidate_tag("content:posts", false)
            .await
            .expect("invalidate");
        println!("hard-invalidated {count} keys under content:posts\n");

        // 4. The custom arm: a counting observer attached to a second
        //    cache receives the same records as data (no subscriber
        //    needed — the seam is the contract, not the rendering).
        let counter = Arc::new(CountingObserver::default());
        let cache = TieredCache::new(512 * 1024 * 1024)
            .with_observer(Arc::clone(&counter) as Arc<dyn CacheObserver>);
        cache.set("k", entry("/demo")).await.expect("set k");
        let _ = cache.get("k").await.expect("get k");
        let _ = cache.get("absent").await.expect("get absent");
        let captured = counter.events.lock().expect("the counting lock").clone();
        for (key, kind) in &captured {
            println!("counting observer saw: {key} → {kind}");
        }

        // 5. The pull channel is untouched: stats() keeps its contract
        //    with or without an observer attached (§20).
        let stats = cache.stats();
        println!(
            "\nstats(): hits={} misses={} entries_memory={}",
            stats.hits, stats.misses, stats.entries_memory
        );
    });
}

//! The cache plane's posture pull walkthrough (`health.rs`, §11.2/§20):
//! the whole tiered stack in one `CacheHealth` value — what a boot probe
//! and a `/-/health`-style endpoint read, beside the two pulls the crate
//! already serves (`stats()`'s counters, the metrics exporter's scrape).
//!
//! The walkthrough builds the full stack (memory cap, disk tier, a remote
//! tier), attaches the observability seam, drives traffic, invalidates a
//! tag, and shows the three readings a host composes:
//!
//! 1. the posture value (`TieredCache::health()`) — tier attachment,
//!    resident occupancy, tag-index pressure, the seam's own state;
//! 2. the §20 gauge exposition (`render_prometheus()`) — the
//!    `oxide_cache_plane_*` gauge families a scraper polls (gauges, not
//!    the exporter's counters: a scrape reads current posture, not
//!    deltas);
//! 3. the introspection seam (`handler_kind()`) — how the opaque remote
//!    names itself (`"tiered"` here; `"http"`, `"redis"`, `"s3"` for the
//!    crate's drivers, `"custom"` for a host's own).
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example health
//! ```

use std::sync::Arc;
use std::time::Duration;

use oxide_fw_cache::{Entry, Policy, TieredCache};

fn entry(body: &[u8], tags: &[&str]) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: tags.iter().map(std::string::ToString::to_string).collect(),
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
    let dir = tempfile::tempdir().expect("tempdir");

    rt.block_on(async {
        // 1. Build the full stack: memory cap, disk tier (L1), a remote
        //    tier (L2 — a second tiered cache here; an `HttpCacheHandler`
        //    reports `"http"` the same way), and the observability seam.
        let remote: Arc<dyn oxide_fw_cache::CacheHandler> = TieredCache::new_memory();
        let cache = TieredCache::new(4 * 1024 * 1024)
            .with_disk(dir.path().join("tier"))
            .expect("the disk tier opens")
            .with_remote(remote)
            .with_observer(Arc::new(oxide_fw_cache::EventFeed::bounded(64)));

        // 2. Boot probe: the empty stack's posture is still a fact —
        //    attached tiers, zero occupancy.
        let boot = cache.health();
        println!("== boot posture ==");
        println!("remote kind     : {:?}", boot.remote_kind);
        println!("observer attached: {}", boot.observer_attached);
        print!("{}", boot.render_prometheus());

        // 3. Traffic: three write-through sets under two tags, then a
        //    hard invalidation of one tag (the affected-key count is the
        //    return value; the posture's tag-index pressure follows).
        for (key, tags) in [
            ("route:/a", &["blog", "v2"] as &[&str]),
            ("route:/b", &["blog"]),
            ("route:/c", &["docs"]),
        ] {
            cache
                .set(key, entry(b"<html>page</html>", tags))
                .await
                .expect("the stack accepts the entry");
        }
        let affected = cache
            .invalidate_tag("blog", false)
            .await
            .expect("invalidate");
        println!("\n== after 3 sets + invalidate_tag(blog) ({affected} keys affected) ==");

        // 4. The serving posture: the two pulls sample the same memory
        //    counters; the posture adds the tier facts around them.
        let h = cache.health();
        let s = cache.stats();
        println!(
            "health.memory_entries {} == stats.entries_memory {}",
            h.memory_entries, s.entries_memory
        );
        println!(
            "tags indexed: {}, keys tracked: {}",
            h.tags_indexed, h.keys_tracked
        );
        println!(
            "disk bytes  : {}, observer attached: {}",
            h.disk_bytes, h.observer_attached
        );
        println!(
            "disk evictions (F52): {} — the disk tier's own S-08 prune total riding the posture",
            h.disk_evictions
        );
        print!("{}", h.render_prometheus());
    });
}

//! The cache plane's eviction walkthrough (F51/F52, §11.2/§20): the S-08
//! budget prune as a first-class cache event — what a tier's own eviction
//! decision looks like on the pull channel, the push seam, and the probe.
//!
//! The walkthrough builds a tiered cache whose disk tier runs a TINY
//! budget (the honest way to make the steady state visible: every
//! write-through prunes past the budget, and with a one-byte budget the
//! entry the set just wrote is its own victim), attaches the metrics
//! exporter, drives the writes, and shows the four readings a host
//! composes:
//!
//! 1. the disk tier's own counters (`DiskTier::evictions()` /
//!    `evicted_bytes()`) — the pull half, unconditional;
//! 2. the push seam's rows — the exporter counted one `Evict` row per
//!    victim and rendered the two appended families;
//! 3. the tiered pulls — `stats().evictions` and the health posture's
//!    `disk_evictions` (F52), the same counter sampled two ways;
//! 4. the feed's replay — the bounded feed retained the eviction rows
//!    FIFO, so a heavy consumer can process them off the call path.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example eviction
//! ```

use std::sync::Arc;
use std::time::Duration;

use oxide_fw_cache::{DiskTier, Entry, MetricsCacheObserver, Policy, TieredCache};

fn entry(body: &[u8], tags: &[&str]) -> Entry {
    Entry {
        body: body.to_vec(),
        status: 200,
        headers: vec![("content-type".into(), "text/html".into())],
        tags: tags.iter().map(|s| (*s).to_string()).collect(),
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
        // 1. Build the stack: a roomy memory cap and a one-byte disk
        //    budget — the steady state where every write-through is also
        //    one completed S-08 eviction. The exporter rides the ONE
        //    observer slot; the eviction event rides the same wire
        //    (F51: `with_observer` wires the disk tier's push half down).
        let metrics = Arc::new(MetricsCacheObserver::new());
        let disk = Arc::new(
            DiskTier::open_with_budget(dir.path().join("tier"), 1, 1 << 20)
                .expect("the tiny budget opens"),
        );
        let cache = TieredCache::new(64 * 1024 * 1024)
            .with_disk_tier(disk.clone())
            .with_observer(metrics.clone());

        // 2. The write storm: five write-through sets under the zero
        //    budget — each one prunes the entry it just wrote (insert,
        //    then prune to target 0), so the disk ends empty and the
        //    counter says exactly that.
        for i in 0..5u64 {
            cache
                .set(
                    &format!("route:/post-{i}"),
                    entry(b"the storm payload", &["blog"]),
                )
                .await
                .expect("the zero-budget tier prunes as it writes");
        }
        println!("== the write storm (five sets, one-byte disk budget) ==");
        println!("DiskTier::evictions()    : {}", disk.evictions());
        println!(
            "DiskTier::evicted_bytes(): {} compressed blob bytes freed",
            disk.evicted_bytes()
        );

        // 3. The push seam: the exporter counted the rows and renders the
        //    two appended families (the F33 order rule — after the
        //    coalesce families, never a reorder).
        let snap = metrics.snapshot();
        println!("\n== the push seam (the exporter's mirror) ==");
        println!("snapshot.evictions     : {}", snap.evictions);
        println!("snapshot.evicted_bytes : {}", snap.evicted_bytes);
        let scrape = metrics.render_prometheus();
        for line in scrape.lines().filter(|l| l.contains("evict")) {
            println!("{line}");
        }

        // 4. The tiered pulls: the same counter sampled two ways —
        //    `stats().evictions` (the per-invocation counters) and the
        //    health posture's `disk_evictions` (F52, the probe's value
        //    shape). The posture's gauge family renders LAST (the append
        //    order).
        println!("\n== the tiered pulls ==");
        println!("stats().evictions          : {}", cache.stats().evictions);
        println!(
            "health().disk_evictions    : {}",
            cache.health().disk_evictions
        );
        let posture = cache.health().render_prometheus();
        println!(
            "posture gauge              : {}",
            posture
                .lines()
                .find(|l| l.starts_with("oxide_cache_plane_disk_evictions "))
                .unwrap_or_default()
        );

        // 5. The explicit delete is NOT an eviction: a roomy-budget second
        //    store, one set, one delete — the eviction counters stay put.
        let roomy = Arc::new(
            DiskTier::open_with_budget(dir.path().join("roomy"), 64 * 1024 * 1024, 1 << 20)
                .expect("the roomy budget opens"),
        );
        let cache2 = TieredCache::new(64 * 1024 * 1024).with_disk_tier(roomy.clone());
        cache2
            .set("route:/kept", entry(b"the roomy payload", &["docs"]))
            .await
            .expect("the roomy tier takes the write");
        cache2
            .delete("route:/kept")
            .await
            .expect("the delete lands");
        println!("\n== the explicit delete (never an eviction) ==");
        println!(
            "roomy evictions after set + delete: {} (the delete is a delete, the F51 contract)",
            roomy.evictions()
        );
    });
}

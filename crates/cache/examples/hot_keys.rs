//! The cache plane's hot-key attribution walkthrough (`hot_keys.rs`,
//! §11.2/§20): the F28 observer seam's per-key arm — a `CacheObserver`
//! that keeps a BOUNDED per-key event ledger, so the dashboard's opening
//! question ("which keys dominate the plane's traffic, and how much of
//! the stream does the ledger actually cover") is answered where the
//! events happen, not by a pull-side sampler storing every intermediate.
//!
//! The walkthrough attaches one `HotKeyObserver` through the F28 builder,
//! drives weighted traffic through the tiered cache (three routes, one
//! hot, plus a tail the tiny demo bound refuses), and shows every read
//! shape a host has:
//!
//! 1. the deterministic top window (`top(n)`) — count desc, key asc on
//!    ties, owned rows;
//! 2. the point lookup (`count(key)`) — the drill-down's answer;
//! 3. the coverage score (`unattributed()`) — the honest read that
//!    renders beside the window ("the ledger covers this much");
//! 4. the reset arm — a fresh window per report;
//! 5. the §20 Prometheus exposition (`render_prometheus(n)`) — the
//!    labeled `oxide_cache_plane_hot_key_total` counter family.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example hot_keys
//! ```

use std::sync::Arc;
use std::time::Duration;

use oxide_fw_cache::{Entry, HotKeyObserver, Policy, TieredCache};

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
        // 1. Attach: the ledger rides the F28 seam (`with_observer`, the
        //    `Arc<dyn CacheObserver>` coercion). The bound is deliberately
        //    tiny for the demo (8 tracked keys) so the coverage story
        //    shows: a real boot sizes the bound to its route cardinality.
        let hot = Arc::new(HotKeyObserver::bounded(8));
        let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(hot.clone()));

        // 2. Weighted traffic: /home dominates, /blog/a follows, the
        //    long tail spreads over ten more routes — past the bound,
        //    so the ledger's coverage score grows while the top window
        //    keeps answering.
        for _ in 0..6 {
            let _ = cache.get("/home").await.expect("the memory tier");
        }
        for _ in 0..3 {
            let _ = cache.get("/blog/a").await.expect("the memory tier");
        }
        for tail in 0..10 {
            let key = format!("/page/{tail}");
            let _ = cache.get(&key).await.expect("the memory tier");
        }
        cache
            .set("/home", entry(b"<html>home</html>"))
            .await
            .expect("the memory tier accepts the entry");

        // 3. The reads: the top window, the drill-down, the coverage.
        println!("top(3):");
        for row in hot.top(3) {
            println!("  {:>12}  {}", row.count, row.key);
        }
        println!(
            "\ncount(\"/home\") = {:?}   count(\"/page/3\") = {:?}",
            hot.count("/home"),
            hot.count("/page/3")
        );
        println!(
            "tracked = {} / capacity = {}   unattributed = {} (the long tail past the bound)",
            hot.len(),
            hot.capacity(),
            hot.unattributed()
        );

        // 4. The reset arm: a fresh window per report — the counts clear,
        //    the coverage score zeroes, the window stands alone.
        hot.reset();
        println!(
            "\nafter reset: tracked = {}   unattributed = {} (the next window stands alone)",
            hot.len(),
            hot.unattributed()
        );

        // 5. The exposition: the labeled counter family over the top
        //    window (what the §20 metrics endpoint appends beside the
        //    exporter's `oxide_cache_plane_*` families on one scrape).
        let _ = cache.get("/home").await.expect("the memory tier");
        println!("\nexporter scrape (the hot-key family):");
        print!("{}", hot.render_prometheus(3));
    });
}

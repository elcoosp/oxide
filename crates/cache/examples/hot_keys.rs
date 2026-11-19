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
//!    labeled `oxide_cache_plane_hot_key_total` counter family;
//! 6. the F39 admission gate — a second, GATED ledger rides the same
//!    fan (`HotKeyKindGate::only`), the deploy-watch shape: it tracks
//!    the keys a deploy touched, never the hot-hit stream;
//! 7. the F41 operator window — the window the §20 scrape renders is
//!    the boot's resolved `OXIDE_CACHE_PLANE_HOT_KEY_TOP` spelling, not
//!    a compile-time constant: default, narrowed, widened-past-occupancy;
//! 8. the F44 typed snapshot and coverage exposition — the whole ledger
//!    answers as ONE value under one lock pass (`snapshot(window)`: the
//!    top window's rows beside the coverage aggregates), and the
//!    coverage families (`render_coverage_prometheus`) render the
//!    unattributed score as the counter a scrape diffs, the occupancy
//!    and the bound as gauges — beside the labeled counter family, not
//!    inside it;
//! 9. the F45 windowed hand-over — `take()` hands the whole window over
//!    as the same value shape the read answers and starts a fresh one
//!    under the same lock pass (the read-and-reset pair the per-interval
//!    reporter hand-rolled, composed atomically: the fresh window stands
//!    alone, the sticky warn re-arms, first sight starts over).
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cache --example hot_keys
//! ```

use std::sync::Arc;
use std::time::Duration;

use oxide_fw_cache::{
    Entry, FanoutCacheObserver, HotKeyKindGate, HotKeyObserver, Policy, TieredCache,
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
        //    Beside it the F39 deploy-watch ledger shares the one fan —
        //    the whole-plane ledger tracks every key, the gated one only
        //    the keys a deploy touched.
        let hot = Arc::new(HotKeyObserver::bounded(8));
        let watch = HotKeyObserver::gated(
            8,
            Arc::new(HotKeyKindGate::only(&["invalidate", "coalesce"])),
        );
        let fan = FanoutCacheObserver::shared(vec![
            hot.clone() as Arc<dyn oxide_fw_cache::CacheObserver>,
            watch.clone() as Arc<dyn oxide_fw_cache::CacheObserver>,
        ]);
        let cache = Arc::new(TieredCache::new(64 * 1024 * 1024).with_observer(fan));

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

        // 6. The F39 gate: the deploy-watch ledger heard the same stream
        //    and kept nothing — every row was a hit/miss/set, refused by
        //    the gate before the ledger lock (admission, not saturation:
        //    the coverage score stays honest at zero). Drive the
        //    deploy-visible rows and the window answers.
        cache
            .set("/posts/1", entry(b"<html>post one</html>"))
            .await
            .expect("the memory tier accepts the entry");
        let _ = cache.get("/posts/1").await.expect("the memory tier");
        cache
            .invalidate_tag("content:posts", true)
            .await
            .expect("the memory tier invalidates");
        cache.record_coalesced("/posts/1", 2);
        println!("\ndeploy-watch ledger (gated to invalidate + coalesce):");
        for row in watch.top(3) {
            println!("  {:>12}  {}", row.count, row.key);
        }
        println!(
            "tracked = {}   unattributed = {} (refused rows are never counted here)",
            watch.len(),
            watch.unattributed()
        );

        // 7. The operator window (F41): the §20 scrape's top-n is not a
        //    compile-time constant — the boot resolves the
        //    `OXIDE_CACHE_PLANE_HOT_KEY_TOP` spelling once (the falsy
        //    spellings `""`/`0`/`false`/`off` keep the default window,
        //    garbage degrades loudly-but-honestly to it, and a parsed
        //    value clamps to the ledger's own bound — rows past the
        //    occupancy do not exist) and every scrape renders that
        //    window. The reads below are the shapes the knob tunes: the
        //    default window (10), an operator's narrowed window (2), an
        //    operator's widened window past the demo ledger's occupancy
        //    (64 — every tracked row renders, nothing extra exists) and
        //    the exposition arm at the same scales.
        println!("\noperator window (OXIDE_CACHE_PLANE_HOT_KEY_TOP, default 10):");
        for (label, window) in [
            ("default (10)", 10),
            ("narrowed (2)", 2),
            ("widened past occupancy (64)", 64),
        ] {
            let rows = hot.top(window);
            println!("  top({window}) [{label}] → {} rows:", rows.len());
            for row in &rows {
                println!("    {:>12}  {}", row.count, row.key);
            }
        }
        println!("\n  render_prometheus(2) — the scrape's labeled family:");
        for line in hot.render_prometheus(2).lines().skip(2) {
            println!("    {line}");
        }

        // 8. The typed snapshot and the coverage exposition (F44): the
        //    whole ledger answers as ONE value under one lock pass — the
        //    top window's rows beside the coverage aggregates — where
        //    hand-composing `top()` + `len()` + `capacity()` +
        //    `unattributed()` takes four lock passes a racing producer
        //    can tear between. The coverage arm renders the families the
        //    dashboard always needed beside the labeled counter family:
        //    the unattributed score as the counter a scrape diffs, the
        //    occupancy and the bound as gauges.
        let snap = hot.snapshot(3);
        println!("\nsnapshot(3) — the one-value read:");
        for row in &snap.rows {
            println!("    {:>12}  {}", row.count, row.key);
        }
        println!(
            "    tracked = {}   capacity = {}   unattributed = {} (one lock pass, not four)",
            snap.tracked, snap.capacity, snap.unattributed
        );
        println!("\n  render_coverage_prometheus() — the coverage families:");
        print!("{}", hot.render_coverage_prometheus());

        // 9. The windowed hand-over (F45): the per-interval reporter's
        //    read-and-reset pair, composed atomically — the whole window
        //    hands over as the same value shape the read answers and the
        //    fresh window stands alone in the same lock pass. Two windows
        //    back to back show the shape: each take carries the window's
        //    rows and its accumulated score, and the next window starts
        //    from zero.
        let _ = cache.get("/home").await.expect("the memory tier");
        let first_window = hot.take();
        println!("\ntake() — window one hands over:");
        for row in &first_window.rows {
            println!("    {:>12}  {}", row.count, row.key);
        }
        println!(
            "    tracked = {}   unattributed = {} → the ledger after: tracked = {}   unattributed = {}",
            first_window.tracked,
            first_window.unattributed,
            hot.len(),
            hot.unattributed()
        );
        let _ = cache.get("/blog/a").await.expect("the memory tier");
        let second_window = hot.take();
        println!("take() — window two (fresh, stands alone):");
        for row in &second_window.rows {
            println!("    {:>12}  {}", row.count, row.key);
        }
        println!(
            "    tracked = {}   unattributed = {} (the fresh window's own traffic)",
            second_window.tracked, second_window.unattributed
        );
    });
}

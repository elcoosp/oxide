//! V2-F12: the hedger walkthrough — the opt-in duplicate-render race the
//! WP row deferred: "for idempotent GET renders still running after
//! `afterFactor` times the route's recent p95, if another isolate is
//! idle, start the same render there and take the first completed; cancel
//! the loser."
//!
//! The walkthrough runs the seam directly (the same shapes the render
//! stage arms): a warm p95 window arms the deadline, the slow primary
//! stalls past it, the duplicate's faster answer wins, and the loser's
//! future is dropped — cancelled, not drained.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-server --example hedger
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use oxide_fw_server::hedger::{Hedger, hedge_race_when};

fn main() {
    println!("— the F12 hedger: afterFactor × recent p95 arms a duplicate-render race —\n");

    // 1. The window: a route's recent render durations. The config gate
    //    (`brownout.hedge.enabled`, DSP-113's afterFactor > 1) is the
    //    boot's construction site; an enabled=false config builds nothing.
    let hedger = Arc::new(Hedger::from_config(true, 2.0).expect("the config enables"));
    let h_cold = Hedger::from_config(false, 2.0);
    println!("1. enabled=false builds nothing: {}", h_cold.is_none());
    // Warm the window: eight renders around 50ms.
    for i in 0..8u64 {
        hedger.record("/slow", Duration::from_millis(48 + i % 5));
    }
    let after = hedger.hedge_after("/slow").expect("the window is warm");
    println!(
        "2. /slow recent p95 ≈ 50ms → afterFactor 2.0 arms a {:?} deadline",
        after
    );
    println!("   (a cold route answers None — the arm never fires on noise)");

    // 3. The race, gate open: the primary stalls past the deadline; the
    //    duplicate's faster answer wins and the loser is dropped.
    let started = Arc::new(AtomicUsize::new(0));
    let started2 = Arc::clone(&started);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (value, hedged) = rt.block_on(hedge_race_when(
        async {
            // The slow primary render (stalls past the deadline).
            tokio::time::sleep(Duration::from_millis(120)).await;
            "primary"
        },
        move || {
            started2.fetch_add(1, Ordering::SeqCst);
            async {
                // The duplicate render on the free isolate.
                "duplicate"
            }
        },
        // The pool's free-isolate verdict (re-checked at the deadline).
        || true,
        after,
    ));
    println!(
        "3. gate open: winner {value:?}, hedged={hedged}, duplicate started={}",
        started.load(Ordering::SeqCst)
    );

    // 4. The gate closed: a saturated pool never starts a duplicate —
    //    the primary runs to completion and `hedged` stays false.
    let (value, hedged) = rt.block_on(hedge_race_when(
        async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            "primary-alone"
        },
        || async { "duplicate" },
        // in_flight == workers: no free isolate.
        || false,
        Duration::from_millis(10),
    ));
    println!(
        "4. gate closed: winner {value:?}, hedged={hedged} — no duplicate on a saturated pool"
    );

    // 5. The record leg: every completed render feeds the window, so the
    //    deadline tracks the RECENT shape.
    for _ in 0..8 {
        hedger.record("/slow", Duration::from_millis(8));
    }
    let tightened = hedger.hedge_after("/slow").expect("warm");
    println!("5. eight fast renders tighten the window: the deadline is now {tightened:?}");
}

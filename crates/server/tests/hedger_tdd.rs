//! The V2-F12 hedger pins (`feat/brownout-hedger`) — the WP row's last
//! deferred leg.
//!
//! The WP row (`docs/spec-v0.2-draft.md` Appendix V2.O, F12) records the
//! wiring as landed and names what remains: "the hedger, the `overload.sh`
//! load test and the F10-pool early shrink". The spec's hedging contract
//! (§V2-F12, "Hedging (opt-in, off by default)"):
//!
//! > `brownout.hedge: { enabled, afterFactor: 2.0 }`: for idempotent GET
//! > renders still running after `afterFactor` times the route's recent
//! > p95, if another isolate is idle, start the same render there and take
//! > the first completed; cancel the loser.
//!
//! The pins below are that contract at the seam level, written RED first
//! (they failed on unresolved-symbol errors before `hedger` existed):
//!
//! 1. [`p95_window_grows_with_the_after_factor_and_needs_samples`] — the
//!    "afterFactor times the route's recent p95" arithmetic and the
//!    cold-window `None` (a route with no recent history never hedges).
//! 2. [`hedge_race_takes_the_primary_when_it_finishes_in_time`] /
//!    [`hedge_race_takes_the_secondary_after_the_deadline`] — "take the
//!    first completed".
//! 3. [`hedge_race_cancels_the_loser`] — "cancel the loser" (the losing
//!    future is DROPPED, not run to completion).
//! 4. [`hedge_race_holds_the_secondary_when_no_isolate_is_idle`] — "if
//!    another isolate is idle": a saturated pool never starts a duplicate.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use oxide_fw_server::hedger::{Hedger, hedge_race, hedge_race_when};

/// A drop-counting future guard: the pins assert the LOSING arm's future
/// is cancelled (dropped), not awaited to completion.
struct DropGuard {
    counter: Arc<AtomicUsize>,
}

impl Drop for DropGuard {
    fn drop(&mut self) {
        self.counter.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn p95_window_grows_with_the_after_factor_and_needs_samples() {
    // The §V2-F12 arithmetic: `afterFactor` × the route's recent p95, and
    // the cold-window `None` — a route with no recent history never
    // hedges (the deadline would be meaningless).
    let h = Hedger::from_config(true, 2.0).expect("the config enables");
    assert!(
        h.hedge_after("/blog/[slug]").is_none(),
        "no recent samples — no hedge deadline"
    );

    // Eight samples of 10ms: the p95 sits near the slowest sample, so the
    // deadline is the factor × p95.
    for _ in 0..8 {
        h.record("/blog/[slug]", Duration::from_millis(10));
    }
    let after = h.hedge_after("/blog/[slug]").expect("the window is warm");
    let p95_ms = after.as_millis();
    assert!(
        (15..=25).contains(&p95_ms),
        "afterFactor 2.0 × p95(10ms) ≈ 20ms, got {p95_ms}ms"
    );

    // The factor scales the deadline: afterFactor 4.0 doubles it again.
    let h4 = Hedger::from_config(true, 4.0).expect("the config enables");
    for _ in 0..8 {
        h4.record("/blog/[slug]", Duration::from_millis(10));
    }
    let after4 = h4.hedge_after("/blog/[slug]").expect("the window is warm");
    assert!(
        after4.as_millis() >= p95_ms * 2 - 2,
        "a larger factor means a larger deadline"
    );

    // Per-route windows: another route stays cold.
    assert!(h.hedge_after("/other").is_none(), "windows are per route");
}

#[test]
fn the_disabled_hedger_never_hedges() {
    // The inert-defaults contract: `enabled: false` builds nothing —
    // `from_config` refuses, so no boot carries a disabled hedger.
    assert!(
        Hedger::from_config(false, 2.0).is_none(),
        "disabled is None"
    );
}

#[tokio::test]
async fn hedge_race_takes_the_primary_when_it_finishes_in_time() {
    // The primary beats the deadline: its value wins, `hedged` is false,
    // and the secondary never starts (the arm never fires).
    let secondary_started = Arc::new(AtomicUsize::new(0));
    let started = Arc::clone(&secondary_started);
    let (value, hedged) = hedge_race(
        async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            "primary"
        },
        {
            let started = Arc::clone(&started);
            move || {
                started.fetch_add(1, Ordering::SeqCst);
                async { "secondary" }
            }
        },
        Duration::from_millis(50),
    )
    .await;
    assert_eq!(value, "primary");
    assert!(!hedged, "the primary won");
    assert_eq!(secondary_started.load(Ordering::SeqCst), 0, "no duplicate");
}

#[tokio::test]
async fn hedge_race_takes_the_secondary_after_the_deadline() {
    // The primary stalls past `afterFactor` × p95: the duplicate starts
    // and its faster answer wins — "take the first completed".
    let (value, hedged) = hedge_race(
        async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            "slow-primary"
        },
        || async { "fast-secondary" },
        Duration::from_millis(20),
    )
    .await;
    assert_eq!(value, "fast-secondary", "the duplicate's answer");
    assert!(hedged, "the race fired");
}

#[tokio::test]
async fn hedge_race_cancels_the_loser() {
    // "cancel the loser": when the secondary wins, the primary's future
    // is DROPPED (its guard fires); when the primary wins, the armed
    // secondary arm (sleep + closure) is dropped before it can start.
    let primary_dropped = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&primary_dropped);
    let (value, hedged) = hedge_race(
        async move {
            let _guard = DropGuard { counter };
            tokio::time::sleep(Duration::from_millis(200)).await;
            "slow-primary"
        },
        || async { "fast-secondary" },
        Duration::from_millis(20),
    )
    .await;
    assert_eq!(value, "fast-secondary");
    assert!(hedged);
    assert_eq!(
        primary_dropped.load(Ordering::SeqCst),
        1,
        "the loser dropped"
    );

    // And the mirror: the primary wins, the armed arm never fires.
    let (value, hedged) = hedge_race(
        async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            "quick-primary"
        },
        || async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            "slow-secondary"
        },
        Duration::from_millis(50),
    )
    .await;
    assert_eq!(value, "quick-primary");
    assert!(!hedged);
}

#[tokio::test]
async fn hedge_race_holds_the_secondary_when_no_isolate_is_idle() {
    // "if another isolate is idle": the idle gate is re-checked at the
    // deadline. A saturated pool (idle → false) never starts a duplicate —
    // the primary runs to completion and its value wins.
    let secondary_started = Arc::new(AtomicUsize::new(0));
    let started = Arc::clone(&secondary_started);
    let (value, hedged) = hedge_race_when(
        async {
            tokio::time::sleep(Duration::from_millis(80)).await;
            "primary-alone"
        },
        {
            let started = Arc::clone(&started);
            move || {
                started.fetch_add(1, Ordering::SeqCst);
                async { "secondary" }
            }
        },
        // The gate: saturated at the deadline (the pool's in_flight has
        // no free isolate for a duplicate).
        || false,
        Duration::from_millis(20),
    )
    .await;
    assert_eq!(
        value, "primary-alone",
        "no duplicate started — the primary wins"
    );
    assert!(!hedged);
    assert_eq!(started.load(Ordering::SeqCst), 0, "the gate held");

    // The free-pool shape: the gate opens and the duplicate races.
    let (value, hedged) = hedge_race_when(
        async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            "slow-primary"
        },
        || async { "fast-secondary" },
        || true,
        Duration::from_millis(20),
    )
    .await;
    assert_eq!(value, "fast-secondary");
    assert!(hedged);
}

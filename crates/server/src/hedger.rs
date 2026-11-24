//! §V2-F12's opt-in hedger — the WP row's last deferred leg: "for
//! idempotent GET renders still running after `afterFactor` times the
//! route's recent p95, if another isolate is idle, start the same render
//! there and take the first completed; cancel the loser."
//!
//! Two pieces, both pure (the config crate types `brownout.hedge`; the
//! dispatch wrapper gates and arms):
//!
//! - [`Hedger`] — the per-route recent-p95 window (`record` on every
//!   completed render, `hedge_after` for the deadline arithmetic). A route
//!   with fewer than [`MIN_SAMPLES`] recent renders has no meaningful
//!   deadline, so it never hedges; the window is bounded, so a hot route's
//!   p95 tracks the recent shape, not the boot-time shape.
//! - [`hedge_race`] / [`hedge_race_when`] — the race seam: the primary
//!   render runs; a sleeping arm wakes at the deadline, re-checks the
//!   idle gate, and starts the duplicate only when the pool still has a
//!   free isolate. `tokio::select` takes the first completed arm and
//!   DROPS the loser — the losing render's future (and its isolate call)
//!   is cancelled, not drained.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// The bounded window depth the p95 reads (the spec's "recent").
const WINDOW_DEPTH: usize = 32;

/// The minimum samples before a route's window arms a deadline: a p95
/// over fewer observations is noise, and hedging on noise duplicates
/// renders for no reason.
const MIN_SAMPLES: usize = 8;

/// The per-route recent-p95 window behind the hedger's deadline.
pub struct Hedger {
    after_factor: f64,
    window: RwLock<BTreeMap<String, Vec<Duration>>>,
}

impl Hedger {
    /// Build the hedger from the typed `brownout.hedge` config. `None`
    /// when disabled (the inert-defaults contract: an `enabled: false`
    /// boot carries no hedger at all, so the request path gains nothing).
    /// A factor the config validator already bounds (`> 1`, `DSP-113`)
    /// saturates to the spec default here — the belt for a
    /// hand-constructed value.
    #[must_use]
    pub fn from_config(enabled: bool, after_factor: f64) -> Option<Self> {
        if !enabled {
            return None;
        }
        let after_factor = if after_factor.is_finite() && after_factor > 1.0 {
            after_factor
        } else {
            2.0
        };
        Some(Self {
            after_factor,
            window: RwLock::new(BTreeMap::new()),
        })
    }

    /// Record one completed render for the route (the dispatch wrapper's
    /// post-render site). The window keeps the most recent
    /// [`WINDOW_DEPTH`] durations.
    pub fn record(&self, route: &str, d: Duration) {
        let mut window = match self.window.write() {
            Ok(w) => w,
            Err(poisoned) => poisoned.into_inner(),
        };
        let slot = window.entry(route.to_string()).or_default();
        slot.push(d);
        if slot.len() > WINDOW_DEPTH {
            let excess = slot.len() - WINDOW_DEPTH;
            slot.drain(0..excess);
        }
    }

    /// The hedge deadline for the route: `afterFactor` × the recent p95,
    /// or `None` when the window is cold (under [`MIN_SAMPLES`] samples)
    /// — a deadline with no recent history behind it would hedge on noise.
    #[must_use]
    pub fn hedge_after(&self, route: &str) -> Option<Duration> {
        let window = match self.window.read() {
            Ok(w) => w,
            Err(poisoned) => poisoned.into_inner(),
        };
        let slot = window.get(route)?;
        if slot.len() < MIN_SAMPLES {
            return None;
        }
        let mut sorted: Vec<Duration> = slot.clone();
        sorted.sort();
        // The p95 rank: the ceil(0.95 · len)-th smallest sample, in pure
        // usize arithmetic (len 8 → the max, len 32 → the 31st).
        let len = sorted.len();
        let rank = (len.saturating_mul(95).saturating_add(99)) / 100;
        let idx = rank.saturating_sub(1).min(len.saturating_sub(1));
        let p95 = sorted[idx].as_secs_f64();
        Some(Duration::from_secs_f64(p95 * self.after_factor))
    }
}

/// The race seam (the spec's hedging shape, unconditional arm): the
/// primary runs; at `after` the duplicate starts and the first completed
/// answer wins. The loser is dropped — cancelled, not drained.
pub async fn hedge_race<T, P, Sf, S>(primary: P, start_secondary: Sf, after: Duration) -> (T, bool)
where
    P: Future<Output = T>,
    Sf: FnOnce() -> S,
    S: Future<Output = T>,
{
    hedge_race_when(primary, start_secondary, || true, after).await
}

/// The race seam with the idle gate: at the deadline the arm re-checks
/// `idle` (the pool's free-isolate verdict) — a saturated pool never
/// starts a duplicate, the primary runs to completion and `hedged` stays
/// `false`.
pub async fn hedge_race_when<T, P, G, Sf, S>(
    primary: P,
    start_secondary: Sf,
    idle: G,
    after: Duration,
) -> (T, bool)
where
    P: Future<Output = T>,
    G: Fn() -> bool,
    Sf: FnOnce() -> S,
    S: Future<Output = T>,
{
    tokio::select! {
        t = primary => (t, false),
        t = async move {
            tokio::time::sleep(after).await;
            if idle() {
                start_secondary().await
            } else {
                // No free isolate for a duplicate: the arm never
                // completes, the primary's completion resolves the
                // select (the pending future is dropped with it).
                std::future::pending::<T>().await
            }
        } => (t, true),
    }
}

/// The boot's hedger slot: `cfg.brownout.hedge` typed, `None` when the
/// opt-in is off (the dispatch wrapper's field stays `None`, the render
/// path byte-identical to the pre-hedger shapes).
#[must_use]
pub fn hedger_from_config(hedge: &oxide_fw_config::HedgeConfig) -> Option<Arc<Hedger>> {
    Hedger::from_config(hedge.enabled, hedge.after_factor).map(Arc::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_factor_saturates_a_bad_value() {
        // The belt for a hand-constructed hedger: the config validator
        // refuses `afterFactor <= 1` (`DSP-113`); the seam saturates.
        let h = Hedger::from_config(true, 0.5).expect("enabled builds");
        for _ in 0..8 {
            h.record("/r", Duration::from_millis(10));
        }
        let after = h.hedge_after("/r").expect("warm");
        assert!(
            (15..=25).contains(&after.as_millis()),
            "saturated to the spec default 2.0, got {}ms",
            after.as_millis()
        );
    }
}

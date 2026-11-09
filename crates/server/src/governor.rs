//! The brownout-mode substrate (spec v0.2, §V2-F12).
//!
//! Under overload the server degrades deliberately — stale content,
//! low-priority shedding — instead of timing out everywhere. A `Governor`
//! computes a level from observable signals every tick (the wiring runs it
//! on a 500 ms cadence); the request path reads one relaxed atomic.
//!
//! This module owns the V8-free pieces:
//!
//! - [`Level`]: `Normal` → `Stale` → `Shed` → `Emergency`.
//! - [`Signals`]: the observables one tick samples — pool queue-wait p99,
//!   event-loop lag, in-flight calls, resident memory.
//! - [`Thresholds`]: the three breach ladders plus the hysteresis knobs
//!   (`exit_ratio_pct`, `calm_ticks`).
//! - [`raw_level`]: the pure breach computation — the highest level whose
//!   any signal reaches its threshold (`>=`, inclusive).
//! - [`next_level`]: the pure transition — escalate immediately, hold in
//!   the hysteresis band, de-escalate one level after `calm_ticks`
//!   consecutive calm ticks.
//! - [`Governor`]: the `AtomicU8` the sampler publishes and the request
//!   path reads (one relaxed load at `Normal` — the acceptance contract).
//! - [`Priority`] / [`serve_decision`]: the enforcement seam — the pure
//!   level × route-priority → [`Decision`] mapping the stage applies.
//! - [`shed_response`]: the `503` shed answer (`retry-after: 5`,
//!   `x-oxide-brownout: <level>`).
//! - [`ready_status`]: `/_oxide/ready` flips to 503 only at `Emergency` so
//!   a balancer can drain; `/_oxide/health` stays 200 (liveness).
//!
//! The wiring (the sampler task reading `pool.stats()`/loop-lag/RSS, the
//! enforcement stage after prelude, the hedger, the `priority` knob
//! flowing from route config into [`Priority`]) is the A01-phase-3 CLI
//! adoption — see `docs/deviations.md`.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::OxideResponse;

/// The degradation ladder (§V2-F12). The numeric values are the
/// `x-oxide-brownout` header payload and the `AtomicU8` representation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Normal = 0,
    Stale = 1,
    Shed = 2,
    Emergency = 3,
}

/// One tick's observables (§V2-F12 "Signals"). `queue_wait_p99_us` comes
/// from the pool gauge (`oxide_pool_queue_wait_p99_us`), `loop_lag_us`
/// from the task that sleeps 100 ms and records the overshoot, `rss_mb`
/// from `/proc/self/statm`, `inflight` from the pool gauge.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Signals {
    pub queue_wait_p99_us: u64,
    pub loop_lag_us: u64,
    pub inflight: u32,
    pub rss_mb: u64,
}

/// The breach ladders and hysteresis knobs (§V2-F12 "Thresholds"). Rung `i`
/// is the level-`i+1` threshold; a signal at or above it breaches that
/// level. The defaults are the spec code block's; `rss_mb` defaults to
/// "never" (`u64::MAX`) because a resident-memory gate is deployment-sized.
/// The hysteresis ratio is an integer percent (`exit_ratio_pct`, spec
/// `exit_ratio: 0.6`) so the transition stays exact, allocation-free and
/// cast-free under the workspace's zero-`#[allow]` lints policy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Thresholds {
    pub queue_wait_us: [u64; 3],
    pub loop_lag_us: [u64; 3],
    pub rss_mb: [u64; 3],
    /// De-escalation requires the signals below `exit_ratio_pct`% of the
    /// level being exited's threshold (the spec's `exit_ratio: 0.6`, as an
    /// integer percent: exact, allocation-free and cast-free under the
    /// workspace's zero-`#[allow]` lints policy).
    pub exit_ratio_pct: u64,
    /// Consecutive calm ticks required to drop one level.
    pub calm_ticks: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            queue_wait_us: [50_000, 200_000, 500_000],
            loop_lag_us: [20_000, 100_000, 300_000],
            rss_mb: [u64::MAX; 3],
            exit_ratio_pct: 60,
            calm_ticks: 6,
        }
    }
}

/// The pure breach computation (§V2-F12 `raw_level`): the highest level
/// whose any signal reaches its rung, `0` when all signals are calm.
#[must_use]
pub fn raw_level(s: &Signals, t: &Thresholds) -> u8 {
    (0..3u8)
        .rev()
        .find(|&i| {
            let i = usize::from(i); // 0..3: lossless
            s.queue_wait_p99_us >= t.queue_wait_us[i]
                || s.loop_lag_us >= t.loop_lag_us[i]
                || s.rss_mb >= t.rss_mb[i]
        })
        .map_or(0, |i| i + 1)
}
/// The pure transition (§V2-F12 `next_level`): escalate immediately, hold
/// in the hysteresis band, and de-escalate one level after `calm_ticks`
/// consecutive calm ticks. `calm` is the caller's streak counter — the
/// sampler owns one per process.
#[must_use]
pub fn next_level(cur: u8, s: &Signals, t: &Thresholds, calm: &mut u32) -> u8 {
    let raw = raw_level(s, t);
    if raw > cur {
        *calm = 0;
        return raw;
    }
    if cur == 0 {
        return 0;
    }
    // The hysteresis band: the level being exited is `cur - 1` (rung
    // index `cur - 1` is that level's threshold). A tick in the band —
    // below the breach, at or above `exit_ratio_pct`% of the exit
    // threshold — is neither a breach nor calm. The comparison runs in
    // `u128` so no side overflows and no cast loses precision.
    let i = usize::from(cur - 1);
    let calm_now = below_exit(s.queue_wait_p99_us, t.queue_wait_us[i], t.exit_ratio_pct)
        && below_exit(s.loop_lag_us, t.loop_lag_us[i], t.exit_ratio_pct)
        && below_exit(s.rss_mb, t.rss_mb[i], t.exit_ratio_pct);
    if calm_now {
        *calm += 1;
        if *calm >= t.calm_ticks {
            *calm = 0;
            return cur - 1;
        }
    } else {
        *calm = 0;
    }
    cur
}

/// The hysteresis band's comparison: `signal < pct% of rung`, in lossless
/// `u128` math (the spec's `signal < exit_ratio × threshold`, with the
/// ratio an integer percent).
fn below_exit(signal: u64, rung: u64, pct: u64) -> bool {
    u128::from(signal) * 100 < u128::from(rung) * u128::from(pct)
}

/// The sampler's published level. `set` from the tick task, `level` from
/// the request path — at `Normal` the whole brownout cost is the one
/// relaxed atomic load in [`Governor::level`].
#[derive(Debug, Default)]
pub struct Governor {
    level: AtomicU8,
}

impl Governor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn level(&self) -> Level {
        match self.level.load(Ordering::Relaxed) {
            0 => Level::Normal,
            1 => Level::Stale,
            2 => Level::Shed,
            _ => Level::Emergency,
        }
    }

    pub fn set(&self, l: u8) {
        self.level.store(l.min(3), Ordering::Relaxed);
    }
}

/// A route's overload priority (§V2-F12 enforcement): declared in
/// `page.server.ts` (`config.priority`) or config `rules`
/// (`{ source: '/api/export', priority: 'low' }`), default `Normal`,
/// stored in the route manifest. The lowercase grammar is the config
/// surface's; [`parse_priority`] refuses everything else so a typo cannot
/// silently widen the shed set.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Priority {
    Low,
    #[default]
    Normal,
    Critical,
}

/// The config grammar's parser: `"low" | "normal" | "critical"`.
#[must_use]
pub fn parse_priority(s: &str) -> Option<Priority> {
    match s {
        "low" => Some(Priority::Low),
        "normal" => Some(Priority::Normal),
        "critical" => Some(Priority::Critical),
        _ => None,
    }
}

/// What the enforcement stage does with a request at the current level.
/// `ServeStale` means "serve, and L0 entries past their SWR window are
/// eligible behind [`STALE_WARNING_HEADER`]" — the stale lookup itself is
/// the cache wiring. The L0-hit / static-asset exemptions at `Emergency`
/// are the caller's (they are request-state, not route-state).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decision {
    Serve,
    ServeStale,
    Shed,
}

/// The pure enforcement seam (§V2-F12 enforcement): level × priority →
/// decision. `Normal` serves everything; `Stale` marks every route
/// stale-eligible; `Shed` sheds `Low`; `Emergency` sheds everything but
/// `Critical` (critical routes always serve).
#[must_use]
pub fn serve_decision(level: Level, priority: Priority) -> Decision {
    match level {
        Level::Normal => Decision::Serve,
        Level::Stale => Decision::ServeStale,
        Level::Shed => match priority {
            Priority::Low => Decision::Shed,
            Priority::Normal | Priority::Critical => Decision::ServeStale,
        },
        Level::Emergency => match priority {
            Priority::Critical => Decision::ServeStale,
            Priority::Low | Priority::Normal => Decision::Shed,
        },
    }
}

/// The shed answer's `retry-after` (seconds) — the spec's `retry-after: 5`.
pub const SHED_RETRY_AFTER_SECS: u64 = 5;

/// The brownout level header (`x-oxide-brownout: <level>`), sent on every
/// shed response.
pub const BROWNOUT_HEADER: &str = "x-oxide-brownout";

/// The stale-serving marker (§V2-F12 step Stale):
/// `warning: 110 - "Response is Stale"` (RFC 9111 §5.5).
pub const STALE_WARNING_HEADER: &str = "warning";
pub const STALE_WARNING_VALUE: &str = "110 - \"Response is Stale\"";

/// The `503` a shed request receives: `retry-after: 5` and the numeric
/// level in `x-oxide-brownout`, so a client (and the load test) can tell
/// deliberate degradation from a crash.
#[must_use]
pub fn shed_response(level: Level) -> OxideResponse {
    let n = match level {
        Level::Normal => 0,
        Level::Stale => 1,
        Level::Shed => 2,
        Level::Emergency => 3,
    };
    let mut resp = OxideResponse::new(
        503,
        format!("brownout: shed at level {n}, retry after {SHED_RETRY_AFTER_SECS}s"),
    )
    .with_header(BROWNOUT_HEADER, n.to_string());
    resp.headers
        .push(("retry-after".to_string(), SHED_RETRY_AFTER_SECS.to_string()));
    resp
}

/// The readiness contract (§V2-F12): `/_oxide/ready` answers 200 until
/// `Emergency`, 503 at `Emergency` so a balancer drains the node.
/// `/_oxide/health` (liveness) stays 200 at every level.
#[must_use]
pub fn ready_status(level: Level) -> u16 {
    match level {
        Level::Emergency => 503,
        _ => 200,
    }
}

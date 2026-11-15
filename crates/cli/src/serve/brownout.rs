//! The CLI's brownout wiring (§V2-F12): the governor the sampler drives,
//! the signals the sampler reads, and the readiness flip the ops endpoints
//! serve. The pure model (`Level`, `Thresholds`, `raw_level`/`next_level`,
//! `serve_decision`, `shed_response`, `ready_status`) lives in
//! `oxide_fw_server::governor` — pinned by `crates/server/tests/
//! governor_tdd.rs`; this module is the serve-side state around it, the
//! same CLI-state pattern the skew pool (`serve::generations`), the live
//! hub (`serve::live`) and the stream store (`serve::streams`) follow.
//!
//! Signal plumbing:
//! - **pool queue-wait + in-flight**: the jshost pool's `stats()` gauges
//!   (PERF-02), read once per sampler tick (the dispatch closure's
//!   per-second gauge refresh stays independent — that one feeds metrics,
//!   this one feeds the governor).
//! - **loop lag**: a probe task sleeps 100 ms and records the overshoot;
//!   the sampler reads the atomic per tick (the spec's probe design).
//! - **RSS**: `/proc/self/statm`'s resident pages × page size (the doctor
//!   convention; Linux-only by the same contract the doctor documents — a
//!   failed read contributes the "no gate" sentinel `0`, which the
//!   thresholds mapping turns into never-breach).
//!
//! Every knob is load-validated (`DSP-113`: ascending ladders, a
//! `staleCeiling` ≥ 1, an `afterFactor` > 1); the belts here exist only
//! for a hand-built config.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use oxide_fw_config::BrownoutConfig;
use oxide_fw_jshost::Pool;
use oxide_fw_server::Metrics;
use oxide_fw_server::governor::{Governor, Signals, Thresholds, parse_priority};

/// The sampler tick (§V2-F12: "A `Governor` computes a level from
/// observable signals every 500 ms; the request path reads one atomic").
pub const SAMPLER_TICK_MS: u64 = 500;

/// The loop-lag probe's sleep (§V2-F12: "loop lag is measured by a task
/// that sleeps 100 ms and records the overshoot").
pub const LOOP_LAG_PROBE_MS: u64 = 100;

/// The spec's `staleCeiling` default multiplier (`"10x"`), as an integer
/// percent (100 × multiplier) so the L0 bound math stays exact under the
/// zero-cast lints policy — the same shape the governor's
/// `exit_ratio_pct` uses.
pub const DEFAULT_STALE_CEILING_PCT: u64 = 10 * 100;

/// The serve-side brownout state: the governor the request path reads,
/// the config the enforcement consults, and the handles the sampler
/// drains. `brownout.enabled = false` constructs nothing (the inert
/// defaults contract) — `from_config` returns `None` and the request path
/// keeps its pre-F12 shape.
pub struct BrownoutWiring {
    /// The governor: the sampler writes, every request reads one relaxed
    /// atomic, the ops endpoints report it.
    pub governor: Arc<Governor>,
    /// The `staleCeiling` multiplier as an integer percent (the spec's
    /// "up to `brownout.staleCeiling`, default 10× TTL" rule the `Stale`
    /// level grants the L0 lookup).
    pub stale_ceiling_pct: u64,
    /// The loop-lag probe's last overshoot, µs (the sampler reads it).
    pub loop_lag_us: Arc<AtomicU64>,
    /// The worker pool the queue-wait/in-flight gauges come from. `None`
    /// (no worker pool — a static-only server) means those signals read
    /// zero and only loop lag / RSS can breach.
    pub pool: Option<Arc<dyn Pool>>,
    /// The metrics registry the sampler publishes
    /// `oxide_brownout_level` to.
    pub metrics: Arc<Metrics>,
}

impl BrownoutWiring {
    /// Build the wiring from the typed `brownout:` config section, or
    /// `None` when the mode is off (the request path gains nothing).
    #[must_use]
    pub fn from_config(
        cfg: &BrownoutConfig,
        pool: Option<Arc<dyn Pool>>,
        metrics: Arc<Metrics>,
    ) -> Option<Self> {
        if !cfg.enabled {
            return None;
        }
        // The ceiling: `parse_stale_ceiling` already validated ≥ 1 at load
        // (`DSP-113`); the percent form keeps the L0 bound math exact. A
        // hand-built config with an unparseable literal falls back to the
        // spec's 10× default.
        let ceiling_pct = (cfg
            .stale_ceiling
            .strip_suffix('x')
            .unwrap_or(&cfg.stale_ceiling)
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite() && *n >= 1.0)
            .map(|n| n * 100.0))
        .map(|pct| {
            // One documented rounding at the config boundary: the percent
            // is an integer; every consumer compares in exact integer
            // math.
            u64::try_from(pct.round() as u128).unwrap_or(u64::MAX)
        })
        .unwrap_or(DEFAULT_STALE_CEILING_PCT);
        Some(Self {
            governor: Arc::new(Governor::new()),
            stale_ceiling_pct: ceiling_pct,
            loop_lag_us: Arc::new(AtomicU64::new(0)),
            pool,
            metrics,
        })
    }

    /// The breach ladders from the config section (§V2-F12 wiring): the
    /// config carries whole milliseconds, the model microseconds — the
    /// ×1000 conversion lives here so `next_level`/`raw_level` stay
    /// spec-verbatim. An `rssMb` rung of `0` is the documented "no RSS
    /// gate" spelling (JSON cannot carry `u64::MAX`); it maps to the
    /// model's never-breach sentinel.
    #[must_use]
    pub fn thresholds(&self, cfg: &BrownoutConfig) -> Thresholds {
        let t = &cfg.thresholds;
        let conv = |ms: u64| ms.saturating_mul(1000);
        let rss = |mib: u64| {
            if mib == 0 { u64::MAX } else { mib }
        };
        Thresholds {
            queue_wait_us: [
                conv(t.queue_wait_ms[0]),
                conv(t.queue_wait_ms[1]),
                conv(t.queue_wait_ms[2]),
            ],
            loop_lag_us: [
                conv(t.loop_lag_ms[0]),
                conv(t.loop_lag_ms[1]),
                conv(t.loop_lag_ms[2]),
            ],
            rss_mb: [rss(t.rss_mb[0]), rss(t.rss_mb[1]), rss(t.rss_mb[2])],
            ..Thresholds::default()
        }
    }

    /// The signals snapshot the sampler feeds `next_level`: the pool's
    /// queue-wait p99 (ms → µs) and in-flight count, the loop-lag probe's
    /// last overshoot, and the process RSS.
    #[must_use]
    pub fn signals(&self) -> Signals {
        let (queue_wait_p99_us, inflight) = match self.pool.as_ref() {
            Some(p) => {
                let s = p.stats();
                (
                    gauge_ms_to_us(s.queue_wait_ms_p99),
                    u32::try_from(s.in_flight).unwrap_or(u32::MAX),
                )
            }
            None => (0, 0),
        };
        Signals {
            queue_wait_p99_us,
            loop_lag_us: self.loop_lag_us.load(Ordering::Relaxed),
            inflight,
            rss_mb: rss_mb().unwrap_or(0),
        }
    }

    /// The sampler task (§V2-F12 wiring): every 500 ms, gather the
    /// signals, run the pure transition and publish — `Governor::set` for
    /// the request path's one atomic read, `Metrics::set_brownout_level`
    /// for the `oxide_brownout_level` gauge. The calm streak is the
    /// sampler's own counter (the model's hysteresis contract).
    #[must_use]
    pub fn spawn_sampler(wiring: &Arc<Self>, cfg: &BrownoutConfig) -> tokio::task::JoinHandle<()> {
        let wiring = Arc::clone(wiring);
        let thresholds = wiring.thresholds(cfg);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(SAMPLER_TICK_MS));
            // Skip the immediate zero-th tick: the boot tick runs before
            // any signal exists and would pin the calm counter.
            tick.tick().await;
            let mut calm: u32 = 0;
            loop {
                tick.tick().await;
                let s = wiring.signals();
                let next = oxide_fw_server::governor::next_level(
                    wiring.governor.level() as u8,
                    &s,
                    &thresholds,
                    &mut calm,
                );
                wiring.governor.set(next);
                wiring.metrics.set_brownout_level(next);
            }
        })
    }

    /// The loop-lag probe task (§V2-F12 wiring): sleep 100 ms, record the
    /// overshoot. The event loop's other work delays the wake-up; the
    /// overshoot IS the lag. The atomic holds the last probe's value —
    /// the sampler's per-tick read is a snapshot, not a max.
    #[must_use]
    pub fn spawn_loop_lag_probe(wiring: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let lag = Arc::clone(&wiring.loop_lag_us);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(LOOP_LAG_PROBE_MS));
            tick.tick().await;
            loop {
                tick.tick().await;
                let start = std::time::Instant::now();
                tokio::time::sleep(Duration::from_millis(LOOP_LAG_PROBE_MS)).await;
                let overshoot_us = start
                    .elapsed()
                    .saturating_sub(Duration::from_millis(LOOP_LAG_PROBE_MS))
                    .as_micros();
                lag.store(
                    u64::try_from(overshoot_us).unwrap_or(u64::MAX),
                    Ordering::Relaxed,
                );
            }
        })
    }
}

/// The pool's `queue_wait_ms_p99` gauge (f64 milliseconds) as exact µs.
/// The round-trip goes through the decimal form — no float→int `as` cast
/// (the workspace's zero-cast lints policy); a NaN/negative/infinite
/// gauge — a pool that never recorded a wait sample — reads as
/// "no signal" (0).
#[must_use]
fn gauge_ms_to_us(ms: f64) -> u64 {
    if !ms.is_finite() {
        return 0;
    }
    let us = (ms.max(0.0) * 1000.0).round();
    us.to_string().parse::<u64>().unwrap_or(0)
}

/// The process resident memory in MiB, from `/proc/self/statm` (the
/// doctor's source). `None` when the file is unreadable or the platform
/// has no statm — the sampler treats `None` as "no RSS signal" (the
/// thresholds' no-gate sentinel).
#[must_use]
pub fn rss_mb() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages = statm.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    let page_bytes = 4096_u128;
    let mib = u128::from(resident_pages) * page_bytes / (1024 * 1024);
    u64::try_from(mib).ok()
}

/// Parse a route's `config.priority` spelling (`low | normal | critical`)
/// through the governor's grammar — `None` (an absent declaration, an
/// unknown literal) is the spec's `normal` default.
#[must_use]
pub fn route_priority(s: Option<&str>) -> oxide_fw_server::governor::Priority {
    s.and_then(parse_priority)
        .unwrap_or(oxide_fw_server::governor::Priority::Normal)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_fw_server::governor::Level;

    /// The config ladders convert ms → µs and the `0` RSS rungs map to the
    /// never-breach sentinel (§V2-F12 deviations 1 + 2 in
    /// docs/deviations.md).
    #[test]
    fn thresholds_convert_the_config_ladders() {
        let wiring = brownout_for_test();
        let t = wiring.thresholds(&BrownoutConfig::default());
        assert_eq!(
            t.queue_wait_us,
            [50_000, 200_000, 500_000],
            "the spec defaults ×1000"
        );
        assert_eq!(t.loop_lag_us, [20_000, 100_000, 300_000]);
        assert_eq!(
            t.rss_mb,
            [u64::MAX; 3],
            "the 0 rungs are the no-gate spelling"
        );
        assert_eq!(t.exit_ratio_pct, 60);
        assert_eq!(t.calm_ticks, 6);
    }

    /// A custom ladder converts rung-for-rung; a nonzero RSS rung passes
    /// through as MiB.
    #[test]
    fn thresholds_pass_custom_rungs_through() {
        let wiring = brownout_for_test();
        let mut cfg = BrownoutConfig::default();
        cfg.thresholds.queue_wait_ms = [10, 20, 30];
        cfg.thresholds.rss_mb = [0, 512, 0];
        let t = wiring.thresholds(&cfg);
        assert_eq!(t.queue_wait_us, [10_000, 20_000, 30_000]);
        assert_eq!(t.rss_mb, [u64::MAX, 512, u64::MAX]);
    }

    /// The ceiling parses to an integer percent: the spec's `"10x"` default
    /// is 1000%; an unparseable literal falls back to the default.
    #[test]
    fn ceiling_parses_to_the_percent_form() {
        let metrics = Arc::new(Metrics::new());
        let mut cfg = BrownoutConfig::default();
        let w =
            BrownoutWiring::from_config(&cfg, None, metrics.clone()).expect("enabled: true wires");
        assert_eq!(
            w.stale_ceiling_pct, DEFAULT_STALE_CEILING_PCT,
            "the 10x default"
        );
        cfg.stale_ceiling = "1.5x".into();
        let w = BrownoutWiring::from_config(&cfg, None, metrics.clone()).expect("wired");
        assert_eq!(w.stale_ceiling_pct, 150, "1.5× = 150%");
        cfg.stale_ceiling = "garbage".into();
        let w = BrownoutWiring::from_config(&cfg, None, metrics.clone()).expect("wired");
        assert_eq!(w.stale_ceiling_pct, DEFAULT_STALE_CEILING_PCT, "the belt");
        cfg.enabled = false;
        assert!(
            BrownoutWiring::from_config(&cfg, None, metrics).is_none(),
            "enabled: false constructs nothing"
        );
    }

    /// `enabled: false` (the default) constructs nothing — the request
    /// path keeps its pre-F12 shape.
    #[test]
    fn disabled_config_constructs_nothing() {
        let metrics = Arc::new(Metrics::new());
        assert!(BrownoutWiring::from_config(&BrownoutConfig::default(), None, metrics).is_none());
    }

    /// The gauge conversion: whole ms → exact µs, a fractional ms rounds
    /// once, and the degenerate gauges read "no signal".
    #[test]
    fn gauge_ms_to_us_converts_exactly() {
        assert_eq!(gauge_ms_to_us(50.0), 50_000, "the spec's first rung");
        assert_eq!(gauge_ms_to_us(0.5), 500, "a fractional sample");
        assert_eq!(gauge_ms_to_us(0.0), 0);
        assert_eq!(gauge_ms_to_us(-1.0), 0, "a negative gauge is no signal");
        assert_eq!(gauge_ms_to_us(f64::NAN), 0, "NaN is no signal");
        assert_eq!(gauge_ms_to_us(f64::INFINITY), 0, "an unrecorded gauge");
    }

    /// The signals snapshot: no pool reads zero queue-wait/in-flight, and
    /// the loop-lag atomic surfaces verbatim.
    #[test]
    fn signals_read_the_probe_and_default_to_zero_without_a_pool() {
        let wiring = brownout_for_test();
        wiring.loop_lag_us.store(7_500, Ordering::Relaxed);
        let s = wiring.signals();
        assert_eq!(s.queue_wait_p99_us, 0, "no pool: no queue signal");
        assert_eq!(s.inflight, 0);
        assert_eq!(s.loop_lag_us, 7_500, "the probe's last overshoot");
        // rss_mb: the sandbox has statm; a nonzero resident set is a
        // sanity bound, not a pin on the exact value.
        assert!(
            s.rss_mb > 0 && s.rss_mb < 64 * 1024,
            "rss reads in a sane MiB range: {}",
            s.rss_mb
        );
    }

    /// The route-priority grammar: an absent/unknown declaration is
    /// `normal` (the spec default), the three literals parse through.
    #[test]
    fn route_priority_defaults_to_normal() {
        assert_eq!(
            route_priority(None),
            oxide_fw_server::governor::Priority::Normal
        );
        assert_eq!(
            route_priority(Some("low")),
            oxide_fw_server::governor::Priority::Low
        );
        assert_eq!(
            route_priority(Some("critical")),
            oxide_fw_server::governor::Priority::Critical
        );
        assert_eq!(
            route_priority(Some("wat")),
            oxide_fw_server::governor::Priority::Normal,
            "an unknown literal is the default, not a refusal at request time"
        );
    }

    /// The governor the wiring builds starts at `Normal` and the level
    /// round-trips through `set` (the enforcement stage's read shape).
    #[test]
    fn governor_starts_normal_and_round_trips() {
        let wiring = brownout_for_test();
        assert_eq!(wiring.governor.level(), Level::Normal);
        wiring.governor.set(2);
        assert_eq!(wiring.governor.level(), Level::Shed);
    }

    /// A wiring handle for the pins (no pool: the queue/in-flight signals
    /// read zero; the loop-lag and RSS signals are exercised directly).
    fn brownout_for_test() -> Arc<BrownoutWiring> {
        let mut cfg = BrownoutConfig::default();
        cfg.enabled = true;
        Arc::new(
            BrownoutWiring::from_config(&cfg, None, Arc::new(Metrics::new()))
                .expect("enabled: true wires"),
        )
    }
}

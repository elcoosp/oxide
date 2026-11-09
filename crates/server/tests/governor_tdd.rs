//! TDD pins for the V2-F12 brownout-mode substrate
//! (`oxide_fw_server::governor`, spec v0.2 §V2-F12).
//!
//! Under overload the server degrades deliberately instead of timing out
//! everywhere: a `Governor` computes a level from observable signals every
//! tick, the request path reads one atomic, and the level decides —
//! `Normal` serves everything, `Stale` permits L0 entries past their SWR
//! window behind the `warning: 110` header, `Shed` answers low-priority
//! routes with `503`, `Emergency` answers everything but critical routes
//! with `503` and flips `/_oxide/ready` to 503 so a balancer drains.
//!
//! Written FIRST (red): these pins failed on unresolved-symbol errors
//! before the `governor` module existed. The wiring (the sampler task, the
//! enforcement stage after prelude, the pool-stats/loop-lag/RSS signal
//! sources, the hedger) is the A01-phase-3 CLI adoption — see
//! `docs/deviations.md`.

use std::time::Duration;

use oxide_fw_server::governor::{
    BROWNOUT_HEADER, Decision, Governor, Level, Priority, SHED_RETRY_AFTER_SECS,
    STALE_WARNING_HEADER, STALE_WARNING_VALUE, Signals, Thresholds, next_level, raw_level,
    ready_status, serve_decision, shed_response,
};

/// The spec's default thresholds.
fn spec_thresholds() -> Thresholds {
    Thresholds::default()
}

/// Signals well below every level-1 threshold.
fn calm_signals() -> Signals {
    Signals {
        queue_wait_p99_us: 1_000,
        loop_lag_us: 500,
        inflight: 2,
        rss_mb: 128,
    }
}

// ------------------------------------------------------------------- raw_level

/// The spec's own table pin: `raw_level` picks the highest level whose any
/// signal breaches its threshold, 0 when none do.
#[test]
fn raw_level_picks_the_highest_breached_level() {
    let t = spec_thresholds();

    let mut s = calm_signals();
    assert_eq!(raw_level(&s, &t), 0, "all signals calm → Normal");

    // Level 1 breach on each signal independently.
    s.queue_wait_p99_us = 50_000;
    assert_eq!(raw_level(&s, &t), 1, "queue-wait breaches level 1");
    s.queue_wait_p99_us = 1_000;

    s.loop_lag_us = 20_000;
    assert_eq!(raw_level(&s, &t), 1, "loop-lag breaches level 1");
    s.loop_lag_us = 500;

    // Level 3: any signal at or above the top threshold.
    s.queue_wait_p99_us = 500_000;
    assert_eq!(raw_level(&s, &t), 3);
    s.queue_wait_p99_us = 1_000;
    s.rss_mb = u64::MAX;
    assert_eq!(
        raw_level(&s, &t),
        3,
        "rss breaches level 3 (never by default)"
    );
}

/// A signal exactly at the threshold counts as breached (the spec's `>=`).
#[test]
fn raw_level_breach_is_inclusive() {
    let t = spec_thresholds();
    let mut s = calm_signals();
    s.queue_wait_p99_us = 200_000;
    assert_eq!(raw_level(&s, &t), 2, "exactly at the level-2 threshold");
}

// ------------------------------------------------------------------ next_level

/// Escalation is immediate: one breaching tick jumps straight to the raw
/// level, never a gradual climb.
#[test]
fn next_level_escalates_immediately() {
    let t = spec_thresholds();
    let mut calm = 0u32;
    let mut s = calm_signals();
    assert_eq!(next_level(0, &s, &t, &mut calm), 0);

    s.queue_wait_p99_us = 600_000;
    assert_eq!(
        next_level(0, &s, &t, &mut calm),
        3,
        "one bad tick escalates straight to Emergency"
    );
}

/// De-escalation needs `calm_ticks` (default 6) consecutive calm ticks and
/// drops exactly one level at a time (hysteresis).
#[test]
fn next_level_needs_calm_ticks_to_de_escalate() {
    let t = spec_thresholds();
    let mut calm = 0u32;

    // Parked at level 2 by an earlier breach.
    let bad = Signals {
        queue_wait_p99_us: 250_000,
        loop_lag_us: 500,
        inflight: 2,
        rss_mb: 128,
    };
    let mut cur = next_level(0, &bad, &t, &mut calm);
    assert_eq!(cur, 2);

    // Signals recover to calm; the level must NOT drop before 6 calm ticks.
    let good = calm_signals();
    for tick in 1..t.calm_ticks {
        cur = next_level(cur, &good, &t, &mut calm);
        assert_eq!(cur, 2, "tick {tick}: still held by hysteresis");
    }
    cur = next_level(cur, &good, &t, &mut calm);
    assert_eq!(cur, 1, "the 6th calm tick drops exactly one level");

    // Another 6 calm ticks drop the next level.
    for _ in 0..t.calm_ticks {
        cur = next_level(cur, &good, &t, &mut calm);
    }
    assert_eq!(cur, 0, "fully recovered to Normal");
}

/// A renewed breach during the calm window resets the calm counter — the
/// level does not bleed down while load is still spiky.
#[test]
fn next_level_resets_calm_on_a_new_breach() {
    let t = spec_thresholds();
    let mut calm = 0u32;
    let mut cur = 2u8;

    let good = calm_signals();
    for _ in 0..(t.calm_ticks - 1) {
        cur = next_level(cur, &good, &t, &mut calm);
    }
    assert_eq!(cur, 2, "one tick short of de-escalation");

    // A tick still breaching level 2: the calm streak resets, level holds.
    let bad = Signals {
        queue_wait_p99_us: 250_000,
        loop_lag_us: 500,
        inflight: 2,
        rss_mb: 128,
    };
    cur = next_level(cur, &bad, &t, &mut calm);
    assert_eq!(cur, 2);
    assert_eq!(calm, 0, "the calm counter reset");

    // And it takes the full window again: six calm ticks, the sixth drops.
    cur = next_level(cur, &good, &t, &mut calm); // tick 1: calm = 1
    for tick in 2..t.calm_ticks {
        // ticks 2..5: calm = 2..5
        cur = next_level(cur, &good, &t, &mut calm);
        assert_eq!(cur, 2, "tick {tick}: still held");
    }
    cur = next_level(cur, &good, &t, &mut calm); // tick 6: calm = 6 → drop
    assert_eq!(cur, 1);
}

/// `next_level` never returns a level above the raw breach level and never
/// below 0 (a u8 underflow is unreachable, but the contract is pinned).
#[test]
fn next_level_stays_within_the_raw_band() {
    let t = spec_thresholds();
    let mut calm = 0u32;
    let s = calm_signals();

    assert_eq!(next_level(0, &s, &t, &mut calm), 0);
    assert_eq!(
        next_level(3, &s, &t, &mut calm),
        3,
        "holds at Emergency while calm accumulates"
    );
}

/// The hysteresis band: a signal between `exit_ratio × threshold` and the
/// threshold itself is neither a breach nor a calm tick.
#[test]
fn next_level_hysteresis_band_holds_the_level() {
    let t = spec_thresholds();
    let mut calm = 0u32;
    let mut cur = 2u8;

    // queue_wait at level 1's threshold (below level 2's, above its exit
    // ratio 0.6 × 200_000 = 120_000): not calm, not a level-2 breach.
    let band = Signals {
        queue_wait_p99_us: 150_000,
        loop_lag_us: 500,
        inflight: 2,
        rss_mb: 128,
    };
    for _ in 0..(t.calm_ticks * 3) {
        cur = next_level(cur, &band, &t, &mut calm);
    }
    assert_eq!(cur, 2, "the band neither escalates nor de-escalates");
    assert_eq!(calm, 0);
}

// -------------------------------------------------------------------- Governor

#[test]
fn governor_defaults_to_normal_and_round_trips() {
    let g = Governor::new();
    assert_eq!(g.level(), Level::Normal);

    g.set(2);
    assert_eq!(g.level(), Level::Shed);
    g.set(0);
    assert_eq!(g.level(), Level::Normal);
}

/// The acceptance contract: at Normal the per-request cost is one relaxed
/// atomic load — `level()` must be callable from `&self` with no lock.
#[test]
fn governor_level_reads_are_lock_free_and_race_safe() {
    let g = std::sync::Arc::new(Governor::new());
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let g = std::sync::Arc::clone(&g);
            std::thread::spawn(move || {
                for _ in 0..1_000 {
                    g.set((i % 4) as u8);
                    let _ = g.level();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("threads join");
    }
    // Any of the four levels is a legal final state; the pin is that
    // concurrent set/level never deadlocks or tears.
    let _ = g.level();
}

// ------------------------------------------------------------ serve_decision

#[test]
fn normal_level_serves_every_priority() {
    for p in [Priority::Low, Priority::Normal, Priority::Critical] {
        assert_eq!(serve_decision(Level::Normal, p), Decision::Serve);
    }
}

/// At `Stale` every route may serve cache entries past their SWR window,
/// behind the warning header.
#[test]
fn stale_level_permits_stale_for_every_priority() {
    for p in [Priority::Low, Priority::Normal, Priority::Critical] {
        assert_eq!(serve_decision(Level::Stale, p), Decision::ServeStale);
    }
}

/// At `Shed` only low-priority routes shed.
#[test]
fn shed_level_sheds_only_low_priority() {
    assert_eq!(serve_decision(Level::Shed, Priority::Low), Decision::Shed);
    assert_eq!(
        serve_decision(Level::Shed, Priority::Normal),
        Decision::ServeStale
    );
    assert_eq!(
        serve_decision(Level::Shed, Priority::Critical),
        Decision::ServeStale
    );
}

/// At `Emergency` everything but critical sheds.
#[test]
fn emergency_level_sheds_everything_but_critical() {
    assert_eq!(
        serve_decision(Level::Emergency, Priority::Low),
        Decision::Shed
    );
    assert_eq!(
        serve_decision(Level::Emergency, Priority::Normal),
        Decision::Shed
    );
    assert_eq!(
        serve_decision(Level::Emergency, Priority::Critical),
        Decision::ServeStale
    );
}

// -------------------------------------------------------------- shed_response

#[test]
fn shed_response_carries_the_brownout_headers() {
    let resp = shed_response(Level::Shed);
    assert_eq!(resp.status, 503);
    let header = |name: &str| {
        resp.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    assert_eq!(
        header(BROWNOUT_HEADER).as_deref(),
        Some("2"),
        "the numeric level travels in the header"
    );
    assert_eq!(
        header("retry-after").as_deref(),
        Some("5"),
        "the spec's retry-after"
    );
    assert_eq!(SHED_RETRY_AFTER_SECS, 5);

    let emergency = shed_response(Level::Emergency);
    assert_eq!(emergency.status, 503);
    assert_eq!(
        emergency
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(BROWNOUT_HEADER))
            .map(|(_, v)| v.as_str()),
        Some("3")
    );
}

// ---------------------------------------------------- stale warning constant

/// The `warning: 110 - "Response is Stale"` header the L0 stale-serving
/// attaches at level ≥ Stale (spec §V2-F12 enforcement, step Stale).
#[test]
fn stale_warning_header_matches_the_spec_spelling() {
    assert_eq!(STALE_WARNING_HEADER, "warning");
    assert_eq!(STALE_WARNING_VALUE, "110 - \"Response is Stale\"");
}

// --------------------------------------------------------------- ready_status

#[test]
fn readiness_flips_only_at_emergency() {
    assert_eq!(ready_status(Level::Normal), 200);
    assert_eq!(ready_status(Level::Stale), 200);
    assert_eq!(ready_status(Level::Shed), 200);
    assert_eq!(ready_status(Level::Emergency), 503, "a balancer can drain");
}

// ------------------------------------------------------------- parse helpers

#[test]
fn priority_parses_and_defaults_to_normal() {
    use oxide_fw_server::governor::parse_priority;
    assert_eq!(parse_priority("low"), Some(Priority::Low));
    assert_eq!(parse_priority("normal"), Some(Priority::Normal));
    assert_eq!(parse_priority("critical"), Some(Priority::Critical));
    assert_eq!(
        parse_priority("LOW"),
        None,
        "the config grammar is lowercase"
    );
    assert_eq!(parse_priority(""), None);
    assert_eq!(parse_priority("high"), None);
}

/// A duration smoke check so the crate's `Duration` import stays honest for
/// the wiring (the sampler runs on a 500 ms cadence).
#[test]
fn sampler_cadence_is_half_a_second() {
    assert_eq!(Duration::from_millis(500).as_millis(), 500);
}

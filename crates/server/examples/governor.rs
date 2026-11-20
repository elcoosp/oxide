//! Consumer walkthrough for the brownout governor (V2-F12): the
//! deliberate-degradation ladder an operator rides under overload.
//!
//! The wiring (the sampler task, the enforcement stage) is the CLI's
//! adoption; this walkthrough shows the pieces the crate owns, end to end:
//!
//! 1. The pure breach ladder (`raw_level`): which rung one tick's signals
//!    reach.
//! 2. The hysteresis transition (`next_level`): escalate immediately,
//!    de-escalate only after `calm_ticks` consecutive calm ticks.
//! 3. The enforcement seam (`serve_decision`): level × route-priority →
//!    what the stage does with the request.
//! 4. The shed answer (`shed_response`): the `503` a shed request
//!    receives (`retry-after: 5`, `x-oxide-brownout: <level>`) —
//!    deliberate degradation, distinguishable from a crash.
//! 5. The readiness contract (`ready_status`): `/_oxide/ready` flips to
//!    `503` only at `Emergency` so a balancer can drain; liveness stays.
//! 6. The `Governor` itself: the sampler publishes, the request path
//!    reads one relaxed atomic.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-server --example governor
//! ```

use oxide_fw_server::Governor;
use oxide_fw_server::governor::{
    Level, Priority, Signals, Thresholds, next_level, raw_level, ready_status, serve_decision,
    shed_response,
};

fn main() {
    let t = Thresholds::default();
    let mut calm = 0u32;

    // 1. The calm tick: nothing breaches, the ladder stays at the floor.
    let calm_signals = Signals::default();
    println!(
        "calm signals -> raw_level={} (Normal)",
        raw_level(&calm_signals, &t)
    );

    // 2. One tick breaches rung 1 (pool queue-wait): the level escalates
    //    immediately — brownout does not wait for a second opinion.
    let queued = Signals {
        queue_wait_p99_us: t.queue_wait_us[0],
        ..Default::default()
    };
    let level = next_level(0, &queued, &t, &mut calm);
    println!(
        "queue-wait breach -> level {} (Stale), calm ticker reset to {calm}",
        level
    );
    println!(
        "  Normal route at Stale -> {:?} (the warning: 110 header rides)",
        serve_decision(Level::Stale, Priority::Normal)
    );

    // 3. A second rung breaches: the ladder steps to Shed — low-priority
    //    routes shed, the shed answer names the level and the retry
    //    window.
    let strained = Signals {
        queue_wait_p99_us: t.queue_wait_us[1],
        ..Default::default()
    };
    let level = next_level(level, &strained, &t, &mut calm);
    println!("strain -> level {level} (Shed)");
    println!(
        "  Low route at Shed -> {:?} (x-oxide-brownout: 2), status {}",
        serve_decision(Level::Shed, Priority::Low),
        shed_response(Level::Shed).status
    );
    println!(
        "  Critical route at Shed -> {:?} (critical routes still answer)",
        serve_decision(Level::Shed, Priority::Critical)
    );

    // 4. A third rung breaches: Emergency — everything but Critical
    //    sheds, and readiness flips so a balancer can drain the node
    //    (liveness stays 200).
    let saturated = Signals {
        queue_wait_p99_us: t.queue_wait_us[2],
        loop_lag_us: t.loop_lag_us[2],
        inflight: u32::MAX,
        rss_mb: t.rss_mb[2],
    };
    let level = next_level(level, &saturated, &t, &mut calm);
    println!("saturation -> level {level} (Emergency)");
    println!(
        "  at Emergency -> ready_status {} (/_oxide/health stays 200)",
        ready_status(Level::Emergency)
    );
    println!(
        "  Critical route at Emergency -> {:?}",
        serve_decision(Level::Emergency, Priority::Critical)
    );

    // 5. The calm tail: de-escalation takes `calm_ticks` CONSECUTIVE calm
    //    ticks, one level at a time — a flickering signal cannot bounce
    //    the ladder.
    println!(
        "\nthe calm tail: calm_ticks={} in the default thresholds",
        t.calm_ticks
    );
    let mut level = 3u8;
    let mut calm = 0u32;
    for tick in 1..=t.calm_ticks {
        level = next_level(level, &calm_signals, &t, &mut calm);
        if tick == 1 || tick == t.calm_ticks {
            println!("calm tick {tick}: level {level}");
        }
    }
    println!(
        "after {} calm ticks: level {level} (one drop per calm stretch)",
        t.calm_ticks
    );

    // 6. The Governor: the sampler publishes; the request path reads one
    //    relaxed atomic.
    let governor = Governor::new();
    governor.set(2);
    println!(
        "\nthe sampler published 2; the request path reads {}",
        governor.level() as u8
    );
}

//! The skew-protection pool walkthrough (§V2-F10): a bounded pool of
//! demoted generations keeps every deploy inside the skew window working
//! for its open tabs, the oldest evicts first when the pool is full, and
//! the same data-plane stage drives the 1-deep `GenerationSet` and the
//! pool with identical verdicts.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dispatch --example skew_pool
//! ```

use std::time::{Duration, Instant};

use oxide_fw_dispatch::build::{BUILD_HEADER, GenerationSet, STALE_STATUS};
use oxide_fw_dispatch::skew::{AcceptedGenerations, GenerationPool, stage_check};

const WINDOW: Duration = Duration::from_mins(15);

/// One data-plane probe: does a request advertising `id` continue?
fn continues(gens: &impl AcceptedGenerations, id: &str, now: Instant) -> bool {
    matches!(
        stage_check(
            "/__data",
            &[(BUILD_HEADER.to_string(), id.to_string())],
            gens,
            now,
        ),
        oxide_fw_dispatch::flow::Flow::Continue
    )
}

fn main() {
    let t0 = Instant::now();
    let stage = |label: &str| println!("{label}");

    // The 1-deep §V2-A04 substrate: promoting twice drops generation
    // N-1 immediately.
    let mut set = GenerationSet::new("aaaaaaaaaaaaaaaa".to_string(), WINDOW);
    set.promote("bbbbbbbbbbbbbbbb".to_string(), t0);
    set.promote("cccccccccccccccc".to_string(), t0 + Duration::from_secs(1));
    stage("GenerationSet after three deploys (1-deep):");
    for id in ["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb", "cccccccccccccccc"] {
        println!(
            "  {id} -> {:?}",
            set.accepts(id, t0 + Duration::from_secs(2))
        );
    }

    // The F10 pool at depth 3: every demotion survives its window.
    let mut pool = GenerationPool::new("aaaaaaaaaaaaaaaa".to_string(), WINDOW, 3);
    pool.promote("bbbbbbbbbbbbbbbb".to_string(), t0);
    pool.promote("cccccccccccccccc".to_string(), t0 + Duration::from_secs(1));
    pool.promote("dddddddddddddddd".to_string(), t0 + Duration::from_secs(2));
    stage("GenerationPool at capacity 3 after the same four deploys:");
    for id in ["aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb", "cccccccccccccccc"] {
        println!(
            "  {id} -> {:?}",
            pool.accepts(id, t0 + Duration::from_secs(3))
        );
    }

    // The 409 verdict for an unknown id (the stage's §V2-A04 contract).
    let resp = stage_check(
        "/__data",
        &[(BUILD_HEADER.to_string(), "9999999999999999".to_string())],
        &pool,
        t0 + Duration::from_secs(3),
    );
    match resp {
        oxide_fw_dispatch::flow::Flow::Respond(r) => {
            let _ = r;
            stage(&format!(
                "unknown id -> {STALE_STATUS} + x-oxide-build-stale"
            ));
        }
        oxide_fw_dispatch::flow::Flow::Continue => stage("unexpected: an unknown id continued"),
    }

    // Eviction: the pool bounds the "how old may a replay be" question —
    // the oldest demotion goes first when the capacity is reached.
    pool.promote("eeeeeeeeeeeeeeee".to_string(), t0 + Duration::from_secs(4));
    stage("pool after a fifth deploy at capacity 3:");
    println!(
        "  aaaa… (the oldest demotion) -> {:?}",
        pool.accepts("aaaaaaaaaaaaaaaa", t0 + Duration::from_secs(5))
    );
    println!(
        "  bbbb… -> {:?}",
        pool.accepts("bbbbbbbbbbbbbbbb", t0 + Duration::from_secs(5))
    );

    // Expiry reclaims: past EVERY demotion's own window (each clock runs
    // from its promote), everything demoted is refused and the
    // housekeeping pass reports the reclamation.
    let late = t0 + WINDOW + Duration::from_secs(5);
    println!(
        "  after the window: expire() reclaimed {} demotions, current still serves: {}",
        pool.expire(late),
        continues(&pool, "eeeeeeeeeeeeeeee", late)
    );
}

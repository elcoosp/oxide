//! The two-generation serving-pools walkthrough (§V2-F10 design steps 1-3):
//! after a deploy the old build's data plane keeps serving old tabs from
//! ITS OWN pool for the skew window, HTML and header-less requests always
//! take the current pool, expiry drops the old handles (unlinks nothing),
//! the brownout early-drop hands the evicted handle back for a graceful
//! shutdown, and the same §V2-A04 stage drives the pools through the
//! `AcceptedGenerations` seam with the 1-deep set's verdicts.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dispatch --example serving_pools
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use oxide_fw_dispatch::build::{BUILD_HEADER, Gen, STALE_STATUS};
use oxide_fw_dispatch::flow::Flow;
use oxide_fw_dispatch::pools::GenerationPools;
use oxide_fw_dispatch::skew::stage_check;

const WINDOW: Duration = Duration::from_mins(15);

/// The fake serving handle: a real server pools workers per dist root;
/// here the identity is the label so the routing is visible.
#[derive(Debug)]
struct Pool(&'static str);

/// One routing probe, printed: which pool serves a request advertising
/// `id` (or none — HTML / header-less)?
fn route(pools: &GenerationPools<Arc<Pool>>, id: Option<&str>, now: Instant) -> String {
    match pools.route(id, now) {
        Some((pool, Gen::Current)) => format!("CURRENT  {}", pool.0),
        Some((pool, Gen::Previous)) => format!("PREVIOUS {}", pool.0),
        None => "409 x-oxide-build-stale (via stage_check)".to_string(),
    }
}

fn main() {
    let t0 = Instant::now();

    // Boot generation 1 (`oxide start` loads dist/ as G1).
    let g1 = Arc::new(Pool("G1 workers"));
    let mut pools =
        GenerationPools::new("aaaaaaaaaaaaaaaa".to_string(), Arc::clone(&g1), WINDOW, 2);

    // Deploy generation 2: the serve side boots a fresh pool over the new
    // dist and promotes it; G1's pool is demoted with its OWN demotion
    // instant — the skew window runs from the demotion.
    let g2 = Arc::new(Pool("G2 workers"));
    let t_deploy = t0 + Duration::from_secs(60);
    pools.promote("bbbbbbbbbbbbbbbb".to_string(), Arc::clone(&g2), t_deploy);

    println!("Two generations live after the deploy (t+60s):");
    println!(
        "  HTML navigation (no id)      -> {}",
        route(&pools, None, t_deploy)
    );
    println!(
        "  /__data with x-oxide-build G2 -> {}",
        route(&pools, Some("bbbbbbbbbbbbbbbb"), t_deploy)
    );
    println!(
        "  /__data with x-oxide-build G1 -> {}",
        route(&pools, Some("aaaaaaaaaaaaaaaa"), t_deploy)
    );
    println!(
        "  /__data with unknown id       -> {}",
        route(&pools, Some("9999999999999999"), t_deploy)
    );

    // The 409 authority is the one §V2-A04 stage, driven through the seam.
    let stale = stage_check(
        "/__data",
        &[(BUILD_HEADER.to_string(), "9999999999999999".to_string())],
        &pools,
        t_deploy,
    );
    let stale_status = match stale {
        Flow::Respond(resp) if resp.status == STALE_STATUS => "409 + x-oxide-build-stale: true",
        _ => "unexpected",
    };
    println!("\n  stage_check over the same pools (unknown id) -> {stale_status}");

    // The rollback path (§V2-F10 step 4 / F11's pointer swap): re-promote
    // G1 — the serve side boots a FRESH pool over the old dist root and
    // hands it in; the stale demoted entry drops.
    let g1_fresh = Arc::new(Pool("G1 workers (rebooted)"));
    let t_rollback = t_deploy + Duration::from_secs(30);
    pools.promote(
        "aaaaaaaaaaaaaaaa".to_string(),
        Arc::clone(&g1_fresh),
        t_rollback,
    );
    println!("\nAfter the rollback to G1 (t+90s):");
    println!(
        "  /__data with x-oxide-build G1 -> {}",
        route(&pools, Some("aaaaaaaaaaaaaaaa"), t_rollback)
    );
    println!(
        "  /__data with x-oxide-build G2 -> {}",
        route(&pools, Some("bbbbbbbbbbbbbbbb"), t_rollback)
    );
    println!("  demoted generations held: {}", pools.len());

    // Expiry (the 30 s housekeeping pass): past G2's window its handle
    // drops — "at window end it drops the previous pool and unlinks
    // nothing" (the client chunks are content-hashed and stay valid).
    let after = t_rollback + WINDOW + Duration::from_millis(1);
    let dropped = pools.expire(after);
    println!("\nHousekeeping at t+105m: expire() dropped {dropped} expired pool(s).");
    println!(
        "  /__data with x-oxide-build G2 -> {}",
        route(&pools, Some("bbbbbbbbbbbbbbbb"), after)
    );
    println!(
        "  /__data with x-oxide-build G1 -> {}",
        route(&pools, Some("aaaaaaaaaaaaaaaa"), after)
    );

    // The brownout early-drop (the F12 hook): when the queue wait crosses
    // the threshold the oldest demoted pool comes back for a graceful
    // drain-and-shutdown instead of leaking the drop.
    let g3 = Arc::new(Pool("G3 workers"));
    pools.promote("cccccccccccccccc".to_string(), g3, after);
    if let Some((id, pool)) = pools.evict_oldest() {
        println!(
            "\nBrownout eviction: {id} hands back {} for drain + shutdown.",
            pool.0
        );
    }
    println!("  demoted generations held: {}", pools.len());
}

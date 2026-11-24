//! The §V2-F10 evict trigger's walkthrough (the F12 hook): the governor's
//! `Emergency` rung releases the OLDEST demoted generation — the id level
//! stops accepting it (the data-plane stage answers 409) and the serving
//! level hands its worker pool + loader runner back, so the current
//! generation reclaims the capacity instead of a lockup leaking the drop.
//!
//! The walkthrough boots the two pools the production serve holds (the
//! id-level [`SkewState`] handle and the [`ServingPools`]), attaches the
//! boot's early-drop seam to the brownout wiring, then walks the ladder:
//! `Normal` and `Shed` keep the old generation's skew contract, and each
//! `Emergency` tick pops the oldest — one per tick, the drain spreading
//! over the 500 ms cadence.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-cli --no-default-features --example brownout_evict
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use oxide_fw_cli::serve::brownout::{BrownoutWiring, drain_one_early_drop};
use oxide_fw_cli::serve::generations::{ServingPools, SkewState, evict_handle_oldest};
use oxide_fw_dispatch::skew::AcceptedGenerations as _;
use oxide_fw_server::governor::Level;

/// A pool that is never called — the walkthrough exercises the routing
/// and the eviction bookkeeping, not the worker.
struct DeadPool;

#[async_trait::async_trait]
impl oxide_fw_jshost::Pool for DeadPool {
    async fn call(
        &self,
        _entry: oxide_fw_jshost::EntryRef,
        _req: oxide_fw_jshost::CallInput,
    ) -> oxide_fw_jshost::Result<oxide_fw_jshost::CallStream> {
        Err(oxide_fw_diagnostics::OxideError(Box::new(
            oxide_fw_diagnostics::Diagnostic::error("TST-POOL", "the walkthrough never calls"),
        )))
    }
    fn stats(&self) -> oxide_fw_jshost::PoolStats {
        oxide_fw_jshost::PoolStats {
            workers: 0,
            in_flight: 0,
            completed: 0,
            failed: 0,
            recycles: 0,
            crashed: 0,
            queue_wait_ms_p50: 0.0,
            queue_wait_ms_p99: 0.0,
        }
    }
    fn abort_request(&self, _request_id: &str) {}
    async fn drain(&self) {}
}

fn generation() -> Arc<crate_serve_gen::GenServing> {
    crate_serve_gen::GenServing {
        pool: Arc::new(DeadPool),
        runner: None,
        routes_root: PathBuf::from("/tmp/unused-snapshot"),
    }
    .into()
}

/// The `GenServing` re-export shim — the type lives under
/// `oxide_fw_cli::serve::generations`; the alias keeps the helper
/// signatures readable.
mod crate_serve_gen {
    pub use oxide_fw_cli::serve::generations::GenServing;
}

fn main() {
    // The id-level pool: the boot id is CURRENT, two reloads demote two
    // generations (the SIGHUP promote's shape, replayed in-process).
    let skew = SkewState::from_boot("aaaaaaaaaaaaaaaa", "15m", 3);
    let id = |seed: u8| format!("{:016x}", u64::from(seed));
    skew.handle()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .promote(id(2), std::time::Instant::now());
    skew.handle()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .promote(id(3), std::time::Instant::now());

    // The serving pools: the same two deploys, each carrying its own
    // (fake) worker pool + runner — the two structures the production
    // boot wires the seam over, in lockstep.
    let pools = ServingPools::from_boot(&id(2), generation(), Duration::from_secs(900), 3);
    pools.promote_booted(&id(3), generation());

    // The brownout wiring (the `brownout.enabled` gate is `from_config`'s
    // own None arm) with the boot's early-drop seam: BOTH pools drain,
    // the id level first — the acceptance verdict flips (the stage 409s
    // the evicted build) at the same tick the handle goes back.
    let cfg = oxide_fw_config::BrownoutConfig {
        enabled: true,
        ..Default::default()
    };
    let wiring = BrownoutWiring::from_config(&cfg, None, Arc::new(oxide_fw_server::Metrics::new()))
        .expect("enabled → wiring");
    let skew_handle = skew.handle();
    let pools_for_seam = Arc::clone(&pools);
    let wiring = wiring
        .with_early_drop(Arc::new(move || {
            let id_level = evict_handle_oldest(&skew_handle);
            let handle_level = pools_for_seam.evict_oldest();
            id_level.or(handle_level)
        }))
        .unwrap_or_else(|_| panic!("the seam sets once"));

    println!("== the ladder ==");
    wiring.governor.set(Level::Normal as u8);
    println!(
        "  Normal:    drain -> {:?}   (the skew window keeps its contract)",
        drain_one_early_drop(&wiring)
    );
    wiring.governor.set(Level::Shed as u8);
    println!(
        "  Shed:      drain -> {:?}   (low-priority routes shed; old builds keep serving)",
        drain_one_early_drop(&wiring)
    );
    wiring.governor.set(Level::Emergency as u8);
    println!(
        "  Emergency: drain -> {:?}   (the oldest generation released)",
        drain_one_early_drop(&wiring)
    );
    println!(
        "  Emergency: drain -> {:?}   (the next-oldest, one per tick)",
        drain_one_early_drop(&wiring)
    );
    println!(
        "  Emergency: drain -> {:?}   (nothing demoted — the honest no-op)",
        drain_one_early_drop(&wiring)
    );
    println!("== after the drain ==");
    println!(
        "  acceptance: id {} -> {:?}  (the stage answers 409 from here)",
        id(1),
        skew.handle()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .accepts(&id(1), std::time::Instant::now())
    );
    println!(
        "  acceptance: id {} -> {:?}  (the current build stands)",
        id(3),
        skew.handle()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .accepts(&id(3), std::time::Instant::now())
    );
    println!(
        "  routing:    id {} -> {:?}   (its handle went with it)",
        id(1),
        pools.route_for(Some(&id(1))).map(|(_, g)| g)
    );
}

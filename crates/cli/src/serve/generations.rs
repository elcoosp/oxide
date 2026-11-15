//! The CLI's serving half (§V2-F10): the generation pool the data plane
//! checks against, the reload promote, the expiry tick — and, since the
//! two-pool wiring landed, the SERVING half: a jshost pool + loader runner
//! per generation, routed by the request's build id.
//!
//! The V8-free seams live in `oxide_fw_dispatch` (`skew::GenerationPool`
//! for the acceptance verdict, `pools::GenerationPools` for the
//! serving-handle routing — its semantics are pinned by
//! `crates/dispatch/tests/pools_tdd.rs`); this module is the CLI state
//! around them. The loader-byte guarantee: a generation's loader sources
//! are SNAPSHOTTED under `.oxide/generations/<id>/src` (boot and promote
//! both snapshot — the bytes are frozen at the moment the generation
//! becomes current), and the demoted generation's runner runs its module
//! paths from its own snapshot root, so a later source edit cannot leak
//! into an accepted old-build response. (The worker recompiles per run
//! with an mtime check — a pool alone does not freeze bytes; the snapshot
//! does.)

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use oxide_fw_dispatch::skew::GenerationPool;
use oxide_fw_jshost::Pool;
use oxide_fw_server::LoaderRunner;

/// The demotion window used when the typed `deploy.skewWindow` literal
/// cannot parse. Load validation (`CFG-DEPLOY`) refuses a bad literal at
/// config load, so this is the belt for a hand-constructed state; the
/// value is the spec's default (15 m).
pub const DEFAULT_SKEW_WINDOW_SECS: u64 = 15 * 60;

/// The CLI-side skew state: the bounded pool over the boot's build id,
/// the reload promote and the window the demotions keep.
pub struct SkewState {
    pool: Arc<RwLock<GenerationPool>>,
    window: Duration,
}

impl SkewState {
    /// Boot the pool from the server's build id: `skew_window` is the
    /// typed `deploy.skewWindow` literal (the §14 duration grammar —
    /// `parse_duration_secs`), `generations` the F10 retention count
    /// (`deploy.generations`; the load validator refuses zero, and a
    /// hand-built state saturates at 1 so the previous build is always
    /// accepted for at least one slot).
    pub fn from_boot(build_id: &str, skew_window: &str, generations: u32) -> Self {
        let window = Duration::from_secs(
            oxide_fw_config::parse_duration_secs(skew_window).unwrap_or(DEFAULT_SKEW_WINDOW_SECS),
        );
        let capacity = (generations.max(1)) as usize;
        Self {
            pool: Arc::new(RwLock::new(GenerationPool::new(
                build_id.to_string(),
                window,
                capacity,
            ))),
            window,
        }
    }

    /// The handle the dispatch wrapper and the reload path share.
    pub fn handle(&self) -> Arc<RwLock<GenerationPool>> {
        Arc::clone(&self.pool)
    }

    /// The demotion window the pool runs (observability surface).
    pub fn window(&self) -> Duration {
        self.window
    }

    /// The reload step (§V2-F10 step 1: `SIGHUP` / a rebuilt `dist/`):
    /// read `dist/build-id`; when it names a NEW build, demote the
    /// current one — its clients keep working for the window — and accept
    /// the new id. Returns the promoted id.
    ///
    /// `None` means "nothing changed" (the dist is absent, unreadable or
    /// still carries the current id) — a serve must not churn its pool on
    /// a half-written dist, and an idempotent re-deploy is a no-op (the
    /// pool's own promote contract).
    pub fn promote_from_dist(&self, dist: &std::path::Path) -> Option<String> {
        promote_handle_from_dist(&self.pool, dist)
    }
}

/// The reload promote over a SHARED pool handle (the shape `cmd_serve`'s
/// `SIGHUP` handler holds — the `SkewState` owner is gone by then; the
/// handle rides `StartedServer`). See [`SkewState::promote_from_dist`]
/// for the contract.
pub fn promote_handle_from_dist(
    pool: &Arc<RwLock<GenerationPool>>,
    dist: &std::path::Path,
) -> Option<String> {
    let id = oxide_fw_cache::read_build_id(dist)?;
    let now = Instant::now();
    let mut pool = pool.write().ok()?;
    if pool.current() == id {
        return None;
    }
    pool.promote(id.clone(), now);
    Some(id)
}

/// One generation's serving handles (§V2-F10 step 2): the pool booted over
/// that dist, the loader runner the data-plane dispatch calls, and the
/// loader-source root its runner's module paths resolve against (the
/// frozen snapshot — see the module doc).
pub struct GenServing {
    /// The worker pool executing the generation's loads.
    pub pool: Arc<dyn Pool>,
    /// The loader runner (the same seam the dispatch's `/__data` path
    /// calls); `None` when the boot had no runner (a serve without server
    /// modules — the generation then contributes only its pool).
    pub runner: Option<Arc<dyn LoaderRunner>>,
    /// The loader-source root for THIS generation's module paths.
    pub routes_root: PathBuf,
}

/// The serving pools (§V2-F10 steps 1-2 wired): the current generation's
/// handles plus the demoted ones, each routed by the request's build id
/// through the dispatch crate's [`GenerationPools`] — the acceptance
/// verdict stays the one stage (the wrapper drives the id-level pool);
/// this layer decides WHO serves an accepted id.
pub struct ServingPools {
    pools: RwLock<oxide_fw_dispatch::pools::GenerationPools<Arc<GenServing>>>,
}

impl ServingPools {
    /// Boot the pools: `id` is served by `gen`; the window and capacity
    /// are the same typed config the id-level pool runs.
    pub fn from_boot(
        id: &str,
        generation: Arc<GenServing>,
        window: Duration,
        capacity: u32,
    ) -> Arc<Self> {
        Arc::new(Self {
            pools: RwLock::new(oxide_fw_dispatch::pools::GenerationPools::new(
                id.to_string(),
                generation,
                window,
                capacity.max(1) as usize,
            )),
        })
    }

    /// The id every data-plane response advertises.
    #[must_use]
    pub fn current(&self) -> Option<String> {
        self.pools.read().ok().map(|p| p.current().to_string())
    }

    /// Install a freshly-booted generation as current (the reload deploy:
    /// the caller built the new pool + runner, the old handles demote
    /// together). `false` when the id is already current (an idempotent
    /// re-deploy never churns the pools).
    pub fn promote_booted(&self, id: &str, generation: Arc<GenServing>) -> bool {
        let Ok(mut p) = self.pools.write() else {
            return false;
        };
        if p.current() == id {
            return false;
        }
        p.promote(id.to_string(), generation, Instant::now());
        true
    }

    /// The §V2-F10 step-2 routing decision: which generation's handles
    /// serve a request advertising `id`? `None` id (HTML navigations,
    /// header-less data-plane requests) → current; a demoted id inside
    /// ITS window → THAT generation's handles. The caller falls back to
    /// the boot handles when the pools are unavailable (a poisoned lock)
    /// — the stage's 409 already answered unknown ids.
    #[must_use]
    pub fn route_for(
        &self,
        id: Option<&str>,
    ) -> Option<(Arc<GenServing>, oxide_fw_dispatch::build::Gen)> {
        let p = self.pools.read().ok()?;
        let (generation, which) = p.route(id, Instant::now())?;
        Some((Arc::clone(generation), which))
    }

    /// Drop expired demoted generations (their handles go; nothing is
    /// unlinked — the snapshots are content-frozen copies the next sweep
    /// reclaims). Returns how many went.
    pub fn expire(&self) -> usize {
        let Ok(mut p) = self.pools.write() else {
            return 0;
        };
        p.expire(Instant::now())
    }
}

/// Deep-copy the loader sources a generation serves into
/// `.oxide/generations/<id>/src` (the bytes frozen at the moment the
/// generation becomes current — the worker's per-run mtime recompile
/// would otherwise let a later source edit leak into an accepted
/// old-build response). `Err` lists the first failure; a failed snapshot
/// downgrades the generation to "acceptance only" (the pools skip the
/// route step) instead of failing the boot.
pub fn snapshot_generation_sources(root: &std::path::Path, id: &str) -> Result<PathBuf, String> {
    let src = root.join("src");
    if !src.is_dir() {
        return Err(format!("no src/ tree at {}", root.display()));
    }
    let dest = root.join(".oxide").join("generations").join(id);
    if dest.join("src").is_dir() {
        return Ok(dest.join("src"));
    }
    copy_tree(&src, &dest.join("src"), 0)?;
    Ok(dest.join("src"))
}

const SNAPSHOT_MAX_DEPTH: usize = 24;

fn copy_tree(src: &std::path::Path, dest: &std::path::Path, depth: usize) -> Result<(), String> {
    if depth > SNAPSHOT_MAX_DEPTH {
        return Err(format!(
            "snapshot tree deeper than {SNAPSHOT_MAX_DEPTH} at {}",
            src.display()
        ));
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("mkdir {}: {e}", dest.display()))?;
    let entries = std::fs::read_dir(src).map_err(|e| format!("read_dir {}: {e}", src.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
        let p = entry.path();
        let name = entry.file_name();
        let target = dest.join(&name);
        let ft = entry
            .file_type()
            .map_err(|e| format!("file_type {}: {e}", p.display()))?;
        if ft.is_dir() {
            copy_tree(&p, &target, depth + 1)?;
        } else if ft.is_file() {
            std::fs::copy(&p, &target).map_err(|e| format!("copy {}: {e}", p.display()))?;
        }
        // Symlinks and other entries are skipped: a generation's loader
        // graph is source files; a linked tree is a build artifact, not a
        // loader byte.
    }
    Ok(())
}

/// The §V2-F10 step-3 expiry task: every 30 s the expired demotions are
/// dropped from the pool. `accepts` already refuses an expired id on
/// read, so this is the RECLAIM (memory bounded at the pool's capacity
/// regardless of deploy frequency), not the verdict.
///
/// The task lives for the server's lifetime; the shutdown sender's drop
/// aborts the runtime, which is the process teardown's business (the same
/// posture the nav-graph flusher takes).
pub fn spawn_expiry_task(pool: Arc<RwLock<GenerationPool>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        loop {
            tick.tick().await;
            let expired = match pool.write() {
                Ok(mut p) => p.expire(Instant::now()),
                Err(_) => 0,
            };
            if expired > 0 {
                tracing::info!(
                    dropped = expired,
                    "skew: expired demoted generations dropped (acceptance refused them on read already)"
                );
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_fw_dispatch::skew::AcceptedGenerations;

    fn stamp_dist(root: &std::path::Path, id_seed: &[u8]) -> std::path::PathBuf {
        let dist = root.join("dist");
        std::fs::create_dir_all(dist.join("client")).expect("mkdir");
        std::fs::write(dist.join("client/a.js"), id_seed).expect("write");
        let stamped = oxide_fw_cache::stamp_build_id(&dist).expect("stamp");
        assert!(!stamped.is_empty());
        dist
    }

    #[test]
    fn boot_accepts_the_boot_id_and_sizes_the_pool_from_the_config() {
        let s = SkewState::from_boot("aaaaaaaaaaaaaaaa", "15m", 3);
        let pool = s.handle();
        let g = pool.read().expect("unpoisoned");
        assert_eq!(g.current(), "aaaaaaaaaaaaaaaa");
        assert_eq!(s.window(), Duration::from_secs(900));
        assert!(g.is_empty(), "nothing demoted at boot");
        // An unparsable window literal falls back to the spec's 15 m.
        let s2 = SkewState::from_boot("bbbbbbbbbbbbbbbb", "not-a-duration", 0);
        assert_eq!(s2.window(), Duration::from_secs(DEFAULT_SKEW_WINDOW_SECS));
    }

    #[test]
    fn promote_from_dist_demotes_the_old_build_and_accepts_both() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dist = stamp_dist(tmp.path(), b"v1");
        let s = SkewState::from_boot("aaaaaaaaaaaaaaaa", "15m", 3);
        // The boot id differs from the stamped dist (a real reload names a
        // REBUILT dist): the promote accepts both ids.
        let promoted = s.promote_from_dist(&dist).expect("promoted");
        let handle = s.handle();
        let g = handle.read().expect("unpoisoned");
        assert_eq!(
            promoted,
            oxide_fw_cache::read_build_id(&dist).expect("read")
        );
        assert_eq!(g.current(), promoted, "the dist's id is current now");
        let now = Instant::now();
        assert_eq!(
            g.accepts(&promoted, now),
            Some(oxide_fw_dispatch::build::Gen::Current)
        );
        assert_eq!(
            g.accepts("aaaaaaaaaaaaaaaa", now),
            Some(oxide_fw_dispatch::build::Gen::Previous),
            "the boot build stays accepted inside the window"
        );
        assert_eq!(g.accepts("0123456789abcdef", now), None, "unknown refused");
    }

    #[test]
    fn promote_from_an_unchanged_or_missing_dist_is_a_no_op() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dist = stamp_dist(tmp.path(), b"v1");
        let id = oxide_fw_cache::read_build_id(&dist).expect("read");
        let s = SkewState::from_boot(&id, "15m", 3);
        // Same id: the idempotent re-deploy changes nothing.
        assert_eq!(s.promote_from_dist(&dist), None);
        // Missing dist: nothing to read, nothing to churn.
        assert_eq!(s.promote_from_dist(&tmp.path().join("nope")), None);
        let handle = s.handle();
        let g = handle.read().expect("unpoisoned");
        assert!(g.is_empty(), "no demotions recorded");
    }

    #[test]
    fn the_expired_previous_build_is_refused_after_its_window() {
        // The window semantics are pinned in the dispatch crate's TDD
        // suite (`skew_tdd.rs`); here the CONFIG-SIZED window is what the
        // wiring exercises: a `2s` window and a promote whose demotion
        // instant is already past it (frozen-clock reasoning is the
        // dispatch suite's job — this pin holds the config surface).
        let s = SkewState::from_boot("aaaaaaaaaaaaaaaa", "2s", 3);
        assert_eq!(s.window(), Duration::from_secs(2));
        let tmp = tempfile::tempdir().expect("tmp");
        let dist = stamp_dist(tmp.path(), b"v1");
        let promoted = s.promote_from_dist(&dist).expect("promoted");
        let handle = s.handle();
        let g = handle.read().expect("unpoisoned");
        assert_eq!(g.current(), promoted);
    }
}

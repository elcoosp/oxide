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

use oxide_fw_diagnostics::OxideError;
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
    // Poison recovery, not a silent bail (the house shape the F47/F48
    // session applied to the render stage's shell-cache read): a poisoned
    // write lock used to make every SIGHUP promote after a panicking
    // holder quietly return `None` — "nothing changed" — so a real deploy
    // was dropped while the server kept advertising the stale generation.
    // The promote is the pool's own contract; recovering the guard hands
    // the deploy through.
    let mut pool = pool
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    ///
    /// The read recovers a poisoned lock (the house shape — a pure read
    /// has no invariant to tear), so an advertise after a panicking
    /// holder answers the honest current id instead of `None`.
    #[must_use]
    pub fn current(&self) -> Option<String> {
        self.pools
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .current()
            .to_string()
            .into()
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
    /// the boot handles when the pools answer no generation (the stage's
    /// 409 already answered unknown ids). The read recovers a poisoned
    /// lock (the house shape — a pure routing read has no invariant to
    /// tear), so a panicking holder no longer silently degrades every
    /// later request to the fallback.
    #[must_use]
    pub fn route_for(
        &self,
        id: Option<&str>,
    ) -> Option<(Arc<GenServing>, oxide_fw_dispatch::build::Gen)> {
        let p = self
            .pools
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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

    /// The brownout early-drop (the F10 evict trigger's serving half):
    /// pop the OLDEST demoted generation and return its id — its worker
    /// pool and loader runner go with it, so the current generation
    /// reclaims the capacity when the governor hits `Emergency` instead
    /// of a lockup leaking the drop. `None` when nothing is demoted.
    /// The read recovers a poisoned lock (the house shape — the drop is
    /// the graceful path a panicking holder must not skip; a silent
    /// `None` here would strand the old pool past the mode's own
    /// shutdown intent, the bug class `route_for`'s recovery fixed).
    pub fn evict_oldest(&self) -> Option<String> {
        let mut p = self
            .pools
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        p.evict_oldest().map(|(id, _pool)| id)
    }
}

/// The brownout early-drop over a SHARED id-level pool handle (the shape
/// the boot's early-drop closure holds — the `SkewState` owner stays with
/// the wiring; the handle rides into the closure like the SIGHUP path's
/// does). See [`oxide_fw_dispatch::skew::GenerationPool::evict_oldest`]
/// for the contract: the OLDEST demotion's id comes back and stops being
/// accepted (the stage 409s it from here on).
pub fn evict_handle_oldest(
    pool: &std::sync::Arc<std::sync::RwLock<oxide_fw_dispatch::skew::GenerationPool>>,
) -> Option<String> {
    let mut p = pool
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    p.evict_oldest()
}

/// The snapshot refusals' coded constructor — the crate-wide diagnostic
/// convention (code + severity + message, `SRV-*` per the registry).
fn gen_err(code: &'static str, msg: impl Into<String>) -> oxide_fw_diagnostics::OxideError {
    oxide_fw_diagnostics::OxideError(Box::new(oxide_fw_diagnostics::Diagnostic::error(code, msg)))
}

/// Deep-copy the loader sources a generation serves into
/// `.oxide/generations/<id>/src` (the bytes frozen at the moment the
/// generation becomes current — the worker's per-run mtime recompile
/// would otherwise let a later source edit leak into an accepted
/// old-build response). `Err` carries the first failure as a coded
/// diagnostic (`SRV-GEN-IO` — the missing `src/` tree and every copy
/// failure with the path in the message; `SRV-GEN-DEPTH` — the depth
/// guard); a failed snapshot downgrades the generation to "acceptance
/// only" (the pools skip the route step) instead of failing the boot.
///
/// The same generations directory also holds the §V2-F10 step-4
/// retention: [`retain_dist_generation`] freezes the emitted
/// `dist/` into `.oxide/generations/<id>/dist`, [`resolve_generation`]
/// names one (`previous` or an explicit id) and
/// [`repoint_dist_to_generation`] swaps it into `dist/` — the rollback
/// path `oxide rollback` and `oxide start --generation` drive.
pub fn snapshot_generation_sources(
    root: &std::path::Path,
    id: &str,
) -> Result<PathBuf, oxide_fw_diagnostics::OxideError> {
    let src = root.join("src");
    if !src.is_dir() {
        return Err(gen_err(
            "SRV-GEN-IO",
            format!("no src/ tree at {}", root.display()),
        ));
    }
    let dest = root.join(".oxide").join("generations").join(id);
    if dest.join("src").is_dir() {
        return Ok(dest.join("src"));
    }
    copy_tree(&src, &dest.join("src"), 0)?;
    Ok(dest.join("src"))
}

const SNAPSHOT_MAX_DEPTH: usize = 24;

fn copy_tree(
    src: &std::path::Path,
    dest: &std::path::Path,
    depth: usize,
) -> Result<(), oxide_fw_diagnostics::OxideError> {
    if depth > SNAPSHOT_MAX_DEPTH {
        return Err(gen_err(
            "SRV-GEN-DEPTH",
            format!(
                "snapshot tree deeper than {SNAPSHOT_MAX_DEPTH} at {}",
                src.display()
            ),
        ));
    }
    std::fs::create_dir_all(dest)
        .map_err(|e| gen_err("SRV-GEN-IO", format!("mkdir {}: {e}", dest.display())))?;
    let entries = std::fs::read_dir(src)
        .map_err(|e| gen_err("SRV-GEN-IO", format!("read_dir {}: {e}", src.display())))?;
    for entry in entries {
        let entry = entry.map_err(|e| gen_err("SRV-GEN-IO", format!("dir entry: {e}")))?;
        let p = entry.path();
        let name = entry.file_name();
        let target = dest.join(&name);
        let ft = entry
            .file_type()
            .map_err(|e| gen_err("SRV-GEN-IO", format!("file_type {}: {e}", p.display())))?;
        if ft.is_dir() {
            copy_tree(&p, &target, depth + 1)?;
        } else if ft.is_file() {
            std::fs::copy(&p, &target)
                .map_err(|e| gen_err("SRV-GEN-IO", format!("copy {}: {e}", p.display())))?;
        }
        // Symlinks and other entries are skipped: a generation's loader
        // graph is source files; a linked tree is a build artifact, not a
        // loader byte.
    }
    Ok(())
}

/// The retention stamp's file name, inside
/// `.oxide/generations/<id>/retained-at`: the unix-epoch nanos the copy
/// was written at. The write ORDER is what `previous` and the prune walk
/// read — build ids are digests, not ordered, so the stamp is the only
/// clock the layout has. Nanos keep two retains in one emit-tick from
/// tying (the tiebreak is the id, ascending).
const RETAINED_AT: &str = "retained-at";

/// Copy one file into the retained generation tree.
///
/// The spec's step-4 letter says "hard-linked copies ... fallback to
/// copy", but a hard link shares the source's inode — and the build's
/// own writers rewrite non-hashed files IN PLACE on the next build
/// (`fs::write` truncates the existing path), which silently mutates the
/// retained snapshot behind the link. The `rollback_swaps_dist` pin
/// caught exactly that: a retained v1 whose chunk bytes had become v2
/// while its build-id file (written through a temp+rename) still said
/// v1. The retained tree is a FROZEN snapshot, so the fallback is the
/// default: a real copy, no shared inodes. Recorded as a deliberate
/// delta in `docs/deviations.md`.
fn copy_dist_file(src: &std::path::Path, dest: &std::path::Path) -> Result<(), OxideError> {
    std::fs::copy(src, dest)
        .map(|_| ())
        .map_err(|e| gen_err("SKW-RETAIN", format!("copy {}: {e}", src.display())))
}

/// The retention walk's tree copier — [`copy_tree`]'s linked twin, coding
/// its failures `SKW-RETAIN` (the build-time degrade) instead of the
/// serve-time `SRV-GEN-IO`.
fn copy_tree_linked(
    src: &std::path::Path,
    dest: &std::path::Path,
    depth: usize,
) -> Result<(), OxideError> {
    if depth > SNAPSHOT_MAX_DEPTH {
        return Err(gen_err(
            "SKW-RETAIN",
            format!(
                "retained tree deeper than {SNAPSHOT_MAX_DEPTH} at {}",
                src.display()
            ),
        ));
    }
    std::fs::create_dir_all(dest)
        .map_err(|e| gen_err("SKW-RETAIN", format!("mkdir {}: {e}", dest.display())))?;
    let entries = std::fs::read_dir(src)
        .map_err(|e| gen_err("SKW-RETAIN", format!("read_dir {}: {e}", src.display())))?;
    for entry in entries {
        let entry = entry.map_err(|e| gen_err("SKW-RETAIN", format!("dir entry: {e}")))?;
        let p = entry.path();
        let target = dest.join(entry.file_name());
        let ft = entry
            .file_type()
            .map_err(|e| gen_err("SKW-RETAIN", format!("file_type {}: {e}", p.display())))?;
        if ft.is_dir() {
            copy_tree_linked(&p, &target, depth + 1)?;
        } else if ft.is_file() {
            copy_dist_file(&p, &target)?;
        }
        // Symlinks skipped, as in `copy_tree`: the retained dist is a
        // build artifact tree; a linked tree outside `dist/` is not one.
    }
    Ok(())
}

/// The unix-epoch nanos (`u128`) as the retention clock; the layout keeps
/// them decimal in `retained-at`.
fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Retain the CURRENT build's `dist/` as
/// `.oxide/generations/<id>/dist` (§V2-F10 step 4: "Keep
/// `.oxide/generations/<id>` as ... copies ... of the last N
/// builds, written at the end of `phase_emit`" — a real copy, not the
/// spec's hard link: see [`copy_dist_file`]'s delta note), then prune
/// the copies past the `keep` bound (the newest `keep.max(1)` survive;
/// the rest lose their `dist/` tree — a serve snapshot's `src/` in the
/// same generation directory is never touched).
///
/// Re-retaining the same build id is a no-op that returns the existing
/// copy (the id is a digest of the dist bytes: same id, same content).
///
/// # Errors
///
/// [`SKW-RETAIN`] when `dist/` is missing, the generation directory
/// cannot be created, a byte cannot be copied, or a pruned
/// tree cannot be removed. The emit call-site degrades a retention
/// failure to a `SKW-RETAIN` warn — a rollback convenience must never
/// fail the build it rides.
pub fn retain_dist_generation(
    root: &std::path::Path,
    build_id: &str,
    keep: u32,
) -> Result<PathBuf, OxideError> {
    let dist = root.join("dist");
    if !dist.is_dir() {
        return Err(gen_err(
            "SKW-RETAIN",
            format!("no dist/ tree at {}", root.display()),
        ));
    }
    if build_id.is_empty() || build_id.contains('/') || build_id.contains('\\') {
        return Err(gen_err(
            "SKW-RETAIN",
            format!("`{build_id}` is not a build id (a directory name, not a path)"),
        ));
    }
    let gen_root = root.join(".oxide").join("generations").join(build_id);
    let dest = gen_root.join("dist");
    if !dest.is_dir() {
        std::fs::create_dir_all(&gen_root)
            .map_err(|e| gen_err("SKW-RETAIN", format!("mkdir {}: {e}", gen_root.display())))?;
        copy_tree_linked(&dist, &dest, 0)?;
        std::fs::write(gen_root.join(RETAINED_AT), now_nanos().to_string()).map_err(|e| {
            gen_err(
                "SKW-RETAIN",
                format!("stamp {}: {e}", gen_root.join(RETAINED_AT).display()),
            )
        })?;
    }
    prune_retained_generations(root, keep, build_id)?;
    Ok(dest)
}

/// The retained build ids, newest-first: the generations directory's
/// entries that carry BOTH a `retained-at` stamp and a `dist/` tree (a
/// serve-only snapshot directory — `src/` without a retained dist — is
/// not a retained build and is never listed or pruned here). Ties (two
/// stamps in the same nanosecond) break on the id, ascending.
///
/// A missing or corrupt stamp file sorts as the oldest possible instant
/// rather than failing the walk: the listing feeds `previous` and the
/// prune bound, and one unreadable stamp must not hide the others.
#[must_use]
pub fn retained_generations(root: &std::path::Path) -> Vec<String> {
    let gens = root.join(".oxide").join("generations");
    let Ok(entries) = std::fs::read_dir(&gens) else {
        return Vec::new();
    };
    let mut retained: Vec<(u128, String)> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter(|e| e.path().join("dist").is_dir())
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            let nanos: u128 = std::fs::read_to_string(e.path().join(RETAINED_AT))
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
            Some((nanos, id))
        })
        .collect();
    retained.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    retained.into_iter().map(|(_, id)| id).collect()
}

/// Prune the retained dist copies past the `keep` bound: the newest
/// `keep.max(1)` copies survive (the current build's own copy is the
/// newest by construction), the rest lose their `dist/` trees. The
/// generation DIRECTORIES stay — a serve snapshot's `src/` beside a
/// pruned copy belongs to a live demoted generation and is never this
/// walk's business.
///
/// # Errors
///
/// [`SKW-RETAIN`] when a victim tree cannot be removed.
fn prune_retained_generations(
    root: &std::path::Path,
    keep: u32,
    current_id: &str,
) -> Result<(), OxideError> {
    let keep = usize::try_from(keep.max(1)).unwrap_or(1);
    let gens = root.join(".oxide").join("generations");
    for victim in retained_generations(root).into_iter().skip(keep) {
        if victim == current_id {
            continue;
        }
        std::fs::remove_dir_all(gens.join(&victim).join("dist")).map_err(|e| {
            gen_err(
                "SKW-RETAIN",
                format!("prune {}: {e}", gens.join(&victim).join("dist").display()),
            )
        })?;
    }
    Ok(())
}

/// Resolve a generation selector to its retained `dist/` tree. `previous`
/// names the newest retained build whose id differs from `current_id`
/// (the spec's "`oxide start --generation previous`"); any other spelling
/// must name a retained build id exactly.
///
/// # Errors
///
/// [`SKW-GEN`] when the selector is not a generation name (empty, or a
/// path-shaped traversal attempt — the trust boundary is the generations
/// directory itself), when nothing was retained yet, or when the named
/// build id carries no retained copy.
pub fn resolve_generation(
    root: &std::path::Path,
    spec: &str,
    current_id: Option<&str>,
) -> Result<PathBuf, OxideError> {
    let path_shaped = spec.is_empty()
        || spec == "."
        || spec == ".."
        || spec.contains('/')
        || spec.contains('\\')
        || spec.contains('\0');
    if path_shaped {
        return Err(gen_err(
            "SKW-GEN",
            format!(
                "`{spec}` is not a generation selector: `previous` or a build id (no path segments)"
            ),
        ));
    }
    let gens = root.join(".oxide").join("generations");
    if spec == "previous" {
        let retained = retained_generations(root);
        let target = retained
            .into_iter()
            .find(|id| current_id != Some(id.as_str()))
            .ok_or_else(|| {
                gen_err(
                    "SKW-GEN",
                    match current_id {
                        Some(current) => format!(
                            "no previous generation is retained besides the current build {current}"
                        ),
                        None => "no generation is retained yet — build once first".to_string(),
                    },
                )
            })?;
        return Ok(gens.join(target).join("dist"));
    }
    let dest = gens.join(spec).join("dist");
    if !dest.is_dir() {
        let retained = retained_generations(root).join(", ");
        let hint = if retained.is_empty() {
            "nothing is retained yet — build once first".to_string()
        } else {
            format!("retained: {retained}")
        };
        return Err(gen_err(
            "SKW-GEN",
            format!("no retained generation `{spec}` ({hint})"),
        ));
    }
    Ok(dest)
}

/// Swap a retained generation's `dist/` tree into the live
/// `<root>/dist` (§V2-F10 step 4: "`oxide rollback` repoints `dist`
/// ... No rebuild"). The live tree is retired under the generations
/// store first, the target renamed into its place (same filesystem —
/// one atomic `rename`), the retired tree removed. A failed swap
/// restores the live tree before returning.
///
/// Re-pointing at the build `dist/` already serves is a no-op: the
/// idempotent re-deploy never disturbs the served bytes. The target must
/// live inside the generations store and carry a `build-id` stamp.
///
/// # Errors
///
/// [`SKW-GEN`] when the target is outside the generations store or
/// carries no build id; [`SKW-RETAIN`] when a rename fails (the live
/// tree is restored first).
pub fn repoint_dist_to_generation(
    root: &std::path::Path,
    gen_dist: &std::path::Path,
) -> Result<(), OxideError> {
    let gens = root.join(".oxide").join("generations");
    if !gen_dist.starts_with(&gens) {
        return Err(gen_err(
            "SKW-GEN",
            format!(
                "{} is not a retained generation tree (the swap only moves trees out of {})",
                gen_dist.display(),
                gens.display()
            ),
        ));
    }
    let Some(target_id) = oxide_fw_cache::read_build_id(gen_dist) else {
        return Err(gen_err(
            "SKW-GEN",
            format!("{} carries no build-id stamp", gen_dist.display()),
        ));
    };
    let dist = root.join("dist");
    if oxide_fw_cache::read_build_id(&dist).as_deref() == Some(target_id.as_str()) {
        return Ok(());
    }
    let retire = gens.join(format!(".retiring-{}-{}", std::process::id(), now_nanos()));
    let had_live = dist.is_dir();
    if had_live {
        std::fs::rename(&dist, &retire)
            .map_err(|e| gen_err("SKW-RETAIN", format!("retire {}: {e}", dist.display())))?;
    }
    match std::fs::rename(gen_dist, &dist) {
        Ok(()) => {
            if had_live {
                let _ = std::fs::remove_dir_all(&retire);
            }
            Ok(())
        }
        Err(e) => {
            if had_live {
                let _ = std::fs::rename(&retire, &dist);
            }
            Err(gen_err(
                "SKW-RETAIN",
                format!(
                    "swap {} -> {}: {e} (the live dist was restored)",
                    gen_dist.display(),
                    dist.display()
                ),
            ))
        }
    }
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

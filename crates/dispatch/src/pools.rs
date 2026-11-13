//! The two-generation serving pools (spec v0.2, §V2-F10 design steps 1-3:
//! "the old build's data plane keeps serving for the skew window while the
//! new one takes traffic").
//!
//! The §V2-A04/F10 id substrates ([`crate::build::GenerationSet`], the
//! bounded [`crate::skew::GenerationPool`]) answer "is this build id still
//! accepted"; they hold no state per generation. The serving pools are the
//! step beyond: a request whose id is accepted must be dispatched to the
//! POOL that was booted for that build — `GenerationPools` stores a
//! serving handle per generation and routes the request's build id to it.
//!
//! # The seam
//!
//! The handle type is the caller's: `P` is generic, so the CLI instantiates
//! the structure over its `Arc<dyn Pool>` (the §V2-F10 sketch's shape —
//! "`GenerationPools { current: Arc<dyn Pool>, previous: ... }`") while
//! this V8-free crate stays pool-agnostic. [`GenerationPools`] implements
//! [`AcceptedGenerations`] (via [`Self::route`]), so the existing
//! [`crate::skew::stage_check`] drives it with the same verdicts as the
//! 1-deep set and the id-only pool (pinned by
//! `tests/pools_tdd.rs::the_stage_drives_set_and_pools_with_identical_
//! verdicts`): the 409 authority stays the one stage, and the routing
//! layer only decides WHO serves an accepted id.
//!
//! # Lifecycle
//!
//! - [`promote`](Self::promote) is the deploy step: the current id and its
//!   LIVE handle are demoted together (each demotion carries its own
//!   demotion instant — the window runs from the demotion, exactly like
//!   the id-only pool), and the caller hands in the handle booted for the
//!   new build. Expired demotions are trimmed first; at capacity the
//!   OLDEST demoted pool drops (the A04 "bounded by config, not deploy
//!   frequency" rule generalized from ids to handles — the spec bounds
//!   memory at two generations; the constructor's `capacity` is the knob).
//! - [`expire`](Self::expire) is the 30 s housekeeping pass (§V2-F10 step
//!   3): expired HANDLES drop, nothing is unlinked — the client chunks are
//!   content-hashed and stay valid.
//! - [`evict_oldest`](Self::evict_oldest) is the brownout early-drop (the
//!   F12 hook): when the pool queue wait crosses the threshold, the oldest
//!   demoted pool comes back to the caller so it can drain and shut it
//!   down gracefully instead of leaking the drop.
//! - Re-promoting a demoted id is the rollback path (§V2-F10 step 4): the
//!   caller boots a fresh pool over the old dist root and hands it in; the
//!   stale demoted entry drops. Promoting the CURRENT id is a no-op that
//!   keeps the live handle — an idempotent re-deploy never churns the
//!   pools.
//!
//! The wiring — booting the pools from `dist/` + F11's `releases/<build-id>/`
//! layout, promoting on `cmd_dev` reload, sizing `window`/`capacity` from
//! the deferred `deploy.skewWindow` config key — rides the A01-phase-3 CLI
//! adoption, exactly like `GenerationSet`'s (see `docs/deviations.md`).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::build::Gen;
use crate::skew::AcceptedGenerations;

/// One demoted SERVING generation: the id, the handle that serves it and
/// the instant its window started running (the demotion instant).
#[derive(Debug)]
struct DemotedPool<P> {
    id: String,
    pool: P,
    since: Instant,
}

/// The serving pools (§V2-F10 design step 1): the current build's pool
/// plus up to `capacity` demoted pools, each accepted for `window` from
/// its own demotion instant and each holding the handle booted for it.
#[derive(Debug)]
pub struct GenerationPools<P> {
    current: String,
    pool: P,
    /// Demoted generations, oldest first.
    demoted: VecDeque<DemotedPool<P>>,
    window: Duration,
    capacity: usize,
}

impl<P> GenerationPools<P> {
    /// Fresh pools: `current` is served by `pool`, nothing is demoted yet.
    /// `capacity` bounds the demoted set (1 is the §V2-F10 sketch's
    /// two-generation shape; 0 keeps only the current build — the
    /// pre-pool behavior).
    #[must_use]
    pub fn new(current: String, pool: P, window: Duration, capacity: usize) -> Self {
        Self {
            current,
            pool,
            demoted: VecDeque::new(),
            window,
            capacity,
        }
    }

    /// The id every data-plane response advertises (`x-oxide-build`).
    #[must_use]
    pub fn current(&self) -> &str {
        &self.current
    }

    /// The handle serving the current build — the pool every HTML
    /// navigation and header-less data-plane request takes.
    #[must_use]
    pub fn current_pool(&self) -> &P {
        &self.pool
    }

    /// How many demoted generations the structure holds (live or expired —
    /// expiry is reclaimed by [`expire`](Self::expire) or
    /// [`promote`](Self::promote)).
    #[must_use]
    pub fn len(&self) -> usize {
        self.demoted.len()
    }

    /// `true` when no generation is demoted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.demoted.is_empty()
    }

    /// Demote the current build and its live handle (the deploy-time step)
    /// and serve `new_current` from `pool` from now on.
    ///
    /// Expired demotions are trimmed first (a deploy doubles as the
    /// housekeeping pass); re-promoting a demoted id — the rollback path —
    /// installs the given handle as current and drops the stale demoted
    /// entry; promoting the current id is a no-op that KEEPS the live
    /// handle (the caller's handle is ignored). When the structure is full
    /// the OLDEST demoted pool drops.
    pub fn promote(&mut self, new_current: String, pool: P, now: Instant) {
        if new_current == self.current {
            return;
        }
        self.expire(now);
        let old = std::mem::replace(&mut self.current, new_current);
        let old_pool = std::mem::replace(&mut self.pool, pool);
        self.demoted.push_back(DemotedPool {
            id: old,
            pool: old_pool,
            since: now,
        });
        // The id coming back to current must not linger demoted (the
        // rollback path: the fresh handle serves, the stale entry — and
        // its stale handle — drop).
        let current = self.current.clone();
        self.demoted.retain(|d| d.id != current);
        while self.demoted.len() > self.capacity {
            self.demoted.pop_front();
        }
    }

    /// The §V2-F10 step-2 routing decision: which pool serves a request
    /// advertising `id`?
    ///
    /// - Absent id — HTML navigations ("everything else, including all
    ///   HTML, goes to current") and header-less data-plane requests
    ///   (curl, tests, old clients): the CURRENT pool, [`Gen::Current`].
    /// - `id` is the current build: the current pool.
    /// - `id` is a demoted generation inside ITS window: THAT generation's
    ///   own handle, [`Gen::Previous`] — the old build's data plane keeps
    ///   serving old tabs (`previous_pool_serves_old_build_header`).
    /// - `id` is unknown or expired: `None` — the caller answers the
    ///   §V2-A04 `409` through [`crate::skew::stage_check`], which drives
    ///   this same structure through [`AcceptedGenerations`].
    #[must_use]
    pub fn route(&self, id: Option<&str>, now: Instant) -> Option<(&P, Gen)> {
        let Some(id) = id else {
            return Some((&self.pool, Gen::Current));
        };
        if id == self.current {
            return Some((&self.pool, Gen::Current));
        }
        self.demoted.iter().find_map(|d| {
            (d.id == id && now.duration_since(d.since) <= self.window)
                .then_some((&d.pool, Gen::Previous))
        })
    }

    /// Drop the expired demoted pools; returns how many went. The
    /// §V2-F10 step-3 housekeeping pass (every 30 s): "at window end it
    /// drops the previous pool and unlinks nothing" — the handles drop,
    /// the content-hashed chunks stay valid. `accepts` already refuses
    /// expired ids on read; this is the reclamation.
    pub fn expire(&mut self, now: Instant) -> usize {
        let before = self.demoted.len();
        self.demoted
            .retain(|d| now.duration_since(d.since) <= self.window);
        before - self.demoted.len()
    }

    /// The brownout early-drop (the F12 hook): pop the OLDEST demoted
    /// generation and hand its handle back, so the caller can drain and
    /// shut the pool down gracefully when the queue wait crosses the
    /// threshold instead of leaking the drop. `None` when nothing is
    /// demoted.
    pub fn evict_oldest(&mut self) -> Option<(String, P)> {
        self.demoted.pop_front().map(|d| (d.id, d.pool))
    }
}

impl<P> AcceptedGenerations for GenerationPools<P> {
    fn accepts(&self, id: &str, now: Instant) -> Option<Gen> {
        self.route(Some(id), now).map(|(_, which)| which)
    }
}

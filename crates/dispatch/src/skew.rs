//! The skew-protection pool substrate (spec v0.2, §V2-F10 step 1-2: "tabs
//! still on the old build keep working for a window" — routing by
//! `x-oxide-build` over a bounded set of accepted generations).
//!
//! The §V2-A04 substrate (`crate::build::GenerationSet`) answers the
//! data-plane build check with ONE demoted generation: promoting twice
//! drops generation N-1 immediately. F10's pool generalizes that to a
//! bounded set — `deploy.skewWindow` keeps the WINDOW the same, and the
//! pool grows the DEPTH, so every deploy inside the window keeps its
//! clients working instead of only the last one.
//!
//! # The seam
//!
//! [`AcceptedGenerations`] is the contract the data-plane stage consumes:
//! "which build ids does this process still answer for, and as what".
//! [`crate::build::GenerationSet`] implements it (delegating to its
//! inherent `accepts`), and the bounded [`GenerationPool`] implements the
//! same contract — so [`stage_check`] drives both with identical verdicts
//! (pinned by `tests/skew_tdd.rs::the_stage_drives_set_and_pool_with_
//! identical_verdicts`). The CLI's dispatch closure picks the source; the
//! stage and the wire contract never change.
//!
//! # Pool semantics
//!
//! - [`GenerationPool::promote`] demotes the current build with its own
//!   demotion instant (the window runs from the DEMOTION, not from a
//!   shared clock — two deploys ten minutes apart expire ten minutes
//!   apart). Expired demotions are trimmed on every promote, so a deploy
//!   doubles as the housekeeping pass.
//! - The pool is bounded by `capacity`: when it is full, the OLDEST
//!   demotion evicts first — the "how old may a replayable response be"
//!   question stays bounded by config, not by deploy frequency (the A04
//!   rule, generalized from depth 1 to depth N).
//! - Re-promoting a demoted id (the rollback path) moves it back to
//!   current; promoting the current id is a no-op, so an idempotent
//!   re-deploy never churns the pool.
//! - `accepts` is a pure lookup (`&self`, like `GenerationSet`'s) —
//!   expiry is refused on read and reclaimed by [`GenerationPool::expire`]
//!   or the next promote.
//!
//! The wiring — promoting the pool when `cmd_dev` reloads a build and
//! sizing it from the (deferred) `deploy.skewWindow` config key — rides
//! the A01-phase-3 CLI adoption, exactly like `GenerationSet`'s (see
//! `docs/deviations.md`).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use bytes::Bytes;
use oxide_fw_server::OxideResponse;

use crate::build::{BUILD_HEADER, Gen, STALE_HEADER, STALE_STATUS, is_data_plane_path};
use crate::flow::Flow;

/// Which generations a process still answer for (§V2-A04's
/// `accepts`, generalized so the 1-deep `GenerationSet` and the F10
/// bounded pool drive the same data-plane stage).
///
/// Blanket-implemented for references, so the boot-time-chosen source can
/// be held as `&dyn AcceptedGenerations` and still drive [`stage_check`].
pub trait AcceptedGenerations {
    /// `Some(_)` when the id is accepted (with the generation it resolved
    /// to); `None` means unknown or expired — the caller answers 409.
    fn accepts(&self, id: &str, now: Instant) -> Option<Gen>;
}

impl<T: AcceptedGenerations + ?Sized> AcceptedGenerations for &T {
    fn accepts(&self, id: &str, now: Instant) -> Option<Gen> {
        (*self).accepts(id, now)
    }
}

impl AcceptedGenerations for crate::build::GenerationSet {
    fn accepts(&self, id: &str, now: Instant) -> Option<Gen> {
        crate::build::GenerationSet::accepts(self, id, now)
    }
}

/// One demoted generation: the id plus the instant its window started
/// running (the demotion instant).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Demoted {
    id: String,
    since: Instant,
}

/// The bounded pool of demoted generations (F10's growth of the A04
/// `GenerationSet`): the current build plus up to `capacity` demoted
/// builds, each accepted for `window` from its own demotion instant.
#[derive(Debug, Clone)]
pub struct GenerationPool {
    current: String,
    /// Demoted generations, oldest first.
    demoted: VecDeque<Demoted>,
    window: Duration,
    capacity: usize,
}

impl GenerationPool {
    /// A fresh pool: `current` is accepted, nothing is demoted yet.
    /// `capacity` bounds the demoted set (0 keeps only the current
    /// build — the pre-pool behavior).
    #[must_use]
    pub fn new(current: String, window: Duration, capacity: usize) -> Self {
        Self {
            current,
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

    /// How many demoted generations the pool holds (live or expired —
    /// expiry is reclaimed by [`expire`](Self::expire) or [`promote`](Self::promote)).
    #[must_use]
    pub fn len(&self) -> usize {
        self.demoted.len()
    }

    /// `true` when no generation is demoted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.demoted.is_empty()
    }

    /// Demote the current build (the deploy-time step) and accept
    /// `new_current` from now on.
    ///
    /// Expired demotions are trimmed first (a deploy doubles as the
    /// housekeeping pass); re-promoting a demoted id — the rollback path
    /// — moves it back to current instead of duplicating it; promoting
    /// the current id is a no-op. When the pool is full the OLDEST
    /// demotion evicts first.
    pub fn promote(&mut self, new_current: String, now: Instant) {
        if new_current == self.current {
            return;
        }
        self.expire(now);
        let old = std::mem::replace(&mut self.current, new_current);
        self.demoted.push_back(Demoted {
            id: old,
            since: now,
        });
        // The id coming back to current must not linger demoted (the
        // rollback path: A demoted → re-promoted → demoted entry gone).
        let current = self.current.clone();
        self.demoted.retain(|d| d.id != current);
        while self.demoted.len() > self.capacity {
            self.demoted.pop_front();
        }
    }

    /// Drop the expired demotions; returns how many went. A pure
    /// housekeeping pass — `accepts` already refuses them on read.
    pub fn expire(&mut self, now: Instant) -> usize {
        let before = self.demoted.len();
        self.demoted
            .retain(|d| now.duration_since(d.since) <= self.window);
        before - self.demoted.len()
    }
}

impl AcceptedGenerations for GenerationPool {
    fn accepts(&self, id: &str, now: Instant) -> Option<Gen> {
        if id == self.current {
            return Some(Gen::Current);
        }
        self.demoted.iter().find_map(|d| {
            (d.id == id && now.duration_since(d.since) <= self.window).then_some(Gen::Previous)
        })
    }
}

/// The §V2-A04 stage, generic over the generation source: check
/// `x-oxide-build` on the data plane (`/__data`, `/_oxide/fn/*`,
/// `/__oxide/live`, `/__oxide/stream/` — [`is_data_plane_path`]).
///
/// - Non-data-plane path: [`Flow::Continue`] (HTML navigations are
///   re-rendered, not replayed — there is nothing to be stale).
/// - Absent header: [`Flow::Continue`] — EXCEPT on `/__oxide/live`, where
///   `EventSource` cannot set headers (§V2-F05 step 2) and the build id
///   travels as the `build` query parameter instead; there the query value
///   is consulted (absent there too: allowed, curl/tests/old clients).
/// - Accepted id (current or any demoted-within-window): [`Flow::Continue`].
/// - Unaccepted id: [`Flow::Respond`] with [`STALE_STATUS`], the
///   `x-oxide-build-stale: true` marker and an empty body.
///
/// [`crate::build::stage_build_check`] is this function specialized to
/// the 1-deep `GenerationSet` — the §V2-A04 signature the existing call
/// sites and pins hold.
pub fn stage_check<T: AcceptedGenerations + ?Sized>(
    path: &str,
    headers: &[(String, String)],
    gens: &T,
    now: Instant,
) -> Flow {
    let (path_only, query) = match path.split_once(['?', '#']) {
        Some((p, q)) => (p, Some(q)),
        None => (path, None),
    };
    if !is_data_plane_path(path_only) {
        return Flow::Continue;
    }
    let id = header_value(headers, BUILD_HEADER).or_else(|| {
        // The query fallback is the live route's contract only: /__data
        // has no `build` query parameter, and a stale id there must
        // travel in the header.
        if path_only == "/__oxide/live" {
            query.and_then(query_build)
        } else {
            None
        }
    });
    let Some(id) = id else {
        return Flow::Continue;
    };
    if gens.accepts(id, now).is_some() {
        return Flow::Continue;
    }
    let mut resp = OxideResponse::new(STALE_STATUS, Bytes::new());
    resp.headers
        .push((STALE_HEADER.to_string(), "true".to_string()));
    Flow::Respond(resp)
}

/// Case-insensitive header lookup over the request's `(name, value)` list
/// (B-06: names compare case-insensitively, values are byte-exact).
///
/// `pub(crate)`: the stage lives here, [`crate::build`] delegates to it.
pub(crate) fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// The `build` query parameter (the live route's header-less spelling).
/// Build ids are 16-hex (§V2-A04), so no percent-decoding is needed; the
/// value is taken byte-exact.
pub(crate) fn query_build(query: &str) -> Option<&str> {
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix("build="))
        .filter(|v| !v.is_empty())
}

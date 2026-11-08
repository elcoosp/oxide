//! The build id + generations substrate (spec v0.2, V2-A04 step 3).
//!
//! One canonical build id, visible to the browser and checked on every
//! data-plane request. The id itself is computed by
//! `oxide_fw_cache::build_id_of_dist` (V8-free, §V2-A04 step 1); this
//! module owns the server side of the contract:
//!
//! - [`GenerationSet`]: the builds this process will answer data-plane
//!   requests for — the current generation plus the one demoted build,
//!   still accepted inside a grace window so a deploy does not cut off
//!   clients mid-flight (the F10 skew-protection pool grows from here).
//! - [`stage_build_check`]: the stage the dispatch closure installs before
//!   the L0 lookup on the data plane (`/__data`, `/_oxide/fn/*`). A request
//!   advertising an unaccepted `x-oxide-build` answers `409` +
//!   `x-oxide-build-stale: true` with an empty body; a request without the
//!   header (curl, tests, old clients) is allowed through.
//! - [`advertise_build`]: every data-plane response carries
//!   `x-oxide-build: <current>` so the client can echo it back.
//!
//! The wiring — installing the stage in the V8-linked dispatch closure,
//! promoting the generation when `cmd_dev` reloads a build, and the client
//! side (`packages/core` `withBuild` / `handleStale`) consuming it — is
//! deferred to the A01-phase-3 CLI adoption; see `docs/deviations.md`.

use std::time::{Duration, Instant};

use bytes::Bytes;
use oxide_fw_server::OxideResponse;

use crate::flow::Flow;

/// The request/response header carrying the build id (§V2-A04).
pub const BUILD_HEADER: &str = "x-oxide-build";

/// The response header marking a stale-generation rejection (§V2-A04).
pub const STALE_HEADER: &str = "x-oxide-build-stale";

/// The status the data plane answers an unaccepted build id with.
pub const STALE_STATUS: u16 = 409;

/// Which generation a request's build id resolved to. The data plane
/// treats both identically today (§V2-A04); F10 skew protection uses the
/// distinction to route replays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gen {
    /// The id is the process's current build.
    Current,
    /// The id is the demoted build, still inside its acceptance window.
    Previous,
}

/// The builds this process will answer data-plane requests for (F10 adds
/// the pool side).
///
/// One current id plus, after [`GenerationSet::promote`], the demoted
/// build accepted for the remainder of its window (`Duration`). The window
/// is 1-deep: promoting twice drops generation N-1 immediately, which
/// keeps the "how old may a replayable response be" question bounded by
/// config instead of by deploy frequency.
#[derive(Debug, Clone)]
pub struct GenerationSet {
    current: String,
    previous: Option<(String, Instant)>,
    window: Duration,
}

impl GenerationSet {
    /// A fresh set: `current` is accepted, nothing is demoted yet.
    #[must_use]
    pub fn new(current: String, window: Duration) -> Self {
        Self {
            current,
            previous: None,
            window,
        }
    }

    /// The id every data-plane response advertises (`x-oxide-build`).
    #[must_use]
    pub fn current(&self) -> &str {
        &self.current
    }

    /// Demote the current build to previous (called when a new generation
    /// is loaded); `now` is the demotion instant the window runs from.
    pub fn promote(&mut self, new_current: String, now: Instant) {
        let old = std::mem::replace(&mut self.current, new_current);
        self.previous = Some((old, now));
    }

    /// `Some(_)` when the id is accepted (with the generation it resolved
    /// to); `None` means unknown or expired — the caller answers 409.
    #[must_use]
    pub fn accepts(&self, id: &str, now: Instant) -> Option<Gen> {
        if id == self.current {
            return Some(Gen::Current);
        }
        match &self.previous {
            Some((p, since)) if p == id && now.duration_since(*since) <= self.window => {
                Some(Gen::Previous)
            }
            _ => None,
        }
    }
}

/// The data plane today (§V2-A04): `/__data` and `/_oxide/fn/*`.
/// `/__oxide/live` and `/__oxide/stream/*` join the set when those routes
/// land (F05 / F07) — until then they must not enforce the header, because
/// no client would send it.
///
/// The wiring passes the request path; a query or fragment is tolerated
/// (stripped) so both the raw target and `uri.path()` spellings work.
#[must_use]
pub fn is_data_plane_path(path: &str) -> bool {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    path == "/__data" || path.starts_with("/_oxide/fn/")
}

/// Case-insensitive header lookup over the request's `(name, value)` list
/// (B-06: names compare case-insensitively, values are byte-exact).
fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// The §V2-A04 stage: check `x-oxide-build` on the data plane.
///
/// - Non-data-plane path: always [`Flow::Continue`] (HTML navigations are
///   re-rendered, not replayed — there is nothing to be stale).
/// - Absent header: [`Flow::Continue`] (curl, tests, old clients).
/// - Accepted id (current or previous-within-window): [`Flow::Continue`].
/// - Unaccepted id: [`Flow::Respond`] with [`STALE_STATUS`], the
///   `x-oxide-build-stale: true` marker and an empty body.
#[must_use]
pub fn stage_build_check(
    path: &str,
    headers: &[(String, String)],
    gens: &GenerationSet,
    now: Instant,
) -> Flow {
    if !is_data_plane_path(path) {
        return Flow::Continue;
    }
    let Some(id) = header_value(headers, BUILD_HEADER) else {
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

/// Append `x-oxide-build: <current>` to a data-plane response.
///
/// Idempotent: a response that already advertises a build id (the wiring
/// may run the advertise step on several layers of the pipeline; a proxy
/// hop may have set its own) is left untouched — first writer wins.
pub fn advertise_build(resp: &mut OxideResponse, current: &str) {
    if !resp
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case(BUILD_HEADER))
    {
        resp.headers
            .push((BUILD_HEADER.to_string(), current.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_set_accepts_its_current_id() {
        let g = GenerationSet::new("abc".into(), Duration::from_secs(60));
        assert_eq!(g.current(), "abc");
        assert_eq!(g.accepts("abc", Instant::now()), Some(Gen::Current));
    }

    #[test]
    fn header_value_matches_names_case_insensitively() {
        let hs = vec![("X-Oxide-Build".to_string(), "v".to_string())];
        assert_eq!(header_value(&hs, "x-oxide-build"), Some("v"));
        assert_eq!(header_value(&hs, "x-other"), None);
    }
}

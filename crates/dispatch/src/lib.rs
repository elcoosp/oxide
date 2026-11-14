//! V8-free request orchestration (spec v0.2, WP V2-A02).
//!
//! `build_dispatch` in `oxide-fw-cli` is a 1,400-line closure whose tests pay
//! the V8 link cost on every iteration (CONVENTIONS §8: ~5 minutes per edit).
//! This crate is the escape hatch the spec orders: the pure, request-shaped
//! logic moves here so its tests compile in seconds against fakes, while the
//! V8-linked CLI keeps only the wiring.
//!
//! Initial surface (V2-A02 + V2-A05 + V2-A06):
//! - [`url`]: the URL/query helpers the dispatch prelude needs —
//!   `path_of`, `normalize_path`, `data_route_of`, plus the §9.3 form and
//!   form-action pair `parse_form_urlencoded` / `action_from_query`.
//! - [`flow`]: the stage protocol (`Flow`) that the phase-3 stage functions
//!   (`stage_prelude` … `stage_finish`) will return.
//! - [`chain`]: the §8.5 middleware chain runner (`run_chain`) — outer to
//!   inner, later-wins locals, last-writer-wins headers, short-circuit /
//!   redirect / rewrite directives with the B-22 resume marker (V2-A05;
//!   CLI adoption deferred, see `docs/deviations.md`).
//! - [`locals`]: the unified request locals ([`RequestLocals`]) with the
//!   pipeline-owned reserved keys `session` / `locale` / `experiments`
//!   and the DSP-101 collision report (V2-A05; CLI adoption deferred).
//! - [`loads`]: the §9.2 parallel load graph (`run_load_graph`) —
//!   independent loads concurrent, `parent()`-needing loads chained, the
//!   substrate `stage_loaders` wires (V2-A06; CLI adoption deferred, see
//!   `docs/deviations.md`).
//! - [`experiments`]: the §15.4 experiments assignment engine (`assign`,
//!   `resolve`) — deterministic blake3 buckets, sticky cookie/header/ip
//!   identities, minted `Set-Cookie`s and the L0 vary suffix the
//!   `stage_experiments` wiring feeds into [`locals`] (V2-F04; CLI
//!   adoption deferred, see `docs/deviations.md`).
//! - [`build`]: the build id + generations substrate ([`GenerationSet`],
//!   the `stage_build_check` data-plane 409 gate and the
//!   `x-oxide-build` advertise step) — the keystone the F10/F11/F13/F14
//!   skew/deploy features grow from (V2-A04; CLI adoption deferred, see
//!   `docs/deviations.md`).
//! - [`skew`]: the skew-protection pool (V2-F10) — the
//!   `AcceptedGenerations` seam the data-plane stage consumes (both the
//!   1-deep set and the bounded id pool drive `stage_check` with
//!   identical verdicts).
//! - [`pools`]: the two-generation serving pools (V2-F10 design steps 1-3)
//!   — [`GenerationPools`] stores a serving handle per generation and
//!   routes a request's build id to the pool booted for it (HTML and
//!   header-less requests always route current), the per-demotion window
//!   expiry drops the old handles, and the brownout early-drop returns
//!   the evicted handle for graceful shutdown; it implements the
//!   [`skew::AcceptedGenerations`] seam, so `stage_check` drives it with
//!   the same verdicts (CLI adoption deferred, see `docs/deviations.md`).
//! - [`record`]: the record-and-replay capture substrate (V2-F15
//!   recorder half) — the versioned [`record::RequestRecord`] schema (the
//!   redacted request, route id + params, locals, loader inputs/outputs,
//!   capped base64 fetch ops, cache ops, the non-deterministic
//!   sequences), the [`record::RecordPolicy`] sampling decision, the
//!   [`record::RecordSink`] storage seam over
//!   `.oxide/records/<request-id>.json` and the
//!   [`record::RecordingLoaderRunner`] decorator the dispatch closure
//!   wraps its runner in (CLI adoption deferred, see `docs/deviations.md`).
//! - [`replay`]: the record-and-replay playback substrate (V2-F15 replay
//!   half) — the ordered [`replay::ReplayMatcher`] (`RPL-002` on an
//!   unexpected call), the [`replay::ReplaySequences`] playback
//!   (`RPL-004` on a drained stream), the `RPL-003` stale-build warning
//!   and the [`replay::replay_diff`] verdict the `oxide replay --diff`
//!   JSON surface emits (CLI adoption deferred, see `docs/deviations.md`).
//!
//! Acceptance (spec V2-A02): `cargo test -p oxide-fw-dispatch` finishes in
//! under 20 seconds after dependencies are built, and
//! `cargo tree -p oxide-fw-dispatch | grep -c jshost` prints 0.

pub mod build;
pub mod chain;
pub mod experiments;
pub mod flow;
pub mod loads;
pub mod locals;
pub mod pools;
pub mod record;
pub mod replay;
pub mod skew;
pub mod url;

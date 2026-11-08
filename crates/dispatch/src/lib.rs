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
pub mod url;

//! V8-free request orchestration (spec v0.2, WP V2-A02).
//!
//! `build_dispatch` in `oxide-fw-cli` is a 1,400-line closure whose tests pay
//! the V8 link cost on every iteration (CONVENTIONS §8: ~5 minutes per edit).
//! This crate is the escape hatch the spec orders: the pure, request-shaped
//! logic moves here so its tests compile in seconds against fakes, while the
//! V8-linked CLI keeps only the wiring.
//!
//! Initial surface (V2-A02 + V2-A06):
//! - [`url`]: the URL/query helpers the dispatch prelude needs —
//!   `path_of`, `normalize_path`, `data_route_of`, plus the §9.3 form and
//!   form-action pair `parse_form_urlencoded` / `action_from_query`.
//! - [`flow`]: the stage protocol (`Flow`) that the phase-3 stage functions
//!   (`stage_prelude` … `stage_finish`) will return.
//! - [`loads`]: the §9.2 parallel load graph (`run_load_graph`) —
//!   independent loads concurrent, `parent()`-needing loads chained, the
//!   substrate `stage_loaders` wires (V2-A06; CLI adoption deferred, see
//!   `docs/deviations.md`).
//!
//! `chain.rs` (middleware chain) remains deliberately absent: the spec
//! assigns it to V2-A05.
//!
//! Acceptance (spec V2-A02): `cargo test -p oxide-fw-dispatch` finishes in
//! under 20 seconds after dependencies are built, and
//! `cargo tree -p oxide-fw-dispatch | grep -c jshost` prints 0.

pub mod flow;
pub mod loads;
pub mod url;

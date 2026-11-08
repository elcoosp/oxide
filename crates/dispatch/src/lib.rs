//! V8-free request orchestration (spec v0.2, WP V2-A02).
//!
//! `build_dispatch` in `oxide-fw-cli` is a 1,400-line closure whose tests pay
//! the V8 link cost on every iteration (CONVENTIONS §8: ~5 minutes per edit).
//! This crate is the escape hatch the spec orders: the pure, request-shaped
//! logic moves here so its tests compile in seconds against fakes, while the
//! V8-linked CLI keeps only the wiring.
//!
//! Initial surface (V2-A02):
//! - [`url`]: the URL/query helpers the dispatch prelude needs —
//!   `path_of`, `normalize_path`, `data_route_of`, plus the §9.3 form and
//!   form-action pair `parse_form_urlencoded` / `action_from_query`.
//! - [`flow`]: the stage protocol (`Flow`) that the phase-3 stage functions
//!   (`stage_prelude` … `stage_finish`) will return.
//!
//! `chain.rs` (middleware chain) and `loads.rs` (parallel load graph) are
//! deliberately absent: the spec assigns them to V2-A05 and V2-A06.
//!
//! Acceptance (spec V2-A02): `cargo test -p oxide-fw-dispatch` finishes in
//! under 20 seconds after dependencies are built, and
//! `cargo tree -p oxide-fw-dispatch | grep -c jshost` prints 0.

pub mod flow;
pub mod url;

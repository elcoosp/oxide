//! The §V2-B08 phase-2 streaming nonce splicer: replace `from` with `to`
//! across arbitrarily chunked bytes without ever holding a second copy of the
//! document.
//!
//! # The seam
//!
//! The CLI's nonce handling still does `html.replace(&nonce, SENTINEL)` —
//! one whole-document copy per prerendered page and again per L0 fill
//! (spec §V2-B08's evidence rows). [`NonceSplicer`] is the chunk-safe
//! replacement the live path and the L0 fill adopt: `push` feeds one
//! chunk, splices every complete `from` occurrence, and holds back the
//! last `from.len() - 1` bytes so a needle split across two chunks is
//! still found; `finish` flushes the carry. The fill side runs
//! `NonceSplicer::new(&nonce, NONCE_SENTINEL)`, the cache-hit side the
//! inverse — the CLI adoption (deleting the `.replace` calls, the dhat
//! acceptance) stays the A01-phase-3 slice (see `docs/deviations.md`).
//!
//! The struct owns its needle (`Finder<'static>` + `from` bytes), so a
//! splicer is `Send` and can ride the tee task.
//!
//! The impl lives in [`oxide_fw_edge_primitives::splice`] (WP E09: the
//! wasm32 edge build reuses it without linking `lol_html`); this module
//! re-exports it byte-identical — the render bench, the `nonce_splice`
//! example, and the CLI's `splice_nonce` wrapper all reach the same type.

pub use oxide_fw_edge_primitives::splice::NonceSplicer;

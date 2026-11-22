//! `oxide-fw-edge-primitives` — the V8-free pure primitives the wasm32 edge
//! build reuses (spec §2 D18, §18 cloudflare target, WP V2-E09).
//!
//! The edge Worker must run the core features where Rust cannot run V8:
//! SSE framing, the heap-free cache key, and the chunk-safe nonce splice.
//! Each of these lived inside a heavier crate (`oxide-fw-server` pulls
//! `tokio`/`hyper`, `oxide-fw-cache` pulls `redb`/`tokio`, `oxide-fw-render`
//! pulls `lol_html`) whose dependency graph cannot link for `wasm32`. This
//! crate is the lean home for the pure pieces: the server/cache/render crates
//! re-export their respective item from here so native behavior is
//! byte-identical, and `oxide-fw-core-wasm` depends on this crate alone to
//! reach them without pulling the V8 graph into the wasm build.
//!
//! The dependency graph is intentionally tiny: `blake3` (pure Rust, wasm32)
//! for [`key::KeyHash`], `memchr` (pure Rust, wasm32) for
//! [`splice::NonceSplicer`], and the `oxide-fw-diagnostics` `OxideError` type
//! the key path threads. `sse_frame` is plain `std`.

/// The SSE event frame (§V2-F05 step 3). Moved verbatim from
/// `oxide-fw-server::live::sse_frame` so the wasm build can frame an event
/// without linking the server's `tokio` graph; the server re-exports this
/// item and its behavior is byte-identical.
pub mod sse;

/// The 128-bit heap-free cache key (D24): [`key::KeyHash`] (16 stack bytes)
/// and [`key::KeyStr`] (32 hex chars). Moved verbatim from
/// `oxide-fw-cache::key`/`keyhash` so the wasm build can derive the L0 key
/// without linking `redb`/`tokio`; the cache crate re-exports both and
/// `key_of` stays there (cache-specific naming).
pub mod key;

/// The chunk-safe streaming nonce splicer (§V2-B08 phase 2). Moved verbatim
/// from `oxide-fw-render::splice::NonceSplicer` so the wasm build can splice
/// the CSP nonce without linking `lol_html`; the render crate re-exports it
/// and the CLI's `splice_nonce` wrapper is byte-identical.
pub mod splice;

pub use key::{KeyHash, KeyStr};
pub use splice::NonceSplicer;
pub use sse::sse_frame;

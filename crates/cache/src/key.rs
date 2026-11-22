//! V2-B05 (D24): the 128-bit [`KeyHash`] — the cache key's in-memory
//! identity, closing perf #6 (`CacheKey::canonical` allocated a hex
//! `String` per lookup).
//!
//! The digest is blake3 over `layer 0x00 route 0x00 params 0x00 vary 0x00
//! build_id`, truncated to the first 16 digest bytes — the same byte
//! sequence [`crate::keyhash::key_of`] hexes, so the raw and hex forms are
//! two views of one value and cannot drift. Per D24, hex exists only at
//! the display/filename boundaries ([`KeyHash::hex`], `Display`); every
//! in-memory comparison and map key is the 16 stack bytes — no heap
//! allocation on the key path.
//!
//! The impl lives in [`oxide_fw_edge_primitives::key`] so the wasm32 edge
//! build can derive the same key without linking `redb`/`tokio` (WP E09).
//! This module re-exports it byte-identical; the cache-specific
//! `CacheKey::canonical` still returns [`KeyHash`].

pub use oxide_fw_edge_primitives::key::KeyHash;

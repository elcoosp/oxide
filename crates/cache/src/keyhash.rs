//! V2-B05: heap-free cache keys — a 128-bit blake3 prefix on the stack.
//!
//! The `CacheHandler` trait is string-keyed, so the string stays the trait
//! boundary; [`KeyStr`] builds it in 32 bytes of stack (no `String`, no heap)
//! and derefs to `&str`. [`key_of`] digests the same NUL-separated component
//! sequence as [`CacheKey::canonical`](crate::CacheKey::canonical) — since the
//! D24 `KeyHash` landed, both are views of ONE digest: [`key_of`] is
//! `CacheKey::canonical().hex()`, the first 32 hex chars of the canonical digest.
//!
//! Hex only at this display/filename boundary (D24): all in-memory comparisons
//! and map keys are the 16 raw digest bytes ([`crate::key::KeyHash`]).
//!
//! `KeyStr`/`KeyHash` live in [`oxide_fw_edge_primitives::key`] (WP E09: the
//! wasm32 edge build reuses them without linking `redb`/`tokio`); this module
//! re-exports `KeyStr` byte-identical and keeps the cache-specific [`key_of`].

use crate::key::KeyHash;
pub use oxide_fw_edge_primitives::key::KeyStr;

/// blake3 over `layer 0x00 route 0x00 params 0x00 vary 0x00 build_id`, hex of the
/// first 16 digest bytes (128 bits) — the display form of the digest
/// [`crate::CacheKey::canonical`] computes, i.e. `CacheKey::canonical().hex()`
/// since the D24 `KeyHash` landed. NUL separators keep the encoding prefix-free.
#[must_use = "the stack key is the derivation's whole point; dropping it loses the key"]
pub fn key_of(layer: &str, route: &str, params: &str, vary: &str, build_id: &str) -> KeyStr {
    KeyHash::of_parts(layer, route, params, vary, build_id).hex()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CacheKey, Layer};

    #[test]
    fn deterministic() {
        assert_eq!(
            &*key_of("L0", "r", "p", "v", "b"),
            &*key_of("L0", "r", "p", "v", "b")
        );
    }

    #[test]
    fn prefix_free() {
        assert_ne!(
            &*key_of("L0", "ab", "c", "", ""),
            &*key_of("L0", "a", "bc", "", "")
        );
    }

    #[test]
    fn matches_canonical_display() {
        // canonical() hashes the same byte sequence, so its 32-char hex
        // display IS key_of — one digest, one display.
        let k = CacheKey {
            layer: Layer::L0,
            route: "r".into(),
            params: "p".into(),
            vary: "v".into(),
            build_id: "b".into(),
        };
        assert_eq!(
            k.canonical().to_string(),
            key_of("L0", "r", "p", "v", "b").as_str()
        );
    }

    #[test]
    fn every_component_participates() {
        let base = key_of("L0", "r", "p", "v", "b");
        assert_ne!(&*base, &*key_of("L1", "r", "p", "v", "b"), "layer differs");
        assert_ne!(&*base, &*key_of("L0", "r2", "p", "v", "b"), "route differs");
        assert_ne!(&*base, &*key_of("L0", "r", "p2", "v", "b"), "params differ");
        assert_ne!(&*base, &*key_of("L0", "r", "p", "v2", "b"), "vary differs");
        assert_ne!(
            &*base,
            &*key_of("L0", "r", "p", "v", "b2"),
            "build id differs"
        );
    }

    /// `key_of` is the display form of the re-exported `KeyHash` (D24): the
    /// 32-char hex is exactly `KeyHash::of_parts(...).hex()`. Pins the
    /// delegation so the cache and the edge build cannot drift.
    #[test]
    fn key_of_is_keyhash_hex() {
        assert_eq!(
            key_of("L0", "routes/blog/[slug]/page", "{}", "", "build").as_str(),
            KeyHash::of_parts("L0", "routes/blog/[slug]/page", "{}", "", "build")
                .hex()
                .as_str()
        );
    }
}

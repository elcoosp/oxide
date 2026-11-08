//! V2-B05: heap-free cache keys — a 128-bit blake3 prefix on the stack.
//!
//! The `CacheHandler` trait is string-keyed, so the string stays the trait
//! boundary; [`KeyStr`] builds it in 32 bytes of stack (no `String`, no heap)
//! and derefs to `&str`. [`key_of`] digests the same NUL-separated component
//! sequence as [`CacheKey::canonical`](crate::CacheKey::canonical), so its hex
//! form is the first 32 chars of the canonical 64-char digest.
//!
//! Hex only at this display/filename boundary (D24): all in-memory comparisons
//! and map keys can move to the 16 raw digest bytes.

use blake3::Hasher;

/// 32 ASCII hex chars (a 128-bit blake3 prefix) on the stack. Derefs to `&str`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyStr([u8; 32]);

impl KeyStr {
    /// The hex form as a borrowed `&str` (always valid ASCII/UTF-8).
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("hex is ascii")
    }

    /// The 16 raw digest bytes (for allocation-free map keys, D24).
    /// (The stored form is self-hex-decoding; kept alongside the hex form so
    /// both views stay in the 32-byte stack value.)
    pub fn as_bytes128(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        for (i, pair) in self.0.as_chunks::<2>().0.iter().enumerate() {
            out[i] = hexval(pair[0]) << 4 | hexval(pair[1]);
        }
        out
    }
}

impl std::ops::Deref for KeyStr {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for KeyStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::fmt::Debug for KeyStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

fn hexval(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        _ => 0,
    }
}

/// blake3 over `layer 0x00 route 0x00 params 0x00 vary 0x00 build_id`, hex of the
/// first 16 digest bytes (128 bits) — the same digest [`crate::CacheKey::canonical`]
/// computes, truncated. NUL separators keep the encoding prefix-free.
pub fn key_of(layer: &str, route: &str, params: &str, vary: &str, build_id: &str) -> KeyStr {
    let mut h = Hasher::new();
    for (i, p) in [layer, route, params, vary, build_id].iter().enumerate() {
        if i > 0 {
            h.update(&[0]); // NUL separators keep the encoding prefix-free
        }
        h.update(p.as_bytes());
    }
    let d = h.finalize();
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; 32];
    for (i, b) in d.as_bytes()[..16].iter().enumerate() {
        out[2 * i] = HEX[(b >> 4) as usize];
        out[2 * i + 1] = HEX[(b & 15) as usize];
    }
    KeyStr(out)
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
    fn matches_canonical_prefix() {
        // canonical() hashes the same byte sequence, so its first 32 hex chars
        // equal key_of.
        let k = CacheKey {
            layer: Layer::L0,
            route: "r".into(),
            params: "p".into(),
            vary: "v".into(),
            build_id: "b".into(),
        };
        assert_eq!(&k.canonical()[..32], &*key_of("L0", "r", "p", "v", "b"));
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

    #[test]
    fn key_str_is_32_hex_chars_and_raw_round_trips() {
        let k = key_of("L0", "routes/blog/[slug]/page", "{}", "", "build");
        assert_eq!(k.as_str().len(), 32);
        assert!(k.as_str().bytes().all(|b| b.is_ascii_hexdigit()));
        let raw = k.as_bytes128();
        assert_eq!(raw.len(), 16);
        // Deterministic: same input, same raw bytes.
        assert_eq!(
            raw,
            key_of("L0", "routes/blog/[slug]/page", "{}", "", "build").as_bytes128()
        );
    }
}

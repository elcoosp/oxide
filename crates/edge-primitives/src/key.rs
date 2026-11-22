//! The 128-bit heap-free cache key (D24), extracted to its V8-free home.
//!
//! [`KeyHash`] (16 stack bytes) and [`KeyStr`] (32 hex chars) moved verbatim
//! from `oxide-fw-cache::key`/`keyhash`: the cache crate re-exports both from
//! here so its key path is byte-identical, and the wasm32 edge build can
//! derive the L0 key without linking `redb`/`tokio`. The digest is blake3
//! over `layer 0x00 route 0x00 params 0x00 vary 0x00 build_id`, truncated to
//! the first 16 digest bytes — the same byte sequence `key_of` hexes, so the
//! raw and hex forms are two views of one value and cannot drift.

use blake3::Hasher;
use std::fmt;

/// 128-bit truncated blake3 of a cache key (D24): 16 raw digest bytes on
/// the stack, `Copy`, `Eq`/`Hash` by content.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyHash([u8; 16]);

impl KeyHash {
    /// blake3 over the NUL-separated `layer 0x00 route 0x00 params 0x00
    /// vary 0x00 build_id`, truncated to 128 bits. NUL separators keep the
    /// encoding prefix-free (component boundaries are part of the digest).
    #[must_use = "the key identity is the derivation's whole point"]
    pub fn of_parts(layer: &str, route: &str, params: &str, vary: &str, build_id: &str) -> Self {
        let mut h = Hasher::new();
        for (i, part) in [layer, route, params, vary, build_id].iter().enumerate() {
            if i > 0 {
                h.update(&[0]); // NUL separators keep the encoding prefix-free
            }
            h.update(part.as_bytes());
        }
        Self::truncate(h.finalize().as_bytes())
    }

    /// The 16 raw digest bytes — the form every in-memory comparison and
    /// map key uses (D24).
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// The 32-char lowercase hex form (the display/filename boundary,
    /// D24) as a stack [`KeyStr`] — no heap allocation.
    #[must_use]
    pub fn hex(&self) -> KeyStr {
        KeyStr::from_digest16(&self.0)
    }

    fn truncate(digest: &[u8; 32]) -> Self {
        let mut out = [0u8; 16];
        out.copy_from_slice(&digest[..16]);
        Self(out)
    }
}

impl From<&[u8]> for KeyHash {
    fn from(bytes: &[u8]) -> Self {
        Self::truncate(blake3::hash(bytes).as_bytes())
    }
}

impl fmt::Display for KeyHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.hex().as_str())
    }
}

impl fmt::Debug for KeyHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.hex().as_str())
    }
}

/// 32 ASCII hex chars (a 128-bit blake3 prefix) on the stack. Derefs to `&str`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyStr([u8; 32]);

impl KeyStr {
    /// The hex form as a borrowed `&str` (always valid ASCII/UTF-8).
    ///
    /// # Panics
    ///
    /// Panics if the stored bytes are not valid UTF-8 — impossible by
    /// construction: the only constructor ([`Self::from_digest16`]) writes
    /// ASCII hex via the `0123456789abcdef` table, so every byte is ASCII.
    #[must_use]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).expect("hex is ascii")
    }

    /// The 16 raw digest bytes (for allocation-free map keys, D24).
    /// (The stored form is self-hex-decoding; kept alongside the hex form so
    /// both views stay in the 32-byte stack value.)
    #[must_use]
    pub fn as_bytes128(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        for (i, pair) in self.0.as_chunks::<2>().0.iter().enumerate() {
            out[i] = hexval(pair[0]) << 4 | hexval(pair[1]);
        }
        out
    }

    /// The hex form of a 16-byte digest — the constructor the D24
    /// [`KeyHash`] renders through. One hex table, so the raw and hex views
    /// of a key cannot drift.
    pub(crate) fn from_digest16(raw: &[u8; 16]) -> Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = [0u8; 32];
        for (i, b) in raw.iter().enumerate() {
            out[2 * i] = HEX[(b >> 4) as usize];
            out[2 * i + 1] = HEX[(b & 15) as usize];
        }
        KeyStr(out)
    }
}

impl std::ops::Deref for KeyStr {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for KeyStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for KeyStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_bytes_is_truncated_blake3() {
        let h = KeyHash::from(b"oxide".as_slice());
        assert_eq!(h.as_bytes().len(), 16);
        assert_eq!(h.hex().as_str().len(), 32);
    }

    #[test]
    fn deterministic() {
        assert_eq!(
            &*KeyHash::of_parts("L0", "r", "p", "v", "b").hex(),
            &*KeyHash::of_parts("L0", "r", "p", "v", "b").hex()
        );
    }

    #[test]
    fn prefix_free() {
        assert_ne!(
            &*KeyHash::of_parts("L0", "ab", "c", "", "").hex(),
            &*KeyHash::of_parts("L0", "a", "bc", "", "").hex()
        );
    }

    #[test]
    fn every_component_participates() {
        let base = KeyHash::of_parts("L0", "r", "p", "v", "b").hex();
        assert_ne!(
            &*base,
            &*KeyHash::of_parts("L1", "r", "p", "v", "b").hex(),
            "layer differs"
        );
        assert_ne!(
            &*base,
            &*KeyHash::of_parts("L0", "r2", "p", "v", "b").hex(),
            "route differs"
        );
        assert_ne!(
            &*base,
            &*KeyHash::of_parts("L0", "r", "p2", "v", "b").hex(),
            "params differ"
        );
        assert_ne!(
            &*base,
            &*KeyHash::of_parts("L0", "r", "p", "v2", "b").hex(),
            "vary differs"
        );
        assert_ne!(
            &*base,
            &*KeyHash::of_parts("L0", "r", "p", "v", "b2").hex(),
            "build id differs"
        );
    }

    #[test]
    fn key_str_is_32_hex_chars_and_raw_round_trips() {
        let k = KeyHash::of_parts("L0", "routes/blog/[slug]/page", "{}", "", "build").hex();
        assert_eq!(k.as_str().len(), 32);
        // The raw view round-trips through from_digest16's inverse.
        let raw = k.as_bytes128();
        assert_eq!(KeyStr::from_digest16(&raw).as_str(), k.as_str());
    }
}

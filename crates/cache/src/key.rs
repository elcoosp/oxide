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

use crate::keyhash::KeyStr;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_bytes_is_truncated_blake3() {
        let h = KeyHash::from(b"oxide".as_slice());
        assert_eq!(h.as_bytes().len(), 16);
        assert_eq!(h.hex().as_str().len(), 32);
    }
}

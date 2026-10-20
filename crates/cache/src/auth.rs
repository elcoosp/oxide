//! S-06: keyed-MAC authentication for L2 (and disk) entries.
//!
//! An L2 payload is untrusted transport: anyone with write access to a shared
//! Redis/S3 (or the disk file) could otherwise mint an `Entry { body: "<script>…",
//! headers: […] }` and serve stored XSS / header injection. Every entry is
//! sealed with a keyed BLAKE3 MAC before it leaves a trusted tier and verified
//! before it is served; a MAC failure is a miss (never served).
//!
//! The key is read from `OXIDE_CACHE_MAC_KEY` (hex or raw) when set, otherwise
//! derived from a process-start random value so a single-process deployment is
//! still protected against an attacker who can only *write* to L2 (it cannot
//! forge a MAC without the in-memory key). A multi-node deployment MUST set the
//! env var so sealed entries interoperate.

use std::sync::OnceLock;

/// 32-byte BLAKE3 MAC prefix length.
pub const MAC_LEN: usize = 32;

static KEY: OnceLock<[u8; 32]> = OnceLock::new();

fn key() -> &'static [u8; 32] {
    KEY.get_or_init(|| {
        if let Ok(v) = std::env::var("OXIDE_CACHE_MAC_KEY") {
            if let Ok(bytes) = hex_decode(&v) {
                if bytes.len() >= 32 {
                    let mut k = [0u8; 32];
                    k.copy_from_slice(&bytes[..32]);
                    return k;
                }
            }
            // Raw (non-hex) value: derive 32 bytes deterministically.
            let mut k = [0u8; 32];
            let h = blake3::hash(v.as_bytes());
            k.copy_from_slice(h.as_bytes());
            return k;
        }
        // No env key: mint an ephemeral per-process key from the OS CSPRNG.
        let mut k = [0u8; 32];
        if getrandom(&mut k).is_err() {
            // Last resort: derive from the process id + time (still unforgeable
            // to an L2-only writer who never reads process memory).
            let mut h = blake3::Hasher::new();
            h.update(&std::process::id().to_le_bytes());
            h.update(
                &std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0)
                    .to_le_bytes(),
            );
            k.copy_from_slice(h.finalize().as_bytes());
        }
        k
    })
}

fn mac(bytes: &[u8]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_keyed(key());
    h.update(bytes);
    *h.finalize().as_bytes()
}

/// Prefix `bytes` with its 32-byte keyed MAC.
pub fn seal_entry(bytes: &[u8]) -> Vec<u8> {
    let m = mac(bytes);
    let mut out = Vec::with_capacity(MAC_LEN + bytes.len());
    out.extend_from_slice(&m);
    out.extend_from_slice(bytes);
    out
}

/// Verify and strip the MAC. Returns `None` (a miss) on a missing/short/exact
/// mismatch — the comparison is constant-time via BLAKE3's `Hash::eq`.
pub fn open_entry(sealed: &[u8]) -> Option<&[u8]> {
    if sealed.len() < MAC_LEN {
        return None;
    }
    let (m, body) = sealed.split_at(MAC_LEN);
    let expect = mac(body);
    // `blake3::Hash` equality is constant-time.
    let mut got = [0u8; 32];
    got.copy_from_slice(m);
    if blake3::Hash::from_bytes(got) == blake3::Hash::from_bytes(expect) {
        Some(body)
    } else {
        None
    }
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return Err(());
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let hi = (b[i] as char).to_digit(16).ok_or(())? as u8;
        let lo = (b[i + 1] as char).to_digit(16).ok_or(())? as u8;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

#[cfg(not(target_family = "wasm"))]
fn getrandom(buf: &mut [u8]) -> Result<(), ()> {
    use ring::rand::SecureRandom as _;
    ring::rand::SystemRandom::new().fill(buf).map_err(|_| ())
}

#[cfg(target_family = "wasm")]
fn getrandom(_buf: &mut [u8]) -> Result<(), ()> {
    Err(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_roundtrip() {
        let sealed = seal_entry(b"hello");
        assert_eq!(sealed.len(), MAC_LEN + 5);
        assert_eq!(open_entry(&sealed), Some(&b"hello"[..]));
    }

    #[test]
    fn tampered_body_is_rejected() {
        let mut sealed = seal_entry(b"hello");
        let last = sealed.len() - 1;
        sealed[last] ^= 0xff;
        assert_eq!(open_entry(&sealed), None);
    }

    #[test]
    fn tampered_mac_is_rejected() {
        let mut sealed = seal_entry(b"hello");
        sealed[0] ^= 0xff;
        assert_eq!(open_entry(&sealed), None);
    }

    #[test]
    fn short_input_is_rejected() {
        assert_eq!(open_entry(&[0u8; 8]), None);
    }
}

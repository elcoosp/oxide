//! §16.6 session primitive: authenticated-encryption cookies with key rotation.
//!
//! The existing cookie helper ([`crate::sign_cookie`]) only *signs* a value with
//! a blake3 MAC, so the payload travels in the clear. §16.6 asks for **signed and
//! encrypted** cookies; this module is the encrypted half. A session token is
//! `base64url(nonce ‖ ChaCha20-Poly1305(ciphertext))` — confidential, tamper-
//! evident, and rotatable: the first key seals, and `open` tries every configured
//! key so a rotation does not invalidate live sessions.
//!
//! `ring` provides the AEAD; there is no hand-rolled cryptography here.

use crate::{Result, err};
use ring::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::Value;
use std::collections::BTreeMap;

/// ChaCha20-Poly1305 authentication tag length.
const TAG_LEN: usize = 16;

/// A keyed codec for encrypted session tokens.
///
/// Construct once at boot from the project's session keys
/// (`security.sessionKeys` / `OXIDE_SESSION_KEYS`). The **first** key seals; every
/// key is tried on open, so prepending a new key rotates without a logout wave.
pub struct SessionCodec {
    keys: Vec<LessSafeKey>,
    rng: SystemRandom,
}

impl std::fmt::Debug for SessionCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print key material.
        f.debug_struct("SessionCodec")
            .field("keys", &self.keys.len())
            .finish()
    }
}

impl SessionCodec {
    /// Build a codec from raw key bytes. At least one key is required; each key
    /// should be ≥ 32 bytes of entropy (the AEAD uses the first 32).
    pub fn new(keys: &[Vec<u8>]) -> Result<Self> {
        if keys.is_empty() {
            return Err(err(
                "SEC-SESSION",
                "a session codec needs at least one key (security.sessionKeys)",
            ));
        }
        let mut built = Vec::with_capacity(keys.len());
        for k in keys {
            if k.len() < 32 {
                return Err(err(
                    "SEC-SESSION",
                    format!("session key too short: {} bytes, need ≥ 32", k.len()),
                ));
            }
            let unbound = UnboundKey::new(&CHACHA20_POLY1305, &k[..32])
                .map_err(|_| err("SEC-SESSION", "invalid session key"))?;
            built.push(LessSafeKey::new(unbound));
        }
        Ok(Self {
            keys: built,
            rng: SystemRandom::new(),
        })
    }

    /// Encrypt `plaintext` into a `base64url(nonce ‖ ciphertext‖tag)` token.
    pub fn seal(&self, plaintext: &[u8]) -> Result<String> {
        let key = self.keys.first().expect("new() requires ≥ 1 key");
        let mut nonce_bytes = [0u8; NONCE_LEN];
        self.rng
            .fill(&mut nonce_bytes)
            .map_err(|_| err("SEC-SESSION", "system RNG unavailable"))?;
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);

        let mut in_out = plaintext.to_vec();
        key.seal_in_place_append_tag(nonce, Aad::empty(), &mut in_out)
            .map_err(|_| err("SEC-SESSION", "seal failed"))?;

        let mut wire = Vec::with_capacity(NONCE_LEN + in_out.len());
        wire.extend_from_slice(&nonce_bytes);
        wire.extend_from_slice(&in_out);
        Ok(b64url_encode(&wire))
    }

    /// Decrypt a token sealed by any configured key. Returns `SEC-SESSION` when
    /// the token is malformed, tampered with, or sealed under an unknown key.
    pub fn open(&self, token: &str) -> Result<Vec<u8>> {
        let wire = b64url_decode(token)?;
        if wire.len() < NONCE_LEN + TAG_LEN {
            return Err(err("SEC-SESSION", "session token too short"));
        }
        let (nonce_bytes, ciphertext) = wire.split_at(NONCE_LEN);
        // `Nonce` is not `Copy`, so it is rebuilt from the same bytes for each
        // key attempt — trying a second key must not consume the first's nonce.
        let nonce_of = || {
            Nonce::try_assume_unique_for_key(nonce_bytes)
                .map_err(|_| err("SEC-SESSION", "invalid session nonce"))
        };

        // Try each key: the first that authenticates wins (rotation support).
        for key in &self.keys {
            let mut buf = ciphertext.to_vec();
            if let Ok(pt) = key.open_in_place(nonce_of()?, Aad::empty(), &mut buf) {
                return Ok(pt.to_vec());
            }
        }
        Err(err(
            "SEC-SESSION",
            "session token failed authentication (tampered, or sealed under a rotated-out key)",
        ))
    }
}

/// A decoded session: an arbitrary JSON map plus the codec round-trip.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Session {
    pub data: BTreeMap<String, Value>,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// The whole session map, for handing to a loader as `locals.session`.
    pub fn to_value(&self) -> Value {
        Value::Object(self.data.clone().into_iter().collect())
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.data.get(key)
    }

    pub fn insert(&mut self, key: impl Into<String>, value: Value) {
        self.data.insert(key.into(), value);
    }

    /// Serialize to an encrypted cookie token.
    pub fn encode(&self, codec: &SessionCodec) -> Result<String> {
        let json = serde_json::to_vec(&self.data)
            .map_err(|e| err("SEC-SESSION", format!("session serialize: {e}")))?;
        codec.seal(&json)
    }

    /// Decode a token produced by [`Session::encode`].
    pub fn decode(codec: &SessionCodec, token: &str) -> Result<Self> {
        let bytes = codec.open(token)?;
        let data: BTreeMap<String, Value> = serde_json::from_slice(&bytes)
            .map_err(|e| err("SEC-SESSION", format!("session deserialize: {e}")))?;
        Ok(Self { data })
    }
}

// ---------------------------------------------------------------- base64url

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Unpadded base64url (RFC 4648 §5). A cookie value may not contain `=`, `+` or
/// `/`, so the URL-safe alphabet without padding is the right encoding — and
/// hand-rolling it here keeps the session codec free of a base64 dependency.
fn b64url_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(B64[((n >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(B64[(n & 63) as usize] as char);
        }
    }
    out
}

fn b64url_decode(s: &str) -> Result<Vec<u8>> {
    let mut rev = [255u8; 128];
    for (i, &c) in B64.iter().enumerate() {
        rev[c as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for &c in s.as_bytes() {
        if c as usize >= 128 {
            return Err(err("SEC-SESSION", "session token is not base64url"));
        }
        let v = rev[c as usize];
        if v == 255 {
            return Err(err("SEC-SESSION", "session token has an invalid character"));
        }
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(byte: u8) -> Vec<u8> {
        vec![byte; 32]
    }

    #[test]
    fn seal_open_roundtrip() {
        let codec = SessionCodec::new(&[key(1)]).unwrap();
        let token = codec.seal(b"hello session").unwrap();
        assert_eq!(codec.open(&token).unwrap(), b"hello session");
    }

    #[test]
    fn seal_is_nondeterministic() {
        // A random nonce per seal means two seals of the same plaintext differ —
        // and both still open.
        let codec = SessionCodec::new(&[key(2)]).unwrap();
        let a = codec.seal(b"same").unwrap();
        let b = codec.seal(b"same").unwrap();
        assert_ne!(a, b, "tokens must differ (random nonce)");
        assert_eq!(codec.open(&a).unwrap(), b"same");
        assert_eq!(codec.open(&b).unwrap(), b"same");
    }

    #[test]
    fn token_is_not_the_plaintext() {
        // Confidentiality: the plaintext must not appear in the token.
        let codec = SessionCodec::new(&[key(3)]).unwrap();
        let token = codec.seal(b"super-secret-user-id").unwrap();
        assert!(!token.contains("super-secret"));
        assert!(!token.contains("user-id"));
    }

    #[test]
    fn wrong_key_fails_to_open() {
        let sealer = SessionCodec::new(&[key(10)]).unwrap();
        let opener = SessionCodec::new(&[key(11)]).unwrap();
        let token = sealer.seal(b"payload").unwrap();
        assert!(opener.open(&token).is_err());
    }

    #[test]
    fn tampered_token_fails() {
        let codec = SessionCodec::new(&[key(20)]).unwrap();
        let token = codec.seal(b"payload").unwrap();
        // Flip a character in the middle of the token.
        let mut bytes: Vec<u8> = token.into_bytes();
        let mid = bytes.len() / 2;
        bytes[mid] = if bytes[mid] == b'A' { b'B' } else { b'A' };
        let tampered = String::from_utf8(bytes).unwrap();
        assert!(codec.open(&tampered).is_err(), "tamper must be rejected");
    }

    #[test]
    fn key_rotation_old_key_still_opens() {
        // A token sealed under key A opens under a codec whose key list is [B, A]:
        // B is the new sealing key, A is retained for existing sessions.
        let old = SessionCodec::new(&[key(30)]).unwrap();
        let token = old.seal(b"pre-rotation").unwrap();

        let rotated = SessionCodec::new(&[key(31), key(30)]).unwrap();
        assert_eq!(rotated.open(&token).unwrap(), b"pre-rotation");

        // New seals use the new first key, which the old codec cannot open.
        let new_token = rotated.seal(b"post-rotation").unwrap();
        assert!(old.open(&new_token).is_err());
    }

    #[test]
    fn malformed_tokens_fail_cleanly() {
        let codec = SessionCodec::new(&[key(40)]).unwrap();
        assert!(codec.open("").is_err());
        assert!(codec.open("!!!not base64!!!").is_err());
        // Valid base64url but too short to carry a nonce + tag.
        assert!(codec.open("AAAA").is_err());
    }

    #[test]
    fn rejects_short_and_empty_key_sets() {
        assert!(SessionCodec::new(&[]).is_err());
        assert!(SessionCodec::new(&[vec![0u8; 16]]).is_err());
    }

    #[test]
    fn session_data_roundtrip() {
        let codec = SessionCodec::new(&[key(50)]).unwrap();
        let mut s = Session::new();
        s.insert("user", json!("alice"));
        s.insert("role", json!("admin"));
        s.insert("count", json!(3));

        let token = s.encode(&codec).unwrap();
        let back = Session::decode(&codec, &token).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.get("user").unwrap(), &json!("alice"));
    }

    #[test]
    fn base64url_roundtrip_various_lengths() {
        for n in 0..40usize {
            let data: Vec<u8> = (0..n).map(|i| (i * 7 + 3) as u8).collect();
            let enc = b64url_encode(&data);
            assert!(!enc.contains('='), "no padding");
            assert!(
                !enc.contains('+') && !enc.contains('/'),
                "url-safe alphabet"
            );
            assert_eq!(b64url_decode(&enc).unwrap(), data, "n={n}");
        }
    }
}

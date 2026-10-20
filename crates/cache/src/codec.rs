//! L2 entry wire codec (B-10): the `body: Vec<u8>` is base64+zstd encoded so a
//! remote payload is ~0.25-1× the raw bytes instead of the ~3-4× a plain serde
//! `Vec<u8>` (binary → JSON number array) costs. The remaining fields are JSON.
//!
//! `MAX_ENTRY_BYTES` caps decompression (S-06's size gate): a hostile 10 GB
//! value is rejected after the zstd header rather than buffered.

use crate::{Entry, Policy, Result, err};
use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// Hard cap on a decompressed entry body at the L2 boundary (32 MiB).
pub const MAX_ENTRY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct WireEntry {
    /// zstd(level 3) then standard base64.
    body_b64: String,
    status: u16,
    headers: Vec<(String, String)>,
    tags: Vec<String>,
    policy: Policy,
    stored_at: u64,
    etag: String,
}

/// Encode an entry for the L2 wire (JSON envelope + base64+zstd body).
pub fn encode_entry(e: &Entry) -> Result<Vec<u8>> {
    let compressed = zstd::bulk::compress(&e.body, 3)
        .map_err(|e| err("CACHE-CODEC", format!("zstd: {e}")))?;
    let wire = WireEntry {
        body_b64: base64::engine::general_purpose::STANDARD.encode(&compressed),
        status: e.status,
        headers: e.headers.clone(),
        tags: e.tags.clone(),
        policy: e.policy.clone(),
        stored_at: e.stored_at,
        etag: e.etag.clone(),
    };
    serde_json::to_vec(&wire).map_err(|e| err("CACHE-CODEC", format!("encode: {e}")))
}

/// Decode an L2 wire entry, enforcing [`MAX_ENTRY_BYTES`] on the decompressed body.
pub fn decode_entry(bytes: &[u8]) -> Result<Entry> {
    // Reject an absurd envelope before allocating the JSON tree.
    if bytes.len() > MAX_ENTRY_BYTES {
        return Err(err(
            "CACHE-CODEC",
            format!("entry envelope {} exceeds cap", bytes.len()),
        ));
    }
    // Compatibility: an entry written by an older build is a plain `Entry`
    // JSON. Detect it (no `body_b64` field) and fall back.
    if let Some(e) = decode_legacy(bytes) {
        return Ok(e);
    }
    let wire: WireEntry = serde_json::from_slice(bytes)
        .map_err(|e| err("CACHE-CODEC", format!("decode: {e}")))?;
    let compressed = base64::engine::general_purpose::STANDARD
        .decode(&wire.body_b64)
        .map_err(|e| err("CACHE-CODEC", format!("b64: {e}")))?;
    let body = zstd::bulk::decompress(&compressed, MAX_ENTRY_BYTES)
        .map_err(|e| err("CACHE-CODEC", format!("zstd: {e}")))?;
    Ok(Entry {
        body,
        status: wire.status,
        headers: wire.headers,
        tags: wire.tags,
        policy: wire.policy,
        stored_at: wire.stored_at,
        etag: wire.etag,
    })
}

fn decode_legacy(bytes: &[u8]) -> Option<Entry> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    if v.get("body_b64").is_some() {
        return None;
    }
    serde_json::from_value(v).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Policy;
    use std::time::Duration;

    fn entry_with(body: Vec<u8>) -> Entry {
        Entry {
            body,
            status: 200,
            headers: vec![("content-type".into(), "text/html".into())],
            tags: vec!["route".into()],
            policy: Policy {
                ttl: Duration::from_secs(60),
                swr: Some(Duration::from_secs(30)),
                stale_if_error: None,
                soft_tags: true,
            },
            stored_at: 42,
            etag: "abc".into(),
        }
    }

    #[test]
    fn roundtrips_and_compresses_repetitive_body() {
        let body = b"<html>hello world</html>".repeat(1024);
        let e = entry_with(body.clone());
        let wire = encode_entry(&e).unwrap();
        // JSON number-array form would be ~4x; base64+zstd must be far smaller.
        assert!(wire.len() < body.len(), "wire {} vs body {}", wire.len(), body.len());
        let back = decode_entry(&wire).unwrap();
        assert_eq!(back.body, body);
        assert_eq!(back.status, 200);
        assert_eq!(back.tags, vec!["route".to_string()]);
        assert_eq!(back.policy.ttl, Duration::from_secs(60));
    }

    #[test]
    fn decodes_legacy_plain_entry() {
        let e = entry_with(b"legacy".to_vec());
        let legacy = serde_json::to_vec(&e).unwrap();
        let back = decode_entry(&legacy).unwrap();
        assert_eq!(back.body, b"legacy");
    }

    #[test]
    fn rejects_oversized_envelope() {
        let big = vec![0u8; MAX_ENTRY_BYTES + 1];
        assert!(decode_entry(&big).is_err());
    }
}

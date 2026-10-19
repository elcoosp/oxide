//! §5.4 rkyv artifact envelope for CAS bodies.
//!
//! The CAS (see [`crate::cas`]) treats artifact bodies as opaque bytes.
//! This module adds a small rkyv envelope with a `blake3` `archived_hash`
//! so a partial write, a disk bit-flip, or a tampered blob is caught on
//! load (`GRAPH-CORRUPT`) instead of being fed back into a build.
//!
//! On-disk shape:
//!
//! ```text
//! [8-byte LE u64 length][rkyv-serialized ArtifactEnvelope]
//! ```
//!
//! The CAS index stays JSON (index ≠ artifact) — this envelope wraps only
//! the artifact bytes.

use crate::{Result, err};
use rkyv::{Archive, Deserialize, Serialize};

/// Artifact kind tag. Forward-compatible (`Other` for arbitrary bytes).
#[derive(Archive, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArtifactKind {
    CompiledJs,
    Chunk,
    Image,
    Css,
    Other,
}

/// On-disk envelope. `body` is opaque to this module; the hash covers it.
#[derive(Archive, Serialize, Deserialize, Clone, Debug)]
pub struct ArtifactEnvelope {
    /// blake3 of `body` — verified on load.
    pub archived_hash: [u8; 32],
    pub kind: ArtifactKind,
    pub body: Vec<u8>,
}

/// Serialize an artifact body + kind to the on-disk envelope.
pub fn seal(kind: ArtifactKind, body: Vec<u8>) -> Result<Vec<u8>> {
    let hash = blake3::hash(&body);
    let env = ArtifactEnvelope {
        archived_hash: *hash.as_bytes(),
        kind,
        body,
    };
    let inner = rkyv::to_bytes::<rkyv::rancor::Error>(&env)
        .map_err(|e| err("GRAPH-ARTIFACT", format!("rkyv serialize: {e}")))?;
    let mut out = Vec::with_capacity(8 + inner.len());
    out.extend_from_slice(&(inner.len() as u64).to_le_bytes());
    out.extend_from_slice(&inner);
    Ok(out)
}

/// Deserialize + verify the archived hash against a fresh blake3 of the body.
pub fn open(bytes: &[u8]) -> Result<ArtifactEnvelope> {
    if bytes.len() < 8 {
        return Err(err("GRAPH-CORRUPT", "artifact: truncated header"));
    }
    let len = u64::from_le_bytes(bytes[..8].try_into().unwrap()) as usize;
    if bytes.len() < 8 + len {
        return Err(err(
            "GRAPH-CORRUPT",
            format!("artifact: truncated body ({} < {})", bytes.len() - 8, len),
        ));
    }
    let inner = &bytes[8..8 + len];
    let archived = rkyv::access::<ArchivedArtifactEnvelope, rkyv::rancor::Error>(inner)
        .map_err(|e| err("GRAPH-CORRUPT", format!("artifact: rkyv access: {e}")))?;
    let env: ArtifactEnvelope = rkyv::deserialize::<_, rkyv::rancor::Error>(archived)
        .map_err(|e| err("GRAPH-CORRUPT", format!("artifact: rkyv deserialize: {e}")))?;
    let fresh = blake3::hash(&env.body);
    if fresh.as_bytes() != &env.archived_hash {
        return Err(err(
            "GRAPH-CORRUPT",
            "artifact: archived_hash mismatch (body tampered or corrupt)",
        ));
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_roundtrips() {
        let bytes = seal(ArtifactKind::CompiledJs, b"const x = 1;".to_vec()).unwrap();
        let env = open(&bytes).unwrap();
        assert!(matches!(env.kind, ArtifactKind::CompiledJs));
        assert_eq!(env.body, b"const x = 1;".to_vec());
    }

    #[test]
    fn corrupted_body_is_caught() {
        let mut bytes = seal(ArtifactKind::Chunk, b"chunk bytes".to_vec()).unwrap();
        let n = bytes.len();
        bytes[n - 1] ^= 0xFF;
        let e = open(&bytes).unwrap_err();
        assert_eq!(e.0.code, "GRAPH-CORRUPT");
    }

    #[test]
    fn truncated_header_is_caught() {
        let e = open(&[0u8; 4]).unwrap_err();
        assert_eq!(e.0.code, "GRAPH-CORRUPT");
    }

    #[test]
    fn truncated_body_is_caught() {
        let mut bytes = seal(ArtifactKind::Other, b"x".to_vec()).unwrap();
        bytes.truncate(8 + 1);
        let e = open(&bytes).unwrap_err();
        assert_eq!(e.0.code, "GRAPH-CORRUPT");
    }
}

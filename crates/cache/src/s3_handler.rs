//! SPEC §11.2: an S3-backed `CacheHandler`.
//!
//! `cache.handler = { type: 's3', bucket }` wires this as the L2 tier. The
//! AWS credentials / region / endpoint come from the environment via
//! `AmazonS3Builder::from_env()` — that's the standard `aws-sdk` convention
//! and it keeps the config surface (`bucket`) tiny.
//!
//! Layout:
//!
//! - `cache/entry/<key>` → JSON blob of [`Entry`].
//! - `cache/tag/<tag>`   → JSON array of entry keys.
//!
//! ## Degradation
//!
//! `get`/`tag_keys`/`invalidate_tag` degrade to miss/no-op on any S3 error
//! — the local tier keeps serving. `set` returns the error.
//!
//! ## `with_store`
//!
//! [`S3CacheHandler::with_store`] takes any `Arc<dyn ObjectStore>` — used
//! by the in-memory test and by projects that want GCS/Azure/etc. through
//! the same handler without adding another constructor.

use crate::{CacheHandler, Entry, Result, auth, codec, err};
use async_trait::async_trait;
use object_store::path::Path as ObjPath;
use object_store::{ObjectStore, PutPayload};
use std::sync::Arc;
use std::time::Duration;

/// S-05: hard timeout on any single S3 op so a hanging store cannot stall the
/// request path (a timeout is a miss + degradation, never a hang).
const L2_OP_TIMEOUT: Duration = Duration::from_millis(500);

/// An S3-backed [`CacheHandler`].
pub struct S3CacheHandler {
    store: Arc<dyn ObjectStore>,
}

const PREFIX_ENTRY: &str = "cache/entry/";
const PREFIX_TAG: &str = "cache/tag/";
/// B-11: a per-key tag marker written alongside the tag set. Object stores
/// have no compare-and-swap, so the tag-set read-modify-write can lose an
/// update under concurrency; scanning the markers recovers the keys. Markers
/// live under `cache/tagmark/<tag>/<key>`.
const PREFIX_TAGMARK: &str = "cache/tagmark/";

impl S3CacheHandler {
    /// Build from a bucket name; AWS credentials/region come from the
    /// environment (`AWS_REGION`, `AWS_ACCESS_KEY_ID`, …). A construction
    /// failure is `Err`, not a panic — the CLI logs it and continues with
    /// the local tier only.
    pub fn new(bucket: impl Into<String>) -> Result<Self> {
        let bucket = bucket.into();
        let store = object_store::aws::AmazonS3Builder::from_env()
            .with_bucket_name(&bucket)
            .build()
            .map_err(|e| err("CACHE-S3", format!("s3 client: {e}")))?;
        Ok(Self {
            store: Arc::new(store),
        })
    }

    /// Wrap any `ObjectStore`. Used by tests (`InMemory`) and by callers
    /// that want a non-S3 backend through this handler.
    pub fn with_store(store: Arc<dyn ObjectStore>) -> Self {
        Self { store }
    }

    fn entry_path(key: &str) -> ObjPath {
        ObjPath::from(format!("{PREFIX_ENTRY}{key}"))
    }
    fn tag_path(tag: &str) -> ObjPath {
        ObjPath::from(format!("{PREFIX_TAG}{tag}"))
    }

    async fn read_tag_keys(&self, tag: &str) -> Result<Vec<String>> {
        let got = tokio::time::timeout(L2_OP_TIMEOUT, self.store.get(&Self::tag_path(tag))).await;
        let bytes = match got {
            Ok(Ok(r)) => match tokio::time::timeout(L2_OP_TIMEOUT, r.bytes()).await {
                Ok(Ok(b)) => b,
                _ => return Ok(Vec::new()),
            },
            _ => return Ok(Vec::new()),
        };
        Ok(serde_json::from_slice::<Vec<String>>(&bytes).unwrap_or_default())
    }
}

#[async_trait]
impl CacheHandler for S3CacheHandler {
    async fn get(&self, key: &str) -> Result<Option<Entry>> {
        let got = tokio::time::timeout(L2_OP_TIMEOUT, self.store.get(&Self::entry_path(key))).await;
        let (bytes, meta_len) = match got {
            Ok(Ok(r)) => {
                // S-06: refuse an oversized object before buffering it.
                let len = r.meta.size;
                if len > crate::MAX_ENTRY_BYTES {
                    tracing::warn!(size = len, "L2 (s3) entry exceeds cap — treated as miss");
                    return Ok(None);
                }
                match tokio::time::timeout(L2_OP_TIMEOUT, r.bytes()).await {
                    Ok(Ok(b)) => (b, len),
                    _ => return Ok(None),
                }
            }
            _ => return Ok(None),
        };
        let _ = meta_len;
        // S-06: verify the keyed MAC before trusting the payload.
        let verified: &[u8] = match auth::open_entry(&bytes) {
            Some(body) => body,
            None => {
                // A1: MAC failure is a miss unless the one-release legacy
                // window is explicitly enabled.
                if auth::legacy_sealing_allowed() && codec::decode_entry(&bytes).is_ok() {
                    tracing::warn!(
                        "L2 (s3) entry is unsealed; serving under OXIDE_CACHE_SEALING=legacy"
                    );
                    &bytes
                } else {
                    tracing::warn!("L2 (s3) entry failed MAC verification — treated as miss");
                    return Ok(None);
                }
            }
        };
        match codec::decode_entry(verified) {
            Ok(e) => Ok(Some(e)),
            Err(e) => Err(err("CACHE-S3", format!("entry decode: {}", e.0.message))),
        }
    }

    async fn set(&self, key: &str, entry: Entry) -> Result<()> {
        // S-06: seal the encoded entry with a keyed MAC.
        let payload = auth::seal_entry(&codec::encode_entry(&entry)?);
        let put = tokio::time::timeout(
            L2_OP_TIMEOUT,
            self.store.put(
                &Self::entry_path(key),
                PutPayload::from(bytes::Bytes::from(payload)),
            ),
        )
        .await
        .map_err(|_| err("CACHE-S3", "put entry: timeout"))?
        .map_err(|e| err("CACHE-S3", format!("put entry: {e}")))?;
        let _ = put;
        for tag in &entry.tags {
            // B-11: register the key in the tag set. The naive read-modify-write
            // races under concurrency (lost updates). We re-read just before the
            // write and append; object-store has no CAS, so we also keep a
            // per-key tag marker (`cache/tagmark/<tag>/<key>`) that
            // `invalidate_tag` scans — a key is found even when the tag-set
            // RMW lost its update (see tag_keys).
            let marker = ObjPath::from(format!("{PREFIX_TAGMARK}{tag}/{key}"));
            let _ = tokio::time::timeout(
                L2_OP_TIMEOUT,
                self.store
                    .put(&marker, PutPayload::from(bytes::Bytes::from_static(b"1"))),
            )
            .await;
            let mut keys = self.read_tag_keys(tag).await.unwrap_or_default();
            if !keys.iter().any(|k| k == key) {
                keys.push(key.to_string());
                let json = serde_json::to_vec(&keys)
                    .map_err(|e| err("CACHE-S3", format!("tag encode: {e}")))?;
                let res = tokio::time::timeout(
                    L2_OP_TIMEOUT,
                    self.store.put(
                        &Self::tag_path(tag),
                        PutPayload::from(bytes::Bytes::from(json)),
                    ),
                )
                .await;
                if !matches!(res, Ok(Ok(_))) {
                    // B-20: the entry is written but its tag registration
                    // failed — remove the orphan so a later `invalidate_tag`
                    // is not the only way to reclaim it, and surface the error.
                    let _ = tokio::time::timeout(
                        L2_OP_TIMEOUT,
                        self.store.delete(&Self::entry_path(key)),
                    )
                    .await;
                    let _ = tokio::time::timeout(L2_OP_TIMEOUT, self.store.delete(&marker)).await;
                    return Err(err(
                        "CACHE-S3",
                        format!("put tag {tag}: registration failed, entry rolled back"),
                    ));
                }
            }
        }
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let _ = self.store.delete(&Self::entry_path(key)).await;
        Ok(())
    }

    async fn tag_keys(&self, tag: &str) -> Result<Vec<String>> {
        // B-11: union the tag set with the per-key markers so a lost RMW update
        // does not drop a key from invalidation.
        let mut keys = self.read_tag_keys(tag).await.unwrap_or_default();
        let prefix = ObjPath::from(format!("{PREFIX_TAGMARK}{tag}"));
        let collected = tokio::time::timeout(L2_OP_TIMEOUT, async {
            use futures::StreamExt as _;
            let mut metas = Vec::new();
            let mut stream = self.store.list(Some(&prefix));
            while let Some(item) = stream.next().await {
                if let Ok(meta) = item {
                    metas.push(meta.location.to_string());
                }
            }
            metas
        })
        .await;
        if let Ok(metas) = collected {
            for loc in metas {
                if let Some(k) = loc.rsplit('/').next()
                    && !keys.iter().any(|e| e == k)
                {
                    keys.push(k.to_string());
                }
            }
        }
        Ok(keys)
    }

    async fn invalidate_tag(&self, tag: &str, _soft: bool) -> Result<usize> {
        let keys = self.tag_keys(tag).await.unwrap_or_default();
        if keys.is_empty() {
            return Ok(0);
        }
        for key in &keys {
            let _ = tokio::time::timeout(L2_OP_TIMEOUT, self.store.delete(&Self::entry_path(key)))
                .await;
            // B-11: drop the marker so the tag can be reused cleanly.
            let marker = ObjPath::from(format!("{PREFIX_TAGMARK}{tag}/{key}"));
            let _ = tokio::time::timeout(L2_OP_TIMEOUT, self.store.delete(&marker)).await;
        }
        let _ = tokio::time::timeout(L2_OP_TIMEOUT, self.store.delete(&Self::tag_path(tag))).await;
        Ok(keys.len())
    }

    async fn publish_invalidation(&self, _tag: &str, _soft: bool) -> Result<()> {
        // S3 has no pub/sub. Peers see the tag-set change on their next
        // read; no delivery guarantee. §11.2 leaves cross-node
        // peer-revalidation implementation-defined.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;

    /// Round-trip an Entry + tag through an in-memory `ObjectStore` — proves
    /// the handler's serialization and tag bookkeeping without an AWS
    /// account. Load-bearing: dropping `PutPayload`/`serde_json` in `set`
    /// makes the round-trip assertion fail.
    #[tokio::test]
    async fn s3_handler_roundtrips_with_in_memory_store() {
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let h = S3CacheHandler::with_store(store);
        let key = "abc";
        let tag = "t1";
        assert!(h.get(key).await.unwrap().is_none());
        let e = Entry {
            body: b"body".to_vec(),
            status: 200,
            headers: vec![],
            tags: vec![tag.into()],
            policy: Default::default(),
            stored_at: 0,
            etag: "e".into(),
        };
        h.set(key, e).await.unwrap();
        let got = h.get(key).await.unwrap().expect("hit");
        assert_eq!(got.body, b"body");
        assert_eq!(h.tag_keys(tag).await.unwrap(), vec![key.to_string()]);
        assert_eq!(h.invalidate_tag(tag, true).await.unwrap(), 1);
        assert!(h.get(key).await.unwrap().is_none());
    }
}

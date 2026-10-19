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

use crate::{CacheHandler, Entry, Result, err};
use async_trait::async_trait;
use object_store::path::Path as ObjPath;
use object_store::{ObjectStore, PutPayload};
use std::sync::Arc;

/// An S3-backed [`CacheHandler`](crate::CacheHandler).
pub struct S3CacheHandler {
    store: Arc<dyn ObjectStore>,
}

const PREFIX_ENTRY: &str = "cache/entry/";
const PREFIX_TAG: &str = "cache/tag/";

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
            .map_err(|e| err("CACHE-S3", format!("s3 client (bucket {bucket}): {e}")))?;
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
        let bytes = match self.store.get(&Self::tag_path(tag)).await {
            Ok(r) => match r.bytes().await {
                Ok(b) => b,
                Err(_) => return Ok(Vec::new()),
            },
            Err(_) => return Ok(Vec::new()),
        };
        Ok(serde_json::from_slice::<Vec<String>>(&bytes).unwrap_or_default())
    }
}

#[async_trait]
impl CacheHandler for S3CacheHandler {
    async fn get(&self, key: &str) -> Result<Option<Entry>> {
        let bytes = match self.store.get(&Self::entry_path(key)).await {
            Ok(r) => match r.bytes().await {
                Ok(b) => b,
                Err(_) => return Ok(None),
            },
            Err(_) => return Ok(None),
        };
        match serde_json::from_slice::<Entry>(&bytes) {
            Ok(e) => Ok(Some(e)),
            Err(e) => Err(err("CACHE-S3", format!("entry decode: {e}"))),
        }
    }

    async fn set(&self, key: &str, entry: Entry) -> Result<()> {
        let json = serde_json::to_vec(&entry)
            .map_err(|e| err("CACHE-S3", format!("entry encode: {e}")))?;
        self.store
            .put(
                &Self::entry_path(key),
                PutPayload::from(bytes::Bytes::from(json)),
            )
            .await
            .map_err(|e| err("CACHE-S3", format!("put entry: {e}")))?;
        for tag in &entry.tags {
            let mut keys = self.read_tag_keys(tag).await.unwrap_or_default();
            if !keys.iter().any(|k| k == key) {
                keys.push(key.to_string());
                let json = serde_json::to_vec(&keys)
                    .map_err(|e| err("CACHE-S3", format!("tag encode: {e}")))?;
                self.store
                    .put(
                        &Self::tag_path(tag),
                        PutPayload::from(bytes::Bytes::from(json)),
                    )
                    .await
                    .map_err(|e| err("CACHE-S3", format!("put tag {tag}: {e}")))?;
            }
        }
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let _ = self.store.delete(&Self::entry_path(key)).await;
        Ok(())
    }

    async fn tag_keys(&self, tag: &str) -> Result<Vec<String>> {
        self.read_tag_keys(tag).await
    }

    async fn invalidate_tag(&self, tag: &str, _soft: bool) -> Result<usize> {
        let keys = self.read_tag_keys(tag).await?;
        if keys.is_empty() {
            return Ok(0);
        }
        for key in &keys {
            let _ = self.store.delete(&Self::entry_path(key)).await;
        }
        let _ = self.store.delete(&Self::tag_path(tag)).await;
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

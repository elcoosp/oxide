//! SPEC-5D: an S3-backed [`RemoteCache`], gated on the
//! `s3` feature. Mirrors [`crate::HttpRemoteCache`]: `fetch` reads the CAS
//! key's blob, `store` writes it. Credentials/region come from `AWS_*` env
//! vars (via `object_store::aws::AmazonS3Builder::from_env`).

use crate::{RemoteCache, Result, err};
use async_trait::async_trait;
use object_store::ObjectStore;
use object_store::PutPayload;
use object_store::path::Path as ObjPath;
use std::sync::Arc;

/// S3-backed remote cache. Keys are hex-prefixed under `cas/`.
pub struct S3RemoteCache {
    store: Arc<dyn ObjectStore>,
    prefix: String,
}

impl S3RemoteCache {
    /// Build from a bucket name; AWS credentials come from the environment.
    pub fn new(bucket: impl Into<String>) -> Result<Self> {
        let bucket = bucket.into();
        let store = object_store::aws::AmazonS3Builder::from_env()
            .with_bucket_name(&bucket)
            .build()
            .map_err(|e| err("GRAPH-S3", format!("s3 client (bucket {bucket}): {e}")))?;
        Ok(Self {
            store: Arc::new(store),
            prefix: "cas/".to_string(),
        })
    }

    /// Test seam: wrap any `ObjectStore`.
    pub fn with_store(store: Arc<dyn ObjectStore>) -> Self {
        Self {
            store,
            prefix: "cas/".to_string(),
        }
    }

    fn path_for(&self, key: &str) -> ObjPath {
        ObjPath::from(format!("{}{}", self.prefix, key))
    }
}

#[async_trait]
impl RemoteCache for S3RemoteCache {
    async fn fetch(&self, key: &crate::cas::CasKey) -> Result<Option<Vec<u8>>> {
        let hex = format!("{key:?}");
        match self.store.get(&self.path_for(&hex)).await {
            Ok(r) => match r.bytes().await {
                Ok(b) => Ok(Some(b.to_vec())),
                Err(_) => Ok(None),
            },
            Err(_) => Ok(None),
        }
    }

    async fn store(&self, key: &crate::cas::CasKey, data: &[u8]) -> Result<()> {
        let hex = format!("{key:?}");
        let payload = PutPayload::from(bytes::Bytes::copy_from_slice(data));
        self.store
            .put(&self.path_for(&hex), payload)
            .await
            .map_err(|e| err("GRAPH-S3", format!("put: {e}")))?;
        Ok(())
    }
}

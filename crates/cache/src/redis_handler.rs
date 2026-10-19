//! SPEC §11.2: a Redis-backed `CacheHandler`.
//!
//! `cache.handler = { type: 'redis', url }` wires this as the L2 tier.
//! Layout:
//!
//! - `oxide:entry:<key>` → JSON-encoded [`Entry`] (string).
//! - `oxide:tag:<tag>`   → Redis SET of entry keys (SADD on set, DEL on
//!   `invalidate_tag`).
//! - `oxide:invalidate`  → pub/sub channel: `{tag, soft}` on invalidation.
//!
//! ## Degradation (§11.2 contract)
//!
//! A Redis failure on `get`/`tag_keys`/`invalidate_tag` is a miss/no-op —
//! the local tier keeps serving and the build proceeds (a remote cache is
//! an optimization, not a correctness requirement). `set` returns the
//! error so a caller can retry or ignore.
//!
//! ## Not published / not deleted
//!
//! Tag sets are append-only until `invalidate_tag` wipes them; a `delete`
//! removes only the entry. The tag set may therefore briefly hold stale
//! keys — harmless (`invalidate_tag` skips missing entries) and matches the
//! HTTP handler's behaviour. `publish_invalidation` publishes; there is no
//! subscriber in this build, so it is a wire-format, not a delivery
//! guarantee.

use crate::{CacheHandler, Entry, Result, err};
use async_trait::async_trait;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;

/// A Redis-backed [`CacheHandler`](crate::CacheHandler).
pub struct RedisCacheHandler {
    conn: ConnectionManager,
}

const PREFIX_ENTRY: &str = "oxide:entry:";
const PREFIX_TAG: &str = "oxide:tag:";
const CHANNEL_INVALIDATE: &str = "oxide:invalidate";

impl RedisCacheHandler {
    /// Connect to a Redis URL (`redis://host:port`, or with AUTH/TLS as the
    /// `redis` crate supports). A connection failure is `Err`, not a panic
    /// — the CLI logs it and continues with the local tier only.
    pub async fn new(url: impl AsRef<str>) -> Result<Self> {
        let url = url.as_ref();
        let client = redis::Client::open(url)
            .map_err(|e| err("CACHE-REDIS", format!("open {url}: {e}")))?;
        let conn = ConnectionManager::new(client)
            .await
            .map_err(|e| err("CACHE-REDIS", format!("connect {url}: {e}")))?;
        Ok(Self { conn })
    }

    fn entry_key(key: &str) -> String {
        format!("{PREFIX_ENTRY}{key}")
    }
    fn tag_key(tag: &str) -> String {
        format!("{PREFIX_TAG}{tag}")
    }
}

#[async_trait]
impl CacheHandler for RedisCacheHandler {
    async fn get(&self, key: &str) -> Result<Option<Entry>> {
        let mut conn = self.conn.clone();
        let raw: Option<String> = match conn.get(Self::entry_key(key)).await {
            Ok(v) => v,
            Err(_) => return Ok(None),
        };
        match raw {
            Some(s) => match serde_json::from_str::<Entry>(&s) {
                Ok(e) => Ok(Some(e)),
                Err(e) => Err(err("CACHE-REDIS", format!("entry decode: {e}"))),
            },
            None => Ok(None),
        }
    }

    async fn set(&self, key: &str, entry: Entry) -> Result<()> {
        let json = serde_json::to_string(&entry)
            .map_err(|e| err("CACHE-REDIS", format!("entry encode: {e}")))?;
        let mut conn = self.conn.clone();
        let _: () = conn
            .set(Self::entry_key(key), json)
            .await
            .map_err(|e| err("CACHE-REDIS", format!("set: {e}")))?;
        for tag in &entry.tags {
            let _: i64 = conn
                .sadd(Self::tag_key(tag), key)
                .await
                .map_err(|e| err("CACHE-REDIS", format!("sadd {tag}: {e}")))?;
        }
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let mut conn = self.conn.clone();
        let _: i64 = conn
            .del(Self::entry_key(key))
            .await
            .map_err(|e| err("CACHE-REDIS", format!("del: {e}")))?;
        Ok(())
    }

    async fn tag_keys(&self, tag: &str) -> Result<Vec<String>> {
        let mut conn = self.conn.clone();
        match conn.smembers::<_, Vec<String>>(Self::tag_key(tag)).await {
            Ok(k) => Ok(k),
            Err(_) => Ok(Vec::new()),
        }
    }

    async fn invalidate_tag(&self, tag: &str, _soft: bool) -> Result<usize> {
        let keys = self.tag_keys(tag).await?;
        if keys.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.clone();
        let entry_keys: Vec<String> = keys.iter().map(|k| Self::entry_key(k)).collect();
        let _: i64 = conn
            .del(&entry_keys)
            .await
            .map_err(|e| err("CACHE-REDIS", format!("del entries: {e}")))?;
        let _: i64 = conn
            .del(Self::tag_key(tag))
            .await
            .map_err(|e| err("CACHE-REDIS", format!("del tag set: {e}")))?;
        Ok(keys.len())
    }

    async fn publish_invalidation(&self, tag: &str, soft: bool) -> Result<()> {
        let payload = serde_json::json!({ "tag": tag, "soft": soft }).to_string();
        let mut conn = self.conn.clone();
        let _: i64 = conn
            .publish(CHANNEL_INVALIDATE, payload)
            .await
            .map_err(|e| err("CACHE-REDIS", format!("publish: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live-server integration test, gated on `OXIDE_TEST_REDIS_URL`. Skipped
    /// with a clear stderr line when unset (no Redis required for CI). Run
    /// locally with:
    ///     OXIDE_TEST_REDIS_URL=redis://127.0.0.1:6379 \
    ///       cargo test -p oxide-cache --features redis -- --ignored
    #[tokio::test]
    #[ignore = "requires OXIDE_TEST_REDIS_URL"]
    async fn redis_roundtrip_and_tag_invalidation() {
        let url = match std::env::var("OXIDE_TEST_REDIS_URL") {
            Ok(u) => u,
            Err(_) => {
                eprintln!("skipped: OXIDE_TEST_REDIS_URL not set");
                return;
            }
        };
        let h = RedisCacheHandler::new(&url).await.expect("connect");
        let nonce = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let key = format!("test-key-{nonce}");
        let tag = format!("test-tag-{nonce}");

        assert!(h.get(&key).await.unwrap().is_none());
        let e = Entry {
            body: b"payload".to_vec(),
            status: 200,
            headers: vec![("x".into(), "y".into())],
            tags: vec![tag.clone()],
            policy: Default::default(),
            stored_at: 0,
            etag: "e".into(),
        };
        h.set(&key, e).await.unwrap();
        let got = h.get(&key).await.unwrap().expect("hit");
        assert_eq!(got.body, b"payload");
        assert_eq!(h.tag_keys(&tag).await.unwrap(), vec![key.clone()]);
        assert_eq!(h.invalidate_tag(&tag, true).await.unwrap(), 1);
        assert!(h.get(&key).await.unwrap().is_none());
        h.publish_invalidation(&tag, true).await.unwrap();
    }
}

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

use crate::{CacheHandler, Entry, Result, auth, codec, err};
use async_trait::async_trait;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// S-05: hard timeout on any single L2 command so a hanging Redis cannot
/// stall the request path (a timeout is a miss + one degraded log).
const L2_OP_TIMEOUT: Duration = Duration::from_millis(250);

/// A Redis-backed [`CacheHandler`](crate::CacheHandler).
pub struct RedisCacheHandler {
    conn: ConnectionManager,
    /// B-18: true once a transport error has been reported so the log isn't
    /// flooded; reset on a successful op.
    degraded: AtomicBool,
}

const PREFIX_ENTRY: &str = "oxide:entry:";
const PREFIX_TAG: &str = "oxide:tag:";
const CHANNEL_INVALIDATE: &str = "oxide:invalidate";

/// S-02: strip the password from a `redis://user:password@host` URL before it
/// can reach a log line or an error string.
pub fn redacted_url(url: &str) -> String {
    match (url.find("://"), url.find('@')) {
        (Some(s), Some(a)) if a > s + 3 => {
            let userinfo = &url[s + 3..a];
            let user = userinfo.split(':').next().unwrap_or("");
            format!("{}://{}:***@{}", &url[..s], user, &url[a + 1..])
        }
        _ => url.to_string(),
    }
}

impl RedisCacheHandler {
    /// Connect to a Redis URL (`redis://host:port`, or with AUTH/TLS as the
    /// `redis` crate supports). A connection failure is `Err`, not a panic
    /// — the CLI logs it and continues with the local tier only.
    pub async fn new(url: impl AsRef<str>) -> Result<Self> {
        let url = url.as_ref();
        let redacted = redacted_url(url);
        let client = redis::Client::open(url)
            .map_err(|e| err("CACHE-REDIS", format!("open {redacted}: {e}")))?;
        // S-05: bound both the connect attempt and every command response so a
        // network black-hole to Redis cannot pin a request forever.
        let config = redis::aio::ConnectionManagerConfig::new()
            .set_response_timeout(L2_OP_TIMEOUT)
            .set_connection_timeout(Duration::from_secs(2));
        let conn = ConnectionManager::new_with_config(client, config)
            .await
            .map_err(|e| err("CACHE-REDIS", format!("connect {redacted}: {e}")))?;
        Ok(Self {
            conn,
            degraded: AtomicBool::new(false),
        })
    }

    fn entry_key(key: &str) -> String {
        format!("{PREFIX_ENTRY}{key}")
    }
    fn tag_key(tag: &str) -> String {
        format!("{PREFIX_TAG}{tag}")
    }

    /// B-18: report the first L2 transport error, then stay quiet until a
    /// successful op resets the flag — an L2 outage must not look like a cold
    /// cache with zero operator signal.
    fn note_degraded(&self, op: &str, e: &str) {
        if !self.degraded.swap(true, Ordering::Relaxed) {
            tracing::error!(
                op,
                error = %e,
                "L2 (redis) degraded — serving local-only until reconnect (further errors suppressed)"
            );
        }
    }

    fn note_recovered(&self) {
        self.degraded.store(false, Ordering::Relaxed);
    }
}

#[async_trait]
impl CacheHandler for RedisCacheHandler {
    async fn get(&self, key: &str) -> Result<Option<Entry>> {
        let mut conn = self.conn.clone();
        // S-05: an outer timeout guards even a stuck connection-manager op.
        let raw: Vec<u8> = match tokio::time::timeout(
            L2_OP_TIMEOUT,
            conn.get::<_, Vec<u8>>(Self::entry_key(key)),
        )
        .await
        {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                self.note_degraded("get", &e.to_string());
                return Ok(None);
            }
            Err(_) => {
                self.note_degraded("get", "timeout");
                return Ok(None);
            }
        };
        self.note_recovered();
        // S-06: verify the keyed MAC before trusting the payload. A MAC failure
        // is a miss + a warning, never a served entry. Legacy/unsealed values
        // (written before this build) are still decoded for compatibility.
        let verified: &[u8] = match auth::open_entry(&raw) {
            Some(body) => body,
            None => {
                // S-06 / A1: MAC failure is a miss. The legacy tolerance is
                // opt-in for one migration window only — see
                // `auth::legacy_sealing_allowed`.
                if auth::legacy_sealing_allowed() && codec::decode_entry(&raw).is_ok() {
                    tracing::warn!(
                        "L2 (redis) entry is unsealed; serving under OXIDE_CACHE_SEALING=legacy"
                    );
                    &raw
                } else {
                    tracing::warn!("L2 (redis) entry failed MAC verification — treated as miss");
                    return Ok(None);
                }
            }
        };
        // B-10: the body travels base64+zstd; decode_entry also accepts the
        // legacy plain-Entry JSON written by an older build.
        match codec::decode_entry(verified) {
            Ok(e) => Ok(Some(e)),
            Err(e) => {
                tracing::warn!(error = %e.0.message, "L2 (redis) entry decode failed");
                Ok(None)
            }
        }
    }

    async fn set(&self, key: &str, entry: Entry) -> Result<()> {
        // S-06: seal the encoded entry with a keyed MAC so a writer to the
        // shared L2 cannot forge a servable payload.
        let payload = auth::seal_entry(&codec::encode_entry(&entry)?);
        let mut conn = self.conn.clone();
        // B-09: hygiene TTL so keys nobody re-reads expire (freshness is still
        // classified at read time by `tiers.rs`, so an early expiry is safe).
        let ttl_secs = (entry.policy.ttl.max(entry.policy.swr.unwrap_or_default()).as_secs())
            .clamp(60, 86_400);
        // P-18: entry + all tag registrations in one round-trip.
        let mut pipe = redis::pipe();
        pipe.set_ex(Self::entry_key(key), payload, ttl_secs).ignore();
        for tag in &entry.tags {
            pipe.sadd(Self::tag_key(tag), key).ignore();
        }
        // S-05: bound the write too (an outer timeout covers a stuck op).
        let res: redis::RedisResult<()> =
            match tokio::time::timeout(L2_OP_TIMEOUT, pipe.query_async(&mut conn)).await {
                Ok(r) => r,
                Err(_) => {
                    self.note_degraded("set", "timeout");
                    return Err(err("CACHE-REDIS", "set: timeout"));
                }
            };
        match res {
            Ok(()) => {
                self.note_recovered();
                Ok(())
            }
            Err(e) => {
                self.note_degraded("set", &e.to_string());
                Err(err("CACHE-REDIS", format!("set: {e}")))
            }
        }
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
            Ok(k) => {
                self.note_recovered();
                Ok(k)
            }
            Err(e) => {
                self.note_degraded("tag_keys", &e.to_string());
                Ok(Vec::new())
            }
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

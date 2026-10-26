#![doc = include_str!("../README.md")]

//! oxide-fw-cache — L0/L1/L2 tiers, tags, single-flight, handlers (spec §11).
//!
//! - L0: full response (HTML, headers, tags) — Rust, no JS on hit.
//! - L1: data from `cache()` / `'use cache'` (ODF bytes).
//! - L2: outbound fetch memoization within a request + optional cross-request.
//!
//! Semantics (§11.3): TTL, stale-while-revalidate, stale-if-error, single-flight
//! coalescing, soft/hard tag invalidation, distributed invalidation via pub/sub,
//! build seeding, HTTP headers (Cache-Control, ETag, Vary, Last-Modified) + 304.
//!
//! Submodules: [`tiers`] (memory/disk/tiered store, §11.2), [`single_flight`]
//! (coalescing, §11.3), [`http`] (Cache-Control/ETag/304 + fetch memo).

#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod auth;
pub mod codec;
pub mod http;
pub mod http_handler;
pub mod single_flight;
pub mod tiers;

pub use auth::{MAC_LEN, open_entry, seal_entry};
pub use codec::{MAX_ENTRY_BYTES, decode_entry, encode_entry};
pub use http::{
    FetchMemo, cache_control, conditional_response, http_date, memo_key, parse_http_date,
};
pub use http_handler::HttpCacheHandler;
pub use single_flight::{FlightOwnerGuard, FlightSlot, SingleFlight};
pub use tiers::{CacheStats, DiskTier, MemoryTier, TieredCache};

#[cfg(feature = "redis")]
#[cfg_attr(docsrs, doc(cfg(feature = "redis")))]
pub mod redis_handler;
#[cfg(feature = "redis")]
#[cfg_attr(docsrs, doc(cfg(feature = "redis")))]
pub use redis_handler::{RedisCacheHandler, redacted_url};

#[cfg(feature = "s3")]
#[cfg_attr(docsrs, doc(cfg(feature = "s3")))]
pub mod s3_handler;
#[cfg(feature = "s3")]
#[cfg_attr(docsrs, doc(cfg(feature = "s3")))]
pub use s3_handler::S3CacheHandler;

use blake3::Hasher;
use oxide_fw_diagnostics::{Diagnostic, OxideError};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

pub type Result<T> = std::result::Result<T, OxideError>;

/// Build a crate-local error with a stable `CAC-NNN`-style code (spec §19.4).
pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Box::new(Diagnostic::error(code, msg)))
}

/// Cache entry policy (§11.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    /// Time to live (fresh window).
    pub ttl: Duration,
    /// Serve stale and regenerate in background within this window.
    pub swr: Option<Duration>,
    /// Serve stale on upstream error within this window.
    pub stale_if_error: Option<Duration>,
    /// Hard vs soft tag invalidation: soft serves stale then regenerates.
    pub soft_tags: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(0),
            swr: None,
            stale_if_error: None,
            soft_tags: true,
        }
    }
}

/// Stored entry with tags + timing for SWR/ETag handling.
///
/// `stored_at` is unix **milliseconds**; all freshness math in this crate uses ms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub body: Vec<u8>,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub tags: Vec<String>,
    pub policy: Policy,
    pub stored_at: u64,
    /// For conditional requests.
    pub etag: String,
}

impl Entry {
    /// Fresh until `stored_at + ttl` (§11.3). `now` is unix ms.
    pub fn is_fresh(&self, now: u64) -> bool {
        now <= self.fresh_until()
    }

    /// Last unix-ms instant at which this entry is fresh.
    pub fn fresh_until(&self) -> u64 {
        self.stored_at
            .saturating_add(self.policy.ttl.as_millis() as u64)
    }

    /// Last unix-ms instant at which this entry may still be served stale (TTL + SWR).
    /// `stale_if_error` is honored by the caller (render layer), not by this window.
    pub fn stale_until(&self) -> u64 {
        let swr = self.policy.swr.map(|d| d.as_millis() as u64).unwrap_or(0);
        self.fresh_until().saturating_add(swr)
    }
}

/// Key components for L0 (§11.2): route id, params, normalized Vary dimensions, build id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheKey {
    pub layer: Layer,
    pub route: String,
    pub params: String,
    pub vary: String,
    pub build_id: String,
}

impl CacheKey {
    /// blake3 over `layer 0x00 route 0x00 params 0x00 vary 0x00 build_id` → 64-char hex.
    /// The NUL separators make the encoding prefix-free (component boundaries are part
    /// of the digest).
    pub fn canonical(&self) -> String {
        Self::canonical_parts(
            self.layer,
            &self.route,
            &self.params,
            &self.vary,
            &self.build_id,
        )
    }

    /// P-04: same digest as [`CacheKey::canonical`] but from borrowed parts —
    /// no intermediate `String` allocations on the hot key path.
    pub fn canonical_parts(
        layer: Layer,
        route: &str,
        params: &str,
        vary: &str,
        build_id: &str,
    ) -> String {
        let mut h = Hasher::new();
        h.update(layer.as_str().as_bytes());
        h.update(b"\x00");
        h.update(route.as_bytes());
        h.update(b"\x00");
        h.update(params.as_bytes());
        h.update(b"\x00");
        h.update(vary.as_bytes());
        h.update(b"\x00");
        h.update(build_id.as_bytes());
        h.finalize().to_hex().to_string()
    }
}

/// Cache layer (§11.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Layer {
    L0,
    L1,
    L2,
}

impl Layer {
    pub fn as_str(&self) -> &'static str {
        match self {
            Layer::L0 => "L0",
            Layer::L1 => "L1",
            Layer::L2 => "L2",
        }
    }
}

/// Lookup outcome — distinguishes fresh / stale-usable / hit-on-error.
#[derive(Debug, Clone)]
pub enum Hit {
    Fresh(Arc<Entry>),
    Stale(Arc<Entry>),
}

/// CacheHandler trait (§11.2): get, set, delete, tag_keys, invalidate_tag,
/// plus optional pub/sub for cross-node invalidation.
#[async_trait::async_trait]
pub trait CacheHandler: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<Entry>>;
    /// P-03: like [`CacheHandler::get`] but returns the shared `Arc<Entry>` so
    /// a hot L0/L1 hit does not copy the body out of the memory tier. The
    /// default impl delegates to `get` (one copy) for handlers that cannot
    /// hand back a shared entry (L2 drivers deserialize anyway); `TieredCache`
    /// overrides it to stay zero-copy.
    async fn get_arc(&self, key: &str) -> Result<Option<Arc<Entry>>> {
        Ok(self.get(key).await?.map(Arc::new))
    }
    async fn set(&self, key: &str, entry: Entry) -> Result<()>;
    async fn delete(&self, key: &str) -> Result<()>;
    async fn tag_keys(&self, tag: &str) -> Result<Vec<String>>;
    async fn invalidate_tag(&self, tag: &str, soft: bool) -> Result<usize>;
    /// Publish an invalidation to other nodes (no-op for local handlers).
    async fn publish_invalidation(&self, tag: &str, soft: bool) -> Result<()>;
}

/// Build id: hash of the build manifest set — part of L0 keys (§11.2).
/// Length-prefixed parts; first 16 hex chars of the blake3 digest.
pub fn build_id(parts: &[&[u8]]) -> String {
    let mut h = Hasher::new();
    for p in parts {
        h.update(&(p.len() as u32).to_le_bytes());
        h.update(p);
    }
    h.finalize().to_hex().to_string()[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_key_is_deterministic_and_prefix_free() {
        let k = |route: &str, params: &str| CacheKey {
            layer: Layer::L0,
            route: route.into(),
            params: params.into(),
            vary: String::new(),
            build_id: "b".into(),
        };
        let a = k("/r", "p").canonical();
        assert_eq!(a.len(), 64);
        assert_eq!(a, k("/r", "p").canonical());
        assert_ne!(a, k("/r", "q").canonical());
        // NUL separators: ("ab","c") and ("a","bc") must not collide.
        assert_ne!(k("ab", "c").canonical(), k("a", "bc").canonical());
        // Layer participates via as_str().
        let l1 = CacheKey {
            layer: Layer::L1,
            ..k("/r", "p")
        };
        assert_ne!(Layer::L0.as_str(), Layer::L1.as_str());
        assert_ne!(a, l1.canonical());
    }

    #[test]
    fn build_id_is_stable_16_hex_and_length_prefixed() {
        let a = build_id(&[b"x", b"y"]);
        assert_eq!(a.len(), 16);
        assert_eq!(a, build_id(&[b"x", b"y"]));
        assert_ne!(a, build_id(&[b"x", b"z"]));
        assert_ne!(build_id(&[b"ab", b"c"]), build_id(&[b"a", b"bc"]));
    }

    #[test]
    fn entry_freshness_windows() {
        let p = Policy {
            ttl: Duration::from_millis(100),
            swr: Some(Duration::from_millis(50)),
            stale_if_error: None,
            soft_tags: true,
        };
        let e = Entry {
            body: vec![],
            status: 200,
            headers: vec![],
            tags: vec![],
            policy: p,
            stored_at: 1_000,
            etag: String::new(),
        };
        assert!(e.is_fresh(1_000) && e.is_fresh(1_100));
        assert!(!e.is_fresh(1_101));
        assert_eq!(e.fresh_until(), 1_100);
        assert_eq!(e.stale_until(), 1_150);
        // No SWR: stale window equals the fresh window.
        let mut e2 = e.clone();
        e2.policy.swr = None;
        assert_eq!(e2.stale_until(), 1_100);
    }

    #[test]
    fn err_carries_stable_code() {
        let e = err("CAC-IO", "boom");
        assert_eq!(e.0.code, "CAC-IO");
        assert_eq!(e.0.message, "boom");
        let d = err("CAC-DB", "bad").0.with_fix("reopen the store");
        assert_eq!(d.fix.as_deref(), Some("reopen the store"));
    }
}

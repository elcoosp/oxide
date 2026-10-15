//! §11 L0 response cache for the production server.
//!
//! The request pipeline had no response cache: every request re-ran the
//! loader + render even for a `static` route. §11.1 defines L0 as "full
//! response (HTML, headers, tags) — Rust, no JS on hit", and §21 targets
//! "L0 cache hit p99 < 1 ms server time". This module is that layer.
//!
//! ## Why nonce substitution
//!
//! A rendered HTML body embeds a per-request CSP nonce: the boot `<script>`
//! tag and any deferred-boundary swapper carry `nonce="<hash of request id>"`,
//! and the response's `Content-Security-Policy` header carries the same value.
//! If a cached body carried request A's nonce, serving it to request B would
//! block its scripts (the CSP header would name a different nonce).
//!
//! The cache therefore stores bodies with a **sentinel** in place of the
//! nonce, and substitutes the live request's nonce on serve. The header set
//! is stored without `content-security-policy` (the pipeline re-derives it
//! per request in `finish_response`).
//!
//! ## Scope
//!
//! Only responses that are safe to share are cached:
//! - method `GET`, status `200`
//! - the route's `render_mode` is `static` or `isr` (a `dynamic` route may
//!   depend on cookies/headers the key does not capture)
//! - the load set no `set-cookie` (a response that varies per user)
//!
//! A hit is served only while fresh; SWR / stale serving is a follow-up that
//! needs the background-revalidation task from §11.3.

use oxide_cache::{CacheHandler, CacheKey, Entry, Layer, Policy};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The sentinel written into a cached body where the live CSP nonce goes.
pub const NONCE_SENTINEL: &str = "__OXIDE_L0_NONCE__";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The L0 layer: a cache handler plus the per-build key salt and default policy.
pub struct L0Cache {
    handler: Arc<dyn CacheHandler>,
    build_id: String,
    ttl: Duration,
}

impl L0Cache {
    pub fn new(handler: Arc<dyn CacheHandler>, build_id: impl Into<String>, ttl: Duration) -> Self {
        Self {
            handler,
            build_id: build_id.into(),
            ttl,
        }
    }

    /// Build the canonical key for a route + params + Vary dimensions (§11.2).
    fn key(&self, route: &str, params: &str, vary: &str) -> String {
        CacheKey {
            layer: Layer::L0,
            route: route.to_string(),
            params: params.to_string(),
            vary: vary.to_string(),
            build_id: self.build_id.clone(),
        }
        .canonical()
    }

    /// Look up a fresh cached response for `route`. Returns `None` on a miss
    /// or a stale entry (stale-serving is a follow-up).
    pub async fn get(
        &self,
        route: &str,
        params: &str,
        vary: &str,
    ) -> Option<CachedResponse> {
        let key = self.key(route, params, vary);
        let entry = self.handler.get(&key).await.ok()??;
        if !entry.is_fresh(now_ms()) {
            return None;
        }
        Some(CachedResponse {
            status: entry.status,
            headers: entry
                .headers
                .into_iter()
                .filter(|(k, _)| !k.eq_ignore_ascii_case("content-security-policy"))
                .collect(),
            body: entry.body,
        })
    }

    /// Store a response. The caller passes the body with [`NONCE_SENTINEL`] in
    /// place of the live nonce, and a header set without the CSP header.
    pub async fn put(
        &self,
        route: &str,
        params: &str,
        vary: &str,
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        tags: Vec<String>,
    ) {
        let key = self.key(route, params, vary);
        let etag = {
            let mut h = blake3::Hasher::new();
            h.update(&body);
            h.finalize().to_hex()[..16].to_string()
        };
        let entry = Entry {
            body,
            status,
            headers: headers
                .into_iter()
                .filter(|(k, _)| !k.eq_ignore_ascii_case("content-security-policy"))
                .collect(),
            tags,
            policy: Policy {
                ttl: self.ttl,
                swr: None,
                stale_if_error: None,
                soft_tags: true,
            },
            stored_at: now_ms(),
            etag,
        };
        let _ = self.handler.set(&key, entry).await;
    }

    /// Invalidate one route's cached responses by tag (§11.3).
    pub async fn invalidate_tag(&self, tag: &str, soft: bool) -> usize {
        self.handler
            .invalidate_tag(tag, soft)
            .await
            .unwrap_or(0)
    }

    /// `revalidate.path('/blog/a')` — same tag convention the cache tier
    /// uses (`path:<path>`), so entries tagged at put time with
    /// `path:<req_path>` are invalidated. Delegates through the
    /// `CacheHandler` so the L1/handler path is honoured too (BUG-01).
    pub async fn invalidate_path(&self, path: &str, soft: bool) -> usize {
        self.handler
            .invalidate_tag(&oxide_cache::tiers::path_tag(path), soft)
            .await
            .unwrap_or(0)
    }
}

/// A cached response ready to serve, with the nonce still a sentinel.
#[derive(Debug, Clone)]
pub struct CachedResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl CachedResponse {
    /// Replace the nonce sentinel with `nonce` and build the final body.
    pub fn materialize(&self, nonce: &str) -> Vec<u8> {
        if !self.body.windows(NONCE_SENTINEL.len()).any(|w| w == NONCE_SENTINEL.as_bytes()) {
            return self.body.clone();
        }
        String::from_utf8_lossy(&self.body)
            .replace(NONCE_SENTINEL, nonce)
            .into_bytes()
    }

    /// True when the body still carries the sentinel (i.e. it was stored with
    /// a substitutable nonce).
    pub fn has_sentinel(&self) -> bool {
        self.body.windows(NONCE_SENTINEL.len()).any(|w| w == NONCE_SENTINEL.as_bytes())
    }
}

/// A Vary fingerprint: the normalized dimensions a cached response depends on
/// (§11.2). Today: the `Accept-Language` primary tag (i18n) and nothing else.
pub fn vary_of(headers: &[(String, String)]) -> String {
    let lang = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("accept-language"))
        .map(|(_, v)| {
            v.split(',')
                .next()
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    format!("lang={lang}")
}

/// True when a route's inferred/explicit mode is safe to cache at L0.
pub fn mode_is_cacheable(mode: Option<&str>) -> bool {
    matches!(mode, Some("static") | Some("isr"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_cache::TieredCache;

    fn cache(ttl_secs: u64) -> L0Cache {
        let tier = Arc::new(TieredCache::new(16 * 1024 * 1024));
        L0Cache::new(tier, "build-abc", Duration::from_secs(ttl_secs))
    }

    #[tokio::test]
    async fn put_then_get_roundtrips() {
        let c = cache(60);
        c.put(
            "routes/index",
            "{}",
            "lang=",
            200,
            vec![("content-type".into(), "text/html".into())],
            b"<html>body</html>".to_vec(),
            vec![],
        )
        .await;
        let got = c.get("routes/index", "{}", "lang=").await.expect("hit");
        assert_eq!(got.status, 200);
        assert_eq!(got.body, b"<html>body</html>");
        assert_eq!(got.headers.len(), 1);
    }

    #[tokio::test]
    async fn miss_on_unknown_key() {
        let c = cache(60);
        assert!(c.get("routes/x", "{}", "lang=").await.is_none());
    }

    #[tokio::test]
    async fn different_params_are_different_entries() {
        let c = cache(60);
        c.put("routes/blog/[slug]", "{\"slug\":\"a\"}", "lang=", 200, vec![], b"A".to_vec(), vec![]).await;
        c.put("routes/blog/[slug]", "{\"slug\":\"b\"}", "lang=", 200, vec![], b"B".to_vec(), vec![]).await;
        assert_eq!(c.get("routes/blog/[slug]", "{\"slug\":\"a\"}", "lang=").await.unwrap().body, b"A");
        assert_eq!(c.get("routes/blog/[slug]", "{\"slug\":\"b\"}", "lang=").await.unwrap().body, b"B");
    }

    #[tokio::test]
    async fn csp_header_is_never_stored() {
        let c = cache(60);
        c.put(
            "r",
            "",
            "",
            200,
            vec![
                ("content-type".into(), "text/html".into()),
                ("content-security-policy".into(), "script-src nonce-XYZ".into()),
            ],
            b"x".to_vec(),
            vec![],
        )
        .await;
        let got = c.get("r", "", "").await.unwrap();
        assert!(
            !got.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-security-policy")),
            "the CSP header must be re-derived per request, never cached"
        );
    }

    #[tokio::test]
    async fn nonce_sentinel_is_substituted_on_materialize() {
        let c = cache(60);
        let body = format!("<script nonce=\"{NONCE_SENTINEL}\">x</script>");
        c.put("r", "", "", 200, vec![], body.clone().into_bytes(), vec![]).await;
        let got = c.get("r", "", "").await.unwrap();
        assert!(got.has_sentinel());
        let served = String::from_utf8(got.materialize("REALNONCE")).unwrap();
        assert!(served.contains("nonce=\"REALNONCE\""));
        assert!(!served.contains(NONCE_SENTINEL));
    }

    #[tokio::test]
    async fn stale_entry_is_a_miss() {
        // An entry past its TTL is not served. TTL 0 means `fresh_until ==
        // stored_at`, and `is_fresh` is `now <= stored_at` (the cache crate
        // pins this boundary), so a same-millisecond read is still fresh —
        // the test stores an entry that is unambiguously in the past.
        use oxide_cache::{CacheHandler as _, Entry, Policy};
        let tier = Arc::new(TieredCache::new(1 << 20));
        let c = L0Cache::new(tier.clone(), "build-x", Duration::from_secs(60));
        let key = {
            use oxide_cache::{CacheKey, Layer};
            CacheKey {
                layer: Layer::L0,
                route: "r".into(),
                params: String::new(),
                vary: String::new(),
                build_id: "build-x".into(),
            }
            .canonical()
        };
        let past = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - 10_000;
        tier.set(
            &key,
            Entry {
                body: b"x".to_vec(),
                status: 200,
                headers: vec![],
                tags: vec![],
                policy: Policy {
                    ttl: Duration::from_secs(1),
                    swr: None,
                    stale_if_error: None,
                    soft_tags: true,
                },
                stored_at: past,
                etag: String::new(),
            },
        )
        .await
        .unwrap();
        assert!(
            c.get("r", "", "").await.is_none(),
            "an entry whose TTL has elapsed is not served"
        );
    }

    #[tokio::test]
    async fn build_id_participates_in_the_key() {
        let tier: Arc<dyn CacheHandler> = Arc::new(TieredCache::new(1 << 20));
        let a = L0Cache::new(Arc::clone(&tier), "build-A", Duration::from_secs(60));
        let b = L0Cache::new(Arc::clone(&tier), "build-B", Duration::from_secs(60));
        a.put("r", "", "", 200, vec![], b"A".to_vec(), vec![]).await;
        // A different build id must not see build-A's entry.
        assert!(b.get("r", "", "").await.is_none());
    }

    #[test]
    fn vary_tracks_accept_language() {
        let none = vary_of(&[]);
        let en = vary_of(&[("accept-language".into(), "en-US,en;q=0.9".into())]);
        let fr = vary_of(&[("accept-language".into(), "fr".into())]);
        assert_ne!(en, fr);
        assert_eq!(none, "lang=");
        assert_eq!(en, "lang=en-us");
    }

    #[test]
    fn only_static_and_isr_are_cacheable() {
        assert!(mode_is_cacheable(Some("static")));
        assert!(mode_is_cacheable(Some("isr")));
        assert!(!mode_is_cacheable(Some("dynamic")));
        assert!(!mode_is_cacheable(Some("spa")));
        assert!(!mode_is_cacheable(None));
    }
}

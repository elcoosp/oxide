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
    /// Look up a cached response. PERF-04: returns a fresh hit, or a **stale**
    /// hit within the SWR window (the caller serves it and revalidates in the
    /// background). Past the SWR window the entry is a miss. The ETag is
    /// surfaced so the caller can answer a conditional request with 304.
    pub async fn get(&self, route: &str, params: &str, vary: &str) -> Option<CachedResponse> {
        let key = self.key(route, params, vary);
        let entry = self.handler.get(&key).await.ok()??;
        let now = now_ms();
        if now > entry.stale_until() {
            return None; // beyond TTL + SWR
        }
        let stale = !entry.is_fresh(now);
        Some(CachedResponse {
            status: entry.status,
            headers: entry
                .headers
                .into_iter()
                .filter(|(k, _)| !k.eq_ignore_ascii_case("content-security-policy"))
                .collect(),
            body: entry.body,
            etag: entry.etag,
            stale,
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
        let entry = self.make_entry(status, headers, body, tags);
        let _ = self.handler.set(&key, entry).await;
    }

    /// PERF-04: build the cache `Entry` without storing it. The single-flight
    /// owner builds it once, stores it, and broadcasts the same `Arc` to
    /// followers.
    pub fn make_entry(
        &self,
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        tags: Vec<String>,
    ) -> Entry {
        let etag = {
            let mut h = blake3::Hasher::new();
            h.update(&body);
            h.finalize().to_hex()[..16].to_string()
        };
        Entry {
            body,
            status,
            headers: headers
                .into_iter()
                .filter(|(k, _)| !k.eq_ignore_ascii_case("content-security-policy"))
                .collect(),
            tags,
            policy: Policy {
                ttl: self.ttl,
                swr: Some(
                    self.ttl
                        .saturating_mul(5)
                        .max(std::time::Duration::from_secs(60)),
                ),
                stale_if_error: None,
                soft_tags: true,
            },
            stored_at: now_ms(),
            etag,
        }
    }

    /// PERF-04: store a pre-built entry under its canonical key.
    pub async fn store_entry(&self, key: &str, entry: Entry) {
        let _ = self.handler.set(key, entry).await;
    }

    /// PERF-04: the canonical L0 key for a (route, params, vary) triple — the
    /// single-flight key must match it exactly.
    pub fn key_of(&self, route: &str, params: &str, vary: &str) -> String {
        self.key(route, params, vary)
    }

    /// PERF-04: the build id this cache scopes keys to.
    pub fn build_id(&self) -> &str {
        &self.build_id
    }

    /// PERF-04: seed an entry at an explicit key. Used by boot seeding,
    /// where the key was computed at build time. The body already carries the
    /// nonce sentinel. §10.2 step 5: `status` carries the adapter's
    /// error-status marker through the seed so a prerendered error page is
    /// served as a real 500 (was always 200).
    pub async fn seed_entry(&self, key: &str, body: Vec<u8>, status: u16) {
        let entry = self.make_entry(
            status,
            vec![(
                "content-type".to_string(),
                "text/html; charset=utf-8".to_string(),
            )],
            body,
            vec![],
        );
        let _ = self.handler.set(key, entry).await;
    }

    /// PERF-04: build the canonical L0 key without an `L0Cache` instance
    /// (the dispatch needs the single-flight key before it has the cache).
    pub fn key_for(route: &str, params: &str, vary: &str, build_id: &str) -> String {
        CacheKey {
            layer: Layer::L0,
            route: route.to_string(),
            params: params.to_string(),
            vary: vary.to_string(),
            build_id: build_id.to_string(),
        }
        .canonical()
    }

    /// Invalidate one route's cached responses by tag (§11.3).
    pub async fn invalidate_tag(&self, tag: &str, soft: bool) -> usize {
        self.handler.invalidate_tag(tag, soft).await.unwrap_or(0)
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
    /// PERF-04: the stored ETag, for a conditional-request 304.
    pub etag: String,
    /// PERF-04: true when served within the SWR window past the TTL — the
    /// caller should trigger a background revalidation.
    pub stale: bool,
}

impl CachedResponse {
    /// Replace the nonce sentinel with `nonce` and build the final body.
    pub fn materialize(&self, nonce: &str) -> Vec<u8> {
        if !self
            .body
            .windows(NONCE_SENTINEL.len())
            .any(|w| w == NONCE_SENTINEL.as_bytes())
        {
            return self.body.clone();
        }
        String::from_utf8_lossy(&self.body)
            .replace(NONCE_SENTINEL, nonce)
            .into_bytes()
    }

    /// True when the body still carries the sentinel (i.e. it was stored with
    /// a substitutable nonce).
    pub fn has_sentinel(&self) -> bool {
        self.body
            .windows(NONCE_SENTINEL.len())
            .any(|w| w == NONCE_SENTINEL.as_bytes())
    }
}

/// A Vary fingerprint: the normalized dimensions a cached response depends on
/// (§11.2 / §14.1 `cache.l0.vary`).
///
/// `vary_headers` names the request headers the key is fingerprinted by
/// (case-insensitive). The fingerprint is a stable `name=value;name=value`
/// string in the configured order — a header absent from the request
/// contributes `name=` so a missing header is distinct from an empty one.
///
/// `accept-language` is always fingerprinted by its primary tag (`en-US,en;q=0.9`
/// → `en-us`), because that is what the i18n negotiation actually keys on.
/// Any other header is fingerprinted by its raw trimmed value.
pub fn vary_of(headers: &[(String, String)], vary_headers: &[String]) -> String {
    if vary_headers.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::with_capacity(vary_headers.len());
    for name in vary_headers {
        let lower = name.to_ascii_lowercase();
        let raw = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(&lower))
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        let value = if lower == "accept-language" {
            // Primary language tag: the first comma-separated entry, stripped
            // of its q-weight, lowercased.
            raw.split(',')
                .next()
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        } else {
            raw.trim().to_string()
        };
        parts.push(format!("{lower}={value}"));
    }
    parts.join(";")
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
        c.put(
            "routes/blog/[slug]",
            "{\"slug\":\"a\"}",
            "lang=",
            200,
            vec![],
            b"A".to_vec(),
            vec![],
        )
        .await;
        c.put(
            "routes/blog/[slug]",
            "{\"slug\":\"b\"}",
            "lang=",
            200,
            vec![],
            b"B".to_vec(),
            vec![],
        )
        .await;
        assert_eq!(
            c.get("routes/blog/[slug]", "{\"slug\":\"a\"}", "lang=")
                .await
                .unwrap()
                .body,
            b"A"
        );
        assert_eq!(
            c.get("routes/blog/[slug]", "{\"slug\":\"b\"}", "lang=")
                .await
                .unwrap()
                .body,
            b"B"
        );
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
                (
                    "content-security-policy".into(),
                    "script-src nonce-XYZ".into(),
                ),
            ],
            b"x".to_vec(),
            vec![],
        )
        .await;
        let got = c.get("r", "", "").await.unwrap();
        assert!(
            !got.headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-security-policy")),
            "the CSP header must be re-derived per request, never cached"
        );
    }

    #[tokio::test]
    async fn nonce_sentinel_is_substituted_on_materialize() {
        let c = cache(60);
        let body = format!("<script nonce=\"{NONCE_SENTINEL}\">x</script>");
        c.put("r", "", "", 200, vec![], body.clone().into_bytes(), vec![])
            .await;
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

    /// PERF-04: an entry past its TTL but within the SWR window is served as
    /// a *stale* hit; past the window it is a miss.
    #[tokio::test]
    async fn swr_window_serves_stale_then_expires() {
        use oxide_cache::{CacheHandler as _, CacheKey, Entry, Layer, Policy};
        let tier = Arc::new(TieredCache::new(1 << 20));
        let c = L0Cache::new(tier.clone(), "b", Duration::from_secs(1));
        let key = CacheKey {
            layer: Layer::L0,
            route: "r".into(),
            params: String::new(),
            vary: String::new(),
            build_id: "b".into(),
        }
        .canonical();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        // Stored 5 s ago, TTL 1 s, SWR 3600 s → stale but servable.
        tier.set(
            &key,
            Entry {
                body: b"BODY".to_vec(),
                status: 200,
                headers: vec![],
                tags: vec![],
                policy: Policy {
                    ttl: Duration::from_secs(1),
                    swr: Some(Duration::from_secs(3600)),
                    stale_if_error: None,
                    soft_tags: true,
                },
                stored_at: now.saturating_sub(5_000),
                etag: "abc".into(),
            },
        )
        .await
        .unwrap();
        let hit = c.get("r", "", "").await.expect("stale hit within SWR");
        assert!(hit.stale, "flagged stale");
        assert_eq!(hit.body, b"BODY");
        assert_eq!(hit.etag, "abc");

        // Stored 10 000 s ago → past TTL + SWR → miss.
        tier.set(
            &key,
            Entry {
                body: b"OLD".to_vec(),
                status: 200,
                headers: vec![],
                tags: vec![],
                policy: Policy {
                    ttl: Duration::from_secs(1),
                    swr: Some(Duration::from_secs(60)),
                    stale_if_error: None,
                    soft_tags: true,
                },
                stored_at: now.saturating_sub(10_000_000),
                etag: String::new(),
            },
        )
        .await
        .unwrap();
        assert!(c.get("r", "", "").await.is_none(), "expired");
    }

    /// PERF-04: a fresh put exposes a non-empty ETag for a 304.
    #[tokio::test]
    async fn fresh_hit_exposes_etag() {
        let c = cache(60);
        c.put("r", "", "", 200, vec![], b"BODY".to_vec(), vec![])
            .await;
        let hit = c.get("r", "", "").await.unwrap();
        assert!(!hit.etag.is_empty(), "etag present");
        assert!(!hit.stale, "fresh");
    }

    #[test]
fn vary_tracks_the_configured_headers() {
    let lang = vec!["accept-language".to_string()];
    // No header -> empty value, but the NAME is still present so a missing
    // header is distinct from an empty configured list.
    assert_eq!(vary_of(&[], &lang), "accept-language=");
    // The primary language tag is normalized (first entry, q-weight stripped,
    // lowercased) — what i18n negotiation actually keys on.
    assert_eq!(
        vary_of(&[("accept-language".into(), "en-US,en;q=0.9".into())], &lang),
        "accept-language=en-us"
    );
    assert_eq!(
        vary_of(&[("Accept-Language".into(), "FR".into())], &lang),
        "accept-language=fr"
    );
    // An empty configured list disables header variance entirely.
    assert_eq!(vary_of(&[("accept-language".into(), "fr".into())], &[]), "");
}

#[test]
fn vary_covers_arbitrary_configured_headers() {
    // A non-language header is fingerprinted by its raw trimmed value.
    let cfg = vec!["accept".to_string(), "x-tenant".to_string()];
    let got = vary_of(
        &[
            ("Accept".into(), "application/odf".into()),
            ("X-Tenant".into(), "acme".into()),
        ],
        &cfg,
    );
    assert_eq!(got, "accept=application/odf;x-tenant=acme");
    // A header absent from the request contributes an empty value.
    assert_eq!(
        vary_of(&[("accept".into(), "application/odf".into())], &cfg),
        "accept=application/odf;x-tenant="
    );
}

    // ---------------------------------------------------------- BUG-01
    // The revalidate path: an entry tagged `path:<path>` must be
    // invalidated by `invalidate_path(<path>)`, and an entry tagged with the
    // route id must be invalidated by `invalidate_tag(<route>)`.

    #[tokio::test]
    async fn invalidate_path_removes_a_path_tagged_entry() {
        let c = cache(60);
        c.put(
            "routes/index",
            "",
            "",
            200,
            vec![],
            b"HOME".to_vec(),
            vec!["routes/index".into(), "path:/".into()],
        )
        .await;
        assert!(c.get("routes/index", "", "").await.is_some(), "cached");

        let n = c.invalidate_path("/", true).await;
        assert!(
            n >= 1,
            "invalidate_path must match the path:-tagged entry, got {n}"
        );
        assert!(
            c.get("routes/index", "", "").await.is_none(),
            "the entry is gone after invalidate_path"
        );
    }

    #[tokio::test]
    async fn invalidate_tag_removes_a_route_tagged_entry() {
        let c = cache(60);
        c.put(
            "routes/index",
            "",
            "",
            200,
            vec![],
            b"HOME".to_vec(),
            vec!["routes/index".into(), "path:/".into()],
        )
        .await;
        let n = c.invalidate_tag("routes/index", true).await;
        assert!(
            n >= 1,
            "invalidate_tag must match the route-tagged entry, got {n}"
        );
        assert!(c.get("routes/index", "", "").await.is_none());
    }

    #[tokio::test]
    async fn l0_and_a_second_consumer_share_one_handler() {
        // BUG-02: the L0 response cache and the worker's `'use cache'`
        // entries must live on the *same* handler, or invalidation on one
        // cannot reach the other. Two L0 caches over one TieredCache stand
        // in for "the L0 cache and the worker tier".
        let tier = Arc::new(TieredCache::new(1 << 20));
        let l0 = L0Cache::new(
            Arc::clone(&tier) as Arc<dyn CacheHandler>,
            "b",
            Duration::from_secs(60),
        );
        // A second writer on the same handler (the worker's cache_set).
        tier.set(
            "worker-entry",
            oxide_cache::Entry {
                body: b"W".to_vec(),
                status: 200,
                headers: vec![],
                tags: vec!["path:/worker".into()],
                policy: oxide_cache::Policy::default(),
                stored_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64,
                etag: String::new(),
            },
        )
        .await
        .unwrap();
        // Invalidating through the L0 cache's handler reaches the worker's
        // entry: they are one tier.
        let n = l0.invalidate_path("/worker", true).await;
        assert!(n >= 1, "invalidation reaches the shared tier, got {n}");
        assert!(tier.get("worker-entry").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn invalidate_path_does_not_match_an_untagged_path() {
        // Before BUG-01 the put site tagged with the route id only; a path
        // event then matched nothing. This pins that an entry *without* the
        // path tag is not swept by an unrelated invalidate_path.
        let c = cache(60);
        c.put(
            "routes/index",
            "",
            "",
            200,
            vec![],
            b"H".to_vec(),
            vec!["routes/index".into()],
        )
        .await;
        let _ = c.invalidate_path("/other", true).await;
        assert!(
            c.get("routes/index", "", "").await.is_some(),
            "unrelated path must not sweep"
        );
    }
}

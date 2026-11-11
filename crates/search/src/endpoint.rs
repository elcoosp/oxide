//! The serve side's L1 query cache (§V2-F09: "results cached in L1 for
//! 60 s keyed by the normalized query").
//!
//! Pure and testable: the TTL math runs off an injectable clock (the
//! conventions' deterministic-tests rule), the cache keys on the
//! normalized query (trimmed, lowercased, whitespace-collapsed), the cap
//! evicts the least-recently-used entry, and a failed computation is
//! never cached (a failing query retries on the next request).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oxide_fw_diagnostics::OxideError;

use crate::query::SearchResponse;

/// The spec's L1 TTL: 60 seconds.
pub const QUERY_CACHE_TTL: Duration = Duration::from_secs(60);

/// The L1 entry cap: 256 normalized queries bound the cache's memory
/// (a hit response is a handful of small strings).
pub const QUERY_CACHE_CAP: usize = 256;

/// The injectable clock (tests advance a fake; production reads the
/// wall clock).
type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

struct CachedEntry {
    response: SearchResponse,
    stored: Instant,
    tick: u64,
}

/// The L1 cache. `get_or_insert` is the one entry point: a fresh query
/// computes and caches, a cached-and-live query clones, an expired or
/// evicted query recomputes.
pub struct QueryCache {
    ttl: Duration,
    cap: usize,
    now: Clock,
    entries: Mutex<CacheState>,
}

struct CacheState {
    map: BTreeMap<String, CachedEntry>,
    tick: u64,
}

impl QueryCache {
    /// The spec's cache: 60 s TTL, 256 entries, wall clock.
    #[must_use]
    pub fn new() -> Self {
        Self::with_clock(QUERY_CACHE_TTL, QUERY_CACHE_CAP, Arc::new(Instant::now))
    }

    /// An explicit cache (tests inject the clock; hosts tune the knobs).
    #[must_use]
    pub fn with_clock(ttl: Duration, cap: usize, now: Clock) -> Self {
        Self {
            ttl,
            cap,
            now,
            entries: Mutex::new(CacheState {
                map: BTreeMap::new(),
                tick: 0,
            }),
        }
    }

    /// The cache key: the normalized query (trimmed, lowercased,
    /// whitespace collapsed — `Rust  BOOK` and `rust book` share an
    /// entry).
    #[must_use]
    pub fn key_for(&self, raw: &str) -> String {
        let mut key = String::with_capacity(raw.len());
        let mut gap = false;
        for c in raw.trim().chars() {
            if c.is_whitespace() {
                gap = true;
            } else {
                if gap && !key.is_empty() {
                    key.push(' ');
                }
                gap = false;
                key.extend(c.to_lowercase());
            }
        }
        key
    }

    /// Answer from the cache or compute. The closure runs only on a
    /// miss; its `Ok` response is cached (a clone is returned both ways)
    /// and its `Err` is passed through uncached.
    ///
    /// # Errors
    ///
    /// Propagates the closure's error, uncached (a failing query must
    /// retry, not serve its failure for 60 seconds).
    pub fn get_or_insert(
        &self,
        key: &str,
        compute: impl FnOnce() -> Result<SearchResponse, OxideError>,
    ) -> Result<SearchResponse, OxideError> {
        let now = (self.now)();
        {
            let mut state = self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let live = match state.map.get(key) {
                Some(entry) if now.duration_since(entry.stored) < self.ttl => {
                    Some(entry.response.clone())
                }
                _ => None,
            };
            if let Some(response) = live {
                state.tick += 1;
                let tick = state.tick;
                if let Some(entry) = state.map.get_mut(key) {
                    entry.tick = tick;
                }
                return Ok(response);
            }
        }
        let computed = compute()?;
        let mut state = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.tick += 1;
        let tick = state.tick;
        state.map.insert(
            key.to_string(),
            CachedEntry {
                response: computed.clone(),
                stored: (self.now)(),
                tick,
            },
        );
        if state.map.len() > self.cap {
            let victim = state
                .map
                .iter()
                .min_by_key(|(_, e)| e.tick)
                .map(|(k, _)| k.clone());
            if let Some(victim) = victim {
                state.map.remove(&victim);
            }
        }
        Ok(computed)
    }
}

impl Default for QueryCache {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for QueryCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let live = self.entries.lock().map(|s| s.map.len()).unwrap_or_default();
        f.debug_struct("QueryCache")
            .field("ttl", &self.ttl)
            .field("cap", &self.cap)
            .field("entries", &live)
            .finish_non_exhaustive()
    }
}

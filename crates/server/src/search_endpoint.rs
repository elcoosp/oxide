//! The `/_oxide/search` endpoint surface (V2-F09, feature `search`).
//!
//! The engine lives in `oxide-fw-search`; this module owns the endpoint
//! contract around it — the same split as `navgraph` (engine data) and
//! the nav arm (wire), and `csp_report_response` (ops-plane plumbing):
//!
//! - [`SearchEndpoint`]: the attached index behind the pipeline's
//!   `Option` — the engine seam ([`oxide_fw_search::SearchIndex`]) plus
//!   the L1 query cache (60 s, keyed by the normalized query + filters).
//! - [`search_response`]: the pure renderer — parse `?q=&limit=&tag=&lang=`,
//!   refuse an empty query with the documented 400, answer
//!   `{"total": …, "hits": [{"id", "url", "title", "snippet", "score"}]}`.
//!
//! The pipeline's native prefix owns the rest of the contract: the
//! not-configured 404 (the image/nav posture — off is a documented
//! shape), the non-GET 405 with `allow: GET`, and the rate limit
//! ([`SEARCH_RATE_MAX`] requests per [`SEARCH_RATE_WINDOW_SECS`] per
//! trusted-proxy-aware peer — the spec's `20 per 10 s`).

use std::sync::Arc;

use bytes::Bytes;
use oxide_fw_diagnostics::OxideError;
use oxide_fw_search::{SearchIndex, SearchQuery, SearchResponse};

use crate::{OxideResponse, percent_decode};

/// The rate limit's per-peer budget (§V2-F09: "Rate limited per IP
/// (default 20 per 10 s)").
pub const SEARCH_RATE_MAX: u32 = 20;

/// The rate limit's window, seconds.
pub const SEARCH_RATE_WINDOW_SECS: u64 = 10;

/// The wire default for `?limit=` (the spec's `limit=10` example).
pub const DEFAULT_RESULT_LIMIT: usize = 10;

/// The wire cap for `?limit=` — one query cannot serialize the whole
/// corpus (the `/_oxide/nav` endpoint caps the same way).
pub const MAX_RESULT_LIMIT: usize = 100;

/// The endpoint's attached state: the read-only engine plus its L1. The
/// cache key covers the filters too — `?tag=` narrows the same text
/// query, and a tag answer must never be served to an unfiltered one.
pub struct SearchEndpoint {
    index: Arc<dyn SearchIndex>,
    cache: oxide_fw_search::QueryCache,
}

impl SearchEndpoint {
    /// Attach an engine (the serve side opens the index read-only and
    /// hands it here; the cache is the spec's 60 s default).
    #[must_use]
    pub fn new(index: Arc<dyn SearchIndex>) -> Self {
        Self {
            index,
            cache: oxide_fw_search::QueryCache::new(),
        }
    }

    /// The attached engine's document count (the readiness probe).
    #[must_use]
    pub fn doc_count(&self) -> u32 {
        self.index.doc_count()
    }

    /// The cache key: the normalized query text plus the two filters.
    fn cache_key(&self, q: &SearchQuery) -> String {
        format!(
            "{}\u{1}{}\u{1}{}",
            self.cache.key_for(&q.text),
            q.tag.as_deref().unwrap_or(""),
            q.lang.as_deref().unwrap_or(""),
        )
    }

    /// Run one parsed query through the L1 (a miss computes against the
    /// engine; a hit clones the cached response).
    ///
    /// # Errors
    ///
    /// Propagates the engine's coded error, uncached.
    pub fn run(&self, q: &SearchQuery) -> Result<SearchResponse, OxideError> {
        let key = self.cache_key(q);
        self.cache.get_or_insert(&key, || self.index.search(q))
    }
}

impl std::fmt::Debug for SearchEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchEndpoint")
            .field("doc_count", &self.index.doc_count())
            .finish_non_exhaustive()
    }
}

/// One percent-decoded query-string parameter (`q`, `limit`, `tag`,
/// `lang` — the renderer's own parse, the nav renderer's shape).
fn query_param(query: &str, name: &str) -> Option<String> {
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
        .map(percent_decode)
}

/// The pure `GET /_oxide/search` renderer: parse the params, run the
/// query through the endpoint's L1, emit the JSON body.
///
/// Contracts: a missing/empty `q` is a 400 (`SRC-104`'s endpoint shape);
/// an engine `SRC-104` is a 400; any other engine code is a 500 naming
/// the code; a `200` carries
/// `{"total": …, "hits": [{id, url, title, snippet, score}]}`.
#[must_use]
pub fn search_response(ep: &SearchEndpoint, query: &str) -> OxideResponse {
    let q = query_param(query, "q").unwrap_or_default();
    let limit = query_param(query, "limit")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|l| *l > 0)
        .map(|l| l.min(MAX_RESULT_LIMIT))
        .unwrap_or(DEFAULT_RESULT_LIMIT);
    let parsed = SearchQuery {
        text: q.clone(),
        limit,
        tag: query_param(query, "tag").filter(|t| !t.trim().is_empty()),
        lang: query_param(query, "lang").filter(|l| !l.trim().is_empty()),
    };
    if parsed.text.trim().is_empty() {
        return search_error_response(
            400,
            "missing or empty `q`: the search endpoint is `GET /_oxide/search?q=<text>`",
        );
    }
    match ep.run(&parsed) {
        Ok(resp) => {
            let body = serde_json::to_value(&resp).unwrap_or(serde_json::Value::Null);
            OxideResponse::json(200, body)
        }
        Err(e) => {
            if e.0.code == "SRC-104" {
                search_error_response(400, &e.0.message)
            } else {
                tracing::error!(code = e.0.code, message = %e.0.message, "search query failed");
                search_error_response(500, &format!("{} {}", e.0.code, e.0.message))
            }
        }
    }
}

/// The endpoint's error spelling: a plain-text body with the status (the
/// nav/csp-report rejection shape — a search UI does not render an
/// OxideError diagnostic envelope).
fn search_error_response(status: u16, msg: &str) -> OxideResponse {
    OxideResponse::new(status, Bytes::from(msg.to_string()))
        .with_header("content-type", "text/plain; charset=utf-8")
}

//! The engine's data contracts: the document, the query, the response
//! and the `SearchIndex` seam (§V2-F09).
//!
//! The trait is the object-safe seam the server's `/_oxide/search`
//! endpoint holds behind an `Arc`. Two implementations ship here:
//! [`TantivyIndex`](crate::build::TantivyIndex) (the Full engine — the
//! Binary/Docker matrix cells) and
//! [`StaticMatcher`](crate::static_index::StaticMatcher) (the Degraded
//! static-JSON fallback — the Node/serverless/static cells).

use oxide_fw_config::SearchConfig;
use oxide_fw_diagnostics::OxideError;
use serde::{Deserialize, Serialize};

use crate::{
    DEFAULT_BODY_BOOST, DEFAULT_RESULT_LIMIT, DEFAULT_SNIPPET_LEN, DEFAULT_TITLE_BOOST, err,
};

/// One indexed entry (§V2-F09 build: "typed frontmatter and body text
/// with Markdown stripped"). The build maps a content-collection entry
/// to this shape; the engine never sees Markdown, frontmatter or a
/// filesystem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct SearchDocument {
    /// The stable identity (`posts/a`) — the content collection's join
    /// key. Required non-empty.
    pub id: String,
    /// The page URL the hit links to (`/posts/a`). Required non-empty.
    pub url: String,
    /// The entry title (may be empty; the engine indexes what it gets).
    pub title: String,
    /// The entry body (Markdown already stripped).
    pub body: String,
    /// The entry tags (the `tag=<t>` filter's values).
    pub tags: Vec<String>,
    /// The entry language (the `lang=<l>` filter's value; the build
    /// defaults to the first `search.languages` entry or `"en"`).
    pub lang: String,
}

/// The engine's query-time knobs, threaded from the typed
/// `search` config (`SearchConfig`: field boosts, snippet length,
/// languages — Appendix V2.L). `None` config values keep the spec's
/// documented defaults (title 3, body 1, snippet 160, `["en"]`).
#[derive(Debug, Clone, PartialEq)]
pub struct SearchEngineOptions {
    /// Query-time boost for `title` matches.
    pub title_boost: f32,
    /// Query-time boost for `body` matches.
    pub body_boost: f32,
    /// Snippet length in characters.
    pub snippet_len: usize,
    /// Index languages (threaded from the typed config; the analyzer
    /// set ships the English stemmer — see the crate doc's delta).
    pub languages: Vec<String>,
}

impl Default for SearchEngineOptions {
    fn default() -> Self {
        Self {
            title_boost: DEFAULT_TITLE_BOOST,
            body_boost: DEFAULT_BODY_BOOST,
            snippet_len: DEFAULT_SNIPPET_LEN,
            languages: vec!["en".to_string()],
        }
    }
}

impl SearchEngineOptions {
    /// Thread the typed `search` config into the engine's knobs: `None`
    /// values keep the spec defaults, present values override them.
    #[must_use]
    pub fn from_config(cfg: &SearchConfig) -> Self {
        Self {
            title_boost: cfg.fields.title.unwrap_or(DEFAULT_TITLE_BOOST),
            body_boost: cfg.fields.body.unwrap_or(DEFAULT_BODY_BOOST),
            snippet_len: cfg
                .snippet
                .and_then(|s| usize::try_from(s).ok())
                .unwrap_or(DEFAULT_SNIPPET_LEN),
            languages: if cfg.languages.is_empty() {
                vec!["en".to_string()]
            } else {
                cfg.languages.clone()
            },
        }
    }
}

/// One query: the text, the cap, and the two optional filters. `limit`
/// defaults to the spec's 10 (the hand-written `Default` impl, not a
/// derive — the default carries the spec's constant).
#[derive(Debug, Clone, PartialEq)]
pub struct SearchQuery {
    /// The query text (empty is refused with `SRC-104`).
    pub text: String,
    /// The result cap (the endpoint clamps its own wire default).
    pub limit: usize,
    /// Keep only documents carrying this tag.
    pub tag: Option<String>,
    /// Keep only documents in this language.
    pub lang: Option<String>,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            text: String::new(),
            limit: DEFAULT_RESULT_LIMIT,
            tag: None,
            lang: None,
        }
    }
}

impl SearchQuery {
    /// The spec's default query: empty text, limit 10, no filters.
    /// (A `Default` impl exists so the endpoint can build queries from
    /// parsed params without spelling every field.)
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }
}

/// One ranked hit. `snippet` carries the engine's `<mark>`-highlighted
/// fragment (the static matcher's snippet is its plain excerpt — the
/// Degraded cell's documented posture).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchHit {
    /// The document identity (the content collection's join key).
    pub id: String,
    /// The page URL the hit links to.
    pub url: String,
    /// The entry title.
    pub title: String,
    /// The `<mark>`-highlighted fragment (engine) or the plain excerpt
    /// (static matcher).
    pub snippet: String,
    /// The relevancy score (BM25 for the engine, the scorer's weight
    /// sum for the static matcher).
    pub score: f32,
}

/// The response shape the endpoint serializes:
/// `{ total, hits: [{ id, url, title, snippet, score }] }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchResponse {
    /// The total number of matching documents (not capped by `limit`).
    pub total: u64,
    /// The ranked hits, capped by the query's `limit`.
    pub hits: Vec<SearchHit>,
}

/// The object-safe engine seam the server holds (`Arc<dyn SearchIndex>`).
///
/// The same contract as every renderer seam (`PageRenderer`,
/// `LoaderRunner`, `HeadProducer`): a pure answer for a parsed request,
/// coded errors for the failure modes — never a panic on the request
/// path.
pub trait SearchIndex: Send + Sync {
    /// Answer one parsed query. An empty query text is refused with
    /// `SRC-104` (the endpoint turns it into the documented 400).
    ///
    /// # Errors
    ///
    /// `SRC-104` on an empty or unexecutable query (the engine's query
    /// construction and collector failures map here).
    fn search(&self, query: &SearchQuery) -> Result<SearchResponse, OxideError>;

    /// The number of documents in the index (the endpoint's readiness
    /// probe; 0 means the index built empty).
    fn doc_count(&self) -> u32;

    /// The corpus identity the endpoint's L1 query cache partitions its
    /// keys on. The default (0) is correct for any engine whose corpus
    /// is fixed for the object's lifetime — [`TantivyIndex`] and
    /// [`StaticMatcher`] both build or open one corpus and never change
    /// it in place, so every answer the object will ever give was
    /// computed against the corpus the cache entry was computed against.
    ///
    /// An implementation that CAN replace its corpus in place (the
    /// hot-reload handle, `SwappableSearchIndex`) must bump the value on
    /// every replacement: the endpoint folds the generation into the
    /// cache key, so a swapped-in corpus repartitions every key and a
    /// stale pre-swap hit can never survive. An unchanged corpus must
    /// keep the value — the live cache entries stay valid.
    fn generation(&self) -> u64 {
        0
    }
}

/// The shared guard: an empty (or whitespace-only) query text is refused
/// with `SRC-104` — both engines route their entry point through here so
/// the refusal is one definition.
pub(crate) fn refuse_empty_query(text: &str) -> Result<(), OxideError> {
    if text.trim().is_empty() {
        return Err(err(
            "SRC-104",
            "the search query is empty: `GET /_oxide/search?q=<text>` requires text",
        ));
    }
    Ok(())
}

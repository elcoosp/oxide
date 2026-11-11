//! The Oxide built-in search engine (spec v0.2, §V2-F09).
//!
//! `oxide build` indexes the configured content collections; the server
//! answers queries natively at `GET /_oxide/search`; static exports ship
//! a compact JSON index with a tiny client matcher. This crate owns the
//! engine substrate:
//!
//! - [`schema::build_schema`]: the spec's six-field schema (`id`, `url`,
//!   `title`, `body`, `tags`, `lang`).
//! - [`query::SearchDocument`]: one indexed entry (typed frontmatter and
//!   body text with Markdown stripped, the build's mapping).
//! - [`build::TantivyIndex`]: the tantivy engine — build in RAM or into
//!   a directory, open read-only (mmap, no writer at runtime), query
//!   with the spec's boosts, distance-1 fuzzy on long terms, tag/lang
//!   filters and `<mark>` snippets.
//! - [`query::SearchIndex`]: the object-safe seam the server holds —
//!   [`build::TantivyIndex`] is the Full engine, [`static_index::StaticMatcher`]
//!   the Degraded static-JSON fallback (the §18 matrix: Search is Degraded
//!   on Node, serverless, static).
//! - [`static_index::StaticIndex`]: the `dist/search/static.json` export
//!   (`{docs, postings}`, prefix terms of length 2 to 4 included).
//! - [`endpoint::QueryCache`]: the serve side's L1 (60 s TTL keyed by the
//!   normalized query).
//!
//! Errors follow the workspace convention: [`OxideError`] with the
//! `SRC-1xx` codes (registry: `docs/errors.toml`).
//!
//! One deliberate spec delta lives here (the rest are the wire-up's and
//! are recorded in `docs/deviations.md`): the analyzer set ships the
//! English stemmer (`en_stem`) plus the plain default tokenizer —
//! `search.languages` is accepted and threaded through
//! [`query::SearchEngineOptions`] but per-language analyzers land with
//! the language-aware corpus work; German text still matches through the
//! lowercase layer.

#![forbid(unsafe_code)]

pub mod build;
pub mod endpoint;
pub mod query;
pub mod schema;
pub mod static_index;

use oxide_fw_diagnostics::{Diagnostic, OxideError};

/// The ops-plane endpoint path (`GET /_oxide/search?q=<text>&limit=10`,
/// Appendix V2.J).
pub const SEARCH_ENDPOINT_PATH: &str = "/_oxide/search";

/// The index directory name under `dist/search/` (the mmap-able
/// tantivy index the serve side opens read-only).
pub const SEARCH_INDEX_DIR: &str = "index";

/// The static fallback file name under `dist/search/` (the compact JSON
/// index the client matcher loads lazily).
pub const SEARCH_STATIC_FILE: &str = "static.json";

/// The spec's documented query-time default boost for `title` (§V2-F09
/// config example: `fields: { title: 3, body: 1 }`).
pub const DEFAULT_TITLE_BOOST: f32 = 3.0;

/// The spec's documented query-time default boost for `body`.
pub const DEFAULT_BODY_BOOST: f32 = 1.0;

/// The spec's documented default snippet length in characters.
pub const DEFAULT_SNIPPET_LEN: usize = 160;

/// The spec's documented default result cap per query.
pub const DEFAULT_RESULT_LIMIT: usize = 10;

/// The crate-local error constructor (`PREFIX-NNN` codes, the §19.4
/// convention every crate shares).
pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Box::new(Diagnostic::error(code, msg)))
}

pub use build::TantivyIndex;
pub use endpoint::QueryCache;
pub use query::{
    SearchDocument, SearchEngineOptions, SearchHit, SearchIndex, SearchQuery, SearchResponse,
};
pub use schema::{Fields, build_schema};
pub use static_index::{StaticDoc, StaticIndex, StaticMatcher};

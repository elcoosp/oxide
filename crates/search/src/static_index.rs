//! The static fallback index (§V2-F09: "the static fallback
//! `dist/search/static.json`") and the degraded matcher over it.
//!
//! The export shape is the spec's: `{ docs: [{id, url, title, excerpt,
//! tags}], postings: { term: [docIdx, ...] } }`, prefix terms of length
//! 2 to 4 included (so a short query reaches `indexer` through `ind`),
//! term keys in sorted order (gzip-friendly). The postings are
//! plain-lowercase tokens — no stemming, no fuzzy: the client matcher is
//! the spec's ten-line scorer, and the Degraded matrix cell documents
//! that posture (`StaticMatcher` implements it in Rust as the reference
//! the TS client ports; its top-3 parity with the engine is pinned in
//! `tests/search_tdd.rs` on an unambiguous corpus).

use std::collections::BTreeMap;

use oxide_fw_diagnostics::OxideError;
use serde::{Deserialize, Serialize};

use crate::query::{
    SearchDocument, SearchEngineOptions, SearchHit, SearchIndex, SearchQuery, SearchResponse,
    refuse_empty_query,
};
use crate::{DEFAULT_TITLE_BOOST, err};

/// The prefix lengths the export adds postings for (§V2-F09: "prefix
/// terms of length 2 to 4 included").
const PREFIX_TERM_LENS: std::ops::RangeInclusive<usize> = 2..=4;

/// One exported document (the spec's shape; `excerpt` is the body's
/// leading slice — the matcher's snippet source).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct StaticDoc {
    /// The document identity (the engine's `id`).
    pub id: String,
    /// The page URL the hit links to.
    pub url: String,
    /// The entry title.
    pub title: String,
    /// The body's leading slice (the matcher's snippet).
    pub excerpt: String,
    /// The entry tags (the client filters on them).
    pub tags: Vec<String>,
}

/// The compact JSON index the static export ships: the documents plus
/// the term → document-index postings (full terms and the 2-to-4 char
/// prefixes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct StaticIndex {
    /// The documents, in index order (a posting's docIdx indexes here).
    pub docs: Vec<StaticDoc>,
    /// `term → [docIdx]`, sorted keys (gzip-friendly runs).
    pub postings: BTreeMap<String, Vec<u32>>,
}

/// The plain-lowercase token split the export and the matcher share
/// (the same shape the spec's ten-line scorer assumes on the client:
/// alphanumeric runs, lowercased — no stemming, no fuzzy).
pub(crate) fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

impl StaticIndex {
    /// Build the export from the corpus (order-insensitive postings;
    /// document order is the corpus order, the client ranks by score).
    #[must_use]
    pub fn build(docs: &[SearchDocument]) -> Self {
        let mut docs_out = Vec::with_capacity(docs.len());
        let mut postings: BTreeMap<String, Vec<u32>> = BTreeMap::new();
        for (idx, doc) in docs.iter().enumerate() {
            let idx32 = u32::try_from(idx).unwrap_or(u32::MAX);
            docs_out.push(StaticDoc {
                id: doc.id.clone(),
                url: doc.url.clone(),
                title: doc.title.clone(),
                excerpt: lead_chars(&doc.body, PREFIX_EXCERPT_FALLBACK),
                tags: doc.tags.clone(),
            });
            for term in tokenize(&doc.title).into_iter().chain(tokenize(&doc.body)) {
                push_unique(postings.entry(term.clone()).or_default(), idx32);
                // Prefix postings: a full term of length >= 2 contributes
                // its 2-to-4 char prefixes, so short queries match.
                let chars = term.chars().count();
                for plen in PREFIX_TERM_LENS {
                    if plen < chars {
                        let prefix: String = term.chars().take(plen).collect();
                        push_unique(postings.entry(prefix).or_default(), idx32);
                    }
                }
            }
        }
        for list in postings.values_mut() {
            list.sort_unstable();
        }
        Self {
            docs: docs_out,
            postings,
        }
    }

    /// The postings for one term (`None` when the term is not indexed).
    #[must_use]
    pub fn posting(&self, term: &str) -> Option<&[u32]> {
        self.postings.get(term).map(Vec::as_slice)
    }

    /// Serialize (the build writes this to `dist/search/static.json`).
    ///
    /// # Errors
    ///
    /// `SRC-105` when serialization fails (a value cannot occur for the
    /// plain-shape structs, but the build refuses to write silently).
    pub fn to_json(&self) -> Result<String, OxideError> {
        serde_json::to_string(self).map_err(|e| err("SRC-105", format!("static index json: {e}")))
    }

    /// Deserialize (the client matcher's host and the tests read the
    /// export back).
    ///
    /// # Errors
    ///
    /// `SRC-105` when the JSON does not parse into the export shape.
    pub fn from_json(json: &str) -> Result<Self, OxideError> {
        serde_json::from_str(json)
            .map_err(|e| err("SRC-105", format!("static index json parse: {e}")))
    }
}

/// The excerpt cap when the engine options are not at hand yet (the
/// matcher carries the real cap from its options).
const PREFIX_EXCERPT_FALLBACK: usize = 160;

/// The body's leading `max` characters on a char boundary (a cut mid-word
/// is acceptable for an excerpt; a cut mid-UTF-8 is not).
fn lead_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn push_unique(list: &mut Vec<u32>, idx: u32) {
    if !list.contains(&idx) {
        list.push(idx);
    }
}

/// The degraded matcher: the spec's ten-line scorer over the static
/// export. Exact postings hits plus word-prefix bonuses (title words
/// carry the title boost, excerpt words the body boost); no stemming, no
/// fuzzy — the Degraded matrix cell's documented posture.
pub struct StaticMatcher {
    index: StaticIndex,
    opts: SearchEngineOptions,
}

impl StaticMatcher {
    /// A matcher over an export and the engine's query-time knobs.
    #[must_use]
    pub fn new(index: StaticIndex, opts: SearchEngineOptions) -> Self {
        Self { index, opts }
    }
}

impl SearchIndex for StaticMatcher {
    fn search(&self, query: &SearchQuery) -> Result<SearchResponse, OxideError> {
        refuse_empty_query(&query.text)?;
        let tokens = tokenize(&query.text);
        if tokens.is_empty() {
            return Ok(SearchResponse::default());
        }
        let limit = query.limit.max(1);
        let mut scored: Vec<(u32, f32)> = Vec::with_capacity(self.index.docs.len());
        for (idx, doc) in self.index.docs.iter().enumerate() {
            let idx32 = u32::try_from(idx).unwrap_or(u32::MAX);
            let mut score = 0.0f32;
            for token in &tokens {
                if self
                    .index
                    .postings
                    .get(token)
                    .is_some_and(|list| list.binary_search(&idx32).is_ok())
                {
                    score += self.opts.body_boost;
                }
                if title_word_starts_with(&doc.title, token) {
                    score += self.opts.title_boost.max(DEFAULT_TITLE_BOOST);
                }
                if excerpt_word_starts_with(&doc.excerpt, token) {
                    score += self.opts.body_boost;
                }
            }
            if score > 0.0 {
                scored.push((idx32, score));
            }
        }
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        let total = u64::from(u32::try_from(scored.len()).unwrap_or(u32::MAX));
        let hits = scored
            .into_iter()
            .take(limit)
            .filter(
                |(idx, _)| match query.tag.as_deref().filter(|t| !t.trim().is_empty()) {
                    Some(tag) => self
                        .index
                        .docs
                        .get(usize::try_from(*idx).unwrap_or(usize::MAX))
                        .is_some_and(|d| d.tags.iter().any(|t| t == tag)),
                    None => true,
                },
            )
            .map(|(idx, score)| {
                let doc = &self.index.docs[usize::try_from(idx).unwrap_or(usize::MAX)];
                SearchHit {
                    id: doc.id.clone(),
                    url: doc.url.clone(),
                    title: doc.title.clone(),
                    snippet: doc.excerpt.clone(),
                    score,
                }
            })
            .collect();
        Ok(SearchResponse { total, hits })
    }

    fn doc_count(&self) -> u32 {
        u32::try_from(self.index.docs.len()).unwrap_or(u32::MAX)
    }
}

/// Whether any whitespace-delimited title word starts with `token`
/// (the matcher's title bonus; the engine's boost applies to real term
/// hits, the static shape's title words are the client-visible analog).
fn title_word_starts_with(title: &str, token: &str) -> bool {
    tokenize(title).iter().any(|w| w.starts_with(token))
}

/// Whether any excerpt word starts with `token` (the prefix reach that
/// keeps a 5-char query on `indexer` without a postings key for it).
fn excerpt_word_starts_with(excerpt: &str, token: &str) -> bool {
    tokenize(excerpt).iter().any(|w| w.starts_with(token))
}

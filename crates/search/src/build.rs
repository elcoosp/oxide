//! The tantivy engine (§V2-F09): build in RAM or into a directory, open
//! read-only, query.
//!
//! Three constructors cover the three owners:
//!
//! - [`TantivyIndex::build_in_ram`] — tests and callers that hold the
//!   index for the process' lifetime (the example walkthrough).
//! - [`TantivyIndex::build_in_dir`] — the build side: commit into a
//!   (fresh) directory, ready to be mmap-opened later.
//! - [`TantivyIndex::open_read_only`] — the serve side: mmap the
//!   segments, no writer exists at runtime (the spec's own posture).
//!
//! The query path implements the spec's contracts directly: `title` and
//! `body` searched with the configured boosts (3/1 by default), fuzzy
//! distance 1 on terms of length 5 or more (exact term queries for the
//! shorter ones), the `tag`/`lang` filters as required term clauses, and
//! snippets via tantivy's snippet generator re-tagged with `<mark>`.

use std::path::Path;

use oxide_fw_diagnostics::OxideError;
use tantivy::collector::{Count, TopDocs};
use tantivy::query::{BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, Query, TermQuery};
use tantivy::schema::{IndexRecordOption, Value as _};
use tantivy::tokenizer::{
    Language, LowerCaser, RemoveLongFilter, SimpleTokenizer, Stemmer, TextAnalyzer,
};
use tantivy::{DocAddress, Index, IndexReader, IndexWriter, TantivyDocument, Term};

use crate::err;
use crate::query::{
    SearchDocument, SearchEngineOptions, SearchHit, SearchIndex, SearchQuery, SearchResponse,
    refuse_empty_query,
};
use crate::schema::{Fields, build_schema};

/// The fuzzy budget's minimum term length (§V2-F09: "fuzzy distance 1 on
/// terms of length 5 or more"; shorter terms match exactly only).
const FUZZY_MIN_TERM_LEN: usize = 5;

/// The tantivy writer heap budget, bytes. A content collection is a
/// build-time corpus (thousands of entries, not millions); 50 MB bounds
/// the worst case without starving the rest of the build.
const WRITER_HEAP_BYTES: usize = 50_000_000;

/// The tokenizer name the schema's text fields index under. Registered
/// on every [`Index`] this crate constructs (`Index` owns its tokenizer
/// manager; the name is the schema's contract).
pub(crate) const EN_STEM_TOKENIZER: &str = "en_stem";

/// Register the crate's analyzers on an index. One stemmer set ships
/// (the crate doc's delta): `en_stem` = simple split → length cap →
/// lowercase → English stem.
pub(crate) fn register_analyzers(index: &Index) {
    let en_stem: TextAnalyzer = TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(RemoveLongFilter::limit(40))
        .filter(LowerCaser)
        .filter(Stemmer::new(Language::English))
        .build();
    index.tokenizers().register(EN_STEM_TOKENIZER, en_stem);
}

/// The tantivy engine: the schema/fields pair, the options, and the
/// read-only reader the queries run against.
pub struct TantivyIndex {
    index: Index,
    fields: Fields,
    opts: SearchEngineOptions,
    reader: IndexReader,
    num_docs: u32,
}

impl TantivyIndex {
    /// Build an in-RAM index over `docs` (tests and process-lifetime
    /// callers).
    ///
    /// # Errors
    ///
    /// `SRC-103` when a document fails validation (empty `id`/`url`),
    /// `SRC-102` when the tantivy writer or commit fails.
    pub fn build_in_ram(
        docs: &[SearchDocument],
        opts: &SearchEngineOptions,
    ) -> Result<Self, OxideError> {
        let (schema, fields) = build_schema();
        let index = Index::create_in_ram(schema);
        Self::finish_build(index, fields, opts, docs, None)
    }

    /// Build a directory-backed index over `docs` (the build side). The
    /// directory is created if missing and must be FRESH — a previous
    /// index in the same directory would be committed onto, duplicating
    /// every document (the CLI's emitter removes the directory first).
    ///
    /// # Errors
    ///
    /// `SRC-103` when a document fails validation, `SRC-102` when the
    /// directory, writer or commit fails.
    pub fn build_in_dir(
        docs: &[SearchDocument],
        opts: &SearchEngineOptions,
        dir: &Path,
    ) -> Result<Self, OxideError> {
        let (schema, fields) = build_schema();
        std::fs::create_dir_all(dir).map_err(|e| {
            err(
                "SRC-102",
                format!("search index dir {}: {e}", dir.display()),
            )
        })?;
        let index = Index::create_in_dir(dir, schema)
            .map_err(|e| err("SRC-102", format!("search index open for build: {e}")))?;
        Self::finish_build(index, fields, opts, docs, Some(dir))
    }

    /// The shared build body: register analyzers, write the documents,
    /// commit, keep the read-only reader.
    fn finish_build(
        index: Index,
        fields: Fields,
        opts: &SearchEngineOptions,
        docs: &[SearchDocument],
        dir: Option<&Path>,
    ) -> Result<Self, OxideError> {
        register_analyzers(&index);
        validate_documents(docs)?;
        let mut writer: IndexWriter<TantivyDocument> = index
            .writer(WRITER_HEAP_BYTES)
            .map_err(|e| err("SRC-102", format!("search index writer: {e}")))?;
        for doc in docs {
            writer
                .add_document(tantivy_document(doc, &fields))
                .map_err(|e| err("SRC-102", format!("search index doc {}: {e}", doc.id)))?;
        }
        writer
            .commit()
            .map_err(|e| err("SRC-102", format!("search index commit: {e}")))?;
        if let Some(dir) = dir {
            tracing::info!(
                dir = %dir.display(),
                docs = docs.len(),
                "search index committed"
            );
        }
        writer
            .wait_merging_threads()
            .map_err(|e| err("SRC-102", format!("search index merge: {e}")))?;
        let reader = index
            .reader()
            .map_err(|e| err("SRC-102", format!("search index reader: {e}")))?;
        let num_docs = u32::try_from(reader.searcher().num_docs()).unwrap_or(u32::MAX);
        Ok(Self {
            index,
            fields,
            opts: opts.clone(),
            reader,
            num_docs,
        })
    }

    /// Open a committed index read-only (the serve side): mmap the
    /// segment files, no writer exists at runtime.
    ///
    /// # Errors
    ///
    /// `SRC-101` when the directory does not hold an openable index
    /// (missing, stale or corrupt `meta.json`).
    pub fn open_read_only(dir: &Path) -> Result<Self, OxideError> {
        let index = Index::open_in_dir(dir).map_err(|e| {
            err(
                "SRC-101",
                format!("search index open {}: {e}", dir.display()),
            )
        })?;
        register_analyzers(&index);
        let fields = Self::index_fields(&index)?;
        let reader = index
            .reader()
            .map_err(|e| err("SRC-101", format!("search index reader: {e}")))?;
        let num_docs = u32::try_from(reader.searcher().num_docs()).unwrap_or(u32::MAX);
        tracing::info!(dir = %dir.display(), docs = num_docs, "search index opened");
        Ok(Self {
            index,
            fields,
            // The persisted index carries no options; the serve side's
            // query-time knobs come from whoever opens it. The defaults
            // are the spec's; `with_options` re-attaches typed config.
            opts: SearchEngineOptions::default(),
            reader,
            num_docs,
        })
    }

    /// Re-attach query-time knobs after [`TantivyIndex::open_read_only`]
    /// (the serve side threads the typed `search` config through this —
    /// the index file itself carries no options).
    #[must_use]
    pub fn with_options(mut self, opts: &SearchEngineOptions) -> Self {
        self.opts = opts.clone();
        self
    }

    /// The index' schema fields (re-derived on open; the schema is part
    /// of the persisted `meta.json`).
    fn index_fields(index: &Index) -> Result<Fields, OxideError> {
        let schema = index.schema();
        let field = |name: &str| -> Result<_, OxideError> {
            schema
                .get_field(name)
                .map_err(|_| err("SRC-101", format!("search index schema missing `{name}`")))
        };
        Ok(Fields {
            id: field("id")?,
            url: field("url")?,
            title: field("title")?,
            body: field("body")?,
            tags: field("tags")?,
            lang: field("lang")?,
        })
    }

    /// The CAS cache key for a corpus (§V2-F09: "the build is CAS-cached
    /// by the content hash of the collection"): blake3 over the canonical
    /// document stream, order-insensitive — sorted by `id` first, so a
    /// rescan that shuffles entries must not bust the cache.
    #[must_use]
    pub fn content_hash(docs: &[SearchDocument]) -> String {
        let mut sorted: Vec<&SearchDocument> = docs.iter().collect();
        sorted.sort_by(|a, b| a.id.cmp(&b.id));
        let mut hasher = blake3::Hasher::new();
        for doc in sorted {
            for part in [
                doc.id.as_str(),
                doc.url.as_str(),
                doc.title.as_str(),
                doc.body.as_str(),
                doc.tags.join("\u{1f}").as_str(),
                doc.lang.as_str(),
            ] {
                hasher.update(part.as_bytes());
                hasher.update(&[0u8]);
            }
        }
        hasher.finalize().to_hex().to_string()
    }
}

/// Validate the corpus before anything is written (the ingest refusal
/// contract: `SRC-103` names the offending document).
fn validate_documents(docs: &[SearchDocument]) -> Result<(), OxideError> {
    for doc in docs {
        if doc.id.trim().is_empty() {
            return Err(err(
                "SRC-103",
                "search document has an empty `id`: every indexed entry needs the collection's stable key",
            ));
        }
        if doc.url.trim().is_empty() {
            return Err(err(
                "SRC-103",
                format!(
                    "search document `{}` has an empty `url`: the hit has nothing to link to",
                    doc.id
                ),
            ));
        }
    }
    Ok(())
}

/// Map a [`SearchDocument`] onto the tantivy schema (one stored string
/// per tag — the multi-valued `tags` field).
fn tantivy_document(doc: &SearchDocument, fields: &Fields) -> TantivyDocument {
    let mut d = TantivyDocument::default();
    d.add_text(fields.id, &doc.id);
    d.add_text(fields.url, &doc.url);
    d.add_text(fields.title, &doc.title);
    d.add_text(fields.body, &doc.body);
    for tag in &doc.tags {
        d.add_text(fields.tags, tag);
    }
    d.add_text(fields.lang, &doc.lang);
    d
}

impl TantivyIndex {
    /// The query-time knobs in effect (the endpoint logs nothing about
    /// them; they are observable through this accessor for tests and
    /// host wiring).
    #[must_use]
    pub fn options(&self) -> &SearchEngineOptions {
        &self.opts
    }
}

impl std::fmt::Debug for TantivyIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TantivyIndex")
            .field("num_docs", &self.num_docs)
            .field("opts", &self.opts)
            .finish_non_exhaustive()
    }
}

impl SearchIndex for TantivyIndex {
    fn search(&self, query: &SearchQuery) -> Result<SearchResponse, OxideError> {
        refuse_empty_query(&query.text)?;
        let searcher = self.reader.searcher();
        let tokens = query_tokens(&self.index, &self.fields, &query.text)?;
        if tokens.is_empty() {
            // Punctuation-only queries tokenize to nothing: no clauses
            // can match, so the answer is an empty response (not an
            // error — the spec's 400 is for the *empty* query, which
            // `refuse_empty_query` already refused).
            return Ok(SearchResponse::default());
        }
        let text_query = build_text_query(&tokens, &self.fields, &self.opts);
        let full_query: Box<dyn Query> = build_filtered_query(text_query, query, &self.fields);
        let limit = query.limit.max(1);
        let total = searcher
            .search(&full_query, &Count)
            .map_err(|e| err("SRC-104", format!("search query count: {e}")))?;
        let top: Vec<(f32, DocAddress)> = searcher
            .search(&full_query, &TopDocs::with_limit(limit).order_by_score())
            .map_err(|e| err("SRC-104", format!("search query: {e}")))?;
        let hits = render_hits(&searcher, &tokens, &self.fields, &self.opts, &top)?;
        Ok(SearchResponse {
            total: u64::try_from(total).unwrap_or(u64::MAX),
            hits,
        })
    }

    fn doc_count(&self) -> u32 {
        self.num_docs
    }
}

/// Tokenize the query text with the body field's analyzer — the same
/// analysis the index used, so query terms are in the indexed vocabulary
/// (stemmed and lowercased exactly like the documents).
fn query_tokens(index: &Index, fields: &Fields, text: &str) -> Result<Vec<String>, OxideError> {
    let mut analyzer = index
        .tokenizer_for_field(fields.body)
        .map_err(|e| err("SRC-104", format!("search query analyzer: {e}")))?;
    let mut stream = analyzer.token_stream(text);
    let mut tokens = Vec::new();
    while let Some(tok) = stream.next() {
        tokens.push(tok.text.clone());
    }
    Ok(tokens)
}

/// The spec's query shape: one should-clause per token per field —
/// `title` at `title_boost`, `body` at `body_boost`; terms of length 5
/// or more match at fuzzy distance 1, shorter ones exactly.
fn build_text_query(
    tokens: &[String],
    fields: &Fields,
    opts: &SearchEngineOptions,
) -> Box<dyn Query> {
    let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(tokens.len() * 2);
    for token in tokens {
        for (field, boost) in [
            (fields.title, opts.title_boost),
            (fields.body, opts.body_boost),
        ] {
            let term = Term::from_field_text(field, token);
            let base: Box<dyn Query> = if token.chars().count() >= FUZZY_MIN_TERM_LEN {
                Box::new(FuzzyTermQuery::new(term, 1, true))
            } else {
                Box::new(TermQuery::new(
                    term,
                    IndexRecordOption::WithFreqsAndPositions,
                ))
            };
            clauses.push((Occur::Should, Box::new(BoostQuery::new(base, boost))));
        }
    }
    Box::new(BooleanQuery::new(clauses))
}

/// Wrap the text query with the `tag`/`lang` filters as required
/// clauses (an unset filter is a no-op). Takes ownership of the text
/// query — it becomes one `Must` clause of the filtered boolean.
fn build_filtered_query(
    text_query: Box<dyn Query>,
    query: &SearchQuery,
    fields: &Fields,
) -> Box<dyn Query> {
    let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
    if let Some(tag) = query.tag.as_deref().filter(|t| !t.trim().is_empty()) {
        clauses.push((
            Occur::Must,
            Box::new(TermQuery::new(
                Term::from_field_text(fields.tags, tag),
                IndexRecordOption::Basic,
            )),
        ));
    }
    if let Some(lang) = query.lang.as_deref().filter(|l| !l.trim().is_empty()) {
        clauses.push((
            Occur::Must,
            Box::new(TermQuery::new(
                Term::from_field_text(fields.lang, lang),
                IndexRecordOption::Basic,
            )),
        ));
    }
    if clauses.is_empty() {
        return text_query;
    }
    clauses.push((Occur::Must, text_query));
    Box::new(BooleanQuery::new(clauses))
}

/// Render the top docs: stored fields plus the `<mark>` snippet.
///
/// The snippet generator is built from a plain term query over the
/// query's tokens, not from the ranked query itself: tantivy's
/// `query_terms` walk does not reach through `BoostQuery`/`FuzzyTermQuery`
/// wrappers, and the snippet's term scoring only wants the exact
/// vocabulary terms anyway (a fuzzy-only query highlights nothing — the
/// documented posture).
fn render_hits(
    searcher: &tantivy::Searcher,
    tokens: &[String],
    fields: &Fields,
    opts: &SearchEngineOptions,
    top: &[(f32, DocAddress)],
) -> Result<Vec<SearchHit>, OxideError> {
    let mut hits = Vec::with_capacity(top.len());
    if top.is_empty() {
        return Ok(hits);
    }
    let terms_query = BooleanQuery::new(
        tokens
            .iter()
            .map(|t| {
                (
                    Occur::Should,
                    Box::new(TermQuery::new(
                        Term::from_field_text(fields.body, t),
                        IndexRecordOption::Basic,
                    )) as Box<dyn Query>,
                )
            })
            .collect(),
    );
    let mut generator =
        tantivy::snippet::SnippetGenerator::create(searcher, &terms_query, fields.body)
            .map_err(|e| err("SRC-104", format!("search snippet generator: {e}")))?;
    generator.set_max_num_chars(opts.snippet_len.max(1));
    for (score, addr) in top {
        let doc: TantivyDocument = searcher
            .doc::<TantivyDocument>(*addr)
            .map_err(|e| err("SRC-104", format!("search doc fetch: {e}")))?;
        let id = stored_str(&doc, fields.id);
        let url = stored_str(&doc, fields.url);
        let title = stored_str(&doc, fields.title);
        let body = stored_str(&doc, fields.body);
        let mut snippet = generator.snippet(&body);
        snippet.set_snippet_prefix_postfix("<mark>", "</mark>");
        hits.push(SearchHit {
            id,
            url,
            title,
            snippet: snippet.to_html(),
            score: *score,
        });
    }
    Ok(hits)
}

/// One stored string field's value (`""` when absent — the engine writes
/// every field, so absence means a foreign index).
fn stored_str(doc: &TantivyDocument, field: tantivy::schema::Field) -> String {
    doc.get_first(field)
        .and_then(|v| v.as_value().as_str().map(str::to_string))
        .unwrap_or_default()
}

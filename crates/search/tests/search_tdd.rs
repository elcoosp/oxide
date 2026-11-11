//! The V2-F09 engine pins (`feat/search-engine-substrate`).
//!
//! Red-first contract for the search engine crate: the schema shape (the
//! spec's six fields), the build/ingest contracts (the SRC-1xx refusals,
//! the CAS content hash), the query contracts (title-beats-body ranking,
//! the boosts, the distance-1 fuzzy on long terms, the tag/lang filters,
//! the `<mark>` snippet), the static-export contracts (the postings
//! shape, the JSON round trip, the degraded matcher's scorer and its
//! parity with tantivy on an unambiguous corpus) and the endpoint's L1
//! query cache (TTL, cap, normalized keys).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use oxide_fw_config::SearchConfig;
use oxide_fw_search::{
    QueryCache, SearchDocument, SearchEngineOptions, SearchIndex, SearchQuery, TantivyIndex,
};

/// The shared corpus: three docs whose term ownership makes each rank
/// pin unambiguous (`rust` lives in `a`'s title, `b`'s body only).
fn corpus() -> Vec<SearchDocument> {
    vec![
        SearchDocument {
            id: "posts/a".into(),
            url: "/posts/a".into(),
            title: "the rust guide".into(),
            body: "rust is a systems language. it ships an indexer.".into(),
            tags: vec!["guide".into()],
            lang: "en".into(),
        },
        SearchDocument {
            id: "posts/b".into(),
            url: "/posts/b".into(),
            title: "shipping notes".into(),
            body: "the word rust appears in the body here.".into(),
            tags: vec!["api".into()],
            lang: "en".into(),
        },
        SearchDocument {
            id: "posts/c".into(),
            url: "/posts/c".into(),
            title: "der deutsche artikel".into(),
            body: "dieser text spricht ueber rust im allgemeinen.".into(),
            tags: vec!["guide".into()],
            lang: "de".into(),
        },
    ]
}

fn default_opts() -> SearchEngineOptions {
    SearchEngineOptions::default()
}

// ---------------------------------------------------------------- schema

/// The schema carries the spec's six fields (§V2-F09 `build_schema`):
/// `id`/`url`/`tags`/`lang` as stored strings, `title`/`body` as stored
/// text with positions (the snippet generator needs the offsets).
#[test]
fn the_schema_carries_the_six_spec_fields() {
    let (schema, fields) = oxide_fw_search::build_schema();
    for (f, name) in [
        (fields.id, "id"),
        (fields.url, "url"),
        (fields.title, "title"),
        (fields.body, "body"),
        (fields.tags, "tags"),
        (fields.lang, "lang"),
    ] {
        let entry = schema.get_field_entry(f);
        assert_eq!(entry.name(), name, "field name mismatch");
    }
    for f in [
        fields.id,
        fields.url,
        fields.title,
        fields.body,
        fields.tags,
        fields.lang,
    ] {
        assert!(
            schema.get_field_entry(f).is_stored(),
            "every spec field is STORED (the static export reads them back)"
        );
    }
}

// ------------------------------------------------------- build + ingest

#[test]
fn build_in_ram_indexes_the_corpus() {
    let idx = TantivyIndex::build_in_ram(&corpus(), &default_opts()).expect("build");
    assert_eq!(idx.doc_count(), 3);
}

#[test]
fn ingest_refuses_an_empty_id_with_the_ingest_code() {
    let mut docs = corpus();
    docs[0].id = String::new();
    let err = TantivyIndex::build_in_ram(&docs, &default_opts()).expect_err("empty id refused");
    assert_eq!(err.0.code, "SRC-103");
}

#[test]
fn ingest_refuses_an_empty_url_with_the_ingest_code() {
    let mut docs = corpus();
    docs[1].url = "  ".into();
    let err = TantivyIndex::build_in_ram(&docs, &default_opts()).expect_err("empty url refused");
    assert_eq!(err.0.code, "SRC-103");
}

/// The CAS cache key (§V2-F09: "the build is CAS-cached by the content
/// hash of the collection"): order-insensitive over the documents (a
/// rescan that shuffles entries must not bust the cache) and sensitive
/// to every indexed byte.
#[test]
fn content_hash_is_order_insensitive_and_content_sensitive() {
    let mut docs = corpus();
    let h1 = TantivyIndex::content_hash(&docs);
    docs.reverse();
    assert_eq!(
        TantivyIndex::content_hash(&docs),
        h1,
        "order must not matter"
    );
    let mut changed = corpus();
    changed[0].body.push_str(" one more word");
    assert_ne!(
        TantivyIndex::content_hash(&changed),
        h1,
        "content must matter"
    );
}

/// The dir-backed build persists a read-only-openable index: build, drop
/// the writer, re-open with the serve-side `open_read_only` (mmap) and
/// query it.
#[test]
fn build_in_dir_persists_an_openable_index() {
    let dir = tempfile::tempdir().expect("tmp");
    let idx = TantivyIndex::build_in_dir(&corpus(), &default_opts(), dir.path()).expect("build");
    drop(idx);
    let reopened = TantivyIndex::open_read_only(dir.path()).expect("reopen");
    assert_eq!(reopened.doc_count(), 3);
    let resp = reopened
        .search(&SearchQuery {
            text: "rust".into(),
            ..SearchQuery::default()
        })
        .expect("query");
    assert_eq!(resp.total, 3, "all three docs mention rust");
}

#[test]
fn open_read_only_refuses_a_missing_index_with_the_open_code() {
    let dir = tempfile::tempdir().expect("tmp");
    let err =
        TantivyIndex::open_read_only(&dir.path().join("absent")).expect_err("missing refused");
    assert_eq!(err.0.code, "SRC-101");
}

// --------------------------------------------------------------- ranking

#[test]
fn title_match_outranks_body_match() {
    let idx = TantivyIndex::build_in_ram(&corpus(), &default_opts()).expect("build");
    let resp = idx
        .search(&SearchQuery {
            text: "rust".into(),
            ..SearchQuery::default()
        })
        .expect("query");
    assert_eq!(resp.hits.first().expect("a winner").id, "posts/a");
}

#[test]
fn field_boosts_from_options_can_invert_the_ranking() {
    // Two docs with disjoint match fields (the shared corpus has `rust`
    // in doc a's body too, which would give it the body boost as well):
    // one matches only in the title, one only in the body.
    let docs = vec![
        SearchDocument {
            id: "posts/title-only".into(),
            url: "/posts/title-only".into(),
            title: "the rust guide".into(),
            body: "nothing to see here at all".into(),
            tags: vec![],
            lang: "en".into(),
        },
        SearchDocument {
            id: "posts/body-only".into(),
            url: "/posts/body-only".into(),
            title: "shipping notes".into(),
            body: "the word rust appears in the body here".into(),
            tags: vec![],
            lang: "en".into(),
        },
    ];
    let opts = SearchEngineOptions {
        title_boost: 1.0,
        body_boost: 10.0,
        ..SearchEngineOptions::default()
    };
    let idx = TantivyIndex::build_in_ram(&docs, &opts).expect("build");
    let resp = idx
        .search(&SearchQuery {
            text: "rust".into(),
            ..SearchQuery::default()
        })
        .expect("query");
    assert_eq!(resp.hits.first().expect("a winner").id, "posts/body-only");
}

/// The typed config's knobs flow into the engine untouched (`None`
/// keeps the spec defaults: title 3, body 1, snippet 160, `["en"]`).
#[test]
fn engine_options_default_from_the_typed_search_config() {
    let opts = SearchEngineOptions::from_config(&SearchConfig::default());
    assert_eq!(opts.title_boost, 3.0);
    assert_eq!(opts.body_boost, 1.0);
    assert_eq!(opts.snippet_len, 160);
    assert_eq!(opts.languages, vec!["en".to_string()]);

    let configured = SearchConfig {
        fields: oxide_fw_config::SearchFieldsConfig {
            title: Some(2.0),
            body: Some(0.5),
        },
        snippet: Some(80),
        languages: vec!["de".into()],
        ..SearchConfig::default()
    };
    let opts = SearchEngineOptions::from_config(&configured);
    assert_eq!(opts.title_boost, 2.0);
    assert_eq!(opts.body_boost, 0.5);
    assert_eq!(opts.snippet_len, 80);
    assert_eq!(opts.languages, vec!["de".to_string()]);
}

// ----------------------------------------------------------------- query

#[test]
fn limit_caps_hits_but_total_counts_every_match() {
    let idx = TantivyIndex::build_in_ram(&corpus(), &default_opts()).expect("build");
    let resp = idx
        .search(&SearchQuery {
            text: "rust".into(),
            limit: 1,
            ..SearchQuery::default()
        })
        .expect("query");
    assert_eq!(resp.hits.len(), 1);
    assert_eq!(resp.total, 3);
}

#[test]
fn an_empty_query_is_refused_with_the_query_code() {
    let idx = TantivyIndex::build_in_ram(&corpus(), &default_opts()).expect("build");
    let err = idx
        .search(&SearchQuery::default())
        .expect_err("empty text refused");
    assert_eq!(err.0.code, "SRC-104");
}

/// §V2-F09: "fuzzy distance 1 on terms of length 5 or more" — `kernl`
/// (5 chars) matches `kernel` at distance 1.
#[test]
fn long_terms_match_at_fuzzy_distance_one() {
    let docs = vec![SearchDocument {
        id: "docs/kernel".into(),
        url: "/docs/kernel".into(),
        title: "the kernel".into(),
        body: "the kernel panics on demand.".into(),
        tags: vec![],
        lang: "en".into(),
    }];
    let idx = TantivyIndex::build_in_ram(&docs, &default_opts()).expect("build");
    let resp = idx
        .search(&SearchQuery {
            text: "kernl".into(),
            ..SearchQuery::default()
        })
        .expect("query");
    assert_eq!(resp.total, 1, "fuzzy distance 1 finds kernel");
}

/// The fuzzy budget is the spec's, not a fuzz-everything posture: terms
/// shorter than 5 characters match exactly only.
#[test]
fn short_terms_match_exactly_only() {
    let idx = TantivyIndex::build_in_ram(&corpus(), &default_opts()).expect("build");
    let miss = idx
        .search(&SearchQuery {
            text: "russ".into(),
            ..SearchQuery::default()
        })
        .expect("query");
    assert_eq!(
        miss.total, 0,
        "4-char term, distance 1 off `rust`: not fuzzy"
    );
    let hit = idx
        .search(&SearchQuery {
            text: "guide".into(),
            ..SearchQuery::default()
        })
        .expect("query");
    assert_eq!(hit.total, 1, "exact terms match regardless of length");
    assert_eq!(hit.hits.first().expect("the guide doc").id, "posts/a");
}

#[test]
fn tag_filter_narrows_the_matches() {
    let idx = TantivyIndex::build_in_ram(&corpus(), &default_opts()).expect("build");
    let resp = idx
        .search(&SearchQuery {
            text: "rust".into(),
            tag: Some("guide".into()),
            ..SearchQuery::default()
        })
        .expect("query");
    assert_eq!(resp.total, 2);
    assert!(
        resp.hits.iter().all(|h| h.id != "posts/b"),
        "the api-tagged doc is filtered"
    );
}

#[test]
fn lang_filter_narrows_the_matches() {
    let idx = TantivyIndex::build_in_ram(&corpus(), &default_opts()).expect("build");
    let resp = idx
        .search(&SearchQuery {
            text: "rust".into(),
            lang: Some("de".into()),
            ..SearchQuery::default()
        })
        .expect("query");
    assert_eq!(resp.total, 1);
    assert_eq!(resp.hits.first().expect("de doc").id, "posts/c");
}

// --------------------------------------------------------------- snippet

#[test]
fn snippets_highlight_with_mark() {
    let idx = TantivyIndex::build_in_ram(&corpus(), &default_opts()).expect("build");
    let resp = idx
        .search(&SearchQuery {
            text: "indexer".into(),
            ..SearchQuery::default()
        })
        .expect("query");
    let hit = resp.hits.first().expect("the a doc");
    assert!(hit.snippet.contains("<mark>"), "snippet: {}", hit.snippet);
    assert!(hit.snippet.contains("indexer"), "snippet: {}", hit.snippet);
}

#[test]
fn snippet_length_follows_the_engine_options() {
    let opts = SearchEngineOptions {
        snippet_len: 40,
        ..SearchEngineOptions::default()
    };
    let idx = TantivyIndex::build_in_ram(&corpus(), &opts).expect("build");
    let resp = idx
        .search(&SearchQuery {
            text: "systems language".into(),
            ..SearchQuery::default()
        })
        .expect("query");
    let hit = resp.hits.first().expect("the a doc");
    assert!(
        hit.snippet.chars().count() <= 40 + "<mark></mark>".len() * 4,
        "snippet respects the configured length: {}",
        hit.snippet
    );
}

// --------------------------------------------------------- static export

#[test]
fn static_index_shape_docs_and_postings_with_prefixes() {
    let docs = corpus();
    let st = oxide_fw_search::StaticIndex::build(&docs);
    assert_eq!(st.docs.len(), 3);
    // The body term `indexer` posts doc 0 (`posts/a`, the first doc).
    let postings = st.posting("indexer").expect("term indexed");
    assert!(postings.contains(&0));
    // Prefix postings (terms of length 2..=4 are exported as prefixes)
    // make the client's prefix scorer possible: `ru` for `rust`.
    assert!(
        st.posting("ru").is_some(),
        "2-char prefix postings are exported"
    );
    assert!(st.posting("rus").is_some(), "3-char prefix postings too");
}

#[test]
fn static_json_round_trips() {
    let st = oxide_fw_search::StaticIndex::build(&corpus());
    let json = st.to_json().expect("serialize");
    let back = oxide_fw_search::StaticIndex::from_json(&json).expect("deserialize");
    assert_eq!(back.docs.len(), st.docs.len());
    assert_eq!(back.posting("indexer"), st.posting("indexer"));
    assert!(
        json.contains("\"docs\"") && json.contains("\"postings\""),
        "spec shape"
    );
}

/// The degraded matcher (the static JSON fallback — spec matrix: Search
/// is Degraded on Node/serverless/static) ranks a title hit over a body
/// hit with the same weights the engine defaults to.
#[test]
fn static_matcher_ranks_title_over_body() {
    let docs = corpus();
    let st = oxide_fw_search::StaticIndex::build(&docs);
    let m = oxide_fw_search::StaticMatcher::new(st, SearchEngineOptions::default());
    let resp = m
        .search(&SearchQuery {
            text: "rust".into(),
            ..SearchQuery::default()
        })
        .expect("query");
    assert_eq!(resp.hits.first().expect("a winner").id, "posts/a");
    assert_eq!(resp.total, 3);
}

/// The spec's parity acceptance, Rust side: on a corpus where each of
/// the 20 queries has unambiguous winners (terms owned by one doc, and
/// pairwise edit-distant so the engine's distance-1 fuzzy cannot widen a
/// term's matches), the static matcher's top-3 equals the engine's top-3
/// as sets (tie order may differ). The full cross-language parity pin is
/// the TS half's.
#[test]
fn static_matcher_top3_matches_the_engine_on_twenty_queries() {
    // 8 docs, each owning two private terms no other doc carries. The
    // words are pairwise edit-distant (a distance-1 fuzzy would make the
    // engine's matches wider than the exact-term matcher's).
    const TITLE_WORDS: [&str; 8] = [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
    ];
    const BODY_WORDS: [&str; 8] = [
        "oscar", "tango", "romeo", "india", "zebra", "yacht", "xenon", "vodka",
    ];
    let mut docs = Vec::new();
    for i in 0..8 {
        docs.push(SearchDocument {
            id: format!("posts/{i}"),
            url: format!("/posts/{i}"),
            title: format!("{} guide", TITLE_WORDS[i]),
            body: format!("{} content with shared filler words here", BODY_WORDS[i]),
            tags: vec![format!("t{i}")],
            lang: "en".into(),
        });
    }
    let engine = TantivyIndex::build_in_ram(&docs, &default_opts()).expect("build");
    let matcher = oxide_fw_search::StaticMatcher::new(
        oxide_fw_search::StaticIndex::build(&docs),
        default_opts(),
    );
    let mut queries = Vec::new();
    for i in 0..8 {
        queries.push(TITLE_WORDS[i].to_string());
        queries.push(BODY_WORDS[i].to_string());
    }
    // Cross pairs: two matching docs each — every query in this set has
    // at most 2 matching documents, so no BM25 tie-break (length
    // normalization the degraded scorer does not reproduce) can flip a
    // top-3 set.
    queries.push("alpha oscar".into());
    queries.push("charlie romeo".into());
    queries.push("echo india".into());
    queries.push("hotel vodka".into());
    assert_eq!(queries.len(), 20);
    for q in &queries {
        let query = SearchQuery {
            text: q.clone(),
            ..SearchQuery::default()
        };
        let engine_ids = top3_ids(&engine.search(&query).expect("engine query"));
        let matcher_ids = top3_ids(&matcher.search(&query).expect("matcher query"));
        let engine_set: std::collections::BTreeSet<&String> = engine_ids.iter().collect();
        let matcher_set: std::collections::BTreeSet<&String> = matcher_ids.iter().collect();
        assert_eq!(
            engine_set, matcher_set,
            "top-3 parity broke for query {q:?}"
        );
    }
}

fn top3_ids(resp: &oxide_fw_search::SearchResponse) -> Vec<String> {
    resp.hits.iter().take(3).map(|h| h.id.clone()).collect()
}

/// The static matcher documents its degraded posture: no fuzzy, no
/// scoring beyond the ten-line scorer — a distance-1 miss stays a miss.
#[test]
fn static_matcher_stays_exact_term_and_prefix_only() {
    let st = oxide_fw_search::StaticIndex::build(&corpus());
    let m = oxide_fw_search::StaticMatcher::new(st, SearchEngineOptions::default());
    let miss = m
        .search(&SearchQuery {
            text: "russ".into(),
            ..SearchQuery::default()
        })
        .expect("query");
    assert_eq!(miss.total, 0, "no fuzzy in the degraded matcher");
    let prefix = m
        .search(&SearchQuery {
            text: "index".into(),
            ..SearchQuery::default()
        })
        .expect("query");
    assert!(
        prefix.total >= 1,
        "prefix terms reach `indexer` via the postings"
    );
}

// --------------------------------------------------------- the L1 cache

/// A controllable clock: the cache's TTL math must be testable without
/// wall-clock sleeps (the conventions: deterministic tests).
fn fake_clock(at: Arc<Mutex<Instant>>) -> Arc<dyn Fn() -> Instant + Send + Sync> {
    Arc::new(move || *at.lock().expect("clock"))
}

#[test]
fn query_cache_serves_within_the_ttl() {
    let now = Arc::new(Mutex::new(Instant::now()));
    let cache = QueryCache::with_clock(
        std::time::Duration::from_secs(60),
        8,
        fake_clock(Arc::clone(&now)),
    );
    let computes = Arc::new(AtomicUsize::new(0));
    let key = cache.key_for("rust book");
    let c2 = Arc::clone(&computes);
    let get = || {
        cache.get_or_insert(&key, || {
            c2.fetch_add(1, Ordering::SeqCst);
            Ok(oxide_fw_search::SearchResponse::default())
        })
    };
    get().expect("first compute");
    get().expect("served from cache");
    get().expect("still cached");
    assert_eq!(
        computes.load(Ordering::SeqCst),
        1,
        "one computation, three reads"
    );
}

#[test]
fn query_cache_recomputes_after_the_ttl() {
    let now = Arc::new(Mutex::new(Instant::now()));
    let cache = QueryCache::with_clock(
        std::time::Duration::from_secs(60),
        8,
        fake_clock(Arc::clone(&now)),
    );
    let computes = Arc::new(AtomicUsize::new(0));
    let key = cache.key_for("rust book");
    let c2 = Arc::clone(&computes);
    let get = || {
        cache
            .get_or_insert(&key, || {
                c2.fetch_add(1, Ordering::SeqCst);
                Ok(oxide_fw_search::SearchResponse::default())
            })
            .expect("get")
    };
    get();
    get();
    // advance 61 seconds on the injected clock
    let mut guard = now.lock().expect("clock");
    *guard += std::time::Duration::from_secs(61);
    drop(guard);
    get();
    assert_eq!(
        computes.load(Ordering::SeqCst),
        2,
        "expired entry recomputes"
    );
}

#[test]
fn query_cache_evicts_the_oldest_at_the_cap() {
    let now = Arc::new(Mutex::new(Instant::now()));
    let cache = QueryCache::with_clock(
        std::time::Duration::from_secs(60),
        2,
        fake_clock(Arc::clone(&now)),
    );
    let computes = Arc::new(AtomicUsize::new(0));
    let c2 = Arc::clone(&computes);
    let get = |k: &str| {
        let key = cache.key_for(k);
        cache
            .get_or_insert(&key, || {
                c2.fetch_add(1, Ordering::SeqCst);
                Ok(oxide_fw_search::SearchResponse::default())
            })
            .expect("get")
    };
    get("a");
    get("b");
    get("c"); // a is now the LRU victim
    get("b");
    get("c");
    get("a"); // must recompute — evicted
    assert_eq!(computes.load(Ordering::SeqCst), 4);
}

#[test]
fn query_cache_keys_are_normalized() {
    let cache = QueryCache::new();
    assert_eq!(cache.key_for("  Rust   BOOK "), cache.key_for("rust book"));
    assert_ne!(cache.key_for("rust book"), cache.key_for("rust bookk"));
}

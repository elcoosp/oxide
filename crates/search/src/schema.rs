//! The search schema (§V2-F09 `build_schema`).
//!
//! Six fields, the spec's exact set: `id`/`url`/`tags`/`lang` are stored
//! strings (term lookups: the tag/lang filters, the identity join back to
//! the content collection), `title`/`body` are stored text with
//! frequencies and positions (relevancy ranking and the snippet
//! generator need the offsets). Everything is `STORED` — the static
//! export reads the documents back out of the index, and the serve side
//! never has a second copy of the corpus.

use tantivy::schema::{
    Field, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions,
};

/// The registered field handles [`build_schema`] returns; keep the pair
/// together — a `Schema` without its `Field`s is untyped.
#[derive(Debug, Clone, Copy)]
pub struct Fields {
    /// The document identity (`posts/a`) — the content collection's
    /// stable join key.
    pub id: Field,
    /// The page URL the hit links to (`/posts/a`).
    pub url: Field,
    /// The entry title (boosted at query time).
    pub title: Field,
    /// The entry body (Markdown already stripped by the build).
    pub body: Field,
    /// The entry tags (multi-valued; the `tag=<t>` filter).
    pub tags: Field,
    /// The entry language (the `lang=<l>` filter).
    pub lang: Field,
}

/// Build the spec's schema (§V2-F09 `build_schema`) — fields plus the
/// analyzer contract: `title`/`body` index with the `en_stem` tokenizer
/// (simple split, lowercase, English stem) with frequencies and
/// positions; the string fields index with tantivy's raw tokenizer.
///
/// The `en_stem` tokenizer itself is registered per-index (a tantivy
/// `Index` owns its tokenizer manager); see `crate::build`.
#[must_use]
pub fn build_schema() -> (Schema, Fields) {
    let mut builder = Schema::builder();
    let text = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer("en_stem")
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    let text = text.set_stored();
    let f = Fields {
        id: builder.add_text_field("id", STRING | STORED),
        url: builder.add_text_field("url", STRING | STORED),
        title: builder.add_text_field("title", text.clone()),
        body: builder.add_text_field("body", text),
        tags: builder.add_text_field("tags", STRING | STORED),
        lang: builder.add_text_field("lang", STRING | STORED),
    };
    (builder.build(), f)
}

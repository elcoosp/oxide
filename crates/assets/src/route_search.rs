//! §16 / K2: a route-level search index.
//!
//! The build already emits a per-collection search index
//! (`build_search_index` in `content_collection.rs` — used by K1's data
//! surface). This module is the *route* index: one entry per prerendered
//! route, with the page's title, `<h1>`–`<h6>` headings, and a short
//! excerpt. A client-side island (a user pattern, not something the
//! framework ships a fixed markup for) reads it to answer a query.
//!
//! Extraction is done on the emitted HTML — not on the source AST —
//! because the search surface should reflect what a user actually sees
//! after prerendering (loader-injected titles, plugin-added headings,
//! `defineMeta` overrides). Extracting from source would drift from the
//! output.
//!
//! No regex dependency: the HTML is our own output (well-formed, no
//! entities in the visible text in practice), so a small tag scanner is
//! sufficient and stays free of the `regex` crate's compile cost for a
//! module that runs once per route at build time.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// One route's entry in the search index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteSearchEntry {
    /// Canonical route id (`routes/index`, `routes/blog/[slug]`).
    pub route: String,
    /// The prerendered URL path (`/`, `/blog/hello`).
    pub url: String,
    /// `<title>` text, trimmed. `None` when the page had no title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// `<h1>`–`<h6>` texts in document order, trimmed, empties dropped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headings: Vec<String>,
    /// The first `<p>`'s text, trimmed, truncated to 160 chars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
}

/// The full index emitted to `dist/client/search-index.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteSearchIndex {
    /// Schema version. Bumped when the shape changes so a stale client
    /// can detect a mismatch and fall back to a no-op rather than
    /// rendering wrong results.
    pub version: u32,
    /// One entry per prerendered route. Sorted by `route` for byte
    /// stability across builds (see the determinism test).
    pub entries: Vec<RouteSearchEntry>,
}

/// Extract the search-relevant text from a rendered HTML document. The
/// route/url are caller-supplied (the build knows them; the HTML does
/// not always carry a canonical route id in a machine-readable way).
pub fn extract_from_html(route: &str, url: &str, html: &str) -> RouteSearchEntry {
    // Headings and the excerpt both run through `strip_tags`; the title did
    // not, so a `<title>` with an entity or an inline tag would carry raw
    // markup into the search index and into whatever renders the result.
    // (A well-formed `<title>` has no children, which is why this was
    // invisible — the inconsistency is what the strip is guarding against.)
    let title = extract_tag_text(html, "title").and_then(|t| {
        let t = strip_tags(&t).trim().to_string();
        (!t.is_empty()).then_some(t)
    });

    let mut headings: Vec<String> = Vec::new();
    for tag in ["h1", "h2", "h3", "h4", "h5", "h6"] {
        for text in extract_all_tag_text(html, tag) {
            let t = strip_tags(&text).trim().to_string();
            if !t.is_empty() {
                headings.push(t);
            }
        }
    }

    let excerpt = extract_tag_text(html, "p").map(|p| {
        let t = strip_tags(&p).trim().to_string();
        // Truncate to 160 chars, cut at a word boundary so an index
        // entry does not look mid-word in a result card. `is_char_boundary`
        // keeps a multibyte codepoint from being split.
        if t.chars().count() <= 160 {
            t
        } else {
            let mut out = String::with_capacity(160);
            let mut n = 0;
            for ch in t.chars() {
                if n >= 160 {
                    break;
                }
                out.push(ch);
                n += 1;
            }
            // Trim to the last space so the result is not mid-word.
            if let Some(sp) = out.rfind(' ') {
                out.truncate(sp);
            }
            out.push('…');
            out
        }
    });
    let excerpt = excerpt.and_then(|e| (!e.is_empty()).then_some(e));

    RouteSearchEntry {
        route: route.to_string(),
        url: url.to_string(),
        title,
        headings,
        excerpt,
    }
}

/// Walk `prerendered` (route_id → list of relative output paths) and build
/// the index. `dist` is the project's `dist/` directory (the paths are
/// relative to it). A route with multiple variants (per-locale outputs)
/// produces one entry per variant — the URL is what the client needs to
/// navigate, and a search result should land on the locale the user is
/// browsing in.
pub fn build_route_search_index(
    prerendered: &BTreeMap<String, Vec<String>>,
    dist: &Path,
) -> RouteSearchIndex {
    let mut entries: Vec<RouteSearchEntry> = Vec::new();
    // Stable iteration: `prerendered` is a BTreeMap (sorted by route_id).
    for (route_id, rels) in prerendered {
        // A single route can have several outputs (per-locale prerender).
        // Iterate in the order the build produced them — the caller
        // appends the default-locale variant first (see O2), so the
        // primary result comes first.
        for rel in rels {
            let path = dist.join(rel);
            let Ok(html) = std::fs::read_to_string(&path) else {
                continue;
            };
            // URL path: strip `prerendered/`, drop the `.html` suffix,
            // map `.../index.html` to the directory form. This mirrors
            // what `url_path_from_rel` produces at build time.
            let url = url_from_rel(rel);
            entries.push(extract_from_html(route_id, &url, &html));
        }
    }
    // Byte-stable order across builds (the determinism test pins this).
    entries.sort_by(|a, b| a.route.cmp(&b.route).then_with(|| a.url.cmp(&b.url)));
    RouteSearchIndex {
        version: 1,
        entries,
    }
}

/// Serialize to pretty JSON. Byte-stable across runs (BTreeMap + sort
/// above + serde_json's stable map ordering means the output does not
/// depend on hashing).
pub fn route_search_index_to_json(idx: &RouteSearchIndex) -> crate::Result<String> {
    serde_json::to_string_pretty(idx)
        .map_err(|e| crate::err("AST-IDX", format!("route search index serialization: {e}")))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Find the first `<tag ...>…</tag>` and return the text between. Handles
/// attributes on the opening tag; does not decode HTML entities (the
/// visible text in our own output is plain).
fn extract_tag_text(html: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let rel_open = html.find(&open)?;
    let text_start = html[rel_open..].find('>')? + rel_open + 1;
    let rel_close = html[text_start..].find(&close)?;
    let text_end = text_start + rel_close;
    Some(html[text_start..text_end].to_string())
}

/// Find every `<tag ...>…</tag>` and return each text between.
fn extract_all_tag_text(html: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(rel_open) = html[i..].find(&open) {
        let open_start = i + rel_open;
        let Some(rel_gt) = html[open_start..].find('>') else {
            break;
        };
        let text_start = open_start + rel_gt + 1;
        let Some(rel_close) = html[text_start..].find(&close) else {
            break;
        };
        let text_end = text_start + rel_close;
        out.push(html[text_start..text_end].to_string());
        i = text_end + close.len();
    }
    out
}

/// Strip HTML tags from a fragment. A tag boundary becomes a space so
/// `Hello <b>world</b>` does not become `Helloworld`; the output is then
/// whitespace-collapsed so a tag that sits between existing spaces does
/// not produce a double space (`with <b>markup</b> inside` becomes
/// `with markup inside`, not `with  markup  inside`).
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    // Collapse runs of whitespace into a single space. `split_whitespace`
    // is already the correct behavior here: it drops leading/trailing
    // whitespace and joins runs with one space.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Map a `prerendered/<...>.html` relative path to a URL path. `.../
/// index.html` maps to the directory form with a trailing slash
/// (`prerendered/index.html` → `/`; `prerendered/blog/index.html` →
/// `/blog/`), matching the request path a user types.
fn url_from_rel(rel: &str) -> String {
    let stripped = rel
        .strip_prefix("prerendered/")
        .or_else(|| rel.strip_prefix("prerendered\\"))
        .unwrap_or(rel);
    let without_ext = stripped.strip_suffix(".html").unwrap_or(stripped);
    if without_ext.is_empty() || without_ext == "index" {
        "/".to_string()
    } else if let Some(dir) = without_ext.strip_suffix("/index") {
        format!("/{dir}/")
    } else {
        format!("/{without_ext}")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_strips_markup_from_the_title_too() {
        // A title carrying an inline tag must not reach the index with raw
        // markup — headings and the excerpt already go through `strip_tags`,
        // the title did not. (`strip_tags` removes tags and collapses the
        // boundary to a space; it does not decode entities, which is a
        // separate concern and consistent with the headings/excerpt path.)
        let html = "<html><head><title>Oxide <em>and friends</em></title></head>\
                    <body><p>x</p></body></html>";
        let e = extract_from_html("routes/index", "/", html);
        assert_eq!(
            e.title.as_deref(),
            Some("Oxide and friends"),
            "the title must be stripped like the headings and excerpt"
        );
        assert!(
            !e.title.as_deref().unwrap_or("").contains('<'),
            "no raw markup in the title: {:?}",
            e.title
        );
    }

    #[test]
    fn extract_reads_title_headings_and_excerpt() {
        let html = "<html><head><title>Blog Hello</title></head>\
                    <body><h1>Welcome</h1><h2>First section</h2>\
                    <p>This is the excerpt, with <b>markup</b> inside.</p></body></html>";
        let e = extract_from_html("routes/blog/[slug]", "/blog/hello", html);
        assert_eq!(e.route, "routes/blog/[slug]");
        assert_eq!(e.url, "/blog/hello");
        assert_eq!(e.title.as_deref(), Some("Blog Hello"));
        assert_eq!(
            e.headings,
            vec!["Welcome".to_string(), "First section".to_string()]
        );
        assert_eq!(
            e.excerpt.as_deref(),
            Some("This is the excerpt, with markup inside.")
        );
    }

    #[test]
    fn extract_handles_missing_elements() {
        let html = "<html><body>No title, no headings.</body></html>";
        let e = extract_from_html("routes/x", "/x", html);
        assert!(e.title.is_none());
        assert!(e.headings.is_empty());
        assert!(e.excerpt.is_none());
    }

    #[test]
    fn extract_truncates_long_excerpt_at_word_boundary() {
        let long = "a".repeat(50) + " " + &"b".repeat(200);
        let html = format!("<p>{long}</p>");
        let e = extract_from_html("routes/x", "/x", &html);
        let ex = e.excerpt.expect("excerpt");
        assert!(ex.ends_with('…'), "truncated form: {ex:?}");
        // The ellipsis is one char; the rest is under 160 chars and
        // ends on a word boundary (the last char before … is a letter
        // that was preceded by a space at some point — the trim-to-
        // last-space rule).
        assert!(ex.chars().count() <= 161, "under 160 + ellipsis");
    }

    #[test]
    fn url_from_rel_covers_the_shapes() {
        assert_eq!(url_from_rel("prerendered/index.html"), "/");
        assert_eq!(url_from_rel("prerendered/about.html"), "/about");
        assert_eq!(url_from_rel("prerendered/blog/index.html"), "/blog/");
        assert_eq!(url_from_rel("prerendered/blog/post.html"), "/blog/post");
        assert_eq!(url_from_rel("prerendered/fr/about.html"), "/fr/about");
        assert_eq!(url_from_rel("prerendered/fr/index.html"), "/fr/");
    }

    #[test]
    fn build_index_is_byte_stable_across_runs() {
        let dir = tempfile::tempdir().unwrap();
        let dist = dir.path();
        std::fs::create_dir_all(dist.join("prerendered")).unwrap();
        std::fs::write(
            dist.join("prerendered/index.html"),
            "<title>Home</title><h1>Welcome</h1><p>Start here.</p>",
        )
        .unwrap();
        std::fs::write(
            dist.join("prerendered/about.html"),
            "<title>About</title><h1>About us</h1><p>Who we are.</p>",
        )
        .unwrap();

        let mut prerendered = BTreeMap::new();
        prerendered.insert(
            "routes/index".to_string(),
            vec!["prerendered/index.html".to_string()],
        );
        prerendered.insert(
            "routes/about".to_string(),
            vec!["prerendered/about.html".to_string()],
        );

        let a = build_route_search_index(&prerendered, dist);
        let b = build_route_search_index(&prerendered, dist);

        let ja = route_search_index_to_json(&a).unwrap();
        let jb = route_search_index_to_json(&b).unwrap();
        assert_eq!(ja, jb, "byte-stable index across runs");
        assert_eq!(a.version, 1);
        assert_eq!(a.entries.len(), 2);
        // Sorted by route id.
        assert_eq!(a.entries[0].route, "routes/about");
        assert_eq!(a.entries[1].route, "routes/index");
    }

    #[test]
    fn build_index_skips_missing_files() {
        // A `prerendered` map that references a file the build never
        // wrote (a partial build, an interrupted write) must not abort
        // the whole index — it just misses that one entry.
        let dir = tempfile::tempdir().unwrap();
        let mut prerendered = BTreeMap::new();
        prerendered.insert(
            "routes/x".to_string(),
            vec!["prerendered/x.html".to_string()],
        );
        let idx = build_route_search_index(&prerendered, dir.path());
        assert!(idx.entries.is_empty(), "missing file → skipped, no panic");
    }
}

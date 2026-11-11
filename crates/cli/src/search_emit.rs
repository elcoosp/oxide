//! The build-side search mapping (V2-F09, feature `search`).
//!
//! Pure helpers for the `emit_search_engine_index` build step (which
//! lives next to `emit_content_search_indexes` in `lib.rs`, the same
//! home the §16.5 emitter uses): mapping one content-collection entry
//! onto the engine's document shape, and the conservative Markdown
//! strip the engine's body needs (the spec: "typed frontmatter and body
//! text with Markdown stripped").

use oxide_fw_assets::CollectionEntry;
use oxide_fw_config::SearchConfig;
use oxide_fw_search::SearchDocument;

/// Map one content-collection entry onto the engine's document shape
/// (the build's §V2-F09 mapping). The id is `<collection>/<slug>` (the
/// cross-collection join key), the URL the default collection-page shape
/// (`/<collection>/<slug>` — the delta the deviations record:
/// collections carry no route template in the typed config, the UI half
/// maps ids onto real routes when it lands). `lang` comes from the
/// entry's frontmatter, else the first `search.languages` entry, else
/// `"en"`.
pub(crate) fn collection_document(
    name: &str,
    entry: &CollectionEntry,
    search: &SearchConfig,
) -> SearchDocument {
    let fm = &entry.frontmatter;
    let title = fm
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| entry.slug.clone());
    let tags = fm
        .get("tags")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let lang = fm
        .get("lang")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| search.languages.first().cloned())
        .unwrap_or_else(|| "en".to_string());
    SearchDocument {
        id: format!("{name}/{}", entry.slug),
        url: format!("/{name}/{}", entry.slug),
        title,
        body: strip_markdown(&entry.body_markdown),
        tags,
        lang,
    }
}

/// Strip the Markdown the engine must not index: fenced code blocks,
/// heading/quote/list line markers, and the inline code spans, image and
/// link targets, emphasis markers. Deliberately conservative — the goal
/// is indexable prose, not a Markdown-to-text conversion guarantee.
pub(crate) fn strip_markdown(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut in_fence = false;
    for line in md.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // Line-start markers: headings, quotes, list bullets.
        let stripped = trimmed.trim_start_matches(['#', '>', '-', '*', '+', ' ']);
        out.push_str(&strip_inline_markdown(stripped));
        out.push(' ');
    }
    out.trim().to_string()
}

/// The inline pass: code spans, images, links and emphasis.
fn strip_inline_markdown(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '`' => i += 1, // code-span markers vanish, the text stays
            '!' if i + 1 < chars.len() && chars[i + 1] == '[' => {
                if let Some(alt) = bracket_group_text(&chars, i + 1) {
                    out.push_str(&alt);
                    // The group scanner skips the alt text and the target.
                    let after_group = skip_bracket_group(&chars, i + 1);
                    i = after_group.max(i + 2);
                } else {
                    i += 2;
                }
            }
            '[' => {
                if let Some(text) = bracket_group_text(&chars, i + 1) {
                    out.push_str(&text);
                    let after_group = skip_bracket_group(&chars, i + 1);
                    i = after_group.max(i + 1);
                } else {
                    out.push('[');
                    i += 1;
                }
            }
            '*' | '_' => i += 1, // emphasis markers vanish
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// The text inside a `[...]` group starting at `open` (the position of
/// `[`), when the group is well-formed and followed by `(` (a link or
/// image target).
fn bracket_group_text(chars: &[char], open: usize) -> Option<String> {
    let mut depth = 1usize;
    let mut i = open;
    while i < chars.len() {
        match chars[i] {
            '[' => depth += 1,
            ']' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    return (chars.get(i + 1) == Some(&'('))
                        .then(|| chars[open..i].iter().collect());
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The index just past a `[...]` group starting at `open`, including a
/// trailing `(target)` group when present.
fn skip_bracket_group(chars: &[char], open: usize) -> usize {
    let mut depth = 1usize;
    let mut i = open;
    while i < chars.len() {
        match chars[i] {
            '[' => depth += 1,
            ']' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    if chars.get(i + 1) == Some(&'(') {
                        let mut pdepth = 0usize;
                        let mut j = i + 1;
                        while j < chars.len() {
                            match chars[j] {
                                '(' => pdepth += 1,
                                ')' if pdepth > 0 => {
                                    pdepth -= 1;
                                    if pdepth == 0 {
                                        return j + 1;
                                    }
                                }
                                _ => {}
                            }
                            j += 1;
                        }
                        return j;
                    }
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    i
}

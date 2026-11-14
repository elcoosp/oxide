//! V2-E07: the `dist/llms.txt` + `dist/llms-full.txt` emission — the
//! LLM-facing route index the build writes next to the sitemap (§16.3's
//! neighbor).
//!
//! - `llms.txt`: the H1 title, the blockquote summary, then a link list of
//!   the included prerendered routes with their meta descriptions — the
//!   same route list the sitemap walks (`dist/prerendered/**.html` as the
//!   ground truth), filtered by the config's `llms.include` patterns.
//! - `llms-full.txt`: the same header, then per included route the page's
//!   text content (the prerendered HTML's visible text, script/style
//!   stripped) — the Markdown-ish body an LLM reads without a browser.
//!   (The spec's "content collections" source is recorded as a delta: the
//!   config carries no collection→route mapping today, so the prerendered
//!   page — the exact bytes a visitor sees — is the honest source.)
//!
//! The matching is glob-lite and pure (`include_matches`): `**` matches any
//! run of segments, `*` within one segment. `emit_llms` is the one call
//! site the build pipeline runs after the sitemap.

use oxide_fw_config::OxideConfig;
use std::path::Path;

use crate::Writer;

/// Glob-lite route-pattern match (`llms.include` semantics): `**` matches
/// any run of path segments (including none), `*` matches within one
/// segment, everything else is literal. A pattern with no wildcard is a
/// prefix match on the path (`/docs` matches `/docs` and `/docs/x`).
#[must_use]
pub fn include_matches(pattern: &str, path: &str) -> bool {
    // Walk pattern segments against path segments; `**` eats any number of
    // path segments (greedy backtracking via recursion on the tail).
    fn seg_match(p: &[&str], s: &[&str]) -> bool {
        match (p.first(), s.first()) {
            (None, _) => s.is_empty(),
            (Some(&"**"), _) => {
                // `**` at the end eats everything; otherwise try every
                // split point.
                if p.len() == 1 {
                    return true;
                }
                (0..=s.len()).any(|i| seg_match(&p[1..], &s[i..]))
            }
            (Some(p_seg), Some(s_seg)) => {
                s.len() >= 1 && seg_star_match(p_seg, s_seg) && seg_match(&p[1..], &s[1..])
            }
            (Some(_), None) => false,
        }
    }
    // One pattern segment against one path segment: `*` within the segment.
    fn seg_star_match(p: &str, s: &str) -> bool {
        if p == "*" {
            return !s.is_empty();
        }
        if !p.contains('*') {
            return p == s;
        }
        let (head, tail) = p.split_once('*').unwrap_or((p, ""));
        match s.strip_prefix(head) {
            Some(rest) => seg_star_match(tail, rest),
            None => false,
        }
    }
    fn norm(s: &str) -> Vec<&str> {
        s.trim_matches('/')
            .split('/')
            .filter(|x| !x.is_empty())
            .collect()
    }
    let p = norm(pattern);
    let s = norm(path);
    if p.is_empty() {
        // "/" matches only the root.
        return s.is_empty();
    }
    // A wildcard-free single-segment pattern (`/docs`) is a prefix match so
    // section roots behave like section globs.
    if p.len() == 1 && !p[0].contains('*') && p[0] != "**" {
        return s.first() == Some(&p[0]);
    }
    seg_match(&p, &s)
}

/// The `<meta name="description" content="…">` value of a prerendered HTML
/// document, when one is present. Single- and double-quoted content both
/// parse; an empty value is `None`.
#[must_use]
pub fn meta_description(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(i) = lower[from..].find("<meta") {
        let start = from + i;
        let Some(end) = lower[start..].find('>') else {
            break;
        };
        let tag = &lower[start..start + end];
        if tag.contains("name=\"description\"") || tag.contains("name='description'") {
            // The value lives in the ORIGINAL casing string at the same
            // offsets — ASCII-lowercasing preserves byte positions.
            let raw = &html[start..start + end];
            for quote in ['"', '\''] {
                if let Some(ci) = tag.find("content=")
                    && tag[ci + 8..].starts_with(quote)
                {
                    let raw_tail = &raw[ci + 9..];
                    if let Some(close) = raw_tail.find(quote) {
                        let v = raw_tail[..close].to_string();
                        if !v.trim().is_empty() {
                            return Some(v);
                        }
                    }
                }
            }
        }
        from = start + end;
    }
    None
}

/// Strip the invisible subtrees and tags from a prerendered HTML document,
/// leaving the visible text with paragraph-ish newlines — the llms-full.txt
/// body. Script/style contents are dropped entirely; block-level closing
/// tags become newlines.
#[must_use]
pub fn html_text(html: &str) -> String {
    // 1. Drop the invisible subtrees entirely (case-insensitive scan on a
    //    lowercase copy, spliced by byte offsets — ASCII lowering keeps
    //    offsets aligned).
    let lower = html.to_ascii_lowercase();
    let mut drop_ranges: Vec<(usize, usize)> = Vec::new();
    for tag in ["script", "style"] {
        let open = format!("<{tag}");
        let close = format!("</{tag}");
        let mut cursor = 0usize;
        while let Some(i) = lower[cursor..].find(&open) {
            let s = cursor + i;
            let Some(e_rel) = lower[s..].find(&close) else {
                break;
            };
            let e = s + e_rel;
            let Some(gt) = lower[e..].find('>') else {
                break;
            };
            drop_ranges.push((s, e + gt + 1));
            cursor = e + gt + 1;
        }
    }
    let mut kept = String::with_capacity(html.len());
    let mut last = 0usize;
    // The per-tag walks run independently, so the ranges arrive unsorted.
    drop_ranges.sort();
    for (s, e) in &drop_ranges {
        kept.push_str(&html[last..*s]);
        last = *e;
    }
    kept.push_str(&html[last..]);

    // 2. Strip the remaining tags; block-level closers become newlines.
    let mut out = String::with_capacity(kept.len());
    let mut text = String::new();
    let mut chars = kept.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '<' {
            // A closing block tag earns a newline before the next text.
            let lower_seg = &lower[i..];
            for b in [
                "</p>",
                "</div>",
                "</section>",
                "</article>",
                "</h1>",
                "</h2>",
                "</h3>",
                "</h4>",
                "</h5>",
                "</h6>",
                "</li>",
                "</tr>",
                "</blockquote>",
                "</pre>",
            ] {
                if lower_seg.starts_with(b) {
                    text.push('\n');
                    break;
                }
            }
            // Skip to the tag's end.
            for (_, sc) in chars.by_ref() {
                if sc == '>' {
                    break;
                }
            }
        } else {
            text.push(c);
        }
    }
    out.push_str(&text);
    // 3. Collapse the whitespace the strip left behind (entities decoded
    //    minimally: the ones a meta description actually carries).
    let decoded = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'");
    let mut collapsed = String::with_capacity(decoded.len());
    let mut spaces = 0usize;
    for ch in decoded.chars() {
        match ch {
            ' ' | '\t' => {
                spaces += 1;
                if spaces == 1 {
                    collapsed.push(' ');
                }
            }
            '\n' => {
                spaces = 0;
                collapsed.push('\n');
            }
            _ => {
                spaces = 0;
                collapsed.push(ch);
            }
        }
    }
    collapsed.trim().to_string()
}

/// Render `llms.txt`: the H1, the optional blockquote summary, then one
/// link line per included route (`- [{path}]({origin}{path}): {desc}`).
#[must_use]
pub fn llms_txt(title: &str, summary: Option<&str>, rows: &[(String, String)]) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {title}\n\n"));
    if let Some(s) = summary {
        out.push_str(&format!("> {s}\n\n"));
    }
    out.push_str("## Routes\n\n");
    for (path, desc) in rows {
        if desc.is_empty() {
            out.push_str(&format!("- {path}\n"));
        } else {
            out.push_str(&format!("- {path}: {desc}\n"));
        }
    }
    out
}

/// Render `llms-full.txt`: the same header, then per route its visible
/// text content.
#[must_use]
pub fn llms_full_txt(title: &str, summary: Option<&str>, docs: &[(String, String)]) -> String {
    let mut out = llms_txt(title, summary, &[]);
    out.push_str("\n\n");
    for (path, body) in docs {
        out.push_str(&format!("## {path}\n\n{body}\n\n"));
    }
    out
}

/// The one call site the build pipeline runs (after the sitemap): when
/// `llms.enabled`, write `dist/llms.txt` (the included prerendered routes
/// with their meta descriptions) and `dist/llms-full.txt` (the same routes'
/// visible text). `llms.include` filters the prerendered walk — the
/// sitemap's own ground truth; empty means every prerendered page.
pub fn emit_llms(cfg: &OxideConfig, dist: &Path, w: Writer<'_>) -> anyhow::Result<()> {
    if !cfg.llms.enabled {
        return Ok(());
    }
    // The prerendered walk, exactly the sitemap's ground truth: every
    // `dist/prerendered/**.html` file, its site-relative path derived from
    // the layout on disk.
    let prerendered = dist.join("prerendered");
    if !prerendered.is_dir() {
        // A build with no prerendered pages (all dynamic) writes nothing —
        // an llms index over zero routes is noise, not a feature.
        tracing::warn!("llms: no prerendered pages; dist/llms.txt not written");
        return Ok(());
    }
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut docs: Vec<(String, String)> = Vec::new();
    let mut stack = vec![prerendered.clone()];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "html") {
                let rel = match p.strip_prefix(&prerendered) {
                    Ok(r) => r.to_string_lossy().into_owned(),
                    Err(_) => continue,
                };
                let path = if rel == "index.html" {
                    "/".to_string()
                } else {
                    format!(
                        "/{}",
                        rel.trim_end_matches(".html").trim_end_matches("index")
                    )
                };
                let path = path.trim_end_matches('/').to_string();
                let path = if path.is_empty() {
                    "/".to_string()
                } else {
                    path
                };
                let included = cfg.llms.include.is_empty()
                    || cfg
                        .llms
                        .include
                        .iter()
                        .any(|pat| include_matches(pat, &path));
                if !included {
                    continue;
                }
                let Ok(html) = std::fs::read_to_string(&p) else {
                    continue;
                };
                let desc = meta_description(&html).unwrap_or_default();
                rows.push((path.clone(), desc));
                docs.push((path, html_text(&html)));
            }
        }
    }
    rows.sort();
    docs.sort();

    // Title: config wins, then the site name (siteUrl host), then the
    // project directory name — an anonymous site still gets a real H1.
    let title: String = cfg.llms.title.clone().unwrap_or_else(|| {
        cfg.site_url
            .as_ref()
            .and_then(|u| {
                u.trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .split('/')
                    .next()
                    .map(String::from)
            })
            .unwrap_or_else(|| "Site".to_string())
    });
    std::fs::write(
        dist.join("llms.txt"),
        llms_txt(&title, cfg.llms.summary.as_deref(), &rows),
    )?;
    std::fs::write(
        dist.join("llms-full.txt"),
        llms_full_txt(&title, cfg.llms.summary.as_deref(), &docs),
    )?;
    // The outln! macro lives in lib.rs's textual scope; same shape here.
    writeln!(
        w,
        "llms: {} route(s), dist/llms.txt + dist/llms-full.txt",
        rows.len()
    )
    .map_err(|e| anyhow::anyhow!("write failed: {e}"))?;
    Ok(())
}

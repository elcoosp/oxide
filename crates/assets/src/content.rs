//! Content pipeline (spec §16.5): markdown → HTML via `comrak`, YAML-ish
//! frontmatter for typed collections, and build-time highlighting via
//! `syntect`. No in-house CommonMark renderer or tokenizer.

use crate::{ContentEntry, Result, err};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

fn comrak_options<'a>() -> comrak::Options<'a> {
    let mut opts = comrak::Options::default();
    opts.extension.table = true;
    opts.extension.strikethrough = true;
    opts.extension.autolink = true;
    opts.extension.tasklist = true;
    opts.extension.footnotes = true;
    opts.render.github_pre_lang = true;
    opts.render.full_info_string = true;
    opts.render.unsafe_ = false; // sanitize raw HTML (spec §17.1)
    opts
}

/// Markdown → HTML via `comrak`. Raw HTML is escaped (spec §17.1 injection
/// hardening); unsafe content never reaches the output.
pub fn markdown_to_html(md: &str) -> String {
    comrak::markdown_to_html(md, &comrak_options())
}

// ---------------- frontmatter ----------------

fn parse_scalar(raw: &str) -> serde_json::Value {
    let raw = raw.trim();
    if raw.len() >= 2
        && ((raw.starts_with('"') && raw.ends_with('"'))
            || (raw.starts_with('\'') && raw.ends_with('\'')))
    {
        return serde_json::Value::String(raw[1..raw.len() - 1].to_string());
    }
    // §16.5: inline YAML arrays — `tags: [a, b]` and `tags: ["a", "b"]`.
    // A flat-YAML subset that dropped this read the whole `[a, b]` as the
    // string "[a, b]", so a `string[]` field inferred as `string`.
    if raw.len() >= 2 && raw.starts_with('[') && raw.ends_with(']') {
        let inner = &raw[1..raw.len() - 1];
        let items: Vec<serde_json::Value> = if inner.trim().is_empty() {
            Vec::new()
        } else {
            inner.split(',').map(|s| parse_scalar(s.trim())).collect()
        };
        return serde_json::Value::Array(items);
    }
    match raw {
        "true" => serde_json::Value::Bool(true),
        "false" => serde_json::Value::Bool(false),
        "null" | "~" => serde_json::Value::Null,
        _ => {
            if let Ok(n) = raw.parse::<i64>() {
                serde_json::Value::Number(n.into())
            } else if let Ok(f) = raw.parse::<f64>() {
                serde_json::Number::from_f64(f)
                    .map(serde_json::Value::Number)
                    .unwrap_or_else(|| serde_json::Value::String(raw.to_string()))
            } else {
                serde_json::Value::String(raw.to_string())
            }
        }
    }
}

/// Parse a content entry (§16.5): `---`-delimited flat frontmatter plus body
/// markdown. Slug from the filename stem.
pub fn parse_content_entry(path: &Path, source: &str) -> Result<ContentEntry> {
    let slug = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let mut frontmatter = BTreeMap::new();
    let mut body = source.to_string();

    let lines: Vec<&str> = source.lines().collect();
    if !lines.is_empty() && lines[0].trim() == "---" {
        let Some(idx) = lines[1..].iter().position(|l| l.trim() == "---") else {
            return Err(err(
                "AST-FRONT",
                format!(
                    "unclosed frontmatter in {} (missing closing `---`)",
                    path.display()
                ),
            ));
        };
        for line in &lines[1..1 + idx] {
            if let Some((k, v)) = line.split_once(':') {
                let key = k.trim();
                if !key.is_empty() {
                    frontmatter.insert(key.to_string(), parse_scalar(v));
                }
            }
        }
        body = lines[1 + idx + 1..].join("\n");
        if source.ends_with('\n') {
            body.push('\n');
        }
    }

    Ok(ContentEntry {
        path: path.to_path_buf(),
        slug,
        frontmatter,
        body_markdown: body,
    })
}

// ---------------- highlighting (syntect) ----------------

fn syntect_set() -> &'static syntect::parsing::SyntaxSet {
    static SET: OnceLock<syntect::parsing::SyntaxSet> = OnceLock::new();
    SET.get_or_init(syntect::parsing::SyntaxSet::load_defaults_newlines)
}

fn syntect_theme() -> &'static syntect::highlighting::Theme {
    static THEME: OnceLock<syntect::highlighting::Theme> = OnceLock::new();
    THEME.get_or_init(|| {
        syntect::highlighting::ThemeSet::load_defaults()
            .themes
            .get("base16-ocean.dark")
            .cloned()
            .unwrap_or_default()
    })
}

/// Syntax highlighting at build time via `syntect`. Output is `<pre>`-friendly
/// HTML spans (`<span style="color:#...">…</span>`); the caller wraps in
/// `<pre><code>`.
pub fn highlight(code: &str, lang: &str) -> String {
    use syntect::easy::HighlightLines;
    use syntect::html::{IncludeBackground, styled_line_to_highlighted_html};
    use syntect::util::LinesWithEndings;

    let ss = syntect_set();
    let syntax = ss
        .find_syntax_by_token(lang)
        .or_else(|| ss.find_syntax_by_extension(lang))
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let mut h = HighlightLines::new(syntax, syntect_theme());
    let mut out = String::new();
    for line in LinesWithEndings::from(code) {
        match h.highlight_line(line, ss) {
            Ok(ranges) => {
                let html = styled_line_to_highlighted_html(&ranges, IncludeBackground::No)
                    .unwrap_or_else(|_| String::new());
                out.push_str(&html);
            }
            Err(_) => out.push_str(&escape_html(line)),
        }
    }
    out
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ---------------------------------------------------------------------------
// §16.5 MDX bridging
// ---------------------------------------------------------------------------

/// Content source format. MDX compilation is deferred to the tooling pool;
/// `assets` never runs JS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentFormat {
    Markdown,
    Mdx,
}

impl ContentFormat {
    /// Infer from a path's extension. Unknown extensions default to Markdown.
    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|e| e.to_str()) {
            Some("mdx") => ContentFormat::Mdx,
            _ => ContentFormat::Markdown,
        }
    }
}

/// A request to compile MDX source through the tooling pool (§4.7). The
/// description is compiler-agnostic; the current canonical target is
/// `@mdx-js/mdx`, invoked via `ToolingModules::call_module`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MdxCompileRequest {
    pub source: String,
    pub filename: String,
    /// npm module name to `require`.
    pub module: String,
    /// Named export to call.
    pub export: String,
    /// Options passed as the second argument (JSX runtime, remark plugins...).
    pub options: serde_json::Value,
}

impl MdxCompileRequest {
    /// Canonical `@mdx-js/mdx` invocation.
    pub fn mdx_js(source: &str, filename: &str) -> Self {
        Self {
            source: source.to_string(),
            filename: filename.to_string(),
            module: "@mdx-js/mdx".to_string(),
            export: "compile".to_string(),
            options: serde_json::json!({
                "outputFormat": "function-body",
                "providerImportSource": "@mdx-js/react",
            }),
        }
    }
}

/// Parse an entry regardless of format. Frontmatter and body are identical
/// for `.md` and `.mdx` at this stage; MDX differs only once compilation runs.
pub fn parse_content_entry_with_format(
    path: &Path,
    source: &str,
    _format: ContentFormat,
) -> Result<ContentEntry> {
    parse_content_entry(path, source)
}

/// Parse an MDX source: produce the standard `ContentEntry` **and** the
/// tooling-pool request that will compile the body.
pub fn parse_mdx_source(path: &Path, source: &str) -> Result<(ContentEntry, MdxCompileRequest)> {
    let entry = parse_content_entry(path, source)?;
    let filename = path.to_string_lossy().into_owned();
    let request = MdxCompileRequest::mdx_js(&entry.body_markdown, &filename);
    Ok((entry, request))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_renders_headings_and_bold_via_comrak() {
        let out = markdown_to_html("# Title\n\n**bold** text");
        assert!(out.contains("<h1>Title</h1>"));
        assert!(out.contains("<strong>bold</strong>"));
    }

    #[test]
    fn markdown_escapes_raw_html() {
        let out = markdown_to_html("<script>alert(1)</script>");
        assert!(!out.contains("<script>"), "{out}");
    }

    #[test]
    fn highlight_emits_spans_with_colors() {
        let out = highlight("fn main() {}", "rust");
        assert!(out.contains("<span"), "{out}");
        assert!(out.contains("main"));
    }

    #[test]
    fn highlight_unknown_lang_falls_back_to_plain() {
        let out = highlight("some code", "not-a-language");
        assert!(out.contains("some code"));
    }

    #[test]
    fn content_entry_parses_frontmatter() {
        let e = parse_content_entry(
            std::path::Path::new("x/hello.md"),
            "---\ntitle: Hi\ncount: 3\n---\nbody\n",
        )
        .unwrap();
        assert_eq!(e.slug, "hello");
        assert_eq!(
            e.frontmatter.get("title").unwrap(),
            &serde_json::json!("Hi")
        );
        assert_eq!(e.frontmatter.get("count").unwrap(), &serde_json::json!(3));
    }
}

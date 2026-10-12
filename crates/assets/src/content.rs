//! Content pipeline internals (spec §16.5): a deterministic, dependency-free
//! CommonMark *subset* renderer (comrak slots in behind the same signature),
//! YAML-ish frontmatter parsing for typed content collections, and a minimal
//! build-time syntax highlighter (tree-sitter/syntect slot in later).

use crate::{err, ContentEntry, Result};
use std::collections::BTreeMap;
use std::path::Path;

// ---------------- shared HTML escaping ----------------

pub(crate) fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

// ---------------- inline rendering ----------------

/// Inline subset: `**bold**`, `*italic*`, `` `code` ``, `[link](url)`, `![img](url)`.
/// HTML is escaped first; code spans are extracted before emphasis so their content
/// is verbatim. URLs with unbalanced parens are not supported (documented subset).
fn inline(src: &str) -> String {
    let mut s = escape_html(src);

    // Extract code spans into placeholders so emphasis never touches them.
    let mut codes: Vec<String> = Vec::new();
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(pos) = rest.find('`') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos + 1..];
        match rest.find('`') {
            Some(end) => {
                codes.push(format!("<code>{}</code>", &rest[..end]));
                out.push_str(&format!("\u{0}{}\u{0}", codes.len() - 1));
                rest = &rest[end + 1..];
            }
            None => {
                out.push('`');
                break;
            }
        }
    }
    out.push_str(rest);
    s = out;

    s = wrap_pair(&s, "![", "](", ")", |alt, url| {
        format!("<img alt=\"{alt}\" src=\"{url}\">")
    });
    s = wrap_pair(&s, "[", "](", ")", |text, url| {
        format!("<a href=\"{url}\">{text}</a>")
    });
    s = wrap_emphasis(&s, "**", |inner| format!("<strong>{inner}</strong>"));
    s = wrap_emphasis(&s, "*", |inner| format!("<em>{inner}</em>"));

    for (i, code) in codes.into_iter().enumerate() {
        s = s.replace(&format!("\u{0}{i}\u{0}"), &code);
    }
    s
}

fn wrap_pair(
    s: &str,
    open: &str,
    mid: &str,
    close: &str,
    f: impl Fn(&str, &str) -> String,
) -> String {
    let mut out = String::new();
    let mut rest = s;
    'outer: loop {
        match rest.find(open) {
            Some(a) => {
                let after_open = &rest[a + open.len()..];
                if let Some(b) = after_open.find(mid) {
                    let inner = &after_open[..b];
                    let after_mid = &after_open[b + mid.len()..];
                    if let Some(c) = after_mid.find(close) {
                        out.push_str(&rest[..a]);
                        out.push_str(&f(inner, &after_mid[..c]));
                        rest = &after_mid[c + close.len()..];
                        continue;
                    }
                }
                // No complete match: emit the opener and continue after it.
                out.push_str(&rest[..a + open.len()]);
                rest = after_open;
                continue;
            }
            None => break 'outer,
        }
    }
    out.push_str(rest);
    out
}

fn wrap_emphasis(s: &str, marker: &str, f: impl Fn(&str) -> String) -> String {
    let mut out = String::new();
    let mut rest = s;
    loop {
        match rest.find(marker) {
            Some(a) => match rest[a + marker.len()..].find(marker) {
                Some(b) if b > 0 => {
                    out.push_str(&rest[..a]);
                    out.push_str(&f(&rest[a + marker.len()..a + marker.len() + b]));
                    rest = &rest[a + marker.len() + b + marker.len()..];
                }
                _ => {
                    out.push_str(rest);
                    break;
                }
            },
            None => {
                out.push_str(rest);
                break;
            }
        }
    }
    out
}

// ---------------- block rendering ----------------

/// Minimal CommonMark subset (deterministic, no deps): headings `#`–`######`,
/// bold/italic/code inlines, links, images, unordered lists (one nesting level),
/// fenced code blocks (routed through [`crate::highlight`]), blockquotes, `---` hr,
/// paragraphs separated by blank lines. All text is HTML-escaped first.
pub fn markdown_to_html(md: &str) -> String {
    let lines: Vec<&str> = md.lines().collect();
    let mut out = String::new();
    let mut para: Vec<&str> = Vec::new();
    let mut i = 0usize;

    macro_rules! flush {
        () => {
            if !para.is_empty() {
                let text = para.join("\n");
                out.push_str(&format!("<p>{}</p>\n", inline(&text)));
                para.clear();
            }
        };
    }

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        if trimmed.is_empty() {
            flush!();
            i += 1;
            continue;
        }

        // Headings: 1–6 `#` followed by a space.
        let hashes = trimmed.bytes().take_while(|&c| c == b'#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            flush!();
            let text = trimmed[hashes..].trim();
            out.push_str(&format!("<h{hashes}>{}</h{hashes}>\n", inline(text)));
            i += 1;
            continue;
        }

        // Fenced code blocks.
        if let Some(lang) = trimmed.strip_prefix("```") {
            flush!();
            let lang = lang.trim();
            let mut code: Vec<&str> = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim().starts_with("```") {
                code.push(lines[i]);
                i += 1;
            }
            i += 1; // closing fence (or EOF — lenient)
            let body = code.join("\n");
            if lang.is_empty() {
                out.push_str(&format!("<pre><code>{}</code></pre>\n", escape_html(&body)));
            } else {
                out.push_str(&format!(
                    "<pre><code class=\"language-{lang}\">{}</code></pre>\n",
                    crate::highlight(&body, lang)
                ));
            }
            continue;
        }

        // Horizontal rule.
        if trimmed == "---" || trimmed == "***" {
            flush!();
            out.push_str("<hr>\n");
            i += 1;
            continue;
        }

        // Blockquote.
        if trimmed.starts_with('>') {
            flush!();
            let mut quote: Vec<String> = Vec::new();
            while i < lines.len() {
                let l = lines[i].trim();
                match l.strip_prefix('>') {
                    Some(t) => {
                        quote.push(t.trim_start_matches(' ').to_string());
                        i += 1;
                    }
                    None => break,
                }
            }
            let inner = markdown_to_html(&quote.join("\n"));
            out.push_str(&format!(
                "<blockquote>{}</blockquote>\n",
                inner.trim_end_matches('\n')
            ));
            continue;
        }

        // Unordered lists (one nesting level: sub-items indented 2+ spaces).
        if trimmed.starts_with("- ") {
            flush!();
            let mut items: Vec<(String, Vec<String>)> = Vec::new();
            while i < lines.len() {
                let l = lines[i];
                if let Some(t) = l.strip_prefix("- ") {
                    items.push((t.to_string(), Vec::new()));
                    i += 1;
                    continue;
                }
                let indent = l.len() - l.trim_start().len();
                if indent >= 2 && !items.is_empty() {
                    if let Some(t) = l.trim_start().strip_prefix("- ") {
                        items.last_mut().expect("checked").1.push(t.to_string());
                        i += 1;
                        continue;
                    }
                }
                break;
            }
            let mut ul = String::from("<ul>");
            for (text, subs) in items {
                ul.push_str(&format!("<li>{}", inline(&text)));
                if !subs.is_empty() {
                    ul.push_str("<ul>");
                    for s in &subs {
                        ul.push_str(&format!("<li>{}</li>", inline(s)));
                    }
                    ul.push_str("</ul>");
                }
                ul.push_str("</li>");
            }
            ul.push_str("</ul>");
            out.push_str(&ul);
            out.push('\n');
            continue;
        }

        para.push(line);
        i += 1;
    }
    flush!();
    out
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

/// Parse a content entry (§16.5): `---`-delimited YAML-ish frontmatter of flat
/// `key: value` lines (strings/numbers/bools), body markdown after the closing
/// delimiter, `slug` from the filename stem. Missing frontmatter → empty map and
/// the whole source as body. Unclosed frontmatter → `AST-FRONT`.
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

// ---------------- highlighting ----------------

const KW_RUST: &[&str] = &[
    "as", "const", "crate", "enum", "fn", "impl", "let", "match", "mod", "mut", "pub", "return",
    "self", "struct", "trait", "use", "where", "Self", "true", "false",
];
const KW_PY: &[&str] = &[
    "and", "as", "class", "def", "elif", "else", "except", "False", "for", "from", "if", "import",
    "in", "is", "lambda", "None", "not", "or", "pass", "return", "self", "True", "try", "while",
    "with",
];
const KW_JS: &[&str] = &[
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "else",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "from",
    "function",
    "if",
    "import",
    "let",
    "new",
    "null",
    "return",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "undefined",
    "var",
    "while",
];

fn span(cls: &str, text: &str) -> String {
    format!("<span class=\"{cls}\">{}</span>", escape_html(text))
}

/// Minimal tokenizer (§16.5): keywords per language (js/ts/tsx share the JS set;
/// rust and py have their own), strings, `//`/`/* */` comments (`#` for py/sh/yaml),
/// and numbers — emitted as `tok-kw`/`tok-str`/`tok-com`/`tok-num` spans. Content is
/// escaped per token, so output is always injection-safe. Unknown languages get
/// strings + numbers only (no keywords, no comment detection).
pub fn highlight(code: &str, lang: &str) -> String {
    let keywords: &[&str] = match lang {
        "rust" | "rs" => KW_RUST,
        "py" | "python" => KW_PY,
        "js" | "jsx" | "ts" | "tsx" | "javascript" | "typescript" => KW_JS,
        _ => &[],
    };
    let hash_comments = matches!(
        lang,
        "py" | "python" | "sh" | "bash" | "yaml" | "yml" | "toml"
    );
    let is_rust = matches!(lang, "rust" | "rs");
    let is_js = matches!(
        lang,
        "js" | "jsx" | "ts" | "tsx" | "javascript" | "typescript"
    );

    let chars: Vec<char> = code.chars().collect();
    let mut out = String::new();
    let mut plain = String::new();
    let mut i = 0usize;

    macro_rules! flush_plain {
        () => {
            if !plain.is_empty() {
                out.push_str(&escape_html(&plain));
                plain.clear();
            }
        };
    }

    while i < chars.len() {
        let c = chars[i];

        // Comments.
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            flush_plain!();
            let start = i;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            out.push_str(&span(
                "tok-com",
                &chars[start..i].iter().collect::<String>(),
            ));
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            flush_plain!();
            let start = i;
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(chars.len());
            out.push_str(&span(
                "tok-com",
                &chars[start..i].iter().collect::<String>(),
            ));
            continue;
        }
        if hash_comments && c == '#' {
            flush_plain!();
            let start = i;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            out.push_str(&span(
                "tok-com",
                &chars[start..i].iter().collect::<String>(),
            ));
            continue;
        }

        // Strings (JS template literals included; rust handled below).
        if (c == '"' || c == '\'' || (is_js && c == '`')) && !is_rust {
            flush_plain!();
            let start = i;
            i += 1;
            while i < chars.len() {
                let d = chars[i];
                i += 1;
                if d == '\\' && i < chars.len() {
                    i += 1;
                    continue;
                }
                if d == c || d == '\n' {
                    break;
                }
            }
            out.push_str(&span(
                "tok-str",
                &chars[start..i].iter().collect::<String>(),
            ));
            continue;
        }

        // Rust: char literals vs lifetimes, then plain strings.
        if is_rust && c == '\'' {
            if chars.get(i + 2) == Some(&'\'') && chars[i + 1] != '\\' {
                flush_plain!();
                out.push_str(&span(
                    "tok-str",
                    &chars[i..i + 3].iter().collect::<String>(),
                ));
                i += 3;
                continue;
            }
            if chars
                .get(i + 1)
                .is_some_and(|n| n.is_alphabetic() || *n == '_')
            {
                // Lifetime like `'a` — plain text.
                plain.push('\'');
                i += 1;
                continue;
            }
            flush_plain!();
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '\'' && chars[i] != '\n' {
                i += 1;
            }
            i = (i + 1).min(chars.len());
            out.push_str(&span(
                "tok-str",
                &chars[start..i].iter().collect::<String>(),
            ));
            continue;
        }
        if is_rust && c == '"' {
            flush_plain!();
            let start = i;
            i += 1;
            while i < chars.len() {
                let d = chars[i];
                i += 1;
                if d == '\\' && i < chars.len() {
                    i += 1;
                    continue;
                }
                if d == '"' {
                    break;
                }
            }
            out.push_str(&span(
                "tok-str",
                &chars[start..i].iter().collect::<String>(),
            ));
            continue;
        }

        // Numbers.
        if c.is_ascii_digit() {
            flush_plain!();
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            out.push_str(&span(
                "tok-num",
                &chars[start..i].iter().collect::<String>(),
            ));
            continue;
        }

        // Identifiers / keywords.
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if keywords.contains(&word.as_str()) {
                flush_plain!();
                out.push_str(&span("tok-kw", &word));
            } else {
                plain.push_str(&word);
            }
            continue;
        }

        plain.push(c);
        i += 1;
    }
    flush_plain!();
    out
}

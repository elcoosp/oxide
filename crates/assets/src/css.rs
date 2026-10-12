//! DefaultCss internals (spec §6.6, D5): a small hand-rolled CSS engine providing
//! the Lightning-CSS *shape* of the pipeline — parse, nesting flatten, CSS-modules
//! scoping, autoprefix subset, minify — without pulling lightningcss in.
//!
//! Documented scope decisions (Task 9):
//! - Escaped class names (`.foo\:bar`) are not scoped (rare; lightningcss swap later).
//! - `@media` nested inside a style rule is hoisted out with the parent selector
//!   applied inside (postcss-nesting behaviour); `@media` inside `@media` stays nested.
//! - Targets (`["chrome 100", ...]`) are accepted and stored but do not yet influence
//!   prefix emission; the prefix set is fixed (matches the "always prefix" simplification).
//! - An `@media` that becomes completely empty is dropped; empty *style* rules are
//!   always dropped; statement at-rules (`@import`/`@charset`) are kept verbatim.

use crate::{err, CssBackend, CssInput, CssKind, CssOutput, Diagnostic, Result};
use blake3::Hasher;
use oxide_diagnostics::{Location, OxideError};
use std::collections::{BTreeMap, BTreeSet};

// ---------------- parser ----------------

#[derive(Debug, Clone)]
struct Decl {
    prop: String,
    value: String,
}

#[derive(Debug, Clone)]
enum Node {
    /// Style rule: selector + own declarations + nested children (style or at rules).
    Style {
        selector: String,
        decls: Vec<Decl>,
        children: Vec<Node>,
    },
    /// At-rule with a block body (`@media`, `@keyframes`, ...).
    At { prelude: String, body: Vec<Node> },
    /// Statement at-rule terminated by `;` (`@import`, `@charset`, ...).
    Stmt(String),
}

struct Parser<'a> {
    src: &'a [u8],
    i: usize,
    line: u32,
    file: Option<String>,
}

impl<'a> Parser<'a> {
    fn css_err(&self, msg: impl Into<String>) -> OxideError {
        // `crate::Result` alias — keep the code stable: AST-CSS.
        let mut e = err("AST-CSS", msg.into());
        e.0.location = Some(Location {
            file: self.file.clone(),
            line: self.line,
            column: 0,
            offset: self.i as u32,
            length: 0,
        });
        e
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.i).copied()
    }

    fn eof(&self) -> bool {
        self.i >= self.src.len()
    }

    fn skip_trivia(&mut self) {
        loop {
            let before = self.i;
            while let Some(c) = self.peek() {
                if matches!(c, b' ' | b'\t' | b'\r' | b'\n') {
                    if c == b'\n' {
                        self.line += 1;
                    }
                    self.i += 1;
                } else {
                    break;
                }
            }
            // /* comments */ — not inside strings (this helper only runs outside strings).
            if self.peek() == Some(b'/') && self.src.get(self.i + 1) == Some(&b'*') {
                self.i += 2;
                while self.i < self.src.len() {
                    if self.src[self.i] == b'*' && self.src.get(self.i + 1) == Some(&b'/') {
                        self.i += 2;
                        break;
                    }
                    if self.src[self.i] == b'\n' {
                        self.line += 1;
                    }
                    self.i += 1;
                }
                continue;
            }
            if self.i == before {
                return;
            }
        }
    }

    /// Read raw text until one of `stops` at top level, tracking strings and parens
    /// (so `url(data:...;base64,...)` never trips the `;` stop). Byte-level scanning
    /// is UTF-8 safe: stop bytes are ASCII and never occur inside multi-byte chars.
    fn read_until(&mut self, stops: &[u8]) -> String {
        let mut out: Vec<u8> = Vec::new();
        let mut paren = 0i32;
        let mut quote: Option<u8> = None;
        while let Some(c) = self.peek() {
            if let Some(q) = quote {
                out.push(c);
                self.i += 1;
                if c == b'\\' {
                    if let Some(n) = self.peek() {
                        if n == b'\n' {
                            self.line += 1;
                        }
                        out.push(n);
                        self.i += 1;
                    }
                } else {
                    if c == q {
                        quote = None;
                    }
                    if c == b'\n' {
                        self.line += 1;
                    }
                }
                continue;
            }
            if c == b'"' || c == b'\'' {
                quote = Some(c);
                out.push(c);
                self.i += 1;
                continue;
            }
            if c == b'(' {
                paren += 1;
            }
            if c == b')' {
                paren -= 1;
            }
            if paren == 0 && stops.contains(&c) {
                break;
            }
            if c == b'\n' {
                self.line += 1;
            }
            out.push(c);
            self.i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn parse_nodes(&mut self, top: bool) -> Result<Vec<Node>> {
        let mut nodes = Vec::new();
        loop {
            self.skip_trivia();
            if self.eof() {
                if top {
                    return Ok(nodes);
                }
                return Err(self.css_err("unexpected end of input: unclosed block (missing `}`)"));
            }
            match self.peek() {
                Some(b'}') => {
                    if top {
                        return Err(self.css_err("unbalanced `}` with no open block"));
                    }
                    self.i += 1;
                    return Ok(nodes);
                }
                Some(b'@') => nodes.push(self.parse_at()?),
                _ => nodes.push(self.parse_style()?),
            }
        }
    }

    fn parse_at(&mut self) -> Result<Node> {
        let prelude = self.read_until(&[b'{', b';']);
        if self.eof() {
            return Err(self.css_err(format!("unclosed at-rule `{}`", prelude.trim())));
        }
        if self.peek() == Some(b';') {
            self.i += 1;
            return Ok(Node::Stmt(prelude.trim().to_string()));
        }
        self.i += 1; // '{'
        let body = self.parse_nodes(false)?;
        Ok(Node::At {
            prelude: prelude.trim().to_string(),
            body,
        })
    }

    fn parse_style(&mut self) -> Result<Node> {
        self.skip_trivia();
        let selector = self.read_until(&[b'{']).trim().to_string();
        if self.peek() != Some(b'{') {
            return Err(self.css_err(format!(
                "unclosed rule: expected `{{` for selector `{selector}`"
            )));
        }
        self.i += 1;
        self.parse_block(selector)
    }

    fn parse_block(&mut self, selector: String) -> Result<Node> {
        let mut decls: Vec<Decl> = Vec::new();
        let mut children: Vec<Node> = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                None => return Err(self.css_err("unclosed block (missing `}`)")),
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Node::Style {
                        selector,
                        decls,
                        children,
                    });
                }
                _ => {}
            }
            let chunk = self.read_until(&[b';', b'{', b'}']);
            match self.peek() {
                Some(b'{') => {
                    self.i += 1;
                    let trimmed = chunk.trim();
                    if trimmed.starts_with('@') {
                        // Nested at-rule (e.g. `@media` inside a style rule): its body
                        // may hold bare declarations for the parent selector, so parse
                        // it as an anonymous block; flatten attaches the parent later.
                        let inner = self.parse_block(String::new())?;
                        children.push(Node::At {
                            prelude: trimmed.to_string(),
                            body: vec![inner],
                        });
                    } else {
                        children.push(self.parse_block(trimmed.to_string())?);
                    }
                }
                Some(b';') => {
                    self.i += 1;
                    push_decl(&mut decls, &chunk);
                }
                _ => push_decl(&mut decls, &chunk), // stopped at `}`: last decl; loop consumes `}`
            }
        }
    }
}

fn push_decl(decls: &mut Vec<Decl>, chunk: &str) {
    let chunk = chunk.trim();
    if chunk.is_empty() {
        return;
    }
    // First `:` outside parens/strings splits property from value.
    let bytes = chunk.as_bytes();
    let mut paren = 0i32;
    let mut quote: Option<u8> = None;
    let mut colon = None;
    for (idx, &c) in bytes.iter().enumerate() {
        match quote {
            Some(q) => {
                if c == b'\\' {
                    continue;
                }
                if c == q {
                    quote = None;
                }
            }
            None => {
                if c == b'"' || c == b'\'' {
                    quote = Some(c);
                } else if c == b'(' {
                    paren += 1;
                } else if c == b')' {
                    paren -= 1;
                } else if c == b':' && paren == 0 {
                    colon = Some(idx);
                    break;
                }
            }
        }
    }
    let Some(ci) = colon else { return };
    let prop = chunk[..ci].trim();
    let value = chunk[ci + 1..].trim();
    if prop.is_empty() || value.is_empty() {
        return;
    }
    decls.push(Decl {
        prop: prop.to_string(),
        value: value.to_string(),
    });
}

fn parse_stylesheet(src: &str, file: Option<&str>) -> Result<Vec<Node>> {
    let mut p = Parser {
        src: src.as_bytes(),
        i: 0,
        line: 1,
        file: file.map(str::to_string),
    };
    p.parse_nodes(true)
}

// ---------------- nesting flatten ----------------

/// Ordered flat output: source order of rules / at-rules / statement at-rules is
/// preserved (CSS cascade order must not change).
#[derive(Default)]
struct Flat {
    items: Vec<FlatItem>,
}

enum FlatItem {
    Rule(String, Vec<Decl>),
    At(String, Flat),
    Stmt(String),
}

fn flatten_nodes(nodes: &[Node], parent: Option<&str>, out: &mut Flat) {
    for n in nodes {
        match n {
            Node::Stmt(s) => out.items.push(FlatItem::Stmt(s.clone())),
            Node::At { prelude, body } => {
                let mut sub = Flat::default();
                if !body.is_empty() {
                    flatten_nodes(body, parent, &mut sub);
                }
                out.items.push(FlatItem::At(prelude.clone(), sub));
            }
            Node::Style {
                selector,
                decls,
                children,
            } => {
                let sel = match parent {
                    Some(p) if selector.contains('&') => selector.replace('&', p),
                    Some(p) if selector.is_empty() => p.to_string(),
                    Some(p) if !p.is_empty() => format!("{p} {selector}"),
                    _ => selector.clone(),
                };
                if !decls.is_empty() {
                    out.items.push(FlatItem::Rule(sel.clone(), decls.clone()));
                }
                if !children.is_empty() {
                    flatten_nodes(children, Some(&sel), out);
                }
            }
        }
    }
}

// ---------------- CSS modules scoping (§6.6) ----------------

/// `.foo` → `.ox_<blake3(file||foo)[..6]hex>_foo`.
fn scope_name(file: &str, name: &str) -> String {
    let mut h = Hasher::new();
    h.update(file.as_bytes());
    h.update(name.as_bytes());
    let hex = h.finalize().to_hex().to_string();
    format!("ox_{}_{}", &hex[..6], name)
}

fn scope_selector(sel: &str, file: &str, exports: &mut BTreeMap<String, String>) -> String {
    let b = sel.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0usize;
    let mut paren = 0i32;
    while i < b.len() {
        let c = b[i];
        if c == b'(' {
            paren += 1;
            out.push(c);
            i += 1;
            continue;
        }
        if c == b')' {
            paren -= 1;
            out.push(c);
            i += 1;
            continue;
        }
        if paren == 0 && c == b'.' {
            // A `.` starts a class token unless escaped (`\.`). The previous char may
            // be an identifier char in compound selectors (`.foo.bar`) — that still
            // scopes the second class.
            let prev_ok = i == 0 || b[i - 1] != b'\\';
            let next_ok = matches!(
                b.get(i + 1),
                Some(b'a'..=b'z') | Some(b'A'..=b'Z') | Some(b'_') | Some(b'-')
            );
            if prev_ok && next_ok {
                let start = i + 1;
                let mut end = start;
                while end < b.len()
                    && matches!(b[end], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-')
                {
                    end += 1;
                }
                let name = &sel[start..end];
                let scoped = scope_name(file, name);
                exports.entry(name.to_string()).or_insert(scoped.clone());
                out.extend_from_slice(format!(".{scoped}").as_bytes());
                i = end;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn scope_flat(
    flat: &mut Flat,
    file: &str,
    in_keyframes: bool,
    exports: &mut BTreeMap<String, String>,
    keyframes: &mut BTreeMap<String, String>,
) {
    for item in flat.items.iter_mut() {
        match item {
            FlatItem::At(prelude, sub) => {
                let is_kf =
                    prelude.starts_with("@keyframes") || prelude.starts_with("@-webkit-keyframes");
                if is_kf {
                    // Scope the animation name (last whitespace-separated token).
                    if let Some(name) = prelude.split_whitespace().last().map(str::to_string) {
                        let scoped = scope_name(file, &name);
                        keyframes.insert(name.clone(), scoped.clone());
                        *prelude = format!("{}{}", &prelude[..prelude.len() - name.len()], scoped);
                    }
                }
                scope_flat(sub, file, in_keyframes || is_kf, exports, keyframes);
            }
            FlatItem::Rule(sel, _) if !in_keyframes => {
                // `from`/`to`/percent selectors inside @keyframes are left alone.
                *sel = scope_selector(sel, file, exports);
            }
            FlatItem::Rule(..) | FlatItem::Stmt(_) => {}
        }
    }
}

/// Replace identifier tokens in `animation`/`animation-name` values with scoped
/// keyframes names (word-exact matches; `spin-2` never matches `spin`).
fn replace_identifiers(value: &str, map: &BTreeMap<String, String>) -> String {
    if map.is_empty() {
        return value.to_string();
    }
    let b = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        let ident_start = c.is_ascii_alphabetic() || c == b'_' || c == b'-';
        if ident_start {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'-') {
                i += 1;
            }
            let word = &value[start..i];
            out.extend_from_slice(map.get(word).map(String::as_str).unwrap_or(word).as_bytes());
        } else {
            out.push(c);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn rewrite_animations(flat: &mut Flat, keyframes: &BTreeMap<String, String>) {
    for item in flat.items.iter_mut() {
        match item {
            FlatItem::Rule(_, decls) => {
                for d in decls.iter_mut() {
                    if d.prop == "animation" || d.prop == "animation-name" {
                        d.value = replace_identifiers(&d.value, keyframes);
                    }
                }
            }
            FlatItem::At(_, sub) => rewrite_animations(sub, keyframes),
            FlatItem::Stmt(_) => {}
        }
    }
}

// ---------------- autoprefix subset ----------------

/// Fixed prefix set (task spec): `targets` do not yet gate emission.
const PREFIXES: &[(&str, &[&str])] = &[
    ("transform", &["-webkit-transform", "-ms-transform"]),
    ("user-select", &["-webkit-user-select", "-moz-user-select"]),
    ("appearance", &["-webkit-appearance"]),
    ("backdrop-filter", &["-webkit-backdrop-filter"]),
];

fn autoprefix(decls: &[Decl]) -> Vec<Decl> {
    let mut out = Vec::with_capacity(decls.len() + 2);
    for d in decls {
        if let Some((_, prefixes)) = PREFIXES.iter().find(|(p, _)| *p == d.prop) {
            for prefixed in *prefixes {
                out.push(Decl {
                    prop: prefixed.to_string(),
                    value: d.value.clone(),
                });
            }
        }
        out.push(d.clone());
    }
    out
}

// ---------------- serialization ----------------

fn serialize(flat: &Flat, out: &mut String, pretty: bool) {
    let nl = if pretty { "\n" } else { "" };
    for item in &flat.items {
        match item {
            FlatItem::Stmt(s) => {
                out.push_str(s);
                out.push(';');
                out.push_str(nl);
            }
            FlatItem::Rule(sel, decls) => {
                if decls.is_empty() {
                    continue; // empty style rules never serialize
                }
                out.push_str(sel);
                out.push('{');
                for (i, d) in autoprefix(decls).into_iter().enumerate() {
                    if i > 0 {
                        out.push(';');
                    }
                    out.push_str(&d.prop);
                    out.push(':');
                    if pretty {
                        out.push(' ');
                    }
                    out.push_str(&d.value);
                }
                out.push('}');
                out.push_str(nl);
            }
            FlatItem::At(prelude, sub) => {
                out.push_str(prelude);
                out.push('{');
                out.push_str(nl);
                serialize(sub, out, pretty);
                out.push('}');
                out.push_str(nl);
            }
        }
    }
}

// ---------------- minify ----------------

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

fn minify_css(css: &str) -> String {
    let b = css.as_bytes();

    // Pass 1: collapse whitespace runs to one space, drop spaces around {}:;,>,
    // strings are preserved verbatim.
    let mut pass1: Vec<u8> = Vec::with_capacity(b.len());
    let mut pending_ws = false;
    let mut just_punct = false;
    let mut quote: Option<u8> = None;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if let Some(q) = quote {
            pass1.push(c);
            if c == b'\\' && i + 1 < b.len() {
                pass1.push(b[i + 1]);
                i += 1;
            } else if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == b'"' || c == b'\'' {
            if pending_ws {
                pass1.push(b' ');
                pending_ws = false;
            }
            just_punct = false;
            quote = Some(c);
            pass1.push(c);
            i += 1;
            continue;
        }
        if is_ws(c) {
            if !just_punct {
                pending_ws = true;
            }
            i += 1;
            continue;
        }
        if matches!(c, b'{' | b'}' | b':' | b';' | b',' | b'>') {
            pending_ws = false;
            just_punct = true;
            pass1.push(c);
            i += 1;
            continue;
        }
        if pending_ws {
            pass1.push(b' ');
            pending_ws = false;
        }
        just_punct = false;
        pass1.push(c);
        i += 1;
    }

    // Pass 2: drop final `;` before `}` (outside strings).
    let mut pass2: Vec<u8> = Vec::with_capacity(pass1.len());
    let mut quote = None;
    let mut j = 0usize;
    while j < pass1.len() {
        let c = pass1[j];
        if let Some(q) = quote {
            pass2.push(c);
            if c == b'\\' && j + 1 < pass1.len() {
                pass2.push(pass1[j + 1]);
                j += 1;
            } else if c == q {
                quote = None;
            }
            j += 1;
            continue;
        }
        if c == b'"' || c == b'\'' {
            quote = Some(c);
            pass2.push(c);
            j += 1;
            continue;
        }
        if c == b';' && pass1.get(j + 1) == Some(&b'}') {
            j += 1; // skip the semicolon, keep the brace
            continue;
        }
        pass2.push(c);
        j += 1;
    }

    // Pass 3: remove empty rules (`sel{}`), keeping @-rules (documented).
    let mut out: Vec<u8> = Vec::with_capacity(pass2.len());
    let mut i = 0usize;
    let mut quote: Option<u8> = None;
    while i < pass2.len() {
        let c = pass2[i];
        if let Some(q) = quote {
            out.push(c);
            if c == b'\\' && i + 1 < pass2.len() {
                out.push(pass2[i + 1]);
                i += 1;
            } else if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == b'"' || c == b'\'' {
            quote = Some(c);
            out.push(c);
            i += 1;
            continue;
        }
        if c == b'{' && pass2.get(i + 1) == Some(&b'}') {
            // Back-scan `out` to the start of the selector segment.
            let mut start = out.len();
            while start > 0 && out[start - 1] != b'}' && out[start - 1] != b'{' {
                start -= 1;
            }
            let segment = String::from_utf8_lossy(&out[start..]).trim().to_string();
            if segment.starts_with('@') {
                out.push(c);
                out.push(b'}');
                i += 2;
                continue;
            }
            out.truncate(start); // drop the selector + the empty block
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------- public entry ----------------

impl CssBackend for crate::DefaultCss {
    fn transform(&self, input: &CssInput) -> Result<CssOutput> {
        let nodes = parse_stylesheet(&input.code, input.file.as_deref())?;
        let mut flat = Flat::default();
        flatten_nodes(&nodes, None, &mut flat);

        let file = input.file.clone().unwrap_or_default();
        let mut exports = BTreeMap::new();
        if input.kind == CssKind::Module {
            let mut keyframes = BTreeMap::new();
            scope_flat(&mut flat, &file, false, &mut exports, &mut keyframes);
            rewrite_animations(&mut flat, &keyframes);
        }

        let mut code = String::new();
        serialize(&flat, &mut code, !input.minify);
        if input.minify {
            code = minify_css(&code);
        }
        Ok(CssOutput {
            code,
            exports,
            diagnostics: Vec::<Diagnostic>::new(),
        })
    }
}

/// Tailwind v4 candidate scanner (§6.6): class-like tokens from `class`/`className`/
/// `class:list`/`:class` attribute values. Split on whitespace, deduped and sorted.
/// Only attribute values are considered — utility-looking words in plain code/text
/// are NOT candidates (the engine itself lives in the tailwind plugin).
pub fn scan_tailwind_candidates(sources: &[&str]) -> Vec<String> {
    let mut set = BTreeSet::new();
    for src in sources {
        scan_source(src, &mut set);
    }
    set.into_iter().collect()
}

fn scan_source(src: &str, out: &mut std::collections::BTreeSet<String>) {
    const ATTRS: [&str; 4] = ["class=", "className=", "class:list=", ":class="];
    for attr in ATTRS {
        let mut from = 0usize;
        while let Some(rel) = src[from..].find(attr) {
            let vstart = from + rel + attr.len();
            let bytes = src.as_bytes();
            let Some(&q) = bytes.get(vstart) else { break };
            if q != b'"' && q != b'\'' {
                from = vstart.max(from + 1);
                continue;
            }
            let Some(end_rel) = src[vstart + 1..].find(q as char) else {
                break;
            };
            let vend = vstart + 1 + end_rel;
            let mut val = &src[vstart + 1..vend];
            // Vue/Astro expression form: :class="'a b'" — strip one layer of quotes.
            if val.len() >= 2
                && ((val.starts_with('\'') && val.ends_with('\''))
                    || (val.starts_with('"') && val.ends_with('"')))
            {
                val = &val[1..val.len() - 1];
            }
            for tok in val.split_whitespace() {
                out.insert(tok.to_string());
            }
            from = vend + 1;
            if from >= src.len() {
                break;
            }
        }
    }
}

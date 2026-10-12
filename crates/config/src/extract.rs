//! Static extraction of `defineConfig({...})` literals from TS source (D14, §14.2).
//!
//! A hand-rolled recursive-descent scanner over a deliberately small literal
//! subset of TS: object/array literals, string & template literals (templates
//! only **without** `${…}` interpolation), numbers, booleans, null, comments,
//! trailing commas and — as factory calls with literal object arguments — the
//! known adapter/plugin factories (`react`/`vue`/`svelte`/`solid` → the
//! `framework` object, `tailwind` → the `{ name: "tailwind" }` plugin).
//! Anything else (identifier references, arithmetic, functions, `process.env`,
//! unknown factories, spread of variables) makes the whole extraction return
//! `None` so the caller falls back to V8 evaluation.

use serde_json::{Map, Value};

/// Known factory functions and the config slot they resolve to.
/// `react|vue|svelte|solid` fill the `framework` object, `tailwind` is a plugin.
fn known_factory(name: &str) -> bool {
    matches!(name, "react" | "vue" | "svelte" | "solid" | "tailwind")
}

pub(crate) fn extract(source: &str) -> Option<Value> {
    let bytes = source.as_bytes();
    let needle = b"defineConfig";
    let mut from = 0;
    while let Some(rel) = find(bytes.get(from..)?, needle) {
        let start = from + rel + needle.len();
        let mut sc = Scanner {
            s: source,
            p: start,
        };
        sc.skip_trivia()?;
        if sc.peek() != Some('(') {
            // e.g. the `import { defineConfig } from 'oxide'` binding — keep looking
            from = start;
            continue;
        }
        sc.bump();
        let value = sc.parse_value()?;
        // Trailing tokens (`as const`, `satisfies …`) after the object are fine
        // to ignore; the object itself is the config.
        return Some(value);
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '$'
}

fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

struct Scanner<'a> {
    s: &'a str,
    p: usize,
}

impl<'a> Scanner<'a> {
    fn peek(&self) -> Option<char> {
        self.s[self.p..].chars().next()
    }

    fn peek_is(&self, c: char) -> bool {
        self.peek() == Some(c)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.p += c.len_utf8();
        Some(c)
    }

    /// Skip whitespace and `//` / `/* */` comments. `None` on an unterminated
    /// block comment.
    fn skip_trivia(&mut self) -> Option<()> {
        loop {
            match self.peek()? {
                c if c.is_whitespace() => {
                    self.bump();
                }
                '/' if self.next_after_slash_is('/') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                '/' if self.next_after_slash_is('*') => {
                    self.bump();
                    self.bump();
                    loop {
                        match self.peek()? {
                            '*' if self.s[self.p + 1..].starts_with('/') => {
                                self.bump();
                                self.bump();
                                break;
                            }
                            _ => {
                                self.bump();
                            }
                        }
                    }
                }
                _ => return Some(()),
            }
        }
    }

    /// `peek() == '/'` precondition; checks the *second* char without consuming.
    fn next_after_slash_is(&self, c: char) -> bool {
        self.s[self.p + 1..].chars().next() == Some(c)
    }

    fn parse_value(&mut self) -> Option<Value> {
        self.skip_trivia()?;
        match self.peek()? {
            '{' => self.parse_object(),
            '[' => self.parse_array(),
            q @ ('"' | '\'') => self.parse_string(q).map(Value::String),
            '`' => self.parse_template().map(Value::String),
            c if c == '-' || c == '+' || c.is_ascii_digit() => self.parse_number(),
            c if is_ident_start(c) => {
                let ident = self.parse_ident()?;
                match ident.as_str() {
                    "true" => Some(Value::Bool(true)),
                    "false" => Some(Value::Bool(false)),
                    "null" => Some(Value::Null),
                    name => {
                        self.skip_trivia()?;
                        if self.peek_is('(') {
                            self.parse_factory_call(name)
                        } else {
                            // Identifier reference (variable, process.env, …) → dynamic.
                            None
                        }
                    }
                }
            }
            _ => None,
        }
    }

    /// `react({ compiler: true })` & friends. Every argument must be a literal
    /// object; unknown factories make the extraction dynamic (`None`).
    fn parse_factory_call(&mut self, name: &str) -> Option<Value> {
        debug_assert!(self.peek_is('('));
        self.bump()?;
        let mut args: Vec<Value> = Vec::new();
        loop {
            self.skip_trivia()?;
            match self.peek()? {
                ')' => {
                    self.bump();
                    break;
                }
                ',' => {
                    self.bump();
                }
                _ => {
                    args.push(self.parse_value()?);
                    self.skip_trivia()?;
                    match self.peek()? {
                        ',' => {
                            self.bump();
                        }
                        ')' => {
                            self.bump();
                            break;
                        }
                        _ => return None,
                    }
                }
            }
        }
        if !known_factory(name) {
            return None;
        }
        let mut obj = Map::new();
        match name {
            "tailwind" => {
                obj.insert("name".into(), Value::String("tailwind".into()));
            }
            framework => {
                obj.insert("name".into(), Value::String(framework.into()));
                obj.insert("adapter".into(), Value::String(framework.into()));
            }
        }
        for arg in args {
            let Value::Object(fields) = arg else {
                return None; // non-object literal argument → dynamic
            };
            for (k, v) in fields {
                obj.insert(k, v);
            }
        }
        Some(Value::Object(obj))
    }

    fn parse_object(&mut self) -> Option<Value> {
        self.skip_trivia()?;
        if !self.peek_is('{') {
            return None;
        }
        self.bump();
        let mut map = Map::new();
        loop {
            self.skip_trivia()?;
            match self.peek()? {
                '}' => {
                    self.bump();
                    return Some(Value::Object(map));
                }
                ',' => {
                    self.bump(); // stray or trailing comma
                }
                '.' if self.spread_marker() => {
                    self.p += 3; // consume `...`
                    self.skip_trivia()?;
                    let ident = self.parse_ident()?;
                    self.skip_trivia()?;
                    if !self.peek_is('(') {
                        return None; // spread of a variable → dynamic
                    }
                    let v = self.parse_factory_call(&ident)?;
                    let Value::Object(fields) = v else {
                        return None;
                    };
                    for (k, val) in fields {
                        map.insert(k, val);
                    }
                }
                _ => {
                    let key = match self.peek()? {
                        q @ ('"' | '\'') => self.parse_string(q)?,
                        c if is_ident_start(c) => self.parse_ident()?,
                        _ => return None,
                    };
                    self.skip_trivia()?;
                    if !self.peek_is(':') {
                        return None;
                    }
                    self.bump();
                    let v = self.parse_value()?;
                    map.insert(key, v);
                }
            }
        }
    }

    fn spread_marker(&self) -> bool {
        self.s[self.p..].starts_with("...")
    }

    fn parse_array(&mut self) -> Option<Value> {
        self.skip_trivia()?;
        if !self.peek_is('[') {
            return None;
        }
        self.bump();
        let mut items = Vec::new();
        loop {
            self.skip_trivia()?;
            match self.peek()? {
                ']' => {
                    self.bump();
                    return Some(Value::Array(items));
                }
                ',' => {
                    self.bump();
                }
                _ => {
                    items.push(self.parse_value()?);
                    self.skip_trivia()?;
                    match self.peek()? {
                        ',' => {
                            self.bump();
                        }
                        ']' => {
                            self.bump();
                            return Some(Value::Array(items));
                        }
                        _ => return None,
                    }
                }
            }
        }
    }

    fn parse_string(&mut self, quote: char) -> Option<String> {
        if !self.peek_is(quote) {
            return None;
        }
        self.bump();
        let mut out = String::new();
        loop {
            match self.bump()? {
                '\\' => {
                    let e = self.bump()?;
                    match e {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        other => out.push(other),
                    }
                }
                c if c == quote => return Some(out),
                c => out.push(c),
            }
        }
    }

    /// Template literal **without** `${…}` interpolation. Any interpolation is
    /// dynamic → `None`.
    fn parse_template(&mut self) -> Option<String> {
        if !self.peek_is('`') {
            return None;
        }
        self.bump();
        let mut out = String::new();
        loop {
            match self.bump()? {
                '\\' => {
                    let e = self.bump()?;
                    match e {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        other => out.push(other),
                    }
                }
                '$' if self.peek_is('{') => return None, // interpolation → dynamic
                '`' => return Some(out),
                c => out.push(c),
            }
        }
    }

    fn parse_number(&mut self) -> Option<Value> {
        self.skip_trivia()?;
        let start = self.p;
        if matches!(self.peek(), Some('-' | '+')) {
            self.bump();
        }
        let (mut dot, mut exp) = (false, false);
        loop {
            match self.peek()? {
                c if c.is_ascii_digit() => {
                    self.bump();
                }
                '.' if !dot && !exp => {
                    dot = true;
                    self.bump();
                }
                'e' | 'E' if !exp => {
                    exp = true;
                    self.bump();
                    if matches!(self.peek(), Some('-' | '+')) {
                        self.bump();
                    }
                }
                _ => break,
            }
        }
        let text = &self.s[start..self.p];
        if text.is_empty() || text == "-" || text == "+" {
            return None;
        }
        if dot || exp {
            text.parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
        } else {
            text.parse::<i64>().ok().map(|i| Value::Number(i.into()))
        }
    }

    fn parse_ident(&mut self) -> Option<String> {
        self.skip_trivia()?;
        let start = self.p;
        while let Some(c) = self.peek() {
            if is_ident_continue(c) {
                self.bump();
            } else {
                break;
            }
        }
        let text = &self.s[start..self.p];
        if text.is_empty() {
            None
        } else {
            Some(text.to_string())
        }
    }
}

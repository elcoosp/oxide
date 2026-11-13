//! Tiny tolerant JS/TS tokenizer backing the scanner passes.
//!
//! **Substitution note (D4):** this is the documented stand-in for the Oxc lexer.
//! It tracks single/double-quoted strings, template literals (including nested
//! `${}` interpolations), line/block comments, and regex literals (heuristic:
//! `/` after an expression-ending token is division, otherwise a regex), so the
//! passes below never see code hidden inside literals or comments. Oxc slots in
//! behind the [`crate::Transformer`] trait without changing any pass.

/// Token kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok {
    /// Identifier or keyword; the raw source slice is the text.
    Ident,
    /// String literal; `value` is the raw text between the quotes (escapes left raw).
    Str { value: String },
    /// `=>`
    Arrow,
    /// Regex literal (contents opaque to the passes).
    Regex,
    /// Numeric literal.
    Num,
    /// Single-character punctuation / operator.
    Punct(char),
    /// Complete template literal without interpolation: `` `text` ``.
    TemplateFull,
    /// Template head: `` `text${ ``
    TemplateStart,
    /// Template middle: `` }text${ ``
    TemplateMid,
    /// Template tail: `` }text` ``
    TemplateEnd,
}

/// A token with byte offsets into the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: Tok,
    pub start: usize,
    pub end: usize,
}

/// Tokenize `src`. Comments are skipped entirely; offsets always index `src`.
///
/// # Panics
///
/// Never: the two `unwrap`s in the interpolation-brace arms are guarded by
/// the `!tmpl_stack.is_empty()` match guard on the same arm.
#[must_use]
pub fn lex(src: &str) -> Vec<Token> {
    let b = src.as_bytes();
    let n = b.len();
    let mut out: Vec<Token> = Vec::new();
    let mut i = 0usize;
    // Brace depth per open `${` interpolation (templates nest).
    let mut tmpl_stack: Vec<u32> = Vec::new();
    // Template text scanning state.
    let mut in_text = false;
    let mut text_start = 0usize;
    let mut text_first = false;

    while i < n {
        if in_text {
            i = scan_template_text(b, n, &mut out, &mut tmpl_stack, text_start, text_first, i);
            in_text = false;
            continue;
        }

        let c = b[i];
        match c {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'/' if i + 1 < n && b[i + 1] == b'/' => {
                while i < n && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < n && b[i + 1] == b'*' => {
                i += 2;
                while i + 1 < n && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(n);
            }
            b'"' | b'\'' => {
                i = scan_string(src, b, n, &mut out, c, i);
            }
            b'`' => {
                text_start = i;
                text_first = true;
                in_text = true;
                i += 1;
            }
            b'0'..=b'9' => {
                i = scan_word(b, n, &mut out, i, Tok::Num);
            }
            b'A'..=b'Z' | b'a'..=b'z' | b'_' | b'$' => {
                i = scan_word(b, n, &mut out, i, Tok::Ident);
            }
            b'=' if i + 1 < n && b[i + 1] == b'>' => {
                out.push(Token {
                    kind: Tok::Arrow,
                    start: i,
                    end: i + 2,
                });
                i += 2;
            }
            b'/' => {
                // Regex vs division: after an expression-ending token it is division.
                i = scan_slash(b, n, &mut out, i);
            }
            b'{' if !tmpl_stack.is_empty() => {
                *tmpl_stack.last_mut().unwrap() += 1;
                out.push(Token {
                    kind: Tok::Punct('{'),
                    start: i,
                    end: i + 1,
                });
                i += 1;
            }
            b'}' if !tmpl_stack.is_empty() => {
                let (next, to_text) = interp_brace_close(&mut out, &mut tmpl_stack, i);
                i = next;
                if to_text {
                    // Closing a `${` returns to template text (the tail).
                    text_start = i;
                    text_first = false;
                    in_text = true;
                }
            }
            _ => {
                out.push(Token {
                    kind: Tok::Punct(c as char),
                    start: i,
                    end: i + 1,
                });
                i += 1;
            }
        }
    }
    out
}

/// Scan one template-text run from `i` (the bytes after a backtick or
/// after the `}` that closed an interpolation) until the next backtick
/// (full/end token) or `${` (start/mid token). Pushes one token and
/// returns the offset to resume lexing at.
fn scan_template_text(
    b: &[u8],
    n: usize,
    out: &mut Vec<Token>,
    tmpl_stack: &mut Vec<u32>,
    start: usize,
    first: bool,
    i: usize,
) -> usize {
    let mut j = i;
    loop {
        if j >= n {
            // unterminated template to EOF
            out.push(Token {
                kind: Tok::TemplateEnd,
                start,
                end: n,
            });
            return n;
        }
        if b[j] == b'\\' {
            j += 2;
            continue;
        }
        if b[j] == b'`' {
            let kind = if first {
                Tok::TemplateFull
            } else {
                Tok::TemplateEnd
            };
            out.push(Token {
                kind,
                start,
                end: j + 1,
            });
            return j + 1;
        }
        if b[j] == b'$' && j + 1 < n && b[j + 1] == b'{' {
            let kind = if first {
                Tok::TemplateStart
            } else {
                Tok::TemplateMid
            };
            out.push(Token {
                kind,
                start,
                end: j + 2,
            });
            tmpl_stack.push(0);
            return j + 2;
        }
        j += 1;
    }
}

/// Scan one single/double-quoted string from `i` (at the opening quote).
/// Pushes the `Str` token (raw inner text, escapes left raw) and returns
/// the offset after the closing quote (or end of input for an
/// unterminated literal).
fn scan_string(
    src: &str,
    b: &[u8],
    n: usize,
    out: &mut Vec<Token>,
    quote: u8,
    mut i: usize,
) -> usize {
    let start = i;
    i += 1;
    while i < n {
        if b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == quote {
            i += 1;
            break;
        }
        i += 1;
    }
    let end = i.min(n);
    let inner_end = if end > start && b[end - 1] == quote {
        end - 1
    } else {
        end
    };
    let value = src[start + 1..inner_end].to_string();
    out.push(Token {
        kind: Tok::Str { value },
        start,
        end,
    });
    end
}

/// Scan one regex literal from `i` (at the opening `/`; the caller has
/// already decided it is a regex, not division). Pushes the `Regex`
/// token (contents opaque) and returns the offset after the closing
/// `/` and its flags.
fn scan_regex(b: &[u8], n: usize, out: &mut Vec<Token>, mut i: usize) -> usize {
    let start = i;
    i += 1;
    let mut in_class = false;
    while i < n {
        if b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == b'[' {
            in_class = true;
        } else if b[i] == b']' {
            in_class = false;
        } else if b[i] == b'/' && !in_class {
            i += 1;
            break;
        }
        i += 1;
    }
    while i < n && b[i].is_ascii_alphabetic() {
        i += 1;
    }
    out.push(Token {
        kind: Tok::Regex,
        start,
        end: i,
    });
    i
}

/// Scan one `/` at `i`: after an expression-ending token it is division
/// (`Punct('/')`); otherwise it opens a regex literal (see [`scan_regex`]).
/// Returns the offset to resume lexing at.
fn scan_slash(b: &[u8], n: usize, out: &mut Vec<Token>, i: usize) -> usize {
    let div_prev = matches!(
        out.last().map(|t| &t.kind),
        Some(
            Tok::Ident
                | Tok::Num
                | Tok::Str { .. }
                | Tok::TemplateFull
                | Tok::TemplateEnd
                | Tok::Regex
                | Tok::Punct(')' | ']' | '}'),
        )
    );
    if div_prev {
        out.push(Token {
            kind: Tok::Punct('/'),
            start: i,
            end: i + 1,
        });
        i + 1
    } else {
        scan_regex(b, n, out, i)
    }
}

/// Scan one identifier-or-number run from `i` (the caller matched the
/// first byte): alphanumeric/`_`/`$` continuation, numbers also absorb
/// `.`. Pushes the token of `kind` and returns the offset after it.
fn scan_word(b: &[u8], n: usize, out: &mut Vec<Token>, mut i: usize, kind: Tok) -> usize {
    let start = i;
    while i < n
        && (b[i].is_ascii_alphanumeric()
            || b[i] == b'_'
            || b[i] == b'$'
            // Number literals absorb the decimal point (`1.5`, `1.5e3`).
            || (b[i] == b'.' && kind == Tok::Num))
    {
        i += 1;
    }
    out.push(Token {
        kind,
        start,
        end: i,
    });
    i
}

/// Pop one interpolation-brace level at the `}` in position `i`: a `}`
/// closing the `${` returns to template text (the tail scan); a nested
/// `{` just decrements the depth. Returns the offset after the `}` and
/// whether lexing returns to template text. The caller guarantees
/// `tmpl_stack` is non-empty (the match arm's guard).
fn interp_brace_close(out: &mut Vec<Token>, tmpl_stack: &mut Vec<u32>, i: usize) -> (usize, bool) {
    let top = tmpl_stack
        .last_mut()
        .expect("the match arm guards a non-empty stack");
    if *top == 0 {
        tmpl_stack.pop();
        out.push(Token {
            kind: Tok::Punct('}'),
            start: i,
            end: i + 1,
        });
        (i + 1, true)
    } else {
        *top -= 1;
        out.push(Token {
            kind: Tok::Punct('}'),
            start: i,
            end: i + 1,
        });
        (i + 1, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_interpolation_nesting_yields_code_tokens() {
        let toks = lex("const t = `a ${ 1 } b ${ `c ${ 'x' } d` } e`;");
        let kinds: Vec<&Tok> = toks.iter().map(|t| &t.kind).collect();
        assert!(kinds.contains(&&Tok::TemplateStart));
        assert!(kinds.contains(&&Tok::TemplateMid));
        assert!(kinds.contains(&&Tok::TemplateEnd));
        // the string 'x' inside the nested interpolation is a Str token
        assert!(
            toks.iter()
                .any(|t| matches!(&t.kind, Tok::Str { value } if value == "x"))
        );
    }

    #[test]
    fn comments_and_strings_are_not_idents() {
        let src = "// import\n/* x */\nconst s = \"hello\";";
        let toks = lex(src);
        let words: Vec<&str> = toks
            .iter()
            .filter(|t| t.kind == Tok::Ident)
            .map(|t| &src[t.start..t.end])
            .collect();
        assert_eq!(words, vec!["const", "s"]);
        assert!(
            toks.iter()
                .any(|t| matches!(&t.kind, Tok::Str { value } if value == "hello"))
        );
    }

    #[test]
    fn regex_after_operator_division_after_ident() {
        let toks = lex("const a = /x/g;\nlet d = a / 2;");
        assert!(toks.iter().any(|t| t.kind == Tok::Regex));
        assert!(toks.iter().any(|t| t.kind == Tok::Punct('/')));
    }
}

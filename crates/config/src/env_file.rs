//! dotenv-compatible env file parsing (spec §14.2).
//!
//! Rules: `KEY=VALUE` per line; blank lines and `#` comments skipped; optional
//! `export ` prefix; values may be unquoted, 'single-' or "double-quoted";
//! double quotes process `\n`, `\t`, `\r`, `\\` and quote escapes; unquoted
//! values drop inline comments (a `#` preceded by whitespace); later lines win.

use std::collections::BTreeMap;

/// Parse env-file `content` into a `KEY → VALUE` map (last occurrence wins).
pub(crate) fn parse(content: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Optional `export ` prefix (shell-style), e.g. `export FOO=1`.
        let line = match line.strip_prefix("export ") {
            Some(rest) => rest.trim(),
            None => line,
        };
        let Some((key_raw, value_raw)) = line.split_once('=') else {
            continue; // not an assignment (e.g. a stray token) — skip
        };
        let key = key_raw.trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        {
            continue; // invalid key — skip rather than abort the whole file
        }
        map.insert(key.to_string(), unquote(value_raw.trim()));
    }
    map
}

/// Strip quotes and process escapes / inline comments.
fn unquote(v: &str) -> String {
    let chars: Vec<char> = v.chars().collect();
    if chars.len() >= 2 {
        let first = chars[0];
        let last = chars[chars.len() - 1];
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            let inner: String = chars[1..chars.len() - 1].iter().collect();
            return unescape(&inner, first);
        }
    }
    // Unquoted: an inline comment starts at `#` preceded by whitespace.
    let mut out = String::new();
    let mut prev_ws = false;
    for c in chars {
        if c == '#' && prev_ws {
            break;
        }
        out.push(c);
        prev_ws = c.is_whitespace();
    }
    out.trim_end().to_string()
}

/// Process escapes inside a quoted value. Double quotes handle `\n`, `\t`,
/// `\r`, `\\`, `\'`, `\"` and `` \` ``; single quotes only handle `\\` and
/// `\'` (shell-style literal). Unknown escapes keep the backslash.
fn unescape(inner: &str, quote: char) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(e) = chars.next() else {
            out.push('\\');
            break;
        };
        let mapped = match (quote, e) {
            ('"', 'n') => Some('\n'),
            ('"', 't') => Some('\t'),
            ('"', 'r') => Some('\r'),
            (_, '\\') => Some('\\'),
            (_, '\'') => Some('\''),
            ('"', '"') => Some('"'),
            ('"', '`') => Some('`'),
            _ => None,
        };
        match mapped {
            Some(m) => out.push(m),
            None => {
                out.push('\\');
                out.push(e);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_crlf_and_blank_lines() {
        let m = parse("A=1\r\n\r\n# c\r\nB=2\r\n");
        assert_eq!(m.get("A").map(String::as_str), Some("1"));
        assert_eq!(m.get("B").map(String::as_str), Some("2"));
    }

    #[test]
    fn last_wins_within_one_file() {
        let m = parse("K=first\nK=second");
        assert_eq!(m.get("K").map(String::as_str), Some("second"));
    }
}

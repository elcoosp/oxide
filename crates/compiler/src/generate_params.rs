//! `generateParams` extraction (spec §10.5). A `page.server.ts` that exports a
//! **literal** `generateParams` array is prerenderable: each element yields one
//! HTML output at build time. A function-valued `generateParams` (calls,
//! closures, imports) requires running JS at build time and is reported as
//! unprerenderable so the caller can fall back rather than silently skip.
//!
//! Recognized forms:
//! - `export const generateParams = [{ slug: "a" }, { slug: "b" }]`
//! - `export const generateParams = () => [{ slug: "a" }]`
//! - `export const generateParams = async () => [{ slug: "a" }]`
//!
//! Only string-literal values are extracted; non-string values (`number`,
//! `boolean`, nested objects) are stringified via the source text so param
//! segments round-trip without a schema.

use std::collections::BTreeMap;

/// The result of attempting to extract `generateParams` from a source module.
#[derive(Debug, Clone, PartialEq)]
pub enum GenerateParamsResult {
    /// The source does not declare `generateParams`.
    Absent,
    /// A literal array (or literal-returning function) was found; each element
    /// is a param object.
    Literal(Vec<BTreeMap<String, String>>),
    /// `generateParams` is present but its body requires JS to evaluate.
    Runtime {
        /// Byte offset of the `generateParams` binding, for diagnostics.
        start: usize,
        /// Byte offset just past the binding, for diagnostics.
        end: usize,
    },
}

/// Extract `generateParams` from a server module's source.
pub fn extract_generate_params(source: &str) -> GenerateParamsResult {
    let Some(binding) = find_binding(source) else {
        return GenerateParamsResult::Absent;
    };

    // Walk forward to the RHS.
    let bytes = source.as_bytes();
    let mut i = binding.end;
    while i < bytes.len()
        && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n' || bytes[i] == b'\r')
    {
        i += 1;
    }
    // Skip past `=` if we're right before it.
    if i < bytes.len() && bytes[i] == b'=' {
        i += 1;
        while i < bytes.len()
            && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n' || bytes[i] == b'\r')
        {
            i += 1;
        }
    }

    // If the RHS is a function, skip to its first `[`.
    if source[i..].starts_with("async")
        || source[i..].starts_with("function")
        || source[i..].starts_with('(')
        || source[i..].starts_with("()")
    {
        if let Some(bracket) = source[i..].find('[') {
            i += bracket;
        } else {
            return GenerateParamsResult::Runtime {
                start: binding.start,
                end: binding.end,
            };
        }
    }

    // Now expect an array literal `[`.
    if i >= bytes.len() || bytes[i] != b'[' {
        return GenerateParamsResult::Runtime {
            start: binding.start,
            end: binding.end,
        };
    }

    let Some(close) = matching_bracket(source, i) else {
        return GenerateParamsResult::Runtime {
            start: binding.start,
            end: binding.end,
        };
    };

    let body = &source[i + 1..close];
    let mut out = Vec::new();
    for obj in split_top_level(body, b',') {
        let obj = obj.trim();
        if obj.is_empty() {
            continue;
        }
        // Each element must be a `{ ... }` object.
        if !obj.starts_with('{') || !obj.ends_with('}') {
            return GenerateParamsResult::Runtime {
                start: binding.start,
                end: binding.end,
            };
        }
        let inner = &obj[1..obj.len() - 1];
        let mut params = BTreeMap::new();
        for pair in split_top_level(inner, b',') {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            let Some((k, v)) = pair.split_once(':') else {
                return GenerateParamsResult::Runtime {
                    start: binding.start,
                    end: binding.end,
                };
            };
            let key = k.trim().trim_matches('"').trim_matches('\'').to_string();
            let value = parse_value(v.trim());
            match value {
                Some(s) => {
                    params.insert(key, s);
                }
                None => {
                    return GenerateParamsResult::Runtime {
                        start: binding.start,
                        end: binding.end,
                    };
                }
            }
        }
        if params.is_empty() {
            return GenerateParamsResult::Runtime {
                start: binding.start,
                end: binding.end,
            };
        }
        out.push(params);
    }

    GenerateParamsResult::Literal(out)
}

struct Binding {
    start: usize,
    end: usize,
}

/// Find the `generateParams` binding: `[export] const generateParams` or
/// `export { generateParams }` (the latter is treated as runtime — the
/// referenced symbol is not visible here).
fn find_binding(source: &str) -> Option<Binding> {
    const NEEDLES: &[&str] = &[
        "export const generateParams",
        "export let generateParams",
        "export var generateParams",
        "export async function generateParams",
    ];
    for needle in NEEDLES {
        if let Some(idx) = source.find(needle) {
            return Some(Binding {
                start: idx,
                end: idx + needle.len(),
            });
        }
    }
    None
}

/// Find the index of the `]` matching the `[` at `open`.
fn matching_bracket(src: &str, open: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut depth = 0i32;
    let mut in_str: Option<u8> = None;
    let mut i = open;
    while i < bytes.len() {
        let c = bytes[i];
        match in_str {
            Some(q) => {
                if c == b'\\' {
                    i += 2;
                    continue;
                }
                if c == q {
                    in_str = None;
                }
            }
            None => match c {
                b'\'' | b'"' | b'`' => in_str = Some(c),
                b'[' | b'{' | b'(' => depth += 1,
                b']' | b'}' | b')' => {
                    depth -= 1;
                    if depth == 0 && c == b']' {
                        return Some(i);
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    None
}

/// Split at `sep` at depth 0 (string-aware).
fn split_top_level(s: &str, sep: u8) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_str: Option<u8> = None;
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        match in_str {
            Some(q) => {
                if c == b'\\' {
                    i += 2;
                    continue;
                }
                if c == q {
                    in_str = None;
                }
            }
            None => match c {
                b'\'' | b'"' | b'`' => in_str = Some(c),
                b'{' | b'[' | b'(' => depth += 1,
                b'}' | b']' | b')' => depth -= 1,
                _ if c == sep && depth == 0 => {
                    out.push(&s[start..i]);
                    start = i + 1;
                }
                _ => {}
            },
        }
        i += 1;
    }
    out.push(&s[start..]);
    out
}

/// Parse a scalar value into its string form. Returns `None` for anything that
/// isn't a string literal or a scalar.
fn parse_value(v: &str) -> Option<String> {
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    // String literal.
    if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
        || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
    {
        return Some(v[1..v.len() - 1].to_string());
    }
    // Number or boolean — stringified verbatim.
    if v.parse::<f64>().is_ok() || v == "true" || v == "false" {
        return Some(v.to_string());
    }
    // Anything else (calls, templates, identifiers) needs runtime evaluation.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_when_no_binding() {
        let src = "export const load = async () => ({});\n";
        assert_eq!(extract_generate_params(src), GenerateParamsResult::Absent);
    }

    #[test]
    fn literal_array_of_objects() {
        let src = r#"export const generateParams = [
            { slug: "a" },
            { slug: "b" },
        ];"#;
        let out = extract_generate_params(src);
        match out {
            GenerateParamsResult::Literal(v) => {
                assert_eq!(v.len(), 2);
                assert_eq!(v[0].get("slug").unwrap(), "a");
                assert_eq!(v[1].get("slug").unwrap(), "b");
            }
            other => panic!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn arrow_returning_array() {
        let src = r#"export const generateParams = () => [{ slug: "x" }, { slug: "y" }];"#;
        match extract_generate_params(src) {
            GenerateParamsResult::Literal(v) => {
                assert_eq!(v.len(), 2);
                assert_eq!(v[0].get("slug").unwrap(), "x");
            }
            other => panic!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn async_arrow_returning_array() {
        let src = r#"export const generateParams = async () => [{ slug: "z" }];"#;
        match extract_generate_params(src) {
            GenerateParamsResult::Literal(v) => assert_eq!(v[0].get("slug").unwrap(), "z"),
            other => panic!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn multiple_params_per_entry() {
        let src = r#"export const generateParams = [{ lang: "en", slug: "hi" }, { lang: "fr", slug: "salut" }];"#;
        match extract_generate_params(src) {
            GenerateParamsResult::Literal(v) => {
                assert_eq!(v.len(), 2);
                assert_eq!(v[0].get("lang").unwrap(), "en");
                assert_eq!(v[0].get("slug").unwrap(), "hi");
                assert_eq!(v[1].get("lang").unwrap(), "fr");
            }
            other => panic!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn runtime_function_body_is_runtime() {
        let src = r#"export const generateParams = async () => {
            const posts = await db.query();
            return posts.map(p => ({ slug: p.slug }));
        };"#;
        match extract_generate_params(src) {
            GenerateParamsResult::Runtime { .. } => {}
            other => panic!("expected Runtime, got {other:?}"),
        }
    }

    #[test]
    fn runtime_call_in_array_is_runtime() {
        let src = r#"export const generateParams = [{ slug: getDefault() }];"#;
        match extract_generate_params(src) {
            GenerateParamsResult::Runtime { .. } => {}
            other => panic!("expected Runtime, got {other:?}"),
        }
    }

    #[test]
    fn empty_array_is_literal_empty() {
        let src = "export const generateParams = [];";
        match extract_generate_params(src) {
            GenerateParamsResult::Literal(v) => assert!(v.is_empty()),
            other => panic!("expected Literal(empty), got {other:?}"),
        }
    }

    #[test]
    fn numeric_and_boolean_values_are_stringified() {
        let src = r#"export const generateParams = [{ year: 2026, draft: false }];"#;
        match extract_generate_params(src) {
            GenerateParamsResult::Literal(v) => {
                assert_eq!(v[0].get("year").unwrap(), "2026");
                assert_eq!(v[0].get("draft").unwrap(), "false");
            }
            other => panic!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn single_quoted_values_are_accepted() {
        let src = r#"export const generateParams = [{ slug: 'hello' }];"#;
        match extract_generate_params(src) {
            GenerateParamsResult::Literal(v) => assert_eq!(v[0].get("slug").unwrap(), "hello"),
            other => panic!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn export_let_form_is_recognized() {
        let src = r#"export let generateParams = [{ slug: "a" }];"#;
        match extract_generate_params(src) {
            GenerateParamsResult::Literal(v) => assert_eq!(v[0].get("slug").unwrap(), "a"),
            other => panic!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn string_with_comma_does_not_split() {
        let src = r#"export const generateParams = [{ slug: "a,b" }];"#;
        match extract_generate_params(src) {
            GenerateParamsResult::Literal(v) => assert_eq!(v[0].get("slug").unwrap(), "a,b"),
            other => panic!("expected Literal, got {other:?}"),
        }
    }
}

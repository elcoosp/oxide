//! Segment encoding shared by the scanner and the matcher (spec §8.1, §8.2).
//!
//! URL pattern segments use a path-to-regexp-lite encoding:
//! - static directory name              -> the name itself
//! - `[param]`                          -> `:param`
//! - `[param]{regex}` (canonical) or `[param=regex]` -> `:param{regex}`
//! - `[...rest]`                        -> `*rest`
//! - `[[...optional]]`                  -> `*?optional`
//! - `(group)`, `@slot`, `(.)intercept` -> never become URL segments (scanner-only)

/// Classification of one pattern segment (borrowed: no allocation on the match path).
pub(crate) enum SegKind<'a> {
    Static(&'a str),
    /// `:name` or `:name{regex}` — the regex, when present, must match the whole
    /// segment value (anchored full match).
    Param {
        name: &'a str,
        constraint: Option<&'a str>,
    },
    /// `*?name` — optional catch-all: matches zero or more segments.
    OptionalCatchAll(&'a str),
    /// `*name` — catch-all: matches one or more segments.
    CatchAll(&'a str),
}

/// Scan-order priority of a segment kind: static (0) > param (1) > optional (2) > catch-all (3).
pub(crate) fn seg_priority(seg: &str) -> u8 {
    match classify(seg) {
        SegKind::Static(_) => 0,
        SegKind::Param { .. } => 1,
        SegKind::OptionalCatchAll(_) => 2,
        SegKind::CatchAll(_) => 3,
    }
}

/// Classify an encoded pattern segment.
pub(crate) fn classify(seg: &str) -> SegKind<'_> {
    if let Some(name) = seg.strip_prefix("*?") {
        SegKind::OptionalCatchAll(name)
    } else if let Some(name) = seg.strip_prefix('*') {
        SegKind::CatchAll(name)
    } else if let Some(rest) = seg.strip_prefix(':') {
        // `:name{regex}` — regex spans from the first `{` to the final `}`.
        if rest.ends_with('}') {
            if let Some(open) = rest.find('{') {
                return SegKind::Param {
                    name: &rest[..open],
                    constraint: Some(&rest[open + 1..rest.len() - 1]),
                };
            }
        }
        SegKind::Param {
            name: rest,
            constraint: None,
        }
    } else {
        SegKind::Static(seg)
    }
}

/// Minimal percent-decoding (no dependency): `%XX` → byte; invalid escapes stay literal.
/// Decoded bytes that are not valid UTF-8 fall back to a lossy conversion.
pub(crate) fn percent_decode(s: &str) -> String {
    fn hexval(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hexval(bytes[i + 1]), hexval(bytes[i + 2])) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Split a request path into decoded, non-empty segments: query/fragment stripped,
/// leading/trailing/duplicate slashes tolerated.
pub(crate) fn split_path(path: &str) -> Vec<String> {
    let bare = path.split(['?', '#']).next().unwrap_or(path);
    bare.split('/')
        .filter(|s| !s.is_empty())
        .map(percent_decode)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_handles_valid_invalid_and_truncated_escapes() {
        assert_eq!(percent_decode("%41%42"), "AB");
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("a%2Fb"), "a/b");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%2"), "%2");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("caf%C3%A9"), "café");
        // Invalid UTF-8 bytes fall back to lossy replacement chars.
        assert_eq!(percent_decode("%FF"), "\u{FFFD}");
    }

    #[test]
    fn split_path_strips_query_and_tolerates_slashes() {
        assert!(split_path("/").is_empty());
        assert!(split_path("").is_empty());
        assert_eq!(split_path("/blog"), vec!["blog"]);
        assert_eq!(split_path("/blog/"), vec!["blog"]);
        assert_eq!(split_path("//blog//x//"), vec!["blog", "x"]);
        assert_eq!(split_path("/blog?x=1#frag"), vec!["blog"]);
        assert_eq!(split_path("/a%20b"), vec!["a b"]);
    }

    #[test]
    fn classify_and_priority_cover_all_kinds() {
        assert!(matches!(classify("blog"), SegKind::Static("blog")));
        assert!(matches!(
            classify(":slug"),
            SegKind::Param {
                name: "slug",
                constraint: None
            }
        ));
        assert!(matches!(
            classify(":n{\\d+}"),
            SegKind::Param {
                name: "n",
                constraint: Some("\\d+")
            }
        ));
        assert!(matches!(classify("*rest"), SegKind::CatchAll("rest")));
        assert!(matches!(
            classify("*?rest"),
            SegKind::OptionalCatchAll("rest")
        ));
        assert_eq!(seg_priority("blog"), 0);
        assert_eq!(seg_priority(":slug"), 1);
        assert_eq!(seg_priority("*?rest"), 2);
        assert_eq!(seg_priority("*rest"), 3);
    }
}

//! HTTP semantics helpers (spec §11.3): conditional requests (304), Cache-Control
//! generation, and the L2 per-request fetch memoization (§11.1).

use crate::{Entry, Policy};
use bytes::Bytes;
use dashmap::DashMap;

// ------------------------------------------------------------------- conditional requests

/// Decide a conditional response for a stored entry (§11.3, RFC 9110 §13).
///
/// Returns `Some(304)` when the request's validators match the entry, `None` when the
/// entry must be served in full. Rules:
///
/// - `If-None-Match` takes precedence over `If-Modified-Since` (RFC 9110 §13.2.2);
///   its comparison is **weak** (`W/` prefixes ignored, comma-separated lists and `*`
///   supported), and a present-but-matching-none header short-circuits to `None`.
/// - `If-Modified-Since` compares against the entry's `Last-Modified` header; an
///   entry without one can never answer IMS. Dates parse as IMF-fixdate
///   (`Sun, 06 Nov 1994 08:49:37 GMT`) or — for tests/dev tooling — as a bare
///   unix-**milliseconds** number (see [`parse_http_date`]).
pub fn conditional_response(req_headers: &[(String, String)], entry: &Entry) -> Option<u16> {
    let get = |name: &str| {
        req_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };

    if let Some(inm) = get("if-none-match") {
        return if etag_list_matches(inm, &entry.etag) {
            Some(304)
        } else {
            None
        };
    }

    if let Some(ims) = get("if-modified-since") {
        let last_modified = entry
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("last-modified"))
            .and_then(|(_, v)| parse_http_date(v));
        if let (Some(lm), Some(ims_secs)) = (last_modified, parse_http_date(ims))
            && lm <= ims_secs
        {
            return Some(304);
        }
    }
    None
}

/// Weak entity-tag comparison of an `If-None-Match` header value (possibly a comma
/// list or `*`) against the entry's etag (RFC 9110 §13.1.2).
fn etag_list_matches(header: &str, entry_etag: &str) -> bool {
    fn weak(etag: &str) -> &str {
        etag.trim().strip_prefix("W/").unwrap_or(etag.trim())
    }
    let target = weak(entry_etag);
    if target.is_empty() {
        return false;
    }
    header.split(',').any(|candidate| {
        let c = candidate.trim();
        c == "*" || (!c.is_empty() && weak(c) == target)
    })
}

// ------------------------------------------------------------------- header generation

/// Generate `Cache-Control` from route policy (§11.3).
///
/// `ttl = 0` → `no-store` (never cacheable, regardless of SWR/SIE). Otherwise
/// `public, max-age=<s>, s-maxage=<s>` plus `stale-while-revalidate` / `stale-if-error`
/// when configured.
pub fn cache_control(policy: &Policy) -> String {
    if policy.ttl.is_zero() {
        return "no-store".to_string();
    }
    let secs = policy.ttl.as_secs();
    let mut out = format!("public, max-age={secs}, s-maxage={secs}");
    if let Some(swr) = policy.swr {
        out.push_str(&format!(", stale-while-revalidate={}", swr.as_secs()));
    }
    if let Some(sie) = policy.stale_if_error {
        out.push_str(&format!(", stale-if-error={}", sie.as_secs()));
    }
    out
}

// ------------------------------------------------------------------- HTTP dates (minimal)

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Parse an HTTP date into unix **seconds**. Accepted forms:
///
/// 1. bare ASCII digits — interpreted as unix **milliseconds** (test/dev convenience,
///    documented deviation from RFC 9110);
/// 2. IMF-fixdate `Sun, 06 Nov 1994 08:49:37 GMT` (weekday is ignored; `GMT` required).
///
/// Obsolete forms (RFC 850, asctime) are deliberately not supported.
pub fn parse_http_date(s: &str) -> Option<u64> {
    let s = s.trim();
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        return s.parse::<u64>().ok().map(|ms| ms / 1000);
    }
    let rest = match s.split_once(',') {
        Some((_, r)) => r.trim(),
        None => return None, // weekday required in the supported form
    };
    let p: Vec<&str> = rest.split_whitespace().collect();
    if p.len() != 5 || !p[4].eq_ignore_ascii_case("GMT") {
        return None;
    }
    let day: i64 = p[0].parse().ok()?;
    let mon = MONTHS.iter().position(|m| p[1].eq_ignore_ascii_case(m))? as i64 + 1;
    let year: i64 = p[2].parse().ok()?;
    let mut hms = p[3].split(':');
    let (h, m, sec) = (
        hms.next()?.parse::<i64>().ok()?,
        hms.next()?.parse::<i64>().ok()?,
        hms.next()?.parse::<i64>().ok()?,
    );
    if !(1..=31).contains(&day)
        || !(0..=23).contains(&h)
        || !(0..=59).contains(&m)
        || !(0..=60).contains(&sec)
    // leap second tolerated
    {
        return None;
    }
    let days = days_from_civil(year, mon as u32, day as u32);
    Some((days * 86_400 + h * 3_600 + m * 60 + sec) as u64)
}

/// Format unix seconds as an IMF-fixdate HTTP date (`Sun, 06 Nov 1994 08:49:37 GMT`).
pub fn http_date(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let rem = unix_secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    let wd = WEEKDAYS[((days + 4).rem_euclid(7)) as usize]; // 1970-01-01 was a Thursday
    format!(
        "{wd}, {d:02} {} {y:04} {:02}:{:02}:{:02} GMT",
        MONTHS[(m - 1) as usize],
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's `days_from_civil` (proleptic Gregorian → days since 1970-01-01).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m as i64 + 9) % 12; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`]; returns `(year, month, day)`.
fn civil_from_days(z: i64) -> (i64, i32, i32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m as i32, d as i32)
}

// ------------------------------------------------------------------- fetch memo (L2)

/// Key for [`FetchMemo`]: `METHOD url blake3(body)` — the body hash makes POST
/// memoization safe; method is uppercased.
pub fn memo_key(method: &str, url: &str, body: &[u8]) -> String {
    let digest = blake3::hash(body).to_string();
    format!("{} {url} {digest}", method.to_ascii_uppercase())
}

/// L2: per-request fetch memoization (§11.1). Tracks `method+url+body-hash →
/// response bytes` for the duration of a request (share one `FetchMemo` per request;
/// wrap in an Arc for a process-wide memo if cross-request reuse is configured).
#[derive(Default)]
pub struct FetchMemo {
    pub inner: DashMap<String, Bytes>,
}

impl FetchMemo {
    /// Store a response under a precomputed key.
    pub fn insert(&self, key: impl Into<String>, response: Bytes) {
        self.inner.insert(key.into(), response);
    }

    /// Look a response up by precomputed key.
    pub fn get(&self, key: &str) -> Option<Bytes> {
        self.inner.get(key).map(|b| Bytes::clone(&b))
    }

    /// Store a response for a request (key derived via [`memo_key`]).
    pub fn remember(&self, method: &str, url: &str, body: &[u8], response: Bytes) {
        self.insert(memo_key(method, url, body), response);
    }

    /// Look a response up for a request within the same request id.
    pub fn get_for_request(&self, method: &str, url: &str, body: &[u8]) -> Option<Bytes> {
        self.get(&memo_key(method, url, body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn http_date_roundtrips_known_rfc_value() {
        // The RFC 9110 example date: 1994-11-06T08:49:37Z = 784111777.
        assert_eq!(
            parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(784_111_777)
        );
        assert_eq!(http_date(784_111_777), "Sun, 06 Nov 1994 08:49:37 GMT");
        // Roundtrip across a range of timestamps (leap years, month ends).
        for secs in [
            0u64,
            86_399,
            951_782_400,
            1_700_000_000,
            2_147_483_647,
            4_102_444_800,
        ] {
            assert_eq!(
                parse_http_date(&http_date(secs)),
                Some(secs),
                "roundtrip {secs}"
            );
        }
    }

    #[test]
    fn parse_http_date_accepts_unix_ms_numbers_and_rejects_garbage() {
        assert_eq!(parse_http_date("784111777000"), Some(784_111_777));
        assert_eq!(parse_http_date("  1700000000000 "), Some(1_700_000_000));
        assert_eq!(parse_http_date("0"), Some(0));
        assert_eq!(parse_http_date("not a date"), None);
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 PST"), None);
        assert_eq!(parse_http_date("32 Foo 1994 08:49:37 GMT"), None);
    }

    #[test]
    fn etag_header_edge_cases() {
        let e = |etag: &str, headers: Vec<(String, String)>| Entry {
            body: vec![],
            status: 200,
            headers,
            tags: vec![],
            policy: Policy::default(),
            stored_at: 0,
            etag: etag.into(),
        };
        let h = |v: &str| vec![("If-None-Match".into(), v.into())];
        // Empty entry etag never matches (even against *).
        assert_eq!(conditional_response(&h("*"), &e("", vec![])), None);
        // Whitespace tolerance + W/ on both sides.
        assert_eq!(
            conditional_response(&h(" W/\"a\" , \"b\" "), &e("W/\"b\"", vec![])),
            Some(304)
        );
        // Header name case-insensitivity.
        assert_eq!(
            conditional_response(
                &[("IF-NONE-MATCH".into(), "\"a\"".into())],
                &e("\"a\"", vec![])
            ),
            Some(304)
        );
    }

    #[test]
    fn cache_control_zero_ttl_is_no_store_even_with_swr() {
        let p = Policy {
            ttl: Duration::from_secs(0),
            swr: Some(Duration::from_secs(9)),
            stale_if_error: Some(Duration::from_secs(9)),
            soft_tags: true,
        };
        assert_eq!(cache_control(&p), "no-store");
    }

    #[test]
    fn fetch_memo_remember_and_get_for_request() {
        let memo = FetchMemo::default();
        memo.remember("get", "https://a/b", b"", Bytes::from_static(b"r1"));
        assert_eq!(
            memo.get_for_request("GET", "https://a/b", b"").unwrap(),
            Bytes::from_static(b"r1")
        );
        assert!(memo.get_for_request("GET", "https://a/b", b"x").is_none());
        memo.remember("GET", "https://a/b", b"x", Bytes::from_static(b"r2"));
        assert_eq!(
            memo.get_for_request("get", "https://a/b", b"x").unwrap(),
            Bytes::from_static(b"r2")
        );
    }
}

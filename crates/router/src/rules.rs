//! Native rules engine (spec §12.1 step 4, D13, §17.1): redirects, rewrites, headers,
//! basic auth gates and rate-limit decisions evaluated in Rust before any JS.

use std::collections::BTreeMap;

use crate::segment::split_path;
use crate::{NativeRule, Result, RuleOutcome, RulesEngine, ferr};

type BucketKey = (f64, std::time::Instant, u32, u64);
type Buckets = std::sync::Arc<std::sync::Mutex<std::collections::BTreeMap<String, BucketKey>>>;

/// Compile-time validation + normalization (see `RulesEngine::compile` docs).
pub(crate) fn compile_impl(rules: Vec<NativeRule>) -> Result<RulesEngine> {
    for (i, r) in rules.iter().enumerate() {
        let n = i + 1;
        if !r.source.starts_with('/') {
            return Err(ferr(
                "RTR-110",
                format!("rule #{n}: source `{}` must start with `/`", r.source),
                "write rule sources as absolute paths, e.g. `/old/:slug`",
            ));
        }
        let segs: Vec<&str> = r.source.split('/').filter(|s| !s.is_empty()).collect();
        if let Some(pos) = segs.iter().position(|s| *s == "*")
            && pos != segs.len() - 1 {
                return Err(ferr(
                    "RTR-110",
                    format!("rule #{n}: `*` must be the last segment of the source"),
                    "move `*` to the end, e.g. `/docs/*`",
                ));
            }
        if r.rule_type.as_deref() == Some("rewrite") && r.destination.is_none() {
            return Err(ferr(
                "RTR-110",
                format!("rule #{n}: rewrite rules require a `destination`"),
                "add `destination` to the rewrite rule in oxide.config.ts",
            ));
        }
        if r.rule_type.as_deref() == Some("ratelimit") && r.rate_limit.is_none() {
            return Err(ferr(
                "RTR-110",
                format!("rule #{n}: rate-limit rules require a `rate_limit` token bucket"),
                "add `rate_limit: {{ key, max, window_secs }}` to the rule",
            ));
        }
    }
    let mut rules = rules;
    for r in rules.iter_mut() {
        if r.rate_limit.is_some() && r.rule_type.is_none() {
            r.rule_type = Some("ratelimit".to_string());
        }
    }
    Ok(RulesEngine {
        rules,
        limiter: TokenBuckets::default(),
    })
}

/// SEC-01: how many distinct keys the bucket map may hold before a sweep
/// evicts refilled (full) buckets. Bounds a unique-key flood.
pub const RATE_LIMITER_MAX_KEYS: usize = 10_000;

/// SEC-01: the shared per-key token buckets. Capacity/refill come from the
/// rule (`max`, `window_secs`); the bucket starts full. A bucket whose token
/// count has refilled to capacity is evictable (it holds no state a fresh
/// bucket would not).
#[derive(Debug, Default)]
pub struct TokenBuckets {
    map: Buckets,
}

impl Clone for TokenBuckets {
    /// Share the bucket map: a cloned engine must see the same counters (a
    /// per-clone copy would reset the limit).
    fn clone(&self) -> Self {
        Self {
            map: std::sync::Arc::clone(&self.map),
        }
    }
}

impl TokenBuckets {
    /// Consume one token for `key`. Returns `Ok(())` when allowed, or
    /// `Err(retry_after_secs)` when the bucket is empty.
    pub fn take(&self, key: &str, max: u32, window_secs: u64) -> std::result::Result<(), u64> {
        let mut map = self.map.lock().unwrap_or_else(|p| p.into_inner());
        if map.len() > RATE_LIMITER_MAX_KEYS {
            // Sweep full buckets (no pending state).
            map.retain(|_, (tokens, _, m, _)| *tokens < *m as f64 - 0.001);
        }
        let now = std::time::Instant::now();
        let rate = max as f64 / window_secs.max(1) as f64;
        let entry = map
            .entry(key.to_string())
            .or_insert((max as f64, now, max, window_secs));
        let elapsed = now.duration_since(entry.1).as_secs_f64();
        entry.0 = (entry.0 + elapsed * rate).min(max as f64);
        entry.1 = now;
        if entry.0 >= 1.0 {
            entry.0 -= 1.0;
            Ok(())
        } else {
            // Seconds until one token refills.
            let deficit = 1.0 - entry.0;
            Err((deficit / rate).ceil().max(1.0) as u64)
        }
    }
}

/// Resolve a rule's `rate_limit.key` against the captured params and the
/// request's key material. `ip`/empty → `client_key`; `:name` → the captured
/// param (`client_key` when absent); anything else is a literal.
fn resolve_bucket_key(
    rule_key: &str,
    params: &std::collections::BTreeMap<String, String>,
    client_key: &str,
) -> String {
    if rule_key.is_empty() || rule_key == "ip" {
        return format!("ip:{client_key}");
    }
    if let Some(name) = rule_key.strip_prefix(':') {
        let v = params.get(name).map(String::as_str).unwrap_or(client_key);
        return format!("{name}:{v}");
    }
    format!("lit:{rule_key}")
}

/// B-16 / S-17: verify an HTTP Basic `Authorization` header in constant time.
/// Returns false for a missing/malformed header or a mismatch.
fn verify_basic_auth(header: Option<&str>, expected: (&str, &str)) -> bool {
    use base64::Engine as _;
    use subtle::ConstantTimeEq as _;
    let Some(h) = header else { return false };
    let Some(b64) = h.strip_prefix("Basic ") else {
        return false;
    };
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64.trim()) else {
        return false;
    };
    let Ok(pair) = std::str::from_utf8(&decoded) else {
        return false;
    };
    let Some((user, pass)) = pair.split_once(':') else {
        return false;
    };
    let ct_eq = |a: &str, b: &str| -> bool {
        let (ab, bb) = (a.as_bytes(), b.as_bytes());
        ab.len() == bb.len() && bool::from(ab.ct_eq(bb))
    };
    // `&` (not `&&`): compare both fields regardless of the first result.
    ct_eq(user, expected.0) & ct_eq(pass, expected.1)
}

pub(crate) fn evaluate_impl(
    engine: &RulesEngine,
    path: &str,
    client_key: &str,
    authorization: Option<&str>,
) -> RuleOutcome {
    let url = split_path(path);
    let mut merged: BTreeMap<String, String> = BTreeMap::new();
    let mut any_headers = false;
    for rule in &engine.rules {
        let Some(params) = match_source(&rule.source, &url) else {
            continue;
        };
        // Terminal outcomes in priority order: rate limit > auth > rewrite > redirect.
        if rule.rule_type.as_deref() == Some("ratelimit") {
            // SEC-01: consult the token bucket. Before this fix the rule
            // returned `RateLimited` unconditionally (the bucket was never
            // consulted), so one configured rule 429'd the path forever.
            if let Some(rl) = &rule.rate_limit {
                let bucket_key = resolve_bucket_key(&rl.key, &params, client_key);
                if let Err(retry_after_secs) =
                    engine.limiter.take(&bucket_key, rl.max, rl.window_secs)
                {
                    return RuleOutcome::RateLimited { retry_after_secs };
                }
                // Allowed: a rate-limit rule is non-terminal (keep scanning
                // so a later rewrite/redirect/header rule can still apply).
                continue;
            }
            return RuleOutcome::RateLimited {
                retry_after_secs: 1,
            };
        }
        if let Some(ba) = &rule.basic_auth {
            // B-16: verify before challenging; an authenticated request falls
            // through to the remaining rules / normal dispatch.
            if !verify_basic_auth(authorization, (&ba.0, &ba.1)) {
                return RuleOutcome::AuthRequired;
            }
        }
        if rule.rule_type.as_deref() == Some("rewrite")
            && let Some(dest) = &rule.destination {
                return RuleOutcome::Rewrite(substitute(dest, &params));
            }
        if let Some(dest) = &rule.destination {
            return RuleOutcome::Redirect {
                destination: substitute(dest, &params),
                status: rule.status.unwrap_or(308),
            };
        }
        if let Some(headers) = &rule.headers {
            // Header rules are non-terminal: keep scanning, merge at the end.
            merged.extend(headers.iter().map(|(k, v)| (k.clone(), v.clone())));
            any_headers = true;
        }
    }
    if any_headers {
        RuleOutcome::Headers(merged)
    } else {
        RuleOutcome::None
    }
}

/// Match a rule source (`:param` segments, trailing `*` wildcard, static segments)
/// against decoded URL segments. Returns captured params, or `None` when no match.
/// `*` consumes zero or more remaining segments and captures them as `:splat`.
pub(crate) fn match_source(source: &str, url: &[String]) -> Option<BTreeMap<String, String>> {
    let src = source.strip_prefix('/').unwrap_or(source);
    let mut params = BTreeMap::new();
    if src.is_empty() {
        return if url.is_empty() { Some(params) } else { None };
    }
    let segs: Vec<&str> = src.split('/').filter(|s| !s.is_empty()).collect();
    let mut i = 0usize;
    for (n, s) in segs.iter().enumerate() {
        let last = n + 1 == segs.len();
        if *s == "*" {
            params.insert("splat".to_string(), url[i..].join("/"));
            return Some(params);
        }
        if i >= url.len() {
            return None;
        }
        if let Some(name) = s.strip_prefix(':') {
            if name.is_empty() {
                return None;
            }
            params.insert(name.to_string(), url[i].clone());
        } else if url[i] != *s {
            return None;
        }
        i += 1;
        if last && i != url.len() {
            return None;
        }
    }
    Some(params)
}

/// Substitute `:name` tokens from captured params into a destination; unknown names
/// stay literal.
pub(crate) fn substitute(destination: &str, params: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = destination;
    while let Some(idx) = rest.find(':') {
        out.push_str(&rest[..idx]);
        let after = &rest[idx + 1..];
        let name_len: usize = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .map(char::len_utf8)
            .sum();
        if name_len > 0 {
            let name = &after[..name_len];
            match params.get(name) {
                Some(v) => out.push_str(v),
                None => {
                    out.push(':');
                    out.push_str(name);
                }
            }
            rest = &after[name_len..];
        } else {
            out.push(':');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RateLimit, RulesEngine};

    fn rule(source: &str) -> NativeRule {
        NativeRule {
            source: source.to_string(),
            destination: None,
            status: None,
            rule_type: None,
            headers: None,
            basic_auth: None,
            rate_limit: None,
        }
    }

    #[test]
    fn redirect_defaults_to_308_and_honors_status() {
        let mut r = rule("/old");
        r.destination = Some("/new".to_string());
        let e = RulesEngine::compile(vec![r.clone()]).unwrap();
        assert_eq!(
            e.evaluate("/old"),
            RuleOutcome::Redirect {
                destination: "/new".to_string(),
                status: 308
            }
        );
        let mut r2 = r.clone();
        r2.status = Some(302);
        let e = RulesEngine::compile(vec![r2]).unwrap();
        assert_eq!(
            e.evaluate("/old"),
            RuleOutcome::Redirect {
                destination: "/new".to_string(),
                status: 302
            }
        );
        assert_eq!(e.evaluate("/other"), RuleOutcome::None);
    }

    #[test]
    fn redirect_substitutes_path_params() {
        let mut r = rule("/blog/:slug");
        r.destination = Some("/posts/:slug".to_string());
        let e = RulesEngine::compile(vec![r]).unwrap();
        assert_eq!(
            e.evaluate("/blog/rust-is-nice"),
            RuleOutcome::Redirect {
                destination: "/posts/rust-is-nice".to_string(),
                status: 308
            }
        );
    }

    #[test]
    fn wildcard_captures_splat_for_destinations() {
        let mut r = rule("/docs/*");
        r.destination = Some("https://docs.example.com/:splat".to_string());
        let e = RulesEngine::compile(vec![r]).unwrap();
        assert_eq!(
            e.evaluate("/docs/a/b"),
            RuleOutcome::Redirect {
                destination: "https://docs.example.com/a/b".to_string(),
                status: 308
            }
        );
        // `*` also matches zero remaining segments.
        assert_eq!(
            e.evaluate("/docs"),
            RuleOutcome::Redirect {
                destination: "https://docs.example.com/".to_string(),
                status: 308
            }
        );
    }

    #[test]
    fn rewrites_rewrite() {
        let mut r = rule("/shop/:item");
        r.rule_type = Some("rewrite".to_string());
        r.destination = Some("/products/:item".to_string());
        let e = RulesEngine::compile(vec![r]).unwrap();
        assert_eq!(
            e.evaluate("/shop/42"),
            RuleOutcome::Rewrite("/products/42".to_string())
        );
    }

    #[test]
    fn headers_merge_across_matching_rules() {
        let mut a = rule("/legacy/*");
        a.headers = Some(BTreeMap::from([
            ("x-a".to_string(), "1".to_string()),
            ("x-both".to_string(), "a".to_string()),
        ]));
        let mut b = rule("/legacy/:page");
        b.headers = Some(BTreeMap::from([
            ("x-b".to_string(), "2".to_string()),
            ("x-both".to_string(), "b".to_string()),
        ]));
        let e = RulesEngine::compile(vec![a, b]).unwrap();
        match e.evaluate("/legacy/home") {
            RuleOutcome::Headers(h) => {
                assert_eq!(h.get("x-a").map(String::as_str), Some("1"));
                assert_eq!(h.get("x-b").map(String::as_str), Some("2"));
                assert_eq!(
                    h.get("x-both").map(String::as_str),
                    Some("b"),
                    "later rules win per key"
                );
            }
            other => panic!("expected headers, got {other:?}"),
        }
    }

    #[test]
    fn header_rules_do_not_mask_later_terminal_rules() {
        let mut h = rule("/app");
        h.headers = Some(BTreeMap::from([(
            "x-frame".to_string(),
            "deny".to_string(),
        )]));
        let mut r = rule("/app");
        r.destination = Some("/login".to_string());
        let e = RulesEngine::compile(vec![h, r]).unwrap();
        assert_eq!(
            e.evaluate("/app"),
            RuleOutcome::Redirect {
                destination: "/login".to_string(),
                status: 308
            },
            "headers are non-terminal; first terminal rule wins"
        );
    }

    #[test]
    fn basic_auth_gates_first() {
        let mut r = rule("/admin/*");
        r.basic_auth = Some(("u".to_string(), "p".to_string()));
        r.destination = Some("/login".to_string());
        let e = RulesEngine::compile(vec![r]).unwrap();
        assert_eq!(e.evaluate("/admin/panel"), RuleOutcome::AuthRequired);
        // B-16: a correct `Basic` header authenticates and falls through to
        // the next terminal rule (here, the redirect) rather than challenging.
        assert_eq!(
            e.evaluate_with_auth("/admin/panel", "", Some("Basic dTpw")),
            RuleOutcome::Redirect {
                destination: "/login".to_string(),
                status: 308
            }
        );
        // Wrong password and malformed header both re-challenge.
        assert_eq!(
            e.evaluate_with_auth("/admin/panel", "", Some("Basic dTp4")),
            RuleOutcome::AuthRequired
        );
        assert_eq!(
            e.evaluate_with_auth("/admin/panel", "", Some("Basic !!!notbase64")),
            RuleOutcome::AuthRequired
        );
    }

    #[test]
    fn ratelimit_rules_report_rate_limited() {
        let mut r = rule("/api/*");
        r.rule_type = Some("ratelimit".to_string());
        r.rate_limit = Some(RateLimit {
            key: "ip".to_string(),
            max: 100,
            window_secs: 60,
        });
        // SEC-01: with max=100 the first 100 calls are allowed; the 101st is
        // rate-limited. (Before this fix the rule 429'd unconditionally.)
        let e = RulesEngine::compile(vec![r.clone()]).unwrap();
        for _ in 0..100 {
            assert_ne!(
                e.evaluate("/api/data"),
                RuleOutcome::RateLimited {
                    retry_after_secs: 0
                },
                "within budget is allowed"
            );
        }
        match e.evaluate("/api/data") {
            RuleOutcome::RateLimited { retry_after_secs } => assert!(retry_after_secs >= 1),
            other => panic!("expected RateLimited, got {other:?}"),
        }
        // A rate_limit field without an explicit type is normalized at compile.
        let mut r2 = rule("/v2/*");
        r2.rate_limit = r.rate_limit.clone();
        let e = RulesEngine::compile(vec![r2]).unwrap();
        assert_eq!(e.rules[0].rule_type.as_deref(), Some("ratelimit"));
        assert_ne!(
            e.evaluate("/v2/x"),
            RuleOutcome::RateLimited {
                retry_after_secs: 0
            }
        );
    }

    #[test]
    fn first_matching_rule_wins_in_declaration_order() {
        let mut a = rule("/a");
        a.destination = Some("/x".to_string());
        let mut b = rule("/a");
        b.destination = Some("/y".to_string());
        let e = RulesEngine::compile(vec![a, b]).unwrap();
        assert_eq!(
            e.evaluate("/a"),
            RuleOutcome::Redirect {
                destination: "/x".to_string(),
                status: 308
            }
        );
    }

    #[test]
    fn inert_rules_fall_through_to_later_rules() {
        // A matching rule with no actionable fields is a no-op.
        let mut inert = rule("/p/:id");
        inert.status = Some(200);
        let mut real = rule("/p/:id");
        real.destination = Some("/q/:id".to_string());
        let e = RulesEngine::compile(vec![inert, real]).unwrap();
        assert_eq!(
            e.evaluate("/p/7"),
            RuleOutcome::Redirect {
                destination: "/q/7".to_string(),
                status: 308
            }
        );
    }

    #[test]
    fn trailing_slashes_and_percent_encoding_are_tolerated() {
        let mut r = rule("/old path/:x");
        r.destination = Some("/new/:x".to_string());
        let e = RulesEngine::compile(vec![r]).unwrap();
        assert_eq!(
            e.evaluate("/old%20path/hello%20world/"),
            RuleOutcome::Redirect {
                destination: "/new/hello world".to_string(),
                status: 308
            }
        );
    }

    #[test]
    fn compile_validates_sources_and_outcomes() {
        // rewrite without destination
        let mut r = rule("/a");
        r.rule_type = Some("rewrite".to_string());
        assert_eq!(RulesEngine::compile(vec![r]).unwrap_err().0.code, "RTR-110");
        // `*` not last
        let r = rule("/*/a");
        assert_eq!(RulesEngine::compile(vec![r]).unwrap_err().0.code, "RTR-110");
        // source must be absolute
        let r = rule("relative");
        assert_eq!(RulesEngine::compile(vec![r]).unwrap_err().0.code, "RTR-110");
        // ratelimit without bucket config
        let mut r = rule("/api");
        r.rule_type = Some("ratelimit".to_string());
        assert_eq!(RulesEngine::compile(vec![r]).unwrap_err().0.code, "RTR-110");
        // a valid table compiles
        let e = RulesEngine::compile(vec![rule("/ok")]).unwrap();
        assert_eq!(e.rules.len(), 1);
        assert_eq!(e.evaluate("/ok"), RuleOutcome::None);
    }

    #[test]
    fn match_source_and_substitute_unit_semantics() {
        let url: Vec<String> = vec!["a".into(), "b".into()];
        assert!(match_source("/a/:x", &url).is_some_and(|p| p["x"] == "b"));
        assert!(
            match_source("/a/:x/extra", &url).is_none(),
            "lengths must match"
        );
        assert!(match_source("/", &[]).is_some());
        assert!(match_source("/", &url).is_none());
        assert!(match_source("/a/*", &url).is_some_and(|p| p["splat"] == "b"));
        let params = BTreeMap::from([("id".to_string(), "9".to_string())]);
        assert_eq!(substitute("/p/:id/edit", &params), "/p/9/edit");
        assert_eq!(
            substitute("/p/:unknown", &params),
            "/p/:unknown",
            "unknown names stay literal"
        );
        assert_eq!(substitute("weird:colon", &params), "weird:colon");
    }
}

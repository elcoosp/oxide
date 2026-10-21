//! TDD suite for oxide-server (spec §12, §17, §9.6).
//!
//! Written BEFORE the implementation; every pub fn of the crate is exercised.

use bytes::Bytes;
use futures::future::BoxFuture;
use oxide_router::{NativeRule, RateLimit, RulesEngine};
use oxide_server::*;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

// ------------------------------------------------------------------ helpers

fn req(method: &str, url: &str, headers: &[(&str, &str)]) -> OxideRequest {
    OxideRequest {
        method: method.into(),
        url: url.into(),
        headers: Arc::new(
            headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        ),
        header_index: Default::default(),
        body: Bytes::new(),
        request_id: "req-test".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

fn rule(source: &str) -> NativeRule {
    NativeRule {
        source: source.into(),
        destination: None,
        status: None,
        rule_type: None,
        headers: None,
        basic_auth: None,
        rate_limit: None,
    }
}

fn pipeline(
    rules: RulesEngine,
    assets: Option<StaticAssets>,
    limits: Limits,
    security: SecuritySettings,
) -> Pipeline {
    Pipeline {
        og_rate_limiter: oxide_server::RateLimiter::default(),
        og_rate_max: 10,
        og_rate_window_secs: 60,
        rules,
        assets,
        limits,
        security,
        i18n: None,
        hmr: None,
        hmr_build_id: String::new(),
        rate_limiter: Arc::new(RateLimiter::default()),
        health_path: "/_oxide/health".into(),
        metrics_path: "/_oxide/metrics".into(),
        metrics: None,
        inspector: None,
        page_renderer: None,
        image_endpoint: None,
        og_endpoint: None,
        server_fns: None,
        actions: None,
        tls: false,
        dev_token: None,
        dev_bind_ip: None,
        sockets: None,
        http3_port: 0,
        http3_advertise: false,
    }
}

fn test_assets() -> StaticAssets {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("app.js"), b"console.log(1)").unwrap();
    std::fs::write(dir.path().join("app.js.zst"), b"ZSTD-APP").unwrap();
    std::fs::write(dir.path().join("app.js.br"), b"BR-APP").unwrap();
    std::fs::create_dir_all(dir.path().join("css")).unwrap();
    std::fs::write(dir.path().join("css/site.css"), b"body{}").unwrap();
    std::fs::write(dir.path().join("css/site.css.gz"), b"GZ-CSS").unwrap();
    // keep the tempdir alive for the process lifetime of the test
    let path = dir.path().to_path_buf();
    std::mem::forget(dir);
    StaticAssets::scan(path).unwrap()
}

fn echo_dispatch() -> impl Fn(OxideRequest) -> BoxFuture<'static, OxideResponse> {
    |r: OxideRequest| {
        Box::pin(async move {
            let body = format!("dispatched {} {}", r.method, r.url);
            if !r.body.is_empty() {
                OxideResponse::html(
                    200,
                    format!("{body} body={}", String::from_utf8_lossy(&r.body)),
                )
            } else {
                OxideResponse::html(200, body)
            }
        })
    }
}

// ------------------------------------------------------------------ StaticAssets::scan

#[test]
fn scan_indexes_files_recursively() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
    std::fs::write(dir.path().join("a/b/page.html"), b"<h1>x</h1>").unwrap();
    let assets = StaticAssets::scan(dir.path()).unwrap();
    let e = assets.index.get("/a/b/page.html").unwrap();
    assert_eq!(e.etag.len(), 16);
    assert_eq!(
        blake3::hash(b"<h1>x</h1>").to_hex()[..16].to_string(),
        e.etag
    );
    assert_eq!(e.size, 10);
    assert!(e.mtime > 1_600_000_000);
    assert_eq!(e.content_type, "text/html; charset=utf-8");
    assert_eq!(assets.root, dir.path());
}

#[test]
fn scan_content_type_map() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "a.js", "b.mjs", "c.json", "d.png", "e.jpg", "f.svg", "g.webp", "h.avif", "i.woff2",
        "j.txt", "k.wasm", "l.map", "m.bin",
    ] {
        std::fs::write(dir.path().join(name), b"x").unwrap();
    }
    let assets = StaticAssets::scan(dir.path()).unwrap();
    let ct = |p: &str| assets.index.get(p).unwrap().content_type;
    assert_eq!(ct("/a.js"), "text/javascript; charset=utf-8");
    assert_eq!(ct("/b.mjs"), "text/javascript; charset=utf-8");
    assert_eq!(ct("/c.json"), "application/json");
    assert_eq!(ct("/d.png"), "image/png");
    assert_eq!(ct("/e.jpg"), "image/jpeg");
    assert_eq!(ct("/f.svg"), "image/svg+xml");
    assert_eq!(ct("/g.webp"), "image/webp");
    assert_eq!(ct("/h.avif"), "image/avif");
    assert_eq!(ct("/i.woff2"), "font/woff2");
    assert_eq!(ct("/j.txt"), "text/plain; charset=utf-8");
    assert_eq!(ct("/k.wasm"), "application/wasm");
    assert_eq!(ct("/l.map"), "application/json");
    assert_eq!(ct("/m.bin"), "application/octet-stream");
}

#[test]
fn scan_skips_hidden_and_maps_precompressed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".secret"), b"nope").unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join(".git/x"), b"nope").unwrap();
    std::fs::write(dir.path().join("app.js"), b"1").unwrap();
    std::fs::write(dir.path().join("app.js.zst"), b"z").unwrap();
    std::fs::write(dir.path().join("app.js.br"), b"b").unwrap();
    std::fs::write(dir.path().join("app.js.gz"), b"g").unwrap();
    let assets = StaticAssets::scan(dir.path()).unwrap();
    assert!(!assets.index.contains_key("/.secret"));
    assert!(!assets.index.contains_key("/.git/x"));
    let pre = &assets.index.get("/app.js").unwrap().precompressed;
    assert_eq!(pre.get("zstd").unwrap().file_name().unwrap(), "app.js.zst");
    assert_eq!(pre.get("br").unwrap().file_name().unwrap(), "app.js.br");
    assert_eq!(pre.get("gzip").unwrap().file_name().unwrap(), "app.js.gz");
}

#[test]
fn scan_missing_root_is_error() {
    let e = StaticAssets::scan("/definitely/not/here/oxide").unwrap_err();
    assert_eq!(e.0.code, "SRV-ASSET");
}

// ------------------------------------------------------------------ StaticAssets::serve

#[test]
fn serve_200_with_headers() {
    let assets = test_assets();
    let resp = assets
        .serve(&req("GET", "/app.js", &[]), "/app.js")
        .unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.header("content-type"),
        Some("text/javascript; charset=utf-8")
    );
    let expected_etag = blake3::hash(b"console.log(1)").to_hex()[..16].to_string();
    assert_eq!(
        resp.header("etag"),
        Some(format!("\"{expected_etag}\"").as_str())
    );
    assert_eq!(
        resp.header("cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert_eq!(resp.header("vary"), Some("Accept-Encoding"));
    assert_eq!(resp.body.as_ref(), b"console.log(1)");
}

#[test]
fn serve_304_on_if_none_match() {
    let assets = test_assets();
    let etag = assets.index.get("/app.js").unwrap().etag.clone();
    let r = req(
        "GET",
        "/app.js",
        &[("if-none-match", &format!("\"{etag}\""))],
    );
    let resp = assets.serve(&r, "/app.js").unwrap();
    assert_eq!(resp.status, 304);
    assert!(resp.body.is_empty());
    assert_eq!(resp.header("etag"), Some(format!("\"{etag}\"").as_str()));
    assert_eq!(
        resp.header("cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    // weak + list + star forms also match
    for inm in [
        format!("W/\"{etag}\""),
        format!("\"nope\", \"{etag}\""),
        "*".to_string(),
    ] {
        let r = req("GET", "/app.js", &[("if-none-match", &inm)]);
        assert_eq!(assets.serve(&r, "/app.js").unwrap().status, 304, "{inm}");
    }
}

#[test]
fn serve_negotiates_precompressed() {
    let assets = test_assets();
    let r = req("GET", "/app.js", &[("accept-encoding", "zstd")]);
    let resp = assets.serve(&r, "/app.js").unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("content-encoding"), Some("zstd"));
    assert_eq!(resp.body.as_ref(), b"ZSTD-APP");
    assert_eq!(resp.header("content-length"), Some("8"));

    let r = req("GET", "/css/site.css", &[("accept-encoding", "gzip, br")]);
    let resp = assets.serve(&r, "/css/site.css").unwrap();
    assert_eq!(resp.header("content-encoding"), Some("gzip"));
    assert_eq!(resp.body.as_ref(), b"GZ-CSS");
}

#[test]
fn serve_no_compression_without_accept_encoding() {
    let assets = test_assets();
    let resp = assets
        .serve(&req("GET", "/app.js", &[]), "/app.js")
        .unwrap();
    assert_eq!(resp.header("content-encoding"), None);
    assert_eq!(resp.body.as_ref(), b"console.log(1)");
}

#[test]
fn serve_range_variants() {
    let assets = test_assets();
    let r = req("GET", "/app.js", &[("range", "bytes=0-6")]);
    let resp = assets.serve(&r, "/app.js").unwrap();
    assert_eq!(resp.status, 206);
    assert_eq!(resp.body.as_ref(), b"console");
    assert_eq!(resp.header("content-range"), Some("bytes 0-6/14"));

    let r = req("GET", "/app.js", &[("range", "bytes=7-")]);
    let resp = assets.serve(&r, "/app.js").unwrap();
    assert_eq!(resp.status, 206);
    assert_eq!(resp.body.as_ref(), b".log(1)");
    assert_eq!(resp.header("content-range"), Some("bytes 7-13/14"));

    let r = req("GET", "/app.js", &[("range", "bytes=-4")]);
    let resp = assets.serve(&r, "/app.js").unwrap();
    assert_eq!(resp.status, 206);
    assert_eq!(resp.body.as_ref(), b"g(1)");
    assert_eq!(resp.header("content-range"), Some("bytes 10-13/14"));
}

#[test]
fn serve_invalid_range_is_416() {
    let assets = test_assets();
    for spec in [
        "bytes=99-100",
        "bytes=9-2",
        "bytes=abc",
        "bytes=-0",
        "bytes=0-1,3-4",
    ] {
        let r = req("GET", "/app.js", &[("range", spec)]);
        let resp = assets.serve(&r, "/app.js").unwrap();
        assert_eq!(resp.status, 416, "{spec}");
        assert_eq!(resp.header("content-range"), Some("bytes */14"), "{spec}");
    }
}

#[test]
fn serve_range_on_compressed_variant_serves_200_full() {
    // Documented simplification (§12.1 step 3): no partial decode of compressed
    // variants — the full negotiated representation is served with 200.
    let assets = test_assets();
    let r = req(
        "GET",
        "/app.js",
        &[("accept-encoding", "zstd"), ("range", "bytes=0-2")],
    );
    let resp = assets.serve(&r, "/app.js").unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("content-encoding"), Some("zstd"));
    assert_eq!(resp.body.as_ref(), b"ZSTD-APP");
    assert_eq!(resp.header("content-range"), None);
}

#[test]
fn serve_head_returns_headers_only() {
    let assets = test_assets();
    let resp = assets
        .serve(&req("HEAD", "/app.js", &[]), "/app.js")
        .unwrap();
    assert_eq!(resp.status, 200);
    assert!(resp.body.is_empty());
    assert_eq!(resp.header("content-length"), Some("14"));
    assert_eq!(
        resp.header("content-type"),
        Some("text/javascript; charset=utf-8")
    );
}

#[test]
fn serve_miss_returns_none() {
    let assets = test_assets();
    assert!(
        assets
            .serve(&req("GET", "/nope.js", &[]), "/nope.js")
            .is_none()
    );
}

// ------------------------------------------------------------------ negotiate_encoding

#[test]
fn negotiate_encoding_table() {
    let all = ["zstd", "br", "gzip"];
    let cases: &[(&str, &[&str], Option<&str>)] = &[
        ("", &all, None),
        ("identity", &all, None),
        ("gzip", &all, Some("gzip")),
        ("br;q=0.8, gzip", &["br", "gzip"], Some("br")),
        ("br;q=0, gzip", &["br", "gzip"], Some("gzip")),
        ("*", &["gzip", "zstd"], Some("zstd")),
        ("gzip;q=0, *", &["gzip", "zstd"], Some("zstd")),
        ("deflate", &all, None),
        ("ZSTD", &all, Some("zstd")),
        ("br;q=bad, gzip", &all, Some("gzip")),
        ("zstd;q=0.001", &all, Some("zstd")),
        ("*", &[], None),
        ("gzip", &["zstd"], None),
        ("gzip;q=0.5, br;q=0.9", &all, Some("br")), // br preferred by client, still lower in server order than zstd but zstd not accepted
    ];
    for (i, (accept, avail, expected)) in cases.iter().enumerate() {
        assert_eq!(
            negotiate_encoding(accept, avail).as_deref(),
            *expected,
            "case {i}: {accept}"
        );
    }
}

// ------------------------------------------------------------------ compress_body

#[test]
fn compress_body_gzip_roundtrip() {
    let body = b"hello gzip world hello gzip world".to_vec();
    let out = compress_body("gzip", &body, 6).unwrap();
    let mut dec = flate2::read::GzDecoder::new(&out[..]);
    let mut back = Vec::new();
    std::io::Read::read_to_end(&mut dec, &mut back).unwrap();
    assert_eq!(back, body);
}

#[test]
fn compress_body_zstd_roundtrip_and_level_clamp() {
    let body = vec![b'a'; 5000];
    let out = compress_body("zstd", &body, 99).unwrap(); // clamps to 19
    let back = zstd::stream::decode_all(&out[..]).unwrap();
    assert_eq!(back, body);
}

#[test]
fn compress_body_br_roundtrips() {
    let body = b"hello world, hello world, hello world";
    let out = compress_body("br", body, 5).unwrap();
    assert!(!out.is_empty());
    let mut decoded = Vec::new();
    {
        let mut r = brotli::Decompressor::new(std::io::Cursor::new(&out[..]), 4096);
        std::io::copy(&mut r, &mut decoded).unwrap();
    }
    assert_eq!(decoded.as_slice(), body.as_slice());
}

#[test]
fn compress_body_identity_passthrough() {
    assert_eq!(
        compress_body("identity", b"same", 5).unwrap().as_ref(),
        b"same"
    );
    assert_eq!(compress_body("", b"same", 5).unwrap().as_ref(), b"same");
    assert!(compress_body("quantum", b"x", 5).is_err());
}

// ------------------------------------------------------------------ security headers / CSP

#[test]
fn security_headers_defaults_and_tls() {
    let h = security_headers(false, None);
    let get = |k: &str| h.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    assert_eq!(get("X-Content-Type-Options"), Some("nosniff"));
    assert_eq!(
        get("Referrer-Policy"),
        Some("strict-origin-when-cross-origin")
    );
    assert_eq!(
        get("Permissions-Policy"),
        Some("camera=(), microphone=(), geolocation=()")
    );
    assert_eq!(get("Cross-Origin-Opener-Policy"), Some("same-origin"));
    assert_eq!(get("X-Frame-Options"), Some("DENY"));
    assert!(get("Strict-Transport-Security").is_none());
    assert!(get("Content-Security-Policy").is_none());

    let h = security_headers(true, Some("n0nce"));
    let get = |k: &str| h.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    assert_eq!(
        get("Strict-Transport-Security"),
        Some("max-age=63072000; includeSubDomains")
    );
    assert_eq!(
        get("Content-Security-Policy"),
        Some(csp_header("strict", "n0nce").as_str())
    );
}

#[test]
fn csp_header_modes() {
    let strict = csp_header("strict", "abc");
    assert!(
        strict.contains("script-src 'self' 'nonce-abc' 'strict-dynamic'"),
        "{strict}"
    );
    assert!(strict.starts_with("default-src 'self'; "), "{strict}");
    assert!(strict.contains("style-src 'self' 'unsafe-inline'"));
    assert!(strict.contains("object-src 'none'"));
    assert!(strict.contains("base-uri 'self'"));
    let hash = csp_header("hash", "abc");
    assert!(hash.contains("script-src 'self';"), "{hash}");
    assert!(!hash.contains("nonce"), "{hash}");
    assert_eq!(csp_header("off", "abc"), "");
    assert_eq!(csp_header("wat", "abc"), "");
}

// ------------------------------------------------------------------ CSRF

#[test]
fn csrf_safe_methods_and_off_mode_pass() {
    for m in ["GET", "HEAD", "OPTIONS"] {
        assert!(csrf_check(&req(m, "/", &[]), "origin").is_ok(), "{m}");
    }
    assert!(csrf_check(&req("POST", "/", &[]), "off").is_ok());
}

#[test]
fn csrf_origin_host_comparison() {
    let ok = req(
        "POST",
        "/",
        &[
            ("origin", "https://app.example.com"),
            ("host", "app.example.com"),
        ],
    );
    assert!(csrf_check(&ok, "origin").is_ok());
    let ok_port = req(
        "POST",
        "/",
        &[
            ("origin", "https://app.example.com:8443"),
            ("host", "app.example.com:8443"),
        ],
    );
    assert!(csrf_check(&ok_port, "origin").is_ok());
    let bad = req(
        "POST",
        "/",
        &[
            ("origin", "https://evil.example.net"),
            ("host", "app.example.com"),
        ],
    );
    let e = csrf_check(&bad, "origin").unwrap_err();
    assert_eq!(e.code, "SRV-CSRF");
    let missing_host = req("POST", "/", &[("origin", "https://app.example.com")]);
    assert!(csrf_check(&missing_host, "origin").is_err());
}

#[test]
fn csrf_sec_fetch_site_signals() {
    for site in ["same-origin", "none"] {
        let r = req(
            "POST",
            "/",
            &[
                ("sec-fetch-site", site),
                ("host", "a.com"),
                ("origin", "https://b.com"),
            ],
        );
        assert!(csrf_check(&r, "origin").is_ok(), "{site}");
    }
    let cross = req(
        "POST",
        "/",
        &[("sec-fetch-site", "cross-site"), ("host", "a.com")],
    );
    assert!(csrf_check(&cross, "origin").is_err());
    // missing Origin on a same-origin fetch is allowed (documented fail-open)
    let no_origin = req("POST", "/", &[("sec-fetch-site", "same-origin")]);
    assert!(csrf_check(&no_origin, "origin").is_ok());
    let silent = req("POST", "/", &[]);
    assert!(csrf_check(&silent, "origin").is_ok());
    let same_site = req(
        "POST",
        "/",
        &[
            ("sec-fetch-site", "same-site"),
            ("origin", "https://b.com"),
            ("host", "a.com"),
        ],
    );
    assert!(csrf_check(&same_site, "origin").is_err());
}

#[test]
fn csrf_token_double_submit() {
    let r = req(
        "POST",
        "/",
        &[
            ("origin", "https://evil.net"),
            ("host", "app.com"),
            ("cookie", "oxide_csrf=tok123"),
            ("x-oxide-csrf", "tok123"),
        ],
    );
    assert!(csrf_check(&r, "token").is_ok());
    let mismatch = req(
        "POST",
        "/",
        &[
            ("origin", "https://evil.net"),
            ("host", "app.com"),
            ("cookie", "oxide_csrf=tok123"),
            ("x-oxide-csrf", "other"),
        ],
    );
    assert!(csrf_check(&mismatch, "token").is_err());
    // in origin mode the same request still fails even with matching tokens
    assert!(csrf_check(&mismatch, "origin").is_err());
}

// ------------------------------------------------------------------ SSRF / host validation

#[test]
fn ssrf_allowed_table() {
    let cases: &[(&str, bool)] = &[
        ("10.0.0.5", false),
        ("172.16.0.1", false),
        ("172.31.255.255", false),
        ("172.32.0.1", true),
        ("192.168.1.1", false),
        ("127.0.0.1", false),
        ("169.254.169.254", false),
        ("0.0.0.0", false),
        ("::1", false),
        ("fd00::1", false),
        ("fe80::1", false),
        ("::ffff:127.0.0.1", false),
        ("8.8.8.8", true),
        ("example.com", true),
        ("localhost", false),
        ("LOCALHOST", false),
        ("myhost.local", false),
        ("metadata.google.internal", false),
        ("127.0.0.1:8080", false),
        ("example.com:443", true),
    ];
    for (host, expected) in cases {
        assert_eq!(ssrf_allowed(host, &[]), *expected, "{host}");
    }
    assert!(ssrf_allowed("10.0.0.5", &["10.0.0.5".to_string()]));
    assert!(ssrf_allowed("localhost", &["localhost".to_string()]));
    assert!(!ssrf_allowed("169.254.169.254", &["10.0.0.5".to_string()]));
}

/// SEC-03: every resolved address is validated before connecting. A
/// hostname that resolves to *any* blocked address is rejected — the
/// resolve-then-check is what defeats DNS rebinding.
#[test]
fn sec03_first_blocked_addr_catches_any_blocked_resolution() {
    use oxide_server::first_blocked_addr;
    use std::net::IpAddr;
    let parse = |s: &str| s.parse::<IpAddr>().unwrap();
    // All public → None.
    assert!(first_blocked_addr(&[parse("93.184.216.34"), parse("8.8.8.8")], &[]).is_none());
    // One private among public → that private address.
    let bad = first_blocked_addr(&[parse("8.8.8.8"), parse("10.0.0.5")], &[]);
    assert_eq!(bad, Some(parse("10.0.0.5")));
    // Link-local metadata range is blocked.
    assert_eq!(
        first_blocked_addr(&[parse("169.254.169.254")], &[]),
        Some(parse("169.254.169.254"))
    );
    // An allow-listed host bypasses the range check.
    assert!(first_blocked_addr(&[parse("10.0.0.5")], &["10.0.0.5".to_string()]).is_none());
}

#[test]
fn host_allowed_rules() {
    let empty: Vec<String> = vec![];
    assert!(host_allowed("anything.com", &empty));
    let list = vec!["example.com".to_string(), ".trusted.io".to_string()];
    assert!(host_allowed("example.com", &list));
    assert!(host_allowed("EXAMPLE.com", &list));
    assert!(host_allowed("sub.example.com", &list) == false); // bare entry is exact-only
    assert!(host_allowed("a.trusted.io", &list)); // leading-dot suffix
    assert!(host_allowed("trusted.io", &list) == false); // dot entry does not match apex
    assert!(host_allowed("example.com:8080", &list));
    assert!(!host_allowed("evil.com", &list));
}

// ------------------------------------------------------------------ cookies

#[test]
fn cookie_sign_verify_and_rotation() {
    let keys: &[&[u8]] = &[b"k1", b"k2"];
    let signed = sign_cookie("session", "abc", keys);
    assert_eq!(
        verify_cookie(&signed, keys),
        Some(("session".into(), "abc".into()))
    );
    // signed with the first key, but still verifiable after rotation to the second
    let signed2 = sign_cookie("session", "abc", &[b"k2"]);
    assert_eq!(
        verify_cookie(&signed2, &[b"k9", b"k2"]),
        Some(("session".into(), "abc".into()))
    );
    assert_eq!(verify_cookie("session.abc.deadbeef", keys), None);
    assert_eq!(verify_cookie("session.abc", keys), None);
    // tampered value
    let mut tampered = signed.clone();
    tampered.replace_range(8..11, "xyz");
    assert_eq!(verify_cookie(&tampered, keys), None);
    // deterministic mac: value.mac tail is blake3(key.name.value)[..24]
    let mac = blake3::Hasher::new()
        .update(b"k1")
        .update(b".")
        .update(b"session")
        .update(b".")
        .update(b"abc")
        .finalize()
        .to_hex()[..24]
        .to_string();
    assert_eq!(signed, format!("session.abc.{mac}"));
    // empty keys default to the empty key
    let unsigned = sign_cookie("n", "v", &[]);
    assert_eq!(
        verify_cookie(&unsigned, &[]),
        Some(("n".into(), "v".into()))
    );
}

// ------------------------------------------------------------------ rate limiter

#[test]
fn rate_limiter_burst_then_refill() {
    let rl = RateLimiter::default();
    for _ in 0..5 {
        assert!(rl.allow("k", 5, 2), "bucket starts full");
    }
    assert!(!rl.allow("k", 5, 2), "exhausted burst blocks");
    // separate keys are independent
    assert!(rl.allow("other", 5, 2));
    // simulate the window passing by rewinding the bucket's last-refill instant
    {
        let mut map = rl.buckets.lock().unwrap();
        let e = map.get_mut("k").unwrap();
        e.1 = Instant::now() - Duration::from_secs(2);
    }
    assert!(
        rl.allow("k", 5, 2),
        "refill restores tokens after the window"
    );
    // tokens are capped at max
    assert!(rl.allow("k", 5, 2));
}

// ------------------------------------------------------------------ Pipeline::serve_request

#[tokio::test]
async fn pipeline_limit_violations_echo_request_id() {
    let limits = Limits {
        max_body: 8,
        max_url_len: 16,
        max_headers: 2,
        ..Default::default()
    };
    let p = pipeline(
        RulesEngine::default(),
        None,
        limits,
        SecuritySettings::default(),
    );
    let d = &echo_dispatch();

    let mut r = req("POST", "/", &[]);
    r.body = Bytes::from_static(b"123456789");
    let resp = p.serve_request(r, d).await;
    assert_eq!(resp.status, 413);
    assert_eq!(resp.header("x-oxide-request-id"), Some("req-test"));

    let resp = p
        .serve_request(
            req("GET", "/this-url-is-way-too-long-for-the-limit", &[]),
            d,
        )
        .await;
    assert_eq!(resp.status, 414);
    assert_eq!(resp.header("x-oxide-request-id"), Some("req-test"));

    let r = req("GET", "/x", &[("a", "1"), ("b", "2"), ("c", "3")]);
    let resp = p.serve_request(r, d).await;
    assert_eq!(resp.status, 431);
    assert_eq!(resp.header("x-oxide-request-id"), Some("req-test"));
}

#[tokio::test]
async fn pipeline_host_validation() {
    let sec = SecuritySettings {
        host_allow_list: vec!["app.com".into()],
        ..Default::default()
    };
    let p = pipeline(RulesEngine::default(), None, Limits::default(), sec);
    let d = &echo_dispatch();
    let resp = p
        .serve_request(req("GET", "/x", &[("host", "evil.net")]), d)
        .await;
    assert_eq!(resp.status, 421);
    let resp = p
        .serve_request(req("GET", "/x", &[("host", "app.com")]), d)
        .await;
    assert_eq!(resp.status, 200);
    // no Host header at all → rejected when an allow-list is configured
    let resp = p.serve_request(req("GET", "/x", &[]), d).await;
    assert_eq!(resp.status, 421);
    // empty allow-list disables the check
    let p = pipeline(
        RulesEngine::default(),
        None,
        Limits::default(),
        SecuritySettings::default(),
    );
    let resp = p
        .serve_request(req("GET", "/x", &[("host", "anything")]), d)
        .await;
    assert_eq!(resp.status, 200);
}

#[tokio::test]
async fn pipeline_health_endpoint() {
    let p = pipeline(
        RulesEngine::default(),
        None,
        Limits::default(),
        SecuritySettings::default(),
    );
    let d = &echo_dispatch();
    let resp = p.serve_request(req("GET", "/_oxide/health", &[]), d).await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body.as_ref(), b"ok");
    // not GET → dispatches
    let resp = p.serve_request(req("POST", "/_oxide/health", &[]), d).await;
    assert_eq!(resp.status, 200);
    assert!(String::from_utf8_lossy(&resp.body).contains("dispatched POST"));
}

// ------------------------------------------------------- §20 metrics

#[tokio::test]
async fn metrics_endpoint_renders_prometheus_text() {
    use oxide_server::Metrics;
    use std::sync::Arc;

    let m = Arc::new(Metrics::new());
    m.observe("/", std::time::Duration::from_millis(3), 200);
    let p = Pipeline {
        og_rate_limiter: oxide_server::RateLimiter::default(),
        og_rate_max: 10,
        og_rate_window_secs: 60,
        rules: RulesEngine::default(),
        assets: None,
        limits: Limits::default(),
        security: SecuritySettings::default(),
        i18n: None,
        hmr: None,
        hmr_build_id: String::new(),
        rate_limiter: Arc::new(RateLimiter::default()),
        health_path: "/_oxide/health".into(),
        metrics_path: "/_oxide/metrics".into(),
        metrics: Some(m),
        inspector: None,
        page_renderer: None,
        image_endpoint: None,
        og_endpoint: None,
        server_fns: None,
        actions: None,
        tls: false,
        dev_token: None,
        dev_bind_ip: None,
        sockets: None,
        http3_port: 0,
        http3_advertise: false,
    };
    let d = &echo_dispatch();
    let resp = p.serve_request(req("GET", "/_oxide/metrics", &[]), d).await;
    assert_eq!(resp.status, 200);
    let ct = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    assert!(ct.starts_with("text/plain"), "content-type: {ct}");
    let body = String::from_utf8_lossy(&resp.body);
    assert!(
        body.contains("# TYPE oxide_requests_total counter"),
        "{body}"
    );
    assert!(body.contains("oxide_requests_total 1"), "{body}");
}

#[tokio::test]
async fn vitals_endpoint_records_samples_into_metrics() {
    // §16.6 / §20: the browser's reporting hook beacons web-vital samples to
    // `/_oxide/vitals`; the endpoint folds them into the Prometheus registry.
    use oxide_server::Metrics;
    use std::sync::Arc;

    let m = Arc::new(Metrics::new());
    let p = Pipeline {
        og_rate_limiter: oxide_server::RateLimiter::default(),
        og_rate_max: 10,
        og_rate_window_secs: 60,
        rules: RulesEngine::default(),
        assets: None,
        limits: Limits::default(),
        security: SecuritySettings::default(),
        i18n: None,
        hmr: None,
        hmr_build_id: String::new(),
        rate_limiter: Arc::new(RateLimiter::default()),
        health_path: "/_oxide/health".into(),
        metrics_path: "/_oxide/metrics".into(),
        metrics: Some(Arc::clone(&m)),
        inspector: None,
        page_renderer: None,
        image_endpoint: None,
        og_endpoint: None,
        server_fns: None,
        actions: None,
        tls: false,
        dev_token: None,
        dev_bind_ip: None,
        sockets: None,
        http3_port: 0,
        http3_advertise: false,
    };
    let body = br#"[{"name":"LCP","value":1234.5,"route":"/home"},{"name":"CLS","value":0.05,"route":"/home"}]"#;
    let mut r = req("POST", "/_oxide/vitals", &[]);
    r.body = bytes::Bytes::from_static(body);
    let resp = p.serve_request(r, &echo_dispatch()).await;
    // A beacon is fire-and-forget: 204, no body.
    assert_eq!(resp.status, 204);
    assert!(resp.body.is_empty(), "beacon response must have no body");

    // The samples landed in the registry.
    let text = m.render();
    assert!(
        text.contains("oxide_web_vital_count{metric=\"LCP\",route=\"/home\"} 1"),
        "{text}"
    );
    assert!(text.contains("oxide_web_vital_sum"), "{text}");
}

#[tokio::test]
async fn vitals_endpoint_tolerates_a_malformed_beacon() {
    // A beacon must never produce a page-visible error; a bad body is still 204.
    use oxide_server::Metrics;
    use std::sync::Arc;
    let p = Pipeline {
        og_rate_limiter: oxide_server::RateLimiter::default(),
        og_rate_max: 10,
        og_rate_window_secs: 60,
        rules: RulesEngine::default(),
        assets: None,
        limits: Limits::default(),
        security: SecuritySettings::default(),
        i18n: None,
        hmr: None,
        hmr_build_id: String::new(),
        rate_limiter: Arc::new(RateLimiter::default()),
        health_path: "/_oxide/health".into(),
        metrics_path: "/_oxide/metrics".into(),
        metrics: Some(Arc::new(Metrics::new())),
        inspector: None,
        page_renderer: None,
        image_endpoint: None,
        og_endpoint: None,
        server_fns: None,
        actions: None,
        tls: false,
        dev_token: None,
        dev_bind_ip: None,
        sockets: None,
        http3_port: 0,
        http3_advertise: false,
    };
    let mut r = req("POST", "/_oxide/vitals", &[]);
    r.body = bytes::Bytes::from_static(b"not json");
    let resp = p.serve_request(r, &echo_dispatch()).await;
    assert_eq!(resp.status, 204);
}

#[tokio::test]
async fn metrics_endpoint_404_when_not_configured() {
    let p = Pipeline {
        og_rate_limiter: oxide_server::RateLimiter::default(),
        og_rate_max: 10,
        og_rate_window_secs: 60,
        rules: RulesEngine::default(),
        assets: None,
        limits: Limits::default(),
        security: SecuritySettings::default(),
        i18n: None,
        hmr: None,
        hmr_build_id: String::new(),
        rate_limiter: Arc::new(RateLimiter::default()),
        health_path: "/_oxide/health".into(),
        metrics_path: "/_oxide/metrics".into(),
        metrics: None,
        inspector: None,
        page_renderer: None,
        image_endpoint: None,
        og_endpoint: None,
        server_fns: None,
        actions: None,
        tls: false,
        dev_token: None,
        dev_bind_ip: None,
        sockets: None,
        http3_port: 0,
        http3_advertise: false,
    };
    let d = &echo_dispatch();
    let resp = p.serve_request(req("GET", "/_oxide/metrics", &[]), d).await;
    assert_eq!(resp.status, 404);
}

#[tokio::test]
async fn pipeline_observes_native_branch_requests() {
    use oxide_server::Metrics;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let m = Arc::new(Metrics::new());
    let p = Pipeline {
        og_rate_limiter: oxide_server::RateLimiter::default(),
        og_rate_max: 10,
        og_rate_window_secs: 60,
        rules: RulesEngine::default(),
        assets: None,
        limits: Limits::default(),
        security: SecuritySettings::default(),
        i18n: None,
        hmr: None,
        hmr_build_id: String::new(),
        rate_limiter: Arc::new(RateLimiter::default()),
        health_path: "/_oxide/health".into(),
        metrics_path: "/_oxide/metrics".into(),
        metrics: Some(Arc::clone(&m)),
        inspector: None,
        page_renderer: None,
        image_endpoint: None,
        og_endpoint: None,
        server_fns: None,
        actions: None,
        tls: false,
        dev_token: None,
        dev_bind_ip: None,
        sockets: None,
        http3_port: 0,
        http3_advertise: false,
    };
    let d = &echo_dispatch();
    // Health is a native-prefix branch (_native label).
    let _ = p.serve_request(req("GET", "/_oxide/health", &[]), d).await;
    let _ = p.serve_request(req("GET", "/_oxide/health", &[]), d).await;
    assert_eq!(m.requests_total.load(Ordering::Relaxed), 2);
    let rendered = m.render();
    assert!(
        rendered.contains("oxide_request_duration_ms_count{route=\"_native\"} 2"),
        "{rendered}"
    );
}

#[tokio::test]
async fn with_metrics_attaches_registry() {
    use oxide_server::Metrics;
    use std::sync::Arc;

    let m = Arc::new(Metrics::new());
    let p = Pipeline {
        og_rate_limiter: oxide_server::RateLimiter::default(),
        og_rate_max: 10,
        og_rate_window_secs: 60,
        rules: RulesEngine::default(),
        assets: None,
        limits: Limits::default(),
        security: SecuritySettings::default(),
        i18n: None,
        hmr: None,
        hmr_build_id: String::new(),
        rate_limiter: Arc::new(RateLimiter::default()),
        health_path: "/_oxide/health".into(),
        metrics_path: "/_oxide/metrics".into(),
        metrics: None,
        inspector: None,
        page_renderer: None,
        image_endpoint: None,
        og_endpoint: None,
        server_fns: None,
        actions: None,
        tls: false,
        dev_token: None,
        dev_bind_ip: None,
        sockets: None,
        http3_port: 0,
        http3_advertise: false,
    };
    let p = p.with_metrics(Arc::clone(&m));
    assert!(p.metrics().is_some());
}

// ------------------------------------------------------- §13.4 inspector

#[derive(Debug)]
struct TestInspector {
    state: serde_json::Value,
    html: String,
}

impl InspectorSnapshot for TestInspector {
    fn state_json(&self) -> serde_json::Value {
        self.state.clone()
    }
    fn render_html(&self) -> String {
        self.html.clone()
    }
}

fn inspector_pipeline() -> Pipeline {
    Pipeline {
        og_rate_limiter: oxide_server::RateLimiter::default(),
        og_rate_max: 10,
        og_rate_window_secs: 60,
        rules: RulesEngine::default(),
        assets: None,
        limits: Limits::default(),
        security: SecuritySettings::default(),
        i18n: None,
        hmr: None,
        hmr_build_id: String::new(),
        rate_limiter: Arc::new(RateLimiter::default()),
        health_path: "/_oxide/health".into(),
        metrics_path: "/_oxide/metrics".into(),
        metrics: None,
        inspector: Some(Arc::new(TestInspector {
            state: serde_json::json!({
                "routes": [{"id": "/blog/:slug", "mode": "isr"}],
                "module_graph": [],
            }),
            html: "<!doctype html><title>Oxide Inspector</title>".into(),
        })),
        page_renderer: None,
        image_endpoint: None,
        og_endpoint: None,
        server_fns: None,
        actions: None,
        tls: false,
        dev_token: None,
        dev_bind_ip: None,
        sockets: None,
        http3_port: 0,
        http3_advertise: false,
    }
}

#[tokio::test]
async fn inspector_endpoint_returns_json_state() {
    let p = inspector_pipeline();
    let d = &echo_dispatch();
    let resp = p.serve_request(req("GET", "/__oxide/state", &[]), d).await;
    assert_eq!(resp.status, 200);
    let ct = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    assert!(ct.contains("json"), "content-type: {ct}");
    let v: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(v["routes"][0]["id"], "/blog/:slug");
    assert_eq!(v["routes"][0]["mode"], "isr");
}

#[tokio::test]
async fn inspector_endpoint_returns_html_at_root() {
    let p = inspector_pipeline();
    let d = &echo_dispatch();
    let resp = p.serve_request(req("GET", "/__oxide", &[]), d).await;
    assert_eq!(resp.status, 200);
    let ct = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    assert!(ct.starts_with("text/html"), "content-type: {ct}");
    assert!(String::from_utf8_lossy(&resp.body).contains("Oxide Inspector"));
}

#[tokio::test]
async fn inspector_endpoint_absent_when_not_configured() {
    let p = pipeline(
        RulesEngine::default(),
        None,
        Limits::default(),
        SecuritySettings::default(),
    );
    let d = &echo_dispatch();
    let resp = p.serve_request(req("GET", "/__oxide/state", &[]), d).await;
    // No inspector → falls through to dispatch. The echo_dispatch returns 200
    // with an echo body; the point is the inspector endpoint was NOT hit.
    let body = String::from_utf8_lossy(&resp.body);
    assert!(
        !body.contains("\"routes\""),
        "inspector JSON must not be produced when unconfigured: {body}"
    );
    assert!(
        body.contains("dispatched"),
        "fall-through went to dispatch: {body}"
    );
}

#[tokio::test]
async fn pipeline_rule_outcomes() {
    let mut headers = BTreeMap::new();
    headers.insert("x-rule".to_string(), "1".to_string());
    let rules = RulesEngine {
        limiter: Default::default(),
        rules: vec![
            NativeRule {
                source: "/old/*".into(),
                destination: Some("/new/:splat".into()),
                status: Some(307),
                ..rule("/old/*")
            },
            NativeRule {
                source: "/rw/*".into(),
                destination: Some("/dest/:splat".into()),
                rule_type: Some("rewrite".into()),
                ..rule("/rw/*")
            },
            NativeRule {
                source: "/hdr".into(),
                headers: Some(headers),
                ..rule("/hdr")
            },
            NativeRule {
                source: "/admin".into(),
                basic_auth: Some(("u".into(), "p".into())),
                ..rule("/admin")
            },
            NativeRule {
                source: "/api".into(),
                rule_type: Some("ratelimit".into()),
                rate_limit: Some(RateLimit {
                    key: "k".into(),
                    max: 1,
                    window_secs: 60,
                }),
                ..rule("/api")
            },
        ],
    };
    let p = pipeline(rules, None, Limits::default(), SecuritySettings::default());
    let d = &echo_dispatch();

    let resp = p.serve_request(req("GET", "/old/abc", &[]), d).await;
    assert_eq!(resp.status, 307);
    assert_eq!(resp.header("location"), Some("/new/abc"));

    let resp = p.serve_request(req("GET", "/rw/42", &[]), d).await;
    assert_eq!(resp.status, 200);
    assert!(
        String::from_utf8_lossy(&resp.body).contains("dispatched GET /dest/42"),
        "{}",
        String::from_utf8_lossy(&resp.body)
    );

    let resp = p.serve_request(req("GET", "/hdr", &[]), d).await;
    assert_eq!(resp.header("x-rule"), Some("1"));

    let resp = p.serve_request(req("GET", "/admin", &[]), d).await;
    assert_eq!(resp.status, 401);
    assert_eq!(
        resp.header("www-authenticate"),
        Some("Basic realm=\"oxide\", charset=\"UTF-8\"")
    );

    // SEC-01: `max: 1` allows the first call; the second is rate-limited
    // (before the fix the rule 429'd the very first call, unconditionally).
    let resp = p.serve_request(req("GET", "/api", &[]), d).await;
    assert_eq!(resp.status, 200, "first call is within the token budget");
    let resp = p.serve_request(req("GET", "/api", &[]), d).await;
    assert_eq!(resp.status, 429, "second call exceeds `max: 1`");
    assert!(
        resp.header("retry-after").is_some(),
        "429 carries Retry-After"
    );
}

#[tokio::test]
async fn pipeline_serves_static_with_security_and_rule_headers() {
    let mut headers = BTreeMap::new();
    headers.insert("x-rule".to_string(), "y".to_string());
    let rules = RulesEngine {
        limiter: Default::default(),
        rules: vec![NativeRule {
            source: "/*".into(),
            headers: Some(headers),
            ..rule("/*")
        }],
    };
    let p = pipeline(
        rules,
        Some(test_assets()),
        Limits::default(),
        SecuritySettings::default(),
    );
    let d = &echo_dispatch();
    let resp = p
        .serve_request(req("GET", "/app.js", &[("accept-encoding", "zstd")]), d)
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body.as_ref(), b"ZSTD-APP");
    assert_eq!(resp.header("x-content-type-options"), Some("nosniff"));
    // SEC-11: §12.1 puts static assets (step 3) BEFORE native rules (step 4),
    // so a static file under a header-rule source is served without the rule
    // header — the asset store answers first.
    assert_eq!(resp.header("x-rule"), None);
    assert_eq!(
        resp.header("server-timing"),
        Some(RequestTimings_default_server_timing().as_str())
    );

    // /_oxide/static prefix serves without an extension
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("_oxide/static")).unwrap();
    std::fs::write(dir.path().join("_oxide/static/blob"), b"BIN").unwrap();
    let assets = StaticAssets::scan(dir.path()).unwrap();
    std::mem::forget(dir);
    let p = pipeline(
        RulesEngine::default(),
        Some(assets),
        Limits::default(),
        SecuritySettings::default(),
    );
    let resp = p
        .serve_request(req("GET", "/_oxide/static/blob", &[]), d)
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body.as_ref(), b"BIN");
}

#[allow(non_snake_case)]
fn RequestTimings_default_server_timing() -> String {
    use oxide_render::RequestTimings;
    RequestTimings::default().server_timing()
}

#[tokio::test]
async fn pipeline_dispatch_appends_security_headers_and_timing_once() {
    let p = pipeline(
        RulesEngine::default(),
        None,
        Limits::default(),
        SecuritySettings::default(),
    );
    let dispatch = |r: OxideRequest| -> BoxFuture<'static, OxideResponse> {
        Box::pin(async move {
            OxideResponse::html(200, format!("dispatched {} {}", r.method, r.url))
                .with_header("Server-Timing", "render;dur=9.99")
        })
    };
    let resp = p.serve_request(req("GET", "/page", &[]), &dispatch).await;
    assert_eq!(resp.status, 200);
    assert!(String::from_utf8_lossy(&resp.body).contains("dispatched GET /page"));
    assert_eq!(
        resp.header("server-timing"),
        Some("render;dur=9.99"),
        "dispatch-set timing must be kept"
    );
    assert_eq!(resp.header("x-frame-options"), Some("DENY"));

    // no dispatch timing → PERF-06 emits a real `total;dur=` (not the old
    // all-zero default).
    let resp = p
        .serve_request(req("GET", "/page", &[]), &echo_dispatch())
        .await;
    let st = resp.header("server-timing").expect("timing present");
    assert!(st.starts_with("total;dur="), "real total: {st}");
}

#[tokio::test]
async fn pipeline_csrf_403_before_dispatch() {
    let p = pipeline(
        RulesEngine::default(),
        None,
        Limits::default(),
        SecuritySettings::default(),
    );
    let resp = p
        .serve_request(
            req(
                "POST",
                "/action",
                &[("origin", "https://evil.net"), ("host", "app.com")],
            ),
            &echo_dispatch(),
        )
        .await;
    assert_eq!(resp.status, 403);
    // same-origin POST passes through
    let resp = p
        .serve_request(
            req(
                "POST",
                "/action",
                &[("origin", "https://app.com"), ("host", "app.com")],
            ),
            &echo_dispatch(),
        )
        .await;
    assert_eq!(resp.status, 200);
}

// ------------------------------------------------------------------ hyper conversions

#[tokio::test]
async fn to_hyper_response_carries_status_headers_body() {
    let resp = OxideResponse::html(201, "<i>ok</i>").with_header("X-Custom", "v");
    let hyper_resp = to_hyper_response(resp);
    assert_eq!(hyper_resp.status(), 201);
    assert_eq!(
        hyper_resp.headers().get("content-type").unwrap(),
        "text/html; charset=utf-8"
    );
    assert_eq!(hyper_resp.headers().get("x-custom").unwrap(), "v");
    assert_eq!(hyper_resp.headers().get("content-length").unwrap(), "9");
    let bytes = http_body_util::BodyExt::collect(hyper_resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    assert_eq!(bytes.as_ref(), b"<i>ok</i>");
}

// ------------------------------------------------------------------ Server::serve

async fn free_port_addr() -> std::net::SocketAddr {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    addr
}

async fn raw_http(addr: std::net::SocketAddr, request: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(request.as_bytes()).await.unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).await.unwrap();
    buf
}

#[tokio::test]
async fn server_serves_health_and_dispatch_then_shuts_down() {
    let addr = free_port_addr().await;
    let p = Arc::new(pipeline(
        RulesEngine::default(),
        None,
        Limits::default(),
        SecuritySettings::default(),
    ));
    let dispatch: Arc<dyn Fn(OxideRequest) -> BoxFuture<'static, OxideResponse> + Send + Sync> =
        Arc::new(|r: OxideRequest| {
            Box::pin(async move { OxideResponse::html(200, format!("echo:{}", r.url)) })
        });
    let (tx, rx) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(Server::serve(addr, p, dispatch, rx));

    // wait for the listener (generous budget for parallel load; success
    // returns on the first accepted connection)
    let mut health = String::new();
    for _ in 0..200 {
        if let Ok(s) = tokio::net::TcpStream::connect(addr).await {
            drop(s);
            health = raw_http(
                addr,
                "GET /_oxide/health HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n",
            )
            .await;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(health.starts_with("HTTP/1.1 200"), "{health}");
    assert!(
        health.contains("\r\n\r\nok") || health.ends_with("ok"),
        "{health}"
    );

    let echoed = raw_http(
        addr,
        "GET /hello?x=1 HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(echoed.starts_with("HTTP/1.1 200"), "{echoed}");
    assert!(echoed.contains("echo:/hello?x=1"), "{echoed}");

    // graceful shutdown: signal, then serve() returns Ok
    tx.send(true).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_ok());
}

#[tokio::test]
async fn server_from_hyper_request_enforces_body_limit() {
    // A POST above the limit → 413 from the real serve loop (from_hyper_request path).
    let addr = free_port_addr().await;
    let limits = Limits {
        max_body: 4,
        ..Default::default()
    };
    let p = Arc::new(pipeline(
        RulesEngine::default(),
        None,
        limits,
        SecuritySettings::default(),
    ));
    let dispatch: Arc<dyn Fn(OxideRequest) -> BoxFuture<'static, OxideResponse> + Send + Sync> =
        Arc::new(|r: OxideRequest| {
            Box::pin(async move {
                OxideResponse::html(200, format!("body={}", String::from_utf8_lossy(&r.body)))
            })
        });
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(Server::serve(addr, p, dispatch, rx));
    let mut response = String::new();
    for _ in 0..200 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            let body = "123456789";
            let http = format!(
                "POST /up HTTP/1.1\r\nHost: t\r\nContent-Length: 9\r\nConnection: close\r\n\r\n{body}"
            );
            response = raw_http(addr, &http).await;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(response.starts_with("HTTP/1.1 413"), "{response}");
    let _ = server;
    let _ = _tx;
}

// ------------------------------------------------------------------ ws codec (§9.6)

use oxide_server::ws::{self, Frame};

#[test]
fn ws_roundtrip_unmasked_frames() {
    for f in [
        Frame::Text("héllo".into()),
        Frame::Binary(vec![0, 1, 254, 255]),
        Frame::Ping(b"px".to_vec()),
        Frame::Pong(b"py".to_vec()),
        Frame::Close(None),
        Frame::Close(Some(1001)),
    ] {
        let bytes = ws::encode(&f, false);
        // server→client frames are unmasked; decode is the server-side decoder, so
        // re-decoding them is a protocol violation — check framing shape only.
        assert_eq!(bytes[0] & 0x80, 0x80, "FIN set");
        assert_eq!(bytes[0] & 0x70, 0, "no RSV bits");
    }
}

#[test]
fn ws_encode_opcode_layout_unmasked() {
    let bytes = ws::encode(&Frame::Text("hi".into()), false);
    assert_eq!(bytes, vec![0x81, 0x02, b'h', b'i']);
    let bytes = ws::encode(&Frame::Close(Some(1000)), false);
    assert_eq!(bytes, vec![0x88, 0x02, 0x03, 0xE8]);
    let bytes = ws::encode(&Frame::Close(None), false);
    assert_eq!(bytes, vec![0x88, 0x00]);
}

#[test]
fn ws_decode_roundtrip_masked() {
    let payload = "one".repeat(10);
    let buf = ws::encode(&Frame::Text(payload.clone()), true)
        .into_iter()
        .chain(ws::encode(&Frame::Binary(vec![1, 2, 3]), true))
        .chain(ws::encode(&Frame::Ping(b"p".to_vec()), true))
        .chain(ws::encode(&Frame::Close(Some(1000)), true))
        .collect::<Vec<u8>>();
    let (frames, consumed) = ws::decode(&buf).unwrap();
    assert_eq!(consumed, buf.len());
    assert_eq!(frames[0], Frame::Text(payload));
    assert_eq!(frames[1], Frame::Binary(vec![1, 2, 3]));
    assert_eq!(frames[2], Frame::Ping(b"p".to_vec()));
    assert_eq!(frames[3], Frame::Close(Some(1000)));
}

#[test]
fn ws_decode_unmasked_client_frame_is_violation() {
    let buf = ws::encode(&Frame::Text("hi".into()), false);
    let e = ws::decode(&buf).unwrap_err();
    assert_eq!(e.0.code, "SRV-WS");
}

#[test]
fn ws_decode_reserved_and_rsv_violations() {
    // reserved data opcode 3
    let mut b = ws::encode(&Frame::Text("x".into()), true);
    b[0] = (b[0] & 0xF0) | 0x03;
    assert_eq!(ws::decode(&b).unwrap_err().0.code, "SRV-WS");
    // reserved control opcode 11
    let mut b = ws::encode(&Frame::Ping(vec![]), true);
    b[0] = (b[0] & 0xF0) | 0x0B;
    assert_eq!(ws::decode(&b).unwrap_err().0.code, "SRV-WS");
    // RSV1 set
    let mut b = ws::encode(&Frame::Text("x".into()), true);
    b[0] |= 0x40;
    assert_eq!(ws::decode(&b).unwrap_err().0.code, "SRV-WS");
    // continuation opcode (fragmentation unsupported for now)
    let mut b = ws::encode(&Frame::Text("x".into()), true);
    b[0] = (b[0] & 0xF0) | 0x00;
    assert_eq!(ws::decode(&b).unwrap_err().0.code, "SRV-WS");
}

#[test]
fn ws_decode_control_frame_constraints() {
    // control frame with FIN=0
    let mut b = ws::encode(&Frame::Ping(vec![]), true);
    b[0] &= !0x80;
    assert_eq!(ws::decode(&b).unwrap_err().0.code, "SRV-WS");
    // control frame with 126-byte payload
    let b = ws::encode(&Frame::Ping(vec![0; 126]), true);
    assert_eq!(ws::decode(&b).unwrap_err().0.code, "SRV-WS");
}

#[test]
fn ws_decode_close_payload_rules() {
    // declared 1-byte close payload (RFC6455: close length must be 0 or ≥ 2)
    let mut b = ws::encode(&Frame::Close(Some(1000)), true);
    b[1] = 0x80 | 1;
    b.truncate(2 + 4 + 1); // header + mask key + 1 declared payload byte
    assert_eq!(ws::decode(&b).unwrap_err().0.code, "SRV-WS");

    // invalid UTF-8 text — hand-built frame with a zero mask key.
    //
    // The previous form (`ws::encode` then overwrite the last byte) was
    // flaky: the last byte is the last *masked* payload byte, and the
    // decoder unmasks by XOR-ing with the mask key. The unmasked result
    // depends on what key `encode` happens to use, so the mutation only
    // produced invalid UTF-8 when `mask_key[last] < 0x80` — roughly half
    // the time for a random key.
    //
    // Hand-building the frame with mask key `[0,0,0,0]` means the masked
    // payload equals the raw payload, so the payload byte below (`0xFF`)
    // is unambiguously invalid UTF-8. Deterministic across processes.
    //   byte 0: FIN | opcode(Text=0x1)  = 0x81
    //   byte 1: MASK | len(1)            = 0x81
    //   bytes 2..6: mask key [0,0,0,0]
    //   byte 6: masked payload [0xFF]
    let f: Vec<u8> = vec![0x81, 0x81, 0x00, 0x00, 0x00, 0x00, 0xFF];
    assert_eq!(ws::decode(&f).unwrap_err().0.code, "SRV-WS");
}

#[test]
fn ws_decode_extended_lengths() {
    // 16-bit length (200 bytes)
    let big = vec![7u8; 200];
    let buf = ws::encode(&Frame::Binary(big.clone()), true);
    let (frames, consumed) = ws::decode(&buf).unwrap();
    assert_eq!(consumed, buf.len());
    assert_eq!(frames, vec![Frame::Binary(big)]);
    // 64-bit length (70_000 bytes)
    let huge = vec![3u8; 70_000];
    let buf = ws::encode(&Frame::Binary(huge.clone()), true);
    let (frames, consumed) = ws::decode(&buf).unwrap();
    assert_eq!(consumed, buf.len());
    assert_eq!(frames, vec![Frame::Binary(huge)]);
}

#[test]
fn ws_decode_partial_and_multi_frame() {
    let full = ws::encode(&Frame::Text("hello".into()), true);
    // truncated header
    let (frames, consumed) = ws::decode(&full[..2]).unwrap();
    assert_eq!((frames, consumed), (vec![], 0));
    // truncated payload
    let (frames, consumed) = ws::decode(&full[..full.len() - 1]).unwrap();
    assert_eq!((frames, consumed), (vec![], 0));
    // two frames plus a partial third
    let mut buf = full.clone();
    buf.extend_from_slice(&full);
    buf.extend_from_slice(&full[..3]);
    let (frames, consumed) = ws::decode(&buf).unwrap();
    assert_eq!(frames.len(), 2);
    assert_eq!(consumed, full.len() * 2);
}

#[test]
fn ws_mask_key_deterministic() {
    let k1 = ws::next_mask_key();
    let k2 = ws::next_mask_key();
    assert_ne!(k1, k2, "counter PRNG advances");
    // encode(mask=true) embeds a fresh PRNG key and masks the payload with it.
    // (Exact-byte prediction against the shared counter is not asserted: other
    // tests consume keys concurrently under cargo's parallel test runner.)
    let payload = b"maskme";
    let bytes = ws::encode(
        &Frame::Text(String::from_utf8(payload.to_vec()).unwrap()),
        true,
    );
    assert_eq!(&bytes[..2], &[0x81, 0x80 | 6u8]);
    let k = &bytes[2..6];
    let unmasked: Vec<u8> = bytes[6..]
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ k[i % 4])
        .collect();
    assert_eq!(unmasked, payload);
    // each masked frame consumes a fresh key from the monotonic counter
    let other = ws::encode(
        &Frame::Text(String::from_utf8(payload.to_vec()).unwrap()),
        true,
    );
    assert_ne!(other[2..6], bytes[2..6], "fresh PRNG key per frame");
}

// ------------------------------------------------------- §20 tracing

/// A subscriber that captures span names into a shared Vec for assertions.
/// `tracing::subscriber::set_default` scopes it to one test at a time.
#[derive(Default)]
struct SpanCapture {
    names: std::sync::Mutex<Vec<String>>,
}

impl tracing::Subscriber for SpanCapture {
    fn enabled(&self, _meta: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        self.names
            .lock()
            .unwrap()
            .push(span.metadata().name().to_string());
        tracing::span::Id::from_u64(self.names.lock().unwrap().len() as u64)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, _event: &tracing::Event<'_>) {}
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

#[tokio::test]
async fn serve_request_emits_trace_span() {
    let capture = Arc::new(SpanCapture::default());
    let _guard = tracing::subscriber::set_default(capture.clone());

    let p = pipeline(
        RulesEngine::default(),
        None,
        Limits::default(),
        SecuritySettings::default(),
    );
    let d = &echo_dispatch();
    // Native branch: health endpoint.
    let _ = p.serve_request(req("GET", "/_oxide/health", &[]), d).await;

    let names = capture.names.lock().unwrap().clone();
    assert!(
        names.iter().any(|n| n == "oxide.request"),
        "expected oxide.request span, got {names:?}"
    );
}

#[tokio::test]
async fn serve_request_span_has_no_path_leak_into_metrics() {
    // A request to an arbitrary deep path must not create a per-path metric
    // label (metrics use route patterns, not resolved paths).
    let capture = Arc::new(SpanCapture::default());
    let _guard = tracing::subscriber::set_default(capture.clone());

    let p = pipeline(
        RulesEngine::default(),
        None,
        Limits::default(),
        SecuritySettings::default(),
    );
    let d = &echo_dispatch();
    let _ = p
        .serve_request(req("GET", "/arbitrary/deep/path/xyz", &[]), d)
        .await;

    // The span exists, but the metrics registry only records the `_native`
    // sentinel (dispatch is delegate-owned).
    let metrics = p.metrics.as_ref();
    if let Some(m) = metrics {
        let text = m.render();
        assert!(
            !text.contains("arbitrary"),
            "path leaked into metrics: {text}"
        );
    }
}

// SEC-02: CSPRNG request ids — two fresh nonces differ (no counter restart
// collision); HSTS only on TLS.
#[test]
fn nonces_are_unpredictable_across_ids() {
    let a = nonce_for("req-aaa");
    let b = nonce_for("req-bbb");
    assert_ne!(a, b, "distinct request ids must yield distinct nonces");
    assert_eq!(a.len(), 32, "nonce is blake3-32hex");
}

#[tokio::test]
async fn hsts_emitted_only_on_tls() {
    let plain = Pipeline {
        tls: false,
        dev_token: None,
        dev_bind_ip: None,
        ..Default::default()
    };
    let tls_p = Pipeline {
        tls: true,
        dev_token: None,
        dev_bind_ip: None,
        ..Default::default()
    };
    let d = &echo_dispatch();
    let r_plain = plain.serve_request(req("GET", "/", &[]), d).await;
    let r_tls = tls_p.serve_request(req("GET", "/", &[]), d).await;
    let has_hsts = |r: &OxideResponse| {
        r.headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("strict-transport-security"))
    };
    assert!(!has_hsts(&r_plain), "plain HTTP must not emit HSTS");
    assert!(has_hsts(&r_tls), "TLS must emit HSTS");
}

#[test]
fn csrf_token_mode_matrix() {
    // SEC-04: token rescue compares oxide_csrf cookie vs x-oxide-csrf header.
    let pair = |h: &str, c: &str| {
        req(
            "POST",
            "/",
            &[
                ("origin", "https://evil.test"),
                ("host", "app.test"),
                ("x-oxide-csrf", h),
                ("cookie", &format!("oxide_csrf={c}")),
            ],
        )
    };
    assert!(csrf_check(&pair("tok123", "tok123"), "token").is_ok());
    assert!(csrf_check(&pair("tok123", "other"), "token").is_err());
    // Missing pair → still rejected on cross-origin POST.
    let bare = req(
        "POST",
        "/",
        &[("origin", "https://evil.test"), ("host", "app.test")],
    );
    assert!(csrf_check(&bare, "token").is_err());
}

#[tokio::test]
async fn csrf_token_issued_on_get_without_cookie() {
    // SEC-04 issuer: GET in token mode without the cookie → Set-Cookie.
    let mut sec = SecuritySettings::default();
    sec.csrf = "token".into();
    let p = pipeline(RulesEngine::default(), None, Limits::default(), sec);
    let d = &echo_dispatch();
    let r = p.serve_request(req("GET", "/", &[]), d).await;
    let set: Vec<&str> = r
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(set.len(), 1, "one csrf cookie issued: {set:?}");
    assert!(set[0].starts_with("oxide_csrf="), "cookie name: {}", set[0]);
    assert!(set[0].contains("SameSite=Lax"), "samesite: {}", set[0]);
    // Second request carrying the cookie → no re-issue.
    let token = set[0].split(';').next().unwrap();
    let r2 = p
        .serve_request(req("GET", "/", &[("cookie", token)]), d)
        .await;
    assert!(
        r2.headers
            .iter()
            .all(|(k, _)| !k.eq_ignore_ascii_case("set-cookie")),
        "no re-issue when cookie present"
    );
    // ...and the issued pair passes the double-submit check on POST.
    let r3 = req(
        "POST",
        "/",
        &[
            ("origin", "https://evil.test"),
            ("host", "app.test"),
            ("x-oxide-csrf", token.trim_start_matches("oxide_csrf=")),
            ("cookie", token),
        ],
    );
    assert!(csrf_check(&r3, "token").is_ok());
}

/// SEC-05: a trusted proxy's X-Forwarded-* headers are honoured; an
/// untrusted peer's are ignored entirely.
#[test]
fn sec05_trusted_proxy_resolution() {
    use oxide_server::{effective_client, peer_is_trusted};
    use std::net::IpAddr;
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();
    let trusted = vec!["10.0.0.0/8".to_string(), "127.0.0.1/32".to_string()];

    // A peer inside a trusted CIDR is trusted.
    assert!(peer_is_trusted(ip("10.1.2.3"), &trusted));
    assert!(peer_is_trusted(ip("127.0.0.1"), &trusted));
    // An arbitrary peer is not.
    assert!(!peer_is_trusted(ip("8.8.8.8"), &trusted));
    // Empty trust list trusts nobody.
    assert!(!peer_is_trusted(ip("127.0.0.1"), &[]));

    // Trusted peer: XFF / proto / host honoured.
    let (client, proto, host) = effective_client(
        ip("10.0.0.5"),
        &trusted,
        Some("203.0.113.7, 10.0.0.5"),
        Some("https"),
        Some("app.example.com"),
        Some("internal.local"),
    );
    assert_eq!(client, Some("203.0.113.7"));
    assert_eq!(proto, Some("https"));
    assert_eq!(host, Some("app.example.com"));

    // Untrusted peer: every forwarded header ignored, real Host only.
    let (client, proto, host) = effective_client(
        ip("8.8.8.8"),
        &trusted,
        Some("203.0.113.7"),
        Some("https"),
        Some("evil.example"),
        Some("real.host"),
    );
    assert_eq!(client, None);
    assert_eq!(proto, None);
    assert_eq!(host, Some("real.host"));
}

/// S-03/S-04: a WebSocket `Origin` must match the request authority (or the
/// allow-list); a foreign origin is rejected, and a missing origin is rejected
/// in production but allowed in dev.
#[test]
fn ws_origin_gate_matches_authority() {
    use oxide_server::ws_origin_ok;
    let none: Vec<String> = Vec::new();
    let allow = vec!["app.example.com".to_string()];

    // Same host → allowed.
    assert!(ws_origin_ok(
        Some("https://app.com"),
        Some("app.com"),
        &none,
        false
    ));
    // Different host, not allow-listed → rejected.
    assert!(!ws_origin_ok(
        Some("https://evil.net"),
        Some("app.com"),
        &none,
        false
    ));
    // Different host but allow-listed → allowed.
    assert!(ws_origin_ok(
        Some("https://app.example.com"),
        Some("app.com"),
        &allow,
        false
    ));
    // Host:port on the authority is stripped before comparison.
    assert!(ws_origin_ok(
        Some("http://localhost:5173"),
        Some("localhost:5173"),
        &none,
        false
    ));
    // No Origin: rejected in production, allowed in dev.
    assert!(!ws_origin_ok(None, Some("app.com"), &none, false));
    assert!(ws_origin_ok(None, Some("app.com"), &none, true));
    // Malformed origin rejected.
    assert!(!ws_origin_ok(
        Some("not a uri"),
        Some("app.com"),
        &none,
        false
    ));
}

/// S-21.1: `dev_host_allowed` accepts loopback literals and the bind IP, and
/// rejects an arbitrary host (DNS rebinding).
#[test]
fn dev_host_gate_rejects_rebinding() {
    use oxide_server::dev_host_allowed;
    let bind: std::net::IpAddr = "127.0.0.1".parse().unwrap();
    assert!(dev_host_allowed("localhost", bind));
    assert!(dev_host_allowed("127.0.0.1:5173", bind));
    assert!(dev_host_allowed("[::1]:5173", bind));
    assert!(!dev_host_allowed("evil.example.com", bind));
}

/// SEC-06: a slowloris (a client that opens a connection and drips headers
/// without ever completing them) is dropped after `slow_request_timeout_ms`,
/// not held forever.
#[tokio::test]
async fn sec06_slow_header_drip_is_dropped() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let addr = free_port_addr().await;
    // Short header-read timeout so the test is fast.
    let limits = Limits {
        slow_request_timeout_ms: 300,
        ..Default::default()
    };
    let p = Arc::new(pipeline(
        RulesEngine::default(),
        None,
        limits,
        SecuritySettings::default(),
    ));
    let dispatch: Arc<dyn Fn(OxideRequest) -> BoxFuture<'static, OxideResponse> + Send + Sync> =
        Arc::new(|_r: OxideRequest| Box::pin(async move { OxideResponse::html(200, "ok") }));
    let (tx, rx) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(Server::serve(addr, p, dispatch, rx));

    // Wait for the listener.
    for _ in 0..200 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    // Drip a partial request line + one header, never terminate with CRLFCRLF.
    s.write_all(b"GET / HTTP/1.1\r\nHost: t\r\n").await.unwrap();
    // The server must close the connection within a few multiples of the
    // timeout; a read then returns 0 (EOF) rather than blocking forever.
    let mut buf = [0u8; 16];
    let res = tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf)).await;
    match res {
        Ok(Ok(0)) => {} // EOF: the server closed the connection — good
        Ok(Ok(n)) => panic!("expected EOF, got {n} bytes: {:?}", &buf[..n]),
        Ok(Err(e)) => panic!("unexpected read error: {e}"),
        Err(_) => panic!("slowloris connection was NOT dropped within 5s"),
    }

    tx.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(10), server).await;
}

/// SEC-09: the dev inspector is gated on a session token; a request without
/// it is 403, with it (query or header) is served.
#[tokio::test]
async fn sec09_inspector_requires_the_dev_token() {
    use oxide_server::InspectorSnapshot;
    #[derive(Debug)]
    struct FakeInsp;
    impl InspectorSnapshot for FakeInsp {
        fn state_json(&self) -> serde_json::Value {
            serde_json::json!({})
        }
    }
    let mut p = pipeline(
        RulesEngine::default(),
        None,
        Limits::default(),
        SecuritySettings::default(),
    );
    p.inspector = Some(Arc::new(FakeInsp));
    p.dev_token = Some("secret-token".into());
    let d = &echo_dispatch();

    // No token → 403.
    let resp = p.serve_request(req("GET", "/__oxide/state", &[]), d).await;
    assert_eq!(resp.status, 403, "inspector without token is refused");

    // Query token → 200.
    let resp = p
        .serve_request(req("GET", "/__oxide/state?token=secret-token", &[]), d)
        .await;
    assert_eq!(resp.status, 200, "query token admits");

    // Header token → 200.
    let resp = p
        .serve_request(
            req(
                "GET",
                "/__oxide/state",
                &[("x-oxide-dev-token", "secret-token")],
            ),
            d,
        )
        .await;
    assert_eq!(resp.status, 200, "header token admits");

    // Wrong token → 403.
    let resp = p
        .serve_request(req("GET", "/__oxide/state?token=nope", &[]), d)
        .await;
    assert_eq!(resp.status, 403, "wrong token refused");
}

/// SEC-09: the dev-host check rejects a non-local Host (DNS rebinding).
#[test]
fn sec09_dev_host_allowed() {
    use oxide_server::dev_host_allowed;
    use std::net::IpAddr;
    let ip = |s: &str| s.parse::<IpAddr>().unwrap();
    assert!(dev_host_allowed("localhost:3000", ip("127.0.0.1")));
    assert!(dev_host_allowed("127.0.0.1", ip("127.0.0.1")));
    assert!(dev_host_allowed("127.0.0.1", ip("0.0.0.0")));
    assert!(!dev_host_allowed("evil.example", ip("127.0.0.1")));
}

/// P-14: header() is case-insensitive, first occurrence wins, and a request
/// clone shares the header vector (Arc) instead of deep-copying it.
#[test]
fn p14_header_lookup_case_insensitive_and_first_wins() {
    let r = req(
        "GET",
        "http://x/",
        &[("X-Test", "a"), ("x-test", "b"), ("Accept", "text/html")],
    );
    assert_eq!(r.header("X-TEST"), Some("a"));
    assert_eq!(r.header("x-Test"), Some("a"));
    assert_eq!(r.header("accept"), Some("text/html"));
    assert_eq!(r.header("missing"), None);
    let c = r.clone();
    assert!(
        std::sync::Arc::ptr_eq(&r.headers, &c.headers),
        "clone shares the header Arc"
    );
    assert_eq!(c.header("Accept"), Some("text/html"), "clone index rebuilt");
}

/// PERF-06: a compressible HTML response is compressed when the client offers
/// an encoding; a non-compressible one is left alone.
#[tokio::test]
async fn perf06_dynamic_compression() {
    let p = pipeline(
        RulesEngine::default(),
        None,
        Limits::default(),
        SecuritySettings::default(),
    );
    // A dispatch returning a large compressible HTML body.
    let body = "<html><body>".to_string() + &"hello oxide ".repeat(200) + "</body></html>";
    let d = {
        let body = body.clone();
        move |_r: OxideRequest| {
            let body = body.clone();
            Box::pin(async move {
                OxideResponse::new(200, body)
                    .with_header("content-type", "text/html; charset=utf-8")
            }) as BoxFuture<'static, OxideResponse>
        }
    };

    // With `accept-encoding: gzip` the response carries Content-Encoding: gzip.
    let resp = p
        .serve_request(req("GET", "/", &[("accept-encoding", "gzip")]), &d)
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.header("content-encoding"),
        Some("gzip"),
        "compressed when offered"
    );
    assert!(resp.body.len() < body.len(), "body shrank");
    assert!(resp.header("vary").is_some(), "Vary: Accept-Encoding set");

    // Without accept-encoding the body is uncompressed.
    let resp = p.serve_request(req("GET", "/", &[]), &d).await;
    assert!(
        resp.header("content-encoding").is_none(),
        "no AE -> identity"
    );
    assert_eq!(resp.body.len(), body.len());
}

/// P-15: a streamed HTML body is compressed on the fly; the concatenated
/// chunks decode to the exact uncompressed bytes.
#[tokio::test]
async fn p15_streamed_html_is_compressed() {
    use futures::StreamExt as _;
    let p = pipeline(
        RulesEngine::default(),
        None,
        Limits::default(),
        SecuritySettings::default(),
    );
    let body = "<html><body>".to_string() + &"streamed oxide ".repeat(400) + "</body></html>";
    let expected = body.clone();
    let d = {
        let body = body.clone();
        move |_r: OxideRequest| {
            let body = body.clone();
            Box::pin(async move {
                let chunks: Vec<std::result::Result<Bytes, std::io::Error>> = body
                    .as_bytes()
                    .chunks(97)
                    .map(|c| Ok(Bytes::copy_from_slice(c)))
                    .collect();
                let st = futures::stream::iter(chunks);
                OxideResponse::stream(200, Box::pin(st))
                    .with_header("content-type", "text/html; charset=utf-8")
            }) as BoxFuture<'static, OxideResponse>
        }
    };

    let resp = p
        .serve_request(req("GET", "/", &[("accept-encoding", "zstd")]), &d)
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("content-encoding"), Some("zstd"));
    assert!(resp.header("vary").is_some(), "Vary: Accept-Encoding set");
    let mut st = resp.stream.expect("streamed body");
    let mut wire = Vec::new();
    while let Some(chunk) = st.next().await {
        wire.extend_from_slice(&chunk.expect("chunk"));
    }
    let decoded = zstd::stream::decode_all(&wire[..]).expect("valid zstd");
    assert_eq!(String::from_utf8(decoded).unwrap(), expected);
}

/// PERF-06: the framework dispatch path emits a real `Server-Timing` total
/// (not the previous all-zero `RequestTimings::default()`).
#[tokio::test]
async fn perf06_dispatch_emits_real_server_timing_total() {
    let p = pipeline(
        RulesEngine::default(),
        None,
        Limits::default(),
        SecuritySettings::default(),
    );
    let d = echo_dispatch();
    // A routed page path (not a native prefix) so the Dispatch arm runs.
    let resp = p.serve_request(req("GET", "/blog", &[]), &d).await;
    let st = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("server-timing"))
        .map(|(_, v)| v.clone())
        .expect("Server-Timing present");
    assert!(st.starts_with("total;dur="), "real total: {st}");
}

// ─── Wave A: ssrf header allowlist (A6) ──────────────────────────────────

#[test]
fn ssrf_forwardable_headers_drops_credentials() {
    let headers = vec![
        ("Authorization".to_string(), "Bearer secret".to_string()),
        ("Cookie".to_string(), "session=abc".to_string()),
        ("Accept".to_string(), "application/json".to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("User-Agent".to_string(), "oxide-test".to_string()),
        ("X-Custom".to_string(), "dropped".to_string()),
    ];
    let out = oxide_server::ssrf_forwardable_headers(&headers);
    let names: Vec<&str> = out.iter().map(|(k, _)| k.as_str()).collect();
    assert!(
        !names
            .iter()
            .any(|n| n.eq_ignore_ascii_case("authorization")),
        "Authorization dropped: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.eq_ignore_ascii_case("cookie")),
        "Cookie dropped: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.eq_ignore_ascii_case("x-custom")),
        "unknown dropped: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("accept")),
        "accept kept"
    );
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("content-type")),
        "content-type kept"
    );
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("user-agent")),
        "user-agent kept"
    );
    assert_eq!(
        out.len(),
        3,
        "exactly the three allowlisted headers survive"
    );
}

// ─── Wave A: asset-cache TTL (A5) ────────────────────────────────────────

#[test]
fn asset_cache_ttl_defaults_and_overrides() {
    // Default when the env var is unset.
    unsafe {
        std::env::remove_var("OXIDE_ASSET_CACHE_TTL_SECS");
    }
    assert_eq!(
        oxide_server::asset_cache_ttl(),
        std::time::Duration::from_secs(300),
        "default TTL is 300s",
    );
    // Override.
    unsafe {
        std::env::set_var("OXIDE_ASSET_CACHE_TTL_SECS", "5");
    }
    assert_eq!(
        oxide_server::asset_cache_ttl(),
        std::time::Duration::from_secs(5),
        "env overrides the TTL",
    );
    unsafe {
        std::env::remove_var("OXIDE_ASSET_CACHE_TTL_SECS");
    }
}

// ─── Wave A: OG rate limit (A3) ──────────────────────────────────────────

#[test]
fn rate_limiter_allows_max_then_denies() {
    let rl = oxide_server::RateLimiter::default();
    // Capacity 10, one-minute window. The bucket starts full, so the first
    // ten calls succeed and the eleventh is denied (no time has passed to
    // refill).
    for i in 0..10 {
        assert!(rl.allow("1.2.3.4", 10, 60), "call {i} allowed");
    }
    assert!(!rl.allow("1.2.3.4", 10, 60), "eleventh denied");
    // A different peer has its own bucket.
    assert!(rl.allow("5.6.7.8", 10, 60), "other peer unaffected");
}

// ─── Wave C: alt-svc honesty valve (C3) ──────────────────────────────────

/// With `http3_advertise` off, no `Alt-Svc` header is emitted — a browser
/// stays on h1/h2 even though the h3 listener may be running.
#[tokio::test]
async fn alt_svc_is_absent_when_advertisement_is_off() {
    let mut p = pipeline(
        RulesEngine::default(),
        None,
        Limits::default(),
        SecuritySettings::default(),
    );
    p.http3_port = 8443;
    p.http3_advertise = false;
    let resp = p
        .serve_request(req("GET", "/page", &[]), &echo_dispatch())
        .await;
    assert!(
        resp.header("alt-svc").is_none(),
        "no alt-svc with the valve off",
    );
}

/// With it on, the header names the configured port.
#[tokio::test]
async fn alt_svc_names_the_port_when_enabled() {
    let mut p = pipeline(
        RulesEngine::default(),
        None,
        Limits::default(),
        SecuritySettings::default(),
    );
    p.http3_port = 8443;
    p.http3_advertise = true;
    let resp = p
        .serve_request(req("GET", "/page", &[]), &echo_dispatch())
        .await;
    let v = resp.header("alt-svc").expect("alt-svc present");
    assert!(v.contains("8443"), "names the port: {v}");
    assert!(v.contains("h3"), "names h3: {v}");
}

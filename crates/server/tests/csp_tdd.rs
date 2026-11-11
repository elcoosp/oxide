//! TDD suite for the V2-E04 CSP half (oxide-fw-server): the Trusted Types
//! directives on the emitted policy, the `report-only` CSP mode
//! (`Content-Security-Policy-Report-Only` + the `Reporting-Endpoints` group
//! the `/_oxide/csp-report` sink backs) and the violation-report endpoint
//! itself (both browser formats, rate limited 20/min/IP, oversized bodies
//! rejected, structured log fields, no cookies).
//!
//! Written BEFORE the implementation; every pin imports public API only.

use oxide_fw_server::*;

fn report_limiter() -> RateLimiter {
    RateLimiter::default()
}

// ------------------------------------------------------------------ Trusted Types directives

#[test]
fn tt_directives_off_is_empty() {
    assert_eq!(tt_directives("off"), "");
    assert_eq!(tt_directives(""), "");
    // An unknown spelling is `off` — the config validator (CFG-SEC) already
    // rejects it at load time; the header path never panics on one.
    assert_eq!(tt_directives("strict"), "");
}

#[test]
fn tt_directives_enforce_names_the_oxide_policy() {
    let d = tt_directives("enforce");
    assert!(d.contains("require-trusted-types-for 'script'"), "{d}");
    assert!(d.contains("trusted-types oxide default"), "{d}");
}

#[test]
fn tt_report_directives_are_the_same_bytes() {
    // `report` rides an additional Report-Only policy — the directive bytes
    // are identical, only the header they land on differs.
    assert_eq!(tt_directives("report"), tt_directives("enforce"));
}

// ------------------------------------------------------------------ csp_headers_for

#[test]
fn strict_mode_emits_one_enforced_header_without_tt() {
    let hs = csp_headers_for("strict", "NONCE1", "off");
    assert_eq!(hs.len(), 1, "{hs:?}");
    assert_eq!(hs[0].0, "Content-Security-Policy");
    assert!(hs[0].1.contains("'nonce-NONCE1'"), "{}", hs[0].1);
    assert!(!hs[0].1.contains("trusted-types"), "{}", hs[0].1);
}

#[test]
fn hash_mode_keeps_the_hash_policy() {
    let hs = csp_headers_for("hash", "NONCE1", "off");
    assert_eq!(hs.len(), 1, "{hs:?}");
    assert_eq!(hs[0].0, "Content-Security-Policy");
    assert!(hs[0].1.contains("default-src 'self'"), "{}", hs[0].1);
}

#[test]
fn off_mode_emits_no_csp_header() {
    assert_eq!(
        csp_headers_for("off", "NONCE1", "off").len(),
        0,
        "off mode emits no header"
    );
}

#[test]
fn tt_enforce_lands_on_the_enforced_header() {
    let hs = csp_headers_for("strict", "NONCE1", "enforce");
    assert_eq!(hs.len(), 1, "{hs:?}");
    assert!(
        hs[0].1.contains("require-trusted-types-for 'script'"),
        "{}",
        hs[0].1
    );
    assert!(
        hs[0].1.contains("trusted-types oxide default"),
        "{}",
        hs[0].1
    );
}

#[test]
fn tt_report_rides_an_additional_report_only_policy() {
    // V2-E04: with `report`, violations are reported, nothing blocked — the
    // enforced header keeps its pre-E04 bytes and the TT directives ride a
    // second, Report-Only header.
    let hs = csp_headers_for("strict", "NONCE1", "report");
    assert_eq!(hs.len(), 2, "{hs:?}");
    assert_eq!(hs[0].0, "Content-Security-Policy");
    assert!(!hs[0].1.contains("trusted-types"), "{}", hs[0].1);
    assert_eq!(hs[1].0, "Content-Security-Policy-Report-Only");
    assert!(
        hs[1].1.contains("require-trusted-types-for 'script'"),
        "{}",
        hs[1].1
    );
}

#[test]
fn report_only_mode_sends_the_report_only_header_and_the_reporting_endpoints_group() {
    // V2-E04: `report-only` — the policy rides
    // `Content-Security-Policy-Report-Only` with a `report-to` group the
    // `/_oxide/csp-report` endpoint backs; the enforced header stays absent.
    let hs = csp_headers_for("report-only", "NONCE1", "off");
    assert_eq!(hs.len(), 2, "{hs:?}");
    assert_eq!(hs[0].0, "Content-Security-Policy-Report-Only");
    assert!(hs[0].1.contains("'nonce-NONCE1'"), "{}", hs[0].1);
    assert!(hs[0].1.contains("report-to"), "{}", hs[0].1);
    assert_eq!(hs[1].0, "Reporting-Endpoints");
    assert!(
        hs[1].1.contains("csp-endpoint=\"/_oxide/csp-report\""),
        "{}",
        hs[1].1
    );
}

#[test]
fn report_only_with_tt_enforce_adds_the_tt_directives_as_the_enforced_policy() {
    // The deployable observation posture: CSP reported, Trusted Types
    // enforced — the enforced header carries only the TT directives.
    let hs = csp_headers_for("report-only", "NONCE1", "enforce");
    assert_eq!(hs.len(), 3, "{hs:?}");
    assert_eq!(hs[0].0, "Content-Security-Policy-Report-Only");
    assert_eq!(hs[1].0, "Reporting-Endpoints");
    assert_eq!(hs[2].0, "Content-Security-Policy");
    assert!(
        hs[2].1.contains("require-trusted-types-for 'script'"),
        "{}",
        hs[2].1
    );
    assert!(!hs[2].1.contains("default-src"), "{}", hs[2].1);
}

#[test]
fn off_mode_with_tt_enforce_still_enforces_trusted_types() {
    // `csp: "off"` + `trustedTypes: "enforce"` — the TT directives are the
    // whole enforced policy (a CSP-off deployment can still want TT).
    let hs = csp_headers_for("off", "NONCE1", "enforce");
    assert_eq!(hs.len(), 1, "{hs:?}");
    assert_eq!(hs[0].0, "Content-Security-Policy");
    assert!(hs[0].1.contains("require-trusted-types-for 'script'"));
}

// ------------------------------------------------------------------ /_oxide/csp-report

/// The classic browser form (`application/csp-report`).
fn csp_report_body(directive: &str) -> Vec<u8> {
    serde_json::json!({
        "csp-report": {
            "effective-directive": directive,
            "source-file": "https://example.com/app.js",
            "line-number": 12,
            "disposition": "report",
        }
    })
    .to_string()
    .into_bytes()
}

/// The Reporting API form (`application/reports+json`).
fn reports_json_body(directive: &str) -> Vec<u8> {
    serde_json::json!([{
        "type": "csp-violation",
        "age": 0,
        "url": "https://example.com/",
        "body": {
            "effectiveDirective": directive,
            "sourceFile": "https://example.com/app.js",
            "lineNumber": 12,
            "disposition": "report",
        }
    }])
    .to_string()
    .into_bytes()
}

#[test]
fn csp_report_accepts_the_classic_form_and_returns_204() {
    let lim = report_limiter();
    let out = csp_report_response(
        &lim,
        "1.2.3.4",
        "POST",
        Some("application/csp-report"),
        &csp_report_body("script-src-elem"),
    );
    assert_eq!(out.status, 204, "{out:?}");
    assert_eq!(out.effective_directive.as_deref(), Some("script-src-elem"));
}

#[test]
fn csp_report_accepts_the_reporting_api_form_and_returns_204() {
    let lim = report_limiter();
    let out = csp_report_response(
        &lim,
        "1.2.3.4",
        "POST",
        Some("application/reports+json"),
        &reports_json_body("script-src"),
    );
    assert_eq!(out.status, 204, "{out:?}");
    assert_eq!(out.effective_directive.as_deref(), Some("script-src"));
}

#[test]
fn csp_report_rejects_an_unrecognized_content_type_with_415() {
    let lim = report_limiter();
    let out = csp_report_response(&lim, "1.2.3.4", "POST", Some("text/plain"), b"{}");
    assert_eq!(out.status, 415, "{out:?}");
}

#[test]
fn csp_report_rejects_an_oversized_body_with_413() {
    let lim = report_limiter();
    // The 64 KiB cap (spec: oversized bodies rejected) — a body of junk
    // past the cap never reaches the parser.
    let body = vec![b'a'; 64 * 1024 + 1];
    let out = csp_report_response(
        &lim,
        "1.2.3.4",
        "POST",
        Some("application/csp-report"),
        &body,
    );
    assert_eq!(out.status, 413, "{out:?}");
}

#[test]
fn csp_report_rejects_unparseable_json_with_400() {
    let lim = report_limiter();
    let out = csp_report_response(
        &lim,
        "1.2.3.4",
        "POST",
        Some("application/csp-report"),
        b"not json at all",
    );
    assert_eq!(out.status, 400, "{out:?}");
}

#[test]
fn csp_report_is_post_only() {
    let lim = report_limiter();
    let out = csp_report_response(
        &lim,
        "1.2.3.4",
        "GET",
        Some("application/csp-report"),
        &csp_report_body("script-src"),
    );
    assert_eq!(out.status, 405, "{out:?}");
}

#[test]
fn csp_report_is_rate_limited_to_20_per_minute_per_ip() {
    let lim = report_limiter();
    let body = csp_report_body("script-src");
    for i in 0..20 {
        let out = csp_report_response(
            &lim,
            "9.9.9.9",
            "POST",
            Some("application/csp-report"),
            &body,
        );
        assert_eq!(out.status, 204, "request {i} must pass, got {out:?}");
    }
    let out = csp_report_response(
        &lim,
        "9.9.9.9",
        "POST",
        Some("application/csp-report"),
        &body,
    );
    assert_eq!(out.status, 429, "the 21st request is over the limit");
    // A different IP has its own bucket.
    let out = csp_report_response(
        &lim,
        "8.8.8.8",
        "POST",
        Some("application/csp-report"),
        &body,
    );
    assert_eq!(out.status, 204, "another IP starts with a full bucket");
}

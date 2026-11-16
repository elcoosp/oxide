//! TDD pins for the SPEC-24/§20 redaction guard on the Debug surfaces.
//!
//! `log_request` redacts through `is_secret_key`, but the `Debug` impls of
//! `OxideRequest` (derived) and `OxideResponse` (manual
//! `.field("headers", …)`) serialized RAW headers — any `{:?}` of a request
//! carrying `cookie`/`authorization`, or of a response carrying
//! `set-cookie` (session tokens), leaked the value into logs, panics and
//! diagnostics. The pins force both Debug surfaces through the same
//! `is_secret_key` class the request log already honors.

use bytes::Bytes;
use oxide_fw_server::{OxideRequest, OxideResponse, is_secret_key};
use std::sync::{Arc, OnceLock};

fn request(headers: &[(&str, &str)]) -> OxideRequest {
    OxideRequest {
        method: "GET".into(),
        url: "/".into(),
        headers: Arc::new(
            headers
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect::<Vec<_>>(),
        ),
        header_index: OnceLock::default(),
        body: Bytes::new(),
        request_id: "req-redact".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

/// SPEC-24: the secret class — the four literal names plus the contains
/// classes exactly as `is_secret_key` defines them; non-secret keys are
/// NOT in the class (the class is the documented surface, widened only by
/// the spec).
#[test]
fn is_secret_key_covers_the_spec_24_class() {
    for k in [
        "cookie",
        "Cookie",
        "authorization",
        "set-cookie",
        "x-oxide-fw-dev-token",
        "db_password",
        "grafana_token",
        "openai_api_key",
        "client_secret",
    ] {
        assert!(is_secret_key(k), "{k} must be in the secret class");
    }
    for k in [
        "content-type",
        "x-oxide-build",
        "accept",
        "user-agent",
        "x-custom",
    ] {
        assert!(!is_secret_key(k), "{k} must not be in the secret class");
    }
}

/// The response Debug must redact secret-class headers and keep the others.
#[test]
fn response_debug_redacts_secret_class_headers() {
    let resp = OxideResponse::new(200, "ok")
        .with_header("content-type", "text/html")
        .with_header("set-cookie", "oxide_session=SECRETVALUE; Path=/")
        .with_header("authorization", "Bearer SECRETBEARER");
    let out = format!("{resp:?}");
    assert!(
        out.contains("[redacted]"),
        "the secret headers must redact: {out}"
    );
    assert!(!out.contains("SECRETVALUE"), "the cookie leaked: {out}");
    assert!(
        !out.contains("SECRETBEARER"),
        "the authorization leaked: {out}"
    );
    assert!(
        out.contains("text/html"),
        "the non-secret header must survive: {out}"
    );
}

/// The request Debug (previously derived) must redact the same class.
#[test]
fn request_debug_redacts_secret_class_headers() {
    let req = request(&[
        ("cookie", "oxide_session=SECRETVALUE"),
        ("x-custom", "visible"),
    ]);
    let out = format!("{req:?}");
    assert!(
        out.contains("[redacted]"),
        "the cookie header must redact: {out}"
    );
    assert!(!out.contains("SECRETVALUE"), "the cookie leaked: {out}");
    assert!(
        out.contains("visible"),
        "the non-secret header must survive: {out}"
    );
}

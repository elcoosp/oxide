//! TDD pins for the record-and-replay PLAYBACK substrate (spec v0.2
//! §V2-F15, the replay half: "it overrides `fetch` with the recorded
//! responses (matched by method, URL and body hash in order; an
//! unexpected call is an error `RPL-002` naming the call), `Date.now`,
//! `Math.random` and `crypto.randomUUID` from the recorded sequences …
//! and with `--diff` compares to the recorded response (normalizing the
//! nonce)" + "a record from an older build id warns (`RPL-003`) but
//! runs").
//!
//! The contracts pinned here:
//!
//! - [`ReplayMatcher`] plays the record's outbound fetch ops IN ORDER:
//!   the next recorded op must equal the caller's `method` + `url` +
//!   request body (the "body hash" identity — the decoded bytes compare
//!   equal), and the recorded response is handed back. Any divergence —
//!   a wrong call or an exhausted record — is the `RPL-002` error, naming
//!   both the call the replay made and the call the record expected.
//! - [`ReplaySequences`] plays the recorded non-deterministic streams in
//!   observation order; a drained sequence is `RPL-004` (the replay made
//!   more non-deterministic observations than the record captured).
//! - [`warn_stale_build`] is the `RPL-003` verdict: a record from an
//!   older build id warns but runs — the function reports the staleness
//!   and logs it; it never refuses.
//! - [`normalize_nonce`] is the `--diff` normalization: the per-request
//!   CSP nonce (`server::nonce_for` = 32 lowercase hex) differs between
//!   the recorded and the replayed response by construction, so the
//!   comparison blanks it first. Non-nonce attributes are untouched.
//! - [`replay_diff`] is the `oxide replay --diff` verdict: status, the
//!   header set (minus the per-request trio the L0 cache already treats
//!   as re-derived state: `content-security-policy`, `date`,
//!   `set-cookie`) and the nonce-normalized body, each divergence
//!   reported with kind + detail; the shape serializes camelCase for the
//!   `--json` surface.

use base64::Engine as _;
use oxide_fw_dispatch::record::{Recording, Redactor};
use oxide_fw_dispatch::replay::{
    ReplayMatcher, ReplaySequences, normalize_nonce, replay_diff, warn_stale_build,
};

/// The record every test replays: two fetch ops, three observations per
/// non-deterministic stream, one response.
fn fixture_record() -> oxide_fw_dispatch::record::RequestRecord {
    let mut rec = Recording::new(Redactor::new(false), "GET", "https://app.test/p");
    rec.record_fetch(
        "GET",
        "https://up.example/a",
        b"",
        200,
        &[("content-type".into(), "application/json".into())],
        b"{\"a\":1}",
    );
    rec.record_fetch(
        "POST",
        "https://up.example/b",
        b"payload",
        201,
        &[],
        b"created",
    );
    rec.record_now(1_700_000_000_000);
    rec.record_now(1_700_000_000_001);
    rec.record_random(0.25);
    rec.record_random(0.5);
    rec.record_random(0.75);
    rec.record_uuid("uuid-1");
    let mut r = rec.finish(
        "req-r",
        "aaaaaaaaaaaaaaaa",
        200,
        &[("content-type".into(), "text/html".into())],
        b"<html nonce=\"11111111111111111111111111111111\">ok</html>",
    );
    // The recorded response body carries a nonce-shaped value: the diff
    // must normalize it like it normalizes the replay's.
    r.response.body_base64 = base64::engine::general_purpose::STANDARD
        .encode("<html nonce=\"22222222222222222222222222222222\">ok</html>");
    r
}

/// The matcher plays the recorded ops in order and hands back the exact
/// recorded responses.
#[test]
fn the_matcher_plays_the_recorded_ops_in_order() {
    let record = fixture_record();
    let mut matcher = ReplayMatcher::from_record(&record);
    assert_eq!(matcher.remaining(), 2);

    let first = matcher
        .take("GET", "https://up.example/a", b"")
        .expect("the first call matches the first op");
    assert_eq!(first.status, 200);
    assert_eq!(first.headers[0].0, "content-type");

    let second = matcher
        .take("POST", "https://up.example/b", b"payload")
        .expect("the second call matches the second op");
    assert_eq!(second.status, 201);

    assert_eq!(matcher.remaining(), 0, "both ops are consumed");
}

/// A call that is not the next recorded op is the `RPL-002` error naming
/// the call — wrong URL, wrong method, wrong body, or an exhausted
/// record all refuse with the same code.
#[test]
fn an_unexpected_call_refuses_with_rpl_002() {
    let record = fixture_record();
    let mut matcher = ReplayMatcher::from_record(&record);

    let wrong_url = matcher
        .take("GET", "https://up.example/wrong", b"")
        .expect_err("a different URL is not the recorded call");
    assert_eq!(wrong_url.0.code, "RPL-002");
    assert!(
        wrong_url.0.message.contains("https://up.example/wrong"),
        "the error names the call the replay made"
    );
    assert!(
        wrong_url.0.message.contains("https://up.example/a"),
        "the error names the call the record expected"
    );

    // The failed take consumed nothing: the correct call still plays.
    let first = matcher
        .take("GET", "https://up.example/a", b"")
        .expect("the failed take did not consume the op");
    assert_eq!(first.status, 200);

    let wrong_method = matcher
        .take("DELETE", "https://up.example/b", b"payload")
        .expect_err("a different method is not the recorded call");
    assert_eq!(wrong_method.0.code, "RPL-002");

    let wrong_body = matcher
        .take("POST", "https://up.example/b", b"other")
        .expect_err("a different body is not the recorded call");
    assert_eq!(wrong_body.0.code, "RPL-002");

    let _ = matcher
        .take("POST", "https://up.example/b", b"payload")
        .expect("the correct call plays");
    let exhausted = matcher
        .take("GET", "https://up.example/c", b"")
        .expect_err("the record is exhausted");
    assert_eq!(exhausted.0.code, "RPL-002");
}

/// The non-deterministic sequences play back in observation order and
/// refuse `RPL-004` when the replay observes more than the record
/// captured.
#[test]
fn the_sequences_play_back_in_observation_order() {
    let record = fixture_record();
    let mut seq = ReplaySequences::from_record(&record);

    assert_eq!(seq.next_now().expect("the first now"), 1_700_000_000_000);
    assert_eq!(seq.next_now().expect("the second now"), 1_700_000_000_001);
    assert_eq!(
        seq.next_now()
            .expect_err("the now stream is drained")
            .0
            .code,
        "RPL-004"
    );

    assert_eq!(seq.next_random().expect("the first random"), 0.25);
    assert_eq!(seq.next_random().expect("the second random"), 0.5);
    assert_eq!(seq.next_random().expect("the third random"), 0.75);
    assert_eq!(
        seq.next_random()
            .expect_err("the random stream is drained")
            .0
            .code,
        "RPL-004"
    );

    assert_eq!(seq.next_uuid().expect("the uuid"), "uuid-1");
    assert_eq!(
        seq.next_uuid()
            .expect_err("the uuid stream is drained")
            .0
            .code,
        "RPL-004"
    );
}

/// A record from an older build id warns (`RPL-003`) but runs: the
/// verdict reports the staleness, the same id does not.
#[test]
fn an_older_build_id_warns_but_the_replay_runs() {
    let record = fixture_record();
    assert!(
        warn_stale_build(&record, "bbbbbbbbbbbbbbbb"),
        "a different current build is stale"
    );
    assert!(
        !warn_stale_build(&record, "aaaaaaaaaaaaaaaa"),
        "the recorded build id is not stale"
    );
}

/// The nonce normalization blanks the 32-hex per-request nonce values and
/// leaves everything else — including non-hex or wrong-length nonce
/// values — untouched.
#[test]
fn normalize_nonce_blanks_only_the_hashed_nonce_values() {
    let body = "<script nonce=\"11111111111111111111111111111111\" defer></script>\
                <script nonce=\"22222222222222222222222222222222\"></script>\
                <div nonce=\"short\">not a nonce</div>";
    let out = normalize_nonce(body);
    assert!(
        out.contains("nonce=\"<nonce>\""),
        "the nonce values are normalized: {out}"
    );
    assert!(!out.contains("11111111111111111111111111111111"));
    assert!(
        out.contains("nonce=\"short\""),
        "a non-hex attribute value is untouched"
    );
    assert!(out.contains("defer"), "the rest of the body is preserved");
}

/// The `--diff` verdict: an identical replay (different nonce) is `ok`;
/// status, header and real body changes are reported as divergences.
#[test]
fn replay_diff_reports_only_real_divergences() {
    let record = fixture_record();
    let recorded_body = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(&record.response.body_base64)
            .expect("the recorded body decodes"),
    )
    .expect("the recorded body is utf-8");

    // The identical replay: same status, same headers, the nonce differs.
    let same = replay_diff(
        &record,
        record.response.status,
        &record.response.headers,
        recorded_body
            .replace(
                "22222222222222222222222222222222",
                "33333333333333333333333333333333",
            )
            .as_str(),
    );
    assert!(same.ok, "a nonce-only body change is not a divergence");
    assert!(
        same.divergences.is_empty(),
        "no divergence entries for the identical replay"
    );

    // A status change is a divergence.
    let status = replay_diff(&record, 500, &record.response.headers, &recorded_body);
    assert!(!status.ok);
    assert_eq!(status.divergences.len(), 1);
    assert_eq!(status.divergences[0].kind, "status");

    // A header change is a divergence.
    let changed = vec![("content-type".to_string(), "text/plain".to_string())];
    let headers = replay_diff(&record, 200, &changed, &recorded_body);
    assert!(!headers.ok);
    assert!(
        headers
            .divergences
            .iter()
            .any(|d| d.kind == "header" && d.detail.contains("content-type")),
        "the header divergence names the header: {:?}",
        headers.divergences
    );

    // A real body change is a divergence.
    let body = replay_diff(
        &record,
        200,
        &record.response.headers,
        "<html>different</html>",
    );
    assert!(!body.ok);
    assert!(
        body.divergences.iter().any(|d| d.kind == "body"),
        "the body divergence is reported: {:?}",
        body.divergences
    );
}

/// The per-request header trio (`content-security-policy`, `date`,
/// `set-cookie`) is re-derived state — the L0 cache already treats CSP as
/// re-derivable — so its presence difference is not a divergence.
#[test]
fn the_per_request_header_trio_is_not_a_divergence() {
    let record = fixture_record();
    let recorded_body = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(&record.response.body_base64)
            .expect("the recorded body decodes"),
    )
    .expect("the recorded body is utf-8");
    let replayed = vec![
        ("content-type".to_string(), "text/html".to_string()),
        (
            "content-security-policy".to_string(),
            "script-src 'nonce-9999'".to_string(),
        ),
        (
            "date".to_string(),
            "Thu, 01 Jan 2026 00:00:00 GMT".to_string(),
        ),
        ("set-cookie".to_string(), "sid=replayed".to_string()),
    ];
    let diff = replay_diff(&record, 200, &replayed, &recorded_body);
    assert!(
        diff.ok,
        "only re-derived headers differ: {:?}",
        diff.divergences
    );
}

/// The diff serializes camelCase for the `--json` surface the CLI hands
/// to agents.
#[test]
fn the_diff_serializes_for_the_json_surface() {
    let record = fixture_record();
    let diff = replay_diff(&record, 500, &[], "different");
    let json = serde_json::to_value(&diff).expect("the diff serializes");
    let obj = json.as_object().expect("the diff is an object");
    assert!(obj.contains_key("ok"));
    assert!(obj.contains_key("divergences"));
    let first = obj["divergences"].as_array().unwrap()[0]
        .as_object()
        .unwrap();
    assert!(first.contains_key("kind"));
    assert!(first.contains_key("detail"));
}

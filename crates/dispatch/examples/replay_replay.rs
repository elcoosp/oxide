//! The record-and-replay playback walkthrough (§V2-F15 replay half): a
//! captured record is replayed — the recorded fetch op is matched in
//! order, the non-deterministic sequences play back, the stale-build
//! warning fires, and the `--diff` verdict compares the replayed response
//! against the record (nonce-normalized).
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dispatch --example replay_replay
//! ```

use base64::Engine as _;
use oxide_fw_dispatch::record::{Recording, Redactor};
use oxide_fw_dispatch::replay::{
    ReplayMatcher, ReplaySequences, normalize_nonce, replay_diff, warn_stale_build,
};

fn main() {
    // The record: one fetch op, one observation per non-det stream, a
    // nonce-bearing response body.
    let mut rec = Recording::new(Redactor::new(false), "GET", "https://app.test/p");
    rec.record_fetch(
        "GET",
        "https://api.example/data",
        b"",
        200,
        &[("content-type".to_string(), "application/json".to_string())],
        br#"{"ok":true}"#,
    );
    rec.record_now(1_700_000_000_000);
    rec.record_random(0.42);
    let mut record = rec.finish(
        "req-7f3a",
        "aaaaaaaaaaaaaaaa",
        200,
        &[("content-type".to_string(), "text/html".to_string())],
        b"<html nonce=\"11111111111111111111111111111111\">ok</html>",
    );
    record.response.body_base64 = base64::engine::general_purpose::STANDARD
        .encode("<html nonce=\"11111111111111111111111111111111\">ok</html>");

    // §V2-F15: "a record from an older build id warns (RPL-003) but runs".
    let stale = warn_stale_build(&record, "bbbbbbbbbbbbbbbb");
    println!("stale build (RPL-003 warn fired): {stale}");

    // The recorded fetch op plays back on the matching call; a wrong call
    // refuses with RPL-002 and consumes nothing.
    let mut matcher = ReplayMatcher::from_record(&record);
    let wrong = matcher.take("GET", "https://api.example/other", b"");
    match wrong {
        Err(e) => println!("wrong call -> RPL code {} ({})", e.0.code, e.0.message),
        Ok(_) => println!("unexpected: the wrong call played"),
    }
    let response = matcher
        .take("GET", "https://api.example/data", b"")
        .expect("the recorded call matches");
    println!(
        "recorded call played: status {}, {} body byte(s); {} op(s) remaining",
        response.status,
        response.body_base64.len(),
        matcher.remaining()
    );

    // The non-deterministic sequences play back in order.
    let mut seq = ReplaySequences::from_record(&record);
    println!(
        "sequences: now={} random={}",
        seq.next_now().expect("now"),
        seq.next_random().expect("random")
    );

    // The --diff verdict: the replay re-rendered with a NEW nonce (the
    // per-request CSP nonce differs by construction) — normalized, so the
    // verdict is ok. A real body change is a divergence.
    let replayed_body =
        normalize_nonce("<html nonce=\"99999999999999999999999999999999\">ok</html>");
    println!("normalized replay body: {replayed_body}");
    let diff = replay_diff(
        &record,
        200,
        &[("content-type".to_string(), "text/html".to_string())],
        "<html nonce=\"99999999999999999999999999999999\">ok</html>",
    );
    println!("diff vs the record: ok={}", diff.ok);
    let broken = replay_diff(&record, 500, &[], "<html>different</html>");
    for d in &broken.divergences {
        println!("divergence {}: {}", d.kind, d.detail);
    }
}

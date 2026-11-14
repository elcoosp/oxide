//! TDD pins for the record-and-replay CAPTURE substrate (spec v0.2 §V2-F15,
//! the recorder half: "store `.oxide/records/<request-id>.json` with:
//! request (method, URL, headers with `authorization`, `cookie`,
//! `set-cookie` replaced by `sha256:<12 hex>` unless `--include-secrets`
//! in dev), matched route id and params, middleware locals, each loader
//! input and ODF output, each outbound `fetch` op call and response
//! (status, headers, body base64, up to 1 MB), cache op results, the build
//! id, and the values returned by non-deterministic sources").
//!
//! The contracts pinned here:
//!
//! - [`Redactor`] redacts exactly the secret headers (`authorization`,
//!   `cookie`, `set-cookie`, case-insensitive) to `sha256:<12 hex>` and
//!   leaves every other header verbatim; `include_secrets` (the dev
//!   `--include-secrets` flag) keeps the original values. The digest is
//!   the cross-implementation SHA-256 identity the F06 patch base ids
//!   already standardized on (D30) — the same algorithm and the same
//!   truncation must be reproducible from TypeScript, so the redaction is
//!   a pure function of the header value.
//! - [`RecordPolicy`] is the sampling decision the spec words as "dev
//!   always when `record: 'errors' | 'all'`, prod sampled
//!   `record: { sample: 0.001, onError: true }`": [`RecordMode::All`]
//!   records everything, [`RecordMode::Errors`] records 5xx-and-worse,
//!   [`RecordMode::Sampled`] records a deterministic fraction (the caller
//!   rolls one uniform sample per request) plus every error when
//!   `on_error` is set. The substrate takes the decision as an API
//!   parameter; the `record:` config key and its schema entry are the
//!   CLI-adoption half (see `docs/deviations.md`).
//! - [`Recording`] accumulates one request's capture: loader calls (the
//!   input and the ODF-bearing output serialized verbatim), outbound
//!   fetch ops (response bodies base64, capped at 1 MiB with a
//!   `truncated` marker), cache ops, the non-deterministic sequences and
//!   the redacted response, then [`Recording::finish`] emits the
//!   versioned [`RequestRecord`].
//! - [`RecordingLoaderRunner`] is the wire-up seam: a [`LoaderRunner`]
//!   decorator the CLI's dispatch closure wraps its real runner in, so
//!   every load that [`run_load_graph`](oxide_fw_dispatch::loads) drives
//!   is captured without the runner or the graph knowing about recording.
//! - [`RecordSink`] is the storage seam; [`DirRecordStore`] writes
//!   `<root>/.oxide/records/<request-id>.json` atomically (temp file +
//!   rename, the F11 pointer-swap convention), refuses traversal-shaped
//!   ids with `RPL-001` and round-trips the record through serde.
//!
//! The replay half (the matcher, the non-deterministic sequence playback,
//! the build-id staleness warning and the `--diff` comparison) is the
//! second F15 milestone — see `docs/deviations.md`.

use std::fs;
use std::sync::{Arc, Mutex};

use base64::Engine as _;

use oxide_fw_dispatch::loads::{LoadNode, run_load_graph};
use oxide_fw_dispatch::record::{
    DirRecordStore, FetchOp, LoaderCall, MAX_CAPTURED_BODY, RecordMode, RecordPolicy, RecordSink,
    Recording, RecordingLoaderRunner, Redactor, RequestRecord, is_secret_header,
};
use oxide_fw_server::{LoaderOutput, LoaderRunInput, LoaderRunner};

/// The dev default: `record: 'all'` — every request is recorded.
#[test]
fn the_dev_all_policy_records_every_status() {
    let policy = RecordPolicy::dev_all();
    for status in [200u16, 302, 404, 500] {
        assert!(
            policy.should_record(status, 0.999_999),
            "mode All records status {status} regardless of the roll"
        );
    }
}

/// `record: 'errors'`: only the failed requests are captured.
#[test]
fn the_errors_policy_records_only_failures() {
    let policy = RecordPolicy {
        mode: RecordMode::Errors,
        include_secrets: false,
    };
    assert!(!policy.should_record(200, 0.0), "2xx is not an error");
    assert!(
        !policy.should_record(302, 0.0),
        "a redirect is not an error"
    );
    assert!(!policy.should_record(404, 0.0), "4xx is a client outcome");
    assert!(
        policy.should_record(500, 0.0),
        "a 5xx (the failing-loader shape) is always captured"
    );
    assert!(
        policy.should_record(503, 0.0),
        "every 5xx is captured regardless of the roll"
    );
}

/// `record: { sample: 0.001, onError: true }`: the roll decides for
/// healthy traffic, the error override decides for failures. The roll is
/// `roll < sample_rate` so `sample: 1.0` records everything and
/// `sample: 0.0` records nothing but the errors.
#[test]
fn the_sampled_policy_rolls_and_always_captures_errors() {
    let policy = RecordPolicy {
        mode: RecordMode::Sampled {
            sample_rate: 0.001,
            on_error: true,
        },
        include_secrets: false,
    };
    assert!(
        !policy.should_record(200, 0.001),
        "at the boundary: not below the rate"
    );
    assert!(
        policy.should_record(200, 0.0009),
        "below the rate: captured"
    );
    assert!(
        !policy.should_record(200, 0.5),
        "a healthy roll above the rate: skipped"
    );
    assert!(
        policy.should_record(500, 0.5),
        "on_error captures the 5xx even on a losing roll"
    );
    let never = RecordPolicy {
        mode: RecordMode::Sampled {
            sample_rate: 0.0,
            on_error: false,
        },
        include_secrets: false,
    };
    assert!(
        !never.should_record(200, 0.0),
        "rate 0 records no healthy traffic"
    );
}

/// The secret-header set and the redaction format: `sha256:` + the first
/// 12 lowercase hex chars of the SHA-256 of the value. Non-secret headers
/// pass through verbatim.
#[test]
fn secret_headers_redact_to_the_sha256_prefix() {
    let redactor = Redactor::new(false);
    assert!(is_secret_header("Authorization"));
    assert!(is_secret_header("cookie"));
    assert!(is_secret_header("SET-COOKIE"));
    assert!(!is_secret_header("x-oxide-build"));

    let value = "sess-abc123";
    let red = redactor.redact("Cookie", value);
    assert!(
        red.starts_with("sha256:"),
        "the redacted value names its algorithm: {red}"
    );
    let hex = red.strip_prefix("sha256:").expect("the sha256: prefix");
    assert_eq!(hex.len(), 12, "exactly 12 hex chars");
    assert!(
        hex.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "lowercase hex only: {hex}"
    );
    // The digest is a pure function of the value (the TS side hashes the
    // same bytes): same input, same output; different input, different out.
    assert_eq!(red, redactor.redact("cookie", value));
    assert_ne!(red, redactor.redact("Cookie", "sess-abc124"));

    // A non-secret header survives verbatim.
    assert_eq!(redactor.redact("content-type", "text/html"), "text/html");
}

/// `include_secrets` (the dev `--include-secrets` flag) keeps the original
/// values — the recorded request can be replayed with real credentials.
#[test]
fn include_secrets_keeps_the_original_values() {
    let redactor = Redactor::new(true);
    assert_eq!(redactor.redact("authorization", "Bearer tok"), "Bearer tok");
    assert_eq!(redactor.redact("Set-Cookie", "a=1"), "a=1");
}

/// Outbound fetch bodies are base64 and capped at 1 MiB; the cap raises
/// the `truncated` marker instead of dropping the op.
#[test]
fn fetch_bodies_are_base64_and_capped_at_one_mib() {
    let mut rec = Recording::new(Redactor::new(false), "GET", "/");
    let big = vec![0xAB_u8; MAX_CAPTURED_BODY + 1];
    rec.record_fetch("GET", "https://up.example/x", &big, 200, &[], &big);
    let ops: &[FetchOp] = rec.fetches();
    assert_eq!(ops.len(), 1, "the oversized op is captured, not dropped");
    assert!(ops[0].truncated, "the request body marks the cap");
    assert!(
        ops[0].response.truncated,
        "the response body marks the cap too"
    );
}

/// The full capture: loaders, fetches, cache ops, the non-det sequences
/// and the response land in the record, the record serializes round-trip
/// and the secret headers arrive redacted.
#[test]
fn a_recording_finishes_into_a_versioned_record() {
    let mut rec = Recording::new(Redactor::new(false), "GET", "https://app.test/p?a=1");
    rec.set_request_headers(&[
        ("authorization".into(), "Bearer tok".into()),
        ("content-type".into(), "text/html".into()),
    ]);
    rec.set_route("routes/p/[slug]", &[("slug".into(), "s".into())]);
    rec.set_locals(serde_json::json!({ "locale": "en" }));
    rec.record_loader(
        &LoaderRunInput::new("/p.ts", "routes/p/[slug]", "/p"),
        &LoaderOutput::ok(serde_json::json!({ "title": "t" })),
    );
    rec.record_fetch(
        "GET",
        "https://up.example/data",
        b"",
        200,
        &[("content-type".into(), "application/json".into())],
        b"{\"ok\":true}",
    );
    rec.record_cache_op("l0.get", "get:/p", Some(true));
    rec.record_now(1_700_000_000_000);
    rec.record_random(0.25);
    rec.record_uuid("0f7b0f7e-9d7a-4c1a-b1c2-3d4e5f6a7b8c");

    let record = rec.finish(
        "req-0001",
        "aaaaaaaaaaaaaaaa",
        200,
        &[("set-cookie".into(), "sid=abc".into())],
        b"<html nonce=\"abcdef0123456789\"></html>",
    );
    assert_eq!(record.version, 1, "the schema is versioned");
    assert_eq!(record.request_id, "req-0001");
    assert_eq!(record.build_id, "aaaaaaaaaaaaaaaa");
    assert_eq!(record.route_id, "routes/p/[slug]");
    assert_eq!(record.params["slug"], "s");
    assert_eq!(record.locals["locale"], "en");
    assert_eq!(record.request.method, "GET");
    assert_eq!(record.request.url, "https://app.test/p?a=1");
    // The inbound request's headers arrive redacted, non-secrets verbatim.
    assert!(
        record.request.headers[0].1.starts_with("sha256:"),
        "the authorization value is redacted: {}",
        record.request.headers[0].1
    );
    assert_eq!(record.request.headers[1].1, "text/html");

    // The loader call: input + ODF-bearing output serialized verbatim.
    let loaders: &[LoaderCall] = &record.loaders;
    assert_eq!(loaders.len(), 1);
    assert_eq!(loaders[0].module_path, "/p.ts");
    assert_eq!(loaders[0].route, "routes/p/[slug]");
    assert_eq!(loaders[0].status, 200);
    assert_eq!(loaders[0].input["route"], "routes/p/[slug]");
    assert_eq!(loaders[0].output["data"]["title"], "t");

    // The fetch op: response body base64 (the standard engine, no padding
    // surprises — the same alphabet the TS `atob` round trip reads).
    let expected = base64::engine::general_purpose::STANDARD.encode(b"{\"ok\":true}");
    assert_eq!(record.fetches.len(), 1);
    assert_eq!(record.fetches[0].response.body_base64, expected);
    assert_eq!(record.fetches[0].response.status, 200);

    assert_eq!(record.cache_ops.len(), 1);
    assert_eq!(record.cache_ops[0].hit, Some(true));
    assert_eq!(record.sequences.now_ms, [1_700_000_000_000]);
    assert_eq!(record.sequences.randoms, [0.25]);
    assert_eq!(record.sequences.uuids.len(), 1);

    // The response's secret headers arrive redacted.
    let set_cookie = &record.response.headers[0].1;
    assert!(
        set_cookie.starts_with("sha256:"),
        "the response set-cookie is redacted: {set_cookie}"
    );

    // The record is a serde round trip: the file bytes reconstruct it.
    let json = serde_json::to_string(&record).expect("the record serializes");
    let back: RequestRecord = serde_json::from_str(&json).expect("the record deserializes");
    assert_eq!(back, record);
}

/// A camelCase wire shape: the serialized keys are the TS spellings the
/// worker-side replay consumes.
#[test]
fn the_record_serializes_camel_case_keys() {
    let rec = Recording::new(Redactor::new(false), "GET", "/");
    let record = rec.finish("r", "b", 200, &[], b"");
    let json = serde_json::to_value(&record).unwrap();
    let obj = json.as_object().unwrap();
    for key in ["requestId", "buildId", "routeId", "cacheOps"] {
        assert!(obj.contains_key(key), "the wire shape carries `{key}`");
    }
    assert!(
        obj["sequences"].as_object().unwrap().contains_key("nowMs"),
        "the sequences are camelCase too"
    );
}

/// The storage sink: an atomic write under `.oxide/records/`, readable
/// back, and the traversal refusal.
#[test]
fn the_dir_store_writes_reads_and_refuses_traversal() {
    let tmp = tempfile::tempdir().expect("the temp root");
    let store = DirRecordStore::new(tmp.path());
    let rec = Recording::new(Redactor::new(false), "GET", "/");
    let record = rec.finish("req-abc-123", "b", 200, &[], b"body");
    store.write(&record).expect("the write succeeds");

    let path = tmp
        .path()
        .join(".oxide")
        .join("records")
        .join("req-abc-123.json");
    let bytes = fs::read(&path).expect("the file exists at the documented path");
    let back: RequestRecord = serde_json::from_slice(&bytes).expect("the bytes round-trip");
    assert_eq!(back.request_id, "req-abc-123");

    // `..`, separators and empty ids are refused, not written.
    let evil = Recording::new(Redactor::new(false), "GET", "/");
    let evil = evil.finish("../escape", "b", 200, &[], b"");
    let err = store.write(&evil).expect_err("the traversal id is refused");
    assert_eq!(err.0.code, "RPL-001", "the store refuses with RPL-001");
    let slash = Recording::new(Redactor::new(false), "GET", "/");
    let slash = slash.finish("a/b", "b", 200, &[], b"");
    assert!(store.write(&slash).is_err(), "a separator id is refused");
}

/// The wire-up: a [`RecordingLoaderRunner`] wrapped around the real
/// runner captures every load the §9.2 graph drives — input, output and
/// status — while the graph and the runner stay recording-agnostic.
#[tokio::test]
async fn the_recording_runner_captures_the_load_graph() {
    struct FakeRunner;
    #[async_trait::async_trait]
    impl LoaderRunner for FakeRunner {
        fn name(&self) -> &'static str {
            "fake"
        }
        async fn run(&self, input: LoaderRunInput) -> oxide_fw_server::Result<LoaderOutput> {
            Ok(LoaderOutput::ok(serde_json::json!({ "ran": input.route })))
        }
    }

    let rec = Recording::new(Redactor::new(false), "GET", "/");
    let rec = Arc::new(Mutex::new(rec));
    let runner: Arc<dyn LoaderRunner> = Arc::new(RecordingLoaderRunner::new(
        Arc::new(FakeRunner),
        Arc::clone(&rec),
    ));

    let nodes = vec![
        LoadNode {
            input: LoaderRunInput::new("/layout.ts", "routes/layout", "/"),
            needs_parent: false,
        },
        LoadNode {
            input: LoaderRunInput::new("/page.ts", "routes/page", "/"),
            needs_parent: true,
        },
    ];
    let result = run_load_graph(runner, nodes).await;
    assert!(
        matches!(result, oxide_fw_dispatch::loads::LoadResult::Data(_)),
        "the graph still completes through the decorator"
    );

    let guard = rec.lock().expect("the recorder lock");
    assert_eq!(
        guard.loaders().len(),
        2,
        "both loads are captured, in graph order"
    );
    assert_eq!(guard.loaders()[0].route, "routes/layout");
    assert_eq!(guard.loaders()[1].route, "routes/page");
    assert_eq!(
        guard.loaders()[1].input["parent_data"]["ran"],
        "routes/layout",
        "the parent data the dependent load received is in the record"
    );
}

//! TDD pins for the §V2-F15 replay-RUNNER seam — the one-definition
//! orchestration the `oxide replay <id> [--diff]` command and the MCP
//! `replay.run` tool drive once the CLI wiring lands (the V8-free half of
//! the F15 completion: the record-store read side plus the runner that
//! loads a record, warns a stale build, re-executes through an injected
//! executor and produces the `--diff` verdict).
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The contracts pinned here:
//!
//! - [`DirRecordStore`] is the record-store READ side as well as the
//!   write side: [`RecordSource::load`] round-trips a finished record,
//!   names a missing record with `RPL-001`, refuses a traversal-shaped
//!   id with `RPL-001` (the write side's guard) and refuses a record
//!   whose schema version it does not know with `RPL-005` (the record
//!   module's "the replay side refuses a record whose version it does
//!   not know" contract, now enforced at the only door a record enters
//!   through).
//! - [`ReplayRunner::run`] is the replay orchestration the CLI command
//!   wraps: load → the `RPL-003` stale-build warn-but-run verdict →
//!   re-execute through the injected [`ReplayExecutor`] → the
//!   [`replay_diff`](oxide_fw_dispatch::replay::replay_diff) verdict. A
//!   byte-identical replay is `ok`; a stale build never refuses; a
//!   divergent replay reports its divergences; the executor's and the
//!   store's refusals surface unchanged.
//! - The outcome serializes camelCase (the TS convention the `--json`
//!   and MCP surfaces speak).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use oxide_fw_dispatch::record::{
    DirRecordStore, RPL_RECORD_IO, RPL_RECORD_VERSION, RecordSink, RecordSource, Recording,
    Redactor,
};
use oxide_fw_dispatch::replay::{ExecutedResponse, ReplayExecutor, ReplayRunner};

// --------------------------------------------------------------- fixtures

/// A populated record: one fetch op, one observation per non-det stream,
/// a nonce-bearing HTML response (the shapes the diff normalizes).
fn fixture_record() -> oxide_fw_dispatch::record::RequestRecord {
    let mut rec = Recording::new(Redactor::new(false), "GET", "https://app.test/p");
    rec.record_fetch(
        "GET",
        "https://up.example/a",
        b"",
        200,
        &[("content-type".to_string(), "application/json".to_string())],
        b"{\"a\":1}",
    );
    rec.record_now(1_700_000_000_000);
    rec.record_random(0.42);
    rec.record_uuid("0d5cfbe8-6b0e-4a1c-9c1a-8f4c0a2b3d4e");
    rec.finish(
        "req-0001",
        "aaaaaaaaaaaaaaaa",
        200,
        &[("content-type".to_string(), "text/html".to_string())],
        b"<html nonce=\"11111111111111111111111111111111\">ok</html>",
    )
}

/// The recorded response, decoded back to raw bytes (what a faithful
/// re-execution produces byte-for-byte).
fn recorded_response(record: &oxide_fw_dispatch::record::RequestRecord) -> ExecutedResponse {
    ExecutedResponse {
        status: record.response.status,
        headers: record.response.headers.clone(),
        body: base64::engine::general_purpose::STANDARD
            .decode(&record.response.body_base64)
            .unwrap_or_default(),
    }
}

/// A fake re-execution host: the seam the CLI's real pipeline fills in
/// behind the same trait.
struct FakeExecutor {
    result: Result<ExecutedResponse, oxide_fw_diagnostics::OxideError>,
    calls: AtomicUsize,
}

impl FakeExecutor {
    fn faithful(record: &oxide_fw_dispatch::record::RequestRecord) -> Self {
        Self {
            result: Ok(recorded_response(record)),
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl ReplayExecutor for FakeExecutor {
    async fn execute(
        &self,
        _record: &oxide_fw_dispatch::record::RequestRecord,
    ) -> Result<ExecutedResponse, oxide_fw_diagnostics::OxideError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.result.clone()
    }
}

/// A store pre-loaded with `records` (the read seam over an in-memory
/// map; the [`DirRecordStore`] pins below exercise the filesystem impl).
struct MemoryStore(Mutex<BTreeMap<String, oxide_fw_dispatch::record::RequestRecord>>);

impl RecordSource for MemoryStore {
    fn load(
        &self,
        request_id: &str,
    ) -> Result<oxide_fw_dispatch::record::RequestRecord, oxide_fw_diagnostics::OxideError> {
        self.0
            .lock()
            .expect("memory store lock poisoned")
            .get(request_id)
            .cloned()
            .ok_or_else(|| {
                oxide_fw_diagnostics::OxideError(Box::new(
                    oxide_fw_diagnostics::Diagnostic::error(
                        RPL_RECORD_IO,
                        format!(
                            "cannot load the record {request_id}: the store has no such record"
                        ),
                    )
                    .with_fix("re-record the request, or check the record id"),
                ))
            })
    }
}

// ------------------------------------------------- the record read side

#[test]
fn load_round_trips_the_finished_record() {
    let tmp = tempfile::tempdir().unwrap();
    let store = DirRecordStore::new(tmp.path());
    let record = fixture_record();
    RecordSink::write(&store, &record).unwrap();
    let loaded = RecordSource::load(&store, "req-0001").unwrap();
    assert_eq!(loaded, record, "the read side round-trips the write side");
}

#[test]
fn load_names_a_missing_record_with_rpl_001() {
    let tmp = tempfile::tempdir().unwrap();
    let store = DirRecordStore::new(tmp.path());
    let err = RecordSource::load(&store, "req-absent").unwrap_err();
    assert_eq!(
        err.0.code, RPL_RECORD_IO,
        "the store IO code names a missing record"
    );
    assert!(
        err.0.message.contains("load"),
        "the message names the failing operation (got: {})",
        err.0.message
    );
}

#[test]
fn load_rejects_a_traversal_shaped_record_id() {
    let tmp = tempfile::tempdir().unwrap();
    let store = DirRecordStore::new(tmp.path());
    let err = RecordSource::load(&store, "../escape").unwrap_err();
    assert_eq!(
        err.0.code, RPL_RECORD_IO,
        "the write side's store-key guard is shared"
    );
}

#[test]
fn load_refuses_an_unsupported_record_version_with_rpl_005() {
    let tmp = tempfile::tempdir().unwrap();
    let store = DirRecordStore::new(tmp.path());
    let record = fixture_record();
    RecordSink::write(&store, &record).unwrap();

    // Hand-mutate the schema version past what this build reads: the
    // record door must refuse it, not hand the replay a foreign shape.
    let mut v99 = serde_json::to_value(&record).unwrap();
    v99["version"] = serde_json::json!(99);
    let dir = store.dir().join("req-0099.json");
    std::fs::write(&dir, serde_json::to_vec_pretty(&v99).unwrap()).unwrap();

    let err = RecordSource::load(&store, "req-0099").unwrap_err();
    assert_eq!(
        err.0.code, RPL_RECORD_VERSION,
        "a foreign version is RPL-005"
    );
    assert!(
        err.0.message.contains("99"),
        "the message names the version it saw (got: {})",
        err.0.message
    );
    assert!(
        err.0.message.contains('1'),
        "the message names the version this build reads (got: {})",
        err.0.message
    );
}

// ------------------------------------------------- the replay runner

#[tokio::test]
async fn runner_replays_a_byte_identical_response_ok() {
    let record = fixture_record();
    let runner = ReplayRunner::new(
        Arc::new(MemoryStore(Mutex::new(BTreeMap::from([(
            "req-0001".to_string(),
            record.clone(),
        )])))),
        "aaaaaaaaaaaaaaaa",
    );
    let executor = FakeExecutor::faithful(&record);
    let outcome = runner.run("req-0001", &executor).await.unwrap();
    assert!(outcome.diff.ok, "the faithful replay is byte-identical");
    assert!(!outcome.stale_build, "the build id matches");
    assert_eq!(
        executor.calls.load(Ordering::SeqCst),
        1,
        "executed exactly once"
    );
    assert!(
        outcome.diff.divergences.is_empty(),
        "no divergences (got: {:?})",
        outcome.diff.divergences
    );
}

#[tokio::test]
async fn runner_warns_but_runs_a_stale_build_record() {
    let record = fixture_record();
    let runner = ReplayRunner::new(
        Arc::new(MemoryStore(Mutex::new(BTreeMap::from([(
            "req-0001".to_string(),
            record.clone(),
        )])))),
        "bbbbbbbbbbbbbbbb", // the record was made on an older build
    );
    let executor = FakeExecutor::faithful(&record);
    let outcome = runner.run("req-0001", &executor).await.unwrap();
    assert!(outcome.stale_build, "the stale-build verdict is reported");
    assert_eq!(
        executor.calls.load(Ordering::SeqCst),
        1,
        "the replay still runs"
    );
    assert!(outcome.diff.ok, "a faithful replay is still ok");
}

#[tokio::test]
async fn runner_reports_a_status_divergence() {
    let record = fixture_record();
    let runner = ReplayRunner::new(
        Arc::new(MemoryStore(Mutex::new(BTreeMap::from([(
            "req-0001".to_string(),
            record.clone(),
        )])))),
        "aaaaaaaaaaaaaaaa",
    );
    let mut divergent = recorded_response(&record);
    divergent.status = 500;
    let executor = FakeExecutor {
        result: Ok(divergent),
        calls: AtomicUsize::new(0),
    };
    let outcome = runner.run("req-0001", &executor).await.unwrap();
    assert!(!outcome.diff.ok, "a divergent replay is not ok");
    assert_eq!(outcome.diff.divergences[0].kind, "status");
}

#[tokio::test]
async fn runner_surfaces_the_executor_refusal() {
    let record = fixture_record();
    let runner = ReplayRunner::new(
        Arc::new(MemoryStore(Mutex::new(BTreeMap::from([(
            "req-0001".to_string(),
            record,
        )])))),
        "aaaaaaaaaaaaaaaa",
    );
    let executor = FakeExecutor {
        result: Err(oxide_fw_diagnostics::OxideError(Box::new(
            oxide_fw_diagnostics::Diagnostic::error(
                "RPL-002",
                "replay: outbound GET https://up.example/b is not in the record",
            ),
        ))),
        calls: AtomicUsize::new(0),
    };
    let err = runner.run("req-0001", &executor).await.unwrap_err();
    assert_eq!(
        err.0.code, "RPL-002",
        "the executor's refusal surfaces unchanged"
    );
}

#[tokio::test]
async fn runner_surfaces_the_store_refusal() {
    let runner = ReplayRunner::new(Arc::new(MemoryStore(Mutex::new(BTreeMap::new()))), "aaaa");
    let record = fixture_record();
    let executor = FakeExecutor::faithful(&record);
    let err = runner.run("req-absent", &executor).await.unwrap_err();
    assert_eq!(
        err.0.code, RPL_RECORD_IO,
        "the store's refusal surfaces unchanged"
    );
}

#[test]
fn outcome_serializes_camel_case_for_the_json_surface() {
    let record = fixture_record();
    let runner = ReplayRunner::new(
        Arc::new(MemoryStore(Mutex::new(BTreeMap::from([(
            "req-0001".to_string(),
            record.clone(),
        )])))),
        "aaaaaaaaaaaaaaaa",
    );
    let executor = FakeExecutor::faithful(&record);
    // The runner is async; the serialization contract is not — build the
    // outcome through the same public path a blocking caller would see.
    let outcome = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(runner.run("req-0001", &executor))
        .unwrap();
    let v = serde_json::to_value(&outcome).unwrap();
    assert!(
        v.get("staleBuild").is_some(),
        "camelCase: staleBuild (got: {v})"
    );
    assert!(
        v.get("diff").is_some(),
        "the diff rides the outcome (got: {v})"
    );
    assert!(
        v.get("response")
            .and_then(|r| r.get("bodyBase64"))
            .is_some(),
        "the executed response body is base64 (the record's wire convention)"
    );
}

//! The record-and-replay WIRE walkthrough (§V2-F15): the exact loop the
//! CLI's recorder wiring performs per request, V8-free — the capture
//! accumulator behind the shared handle (the shape the dispatch closure
//! and the `RecordingLoaderRunner` decorator hold), the worker drain's
//! reply folded in (the worker's `replay:drain` payload, in its own wire
//! shape), the finish under the policy gate, the store write, and the
//! replay through the [`ReplayRunner`] one definition — matching,
//! stale-build (`RPL-003`) and divergent endings.
//!
//! The CLI's real wiring differs only in the transport: its executor is
//! one loopback HTTP request against a dev-mode stack, and its drain
//! broadcasts the op over the worker pool. Everything here — the
//! accumulator, the drain shapes, the policy, the diff — is the same code
//! that runs there.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dispatch --example replay_wire
//! ```

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use oxide_fw_dispatch::record::{
    CacheOp, DirRecordStore, FetchOp, NonDetSequences, RecordMode, RecordPolicy, RecordSink,
    RecordSource, Recording, RecordingLoaderRunner, Redactor, sample_roll_for,
};
use oxide_fw_dispatch::replay::{ExecutedResponse, ReplayExecutor, ReplayRunner};
use oxide_fw_server::{LoaderOutput, LoaderRunInput, LoaderRunner};

/// The shared fake the decorator runs the load through (the CLI wires the
/// real worker-backed runner; the walkthrough proves the decorator feeds
/// the record without the runner knowing).
struct FakeRunner;

#[async_trait::async_trait]
impl LoaderRunner for FakeRunner {
    fn name(&self) -> &str {
        "fake"
    }
    async fn run(&self, input: LoaderRunInput) -> oxide_fw_server::Result<LoaderOutput> {
        Ok(LoaderOutput::ok(
            serde_json::json!({ "route": input.route }),
        ))
    }
}

/// The faithful re-execution host: hands back exactly the recorded
/// response (what a byte-identical replay produces).
struct FaithfulExecutor(ExecutedResponse);

#[async_trait::async_trait]
impl ReplayExecutor for FaithfulExecutor {
    async fn execute(
        &self,
        _record: &oxide_fw_dispatch::record::RequestRecord,
    ) -> Result<ExecutedResponse, oxide_fw_diagnostics::OxideError> {
        Ok(self.0.clone())
    }
}

/// The divergent host: the loader fetched an op the record never saw —
/// the response body carries the RPL-002 message, the diff reports it.
struct DivergentExecutor;

#[async_trait::async_trait]
impl ReplayExecutor for DivergentExecutor {
    async fn execute(
        &self,
        _record: &oxide_fw_dispatch::record::RequestRecord,
    ) -> Result<ExecutedResponse, oxide_fw_diagnostics::OxideError> {
        Ok(ExecutedResponse {
            status: 200,
            headers: vec![("content-type".to_string(), "text/html".to_string())],
            body: b"<html>replay: outbound GET https://api.example/other is not \
                    the recorded call (RPL-002)</html>"
                .to_vec(),
        })
    }
}

fn main() {
    let tmp = tempfile::tempdir().expect("the walkthrough root");
    let request_id = "req-9f21";

    // ---- 1. The capture, behind the shared handle -----------------------
    // The wiring constructs the accumulator at the top of the dispatch
    // closure and shares it with the loader decorator and the response
    // tee; the finish runs through the MutexGuard (`finish_shared`).
    let redactor = Redactor::new(false);
    let shared = Arc::new(Mutex::new(Recording::new(redactor, "GET", "/ox-replay")));
    {
        let mut rec = shared.lock().expect("the accumulator lock");
        rec.set_request_headers(&[
            ("accept".to_string(), "text/html".to_string()),
            ("cookie".to_string(), "sid=secret-value".to_string()),
        ]);
        rec.set_route("routes/ox-replay", &[("slug".to_string(), "a".to_string())]);
        rec.set_locals(serde_json::json!({ "session": { "user": "a" } }));
        // The loads run through the decorator — one captured call.
        let runner: Arc<dyn LoaderRunner> = Arc::new(RecordingLoaderRunner::new(
            Arc::new(FakeRunner),
            Arc::clone(&shared),
        ));
        let mut input =
            LoaderRunInput::new("/abs/ox-replay.server.ts", "routes/ox-replay", "/ox-replay");
        input
            .headers
            .push(("cookie".to_string(), "sid=secret-value".to_string()));
        let _ = futures::executor::block_on(runner.run(input)).expect("the fake load runs");
    }

    // ---- 2. The worker drain folds in ------------------------------------
    // The CLI broadcasts `replay:drain` over the pool and parses each
    // reply — the worker's camelCase wire shape — straight into the
    // substrate's capture shapes.
    let drained_json = serde_json::json!({
        "fetches": [{
            "method": "GET",
            "url": "https://api.example/data",
            "requestBodyBase64": "",
            "truncated": false,
            "response": {
                "status": 200,
                "headers": [["content-type", "application/json"]],
                "bodyBase64": base64::engine::general_purpose::STANDARD.encode(br#"{"ok":true}"#),
                "truncated": false,
            },
        }],
        "cacheOps": [{ "op": "l1.get", "key": "l1:routes/ox-replay", "hit": true }],
        "sequences": {
            "nowMs": [1_700_000_000_123u64],
            "randoms": [0.42],
            "uuids": ["00000000-0000-4000-8000-000000000009"],
        },
    });
    let fetches: Vec<FetchOp> =
        serde_json::from_value(drained_json["fetches"].clone()).expect("the drain's fetches parse");
    let cache_ops: Vec<CacheOp> = serde_json::from_value(drained_json["cacheOps"].clone())
        .expect("the drain's cache ops parse");
    let sequences: NonDetSequences = serde_json::from_value(drained_json["sequences"].clone())
        .expect("the drain's sequences parse");

    // ---- 3. The finish: drain folded, policy applied, record written -----
    let policy = RecordPolicy {
        mode: RecordMode::All,
        include_secrets: false,
    };
    let roll = sample_roll_for(request_id);
    let record = {
        let mut rec = shared.lock().expect("the accumulator lock");
        for f in &fetches {
            rec.record_fetch(
                &f.method,
                &f.url,
                &base64::engine::general_purpose::STANDARD
                    .decode(&f.request_body_base64)
                    .unwrap_or_default(),
                f.response.status,
                &f.response.headers,
                &base64::engine::general_purpose::STANDARD
                    .decode(&f.response.body_base64)
                    .unwrap_or_default(),
            );
        }
        for c in &cache_ops {
            rec.record_cache_op(&c.op, &c.key, c.hit);
        }
        for ms in &sequences.now_ms {
            rec.record_now(*ms);
        }
        for r in &sequences.randoms {
            rec.record_random(*r);
        }
        for u in &sequences.uuids {
            rec.record_uuid(u);
        }
        let record = rec.finish_shared(
            request_id,
            "aaaaaaaaaaaaaaaa",
            200,
            &[("content-type".to_string(), "text/html".to_string())],
            b"<html nonce=\"11111111111111111111111111111111\">ok</html>",
        );
        // The decision needs the status, which only exists now. `All`
        // records; an `errors` policy on a 200 would drop it here.
        assert!(policy.should_record(record.response.status, roll));
        record
    };
    let store = DirRecordStore::new(tmp.path());
    RecordSink::write(&store, &record).expect("the record writes");
    println!(
        "record {} written to {}",
        record.request_id,
        store.dir().display()
    );
    println!(
        "  loaders: {}  fetches: {}  cache ops: {}  sequences: {} now / {} random / {} uuid",
        record.loaders.len(),
        record.fetches.len(),
        record.cache_ops.len(),
        record.sequences.now_ms.len(),
        record.sequences.randoms.len(),
        record.sequences.uuids.len(),
    );
    let cookie = record
        .request
        .headers
        .iter()
        .find(|(k, _)| k == "cookie")
        .expect("the cookie header is in the record");
    println!("  redaction: cookie → {}", cookie.1);
    assert!(
        cookie.1.starts_with("sha256:"),
        "the secret trio is redacted"
    );

    // ---- 4. The replay: one definition, three endings ---------------------
    let recorded_response = ExecutedResponse {
        status: record.response.status,
        headers: record.response.headers.clone(),
        body: base64::engine::general_purpose::STANDARD
            .decode(&record.response.body_base64)
            .expect("the recorded body decodes"),
    };

    // (a) The faithful replay: byte-identical, not stale.
    let runner = ReplayRunner::new(
        Arc::new(DirRecordStore::new(tmp.path())),
        "aaaaaaaaaaaaaaaa",
    );
    let outcome = futures::executor::block_on(
        runner.run(request_id, &FaithfulExecutor(recorded_response.clone())),
    )
    .expect("the faithful replay runs");
    println!(
        "replay (same build): ok={} stale_build={}",
        outcome.diff.ok, outcome.stale_build
    );
    assert!(outcome.diff.ok);
    assert!(!outcome.stale_build);

    // (b) The stale build: warns RPL-003 but RUNS — the diff still answers.
    let runner = ReplayRunner::new(
        Arc::new(DirRecordStore::new(tmp.path())),
        "bbbbbbbbbbbbbbbb",
    );
    let outcome = futures::executor::block_on(
        runner.run(request_id, &FaithfulExecutor(recorded_response.clone())),
    )
    .expect("the stale replay runs");
    println!(
        "replay (older build): ok={} stale_build={}",
        outcome.diff.ok, outcome.stale_build
    );
    assert!(outcome.stale_build, "RPL-003 fires as the verdict");
    assert!(outcome.diff.ok, "the warn never refuses the run");

    // (c) The divergent replay: an unrecorded fetch changes the body.
    let runner = ReplayRunner::new(
        Arc::new(DirRecordStore::new(tmp.path())),
        "aaaaaaaaaaaaaaaa",
    );
    let outcome = futures::executor::block_on(runner.run(request_id, &DivergentExecutor))
        .expect("the divergent replay runs");
    println!(
        "replay (divergent): ok={} divergences={}",
        outcome.diff.ok,
        outcome.diff.divergences.len()
    );
    assert!(!outcome.diff.ok, "the divergence is reported");
    assert!(
        outcome.diff.divergences.iter().any(|d| d.kind == "body"),
        "the body divergence names the first differing byte"
    );

    // The load door: a missing id is RPL-001, a foreign version RPL-005.
    let err = RecordSource::load(&store, "req-missing").expect_err("missing record refuses");
    println!("missing record → {}", err.0.code);
    assert_eq!(err.0.code, "RPL-001");

    println!("replay_wire walkthrough complete");
}

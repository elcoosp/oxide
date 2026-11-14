//! The replay-runner walkthrough (§V2-F15, the one-definition replay
//! orchestration): a record is captured to disk, then replayed through
//! the [`ReplayRunner`] — the store load (the `RPL-001`/`RPL-005` doors),
//! the `RPL-003` stale-build warn-but-run verdict, the re-execution
//! through the injected [`ReplayExecutor`] (here a faithful host; the
//! CLI's real implementation drives the V8 pipeline) and the `--diff`
//! verdict, printed as the camelCase JSON the `--json` and MCP surfaces
//! speak.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dispatch --example replay_runner
//! ```

use std::sync::Arc;

use base64::Engine as _;
use oxide_fw_dispatch::record::{DirRecordStore, RecordSink, Recording, Redactor};
use oxide_fw_dispatch::replay::{ExecutedResponse, ReplayExecutor, ReplayRunner};

/// The faithful re-execution host: hands back exactly the recorded
/// response (what a byte-identical replay produces). The CLI's real
/// executor wraps this trait over the actual pipeline instead.
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

fn main() {
    // The record: one fetch op, one observation per non-det stream, a
    // nonce-bearing HTML response, persisted under .oxide/records/.
    let tmp = tempfile::tempdir().expect("the walkthrough root");
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
    let record = rec.finish(
        "req-7f3a",
        "aaaaaaaaaaaaaaaa",
        200,
        &[("content-type".to_string(), "text/html".to_string())],
        b"<html nonce=\"11111111111111111111111111111111\">ok</html>",
    );
    let store = DirRecordStore::new(tmp.path());
    RecordSink::write(&store, &record).expect("the walkthrough record writes");

    // §V2-F15: the runner is the one definition the `oxide replay <id>`
    // command wraps. Here the process "runs" build bbbb…, so the record
    // (built on aaaa…) is stale — RPL-003 warns and the replay proceeds.
    let runner = ReplayRunner::new(
        Arc::new(DirRecordStore::new(tmp.path())),
        "bbbbbbbbbbbbbbbb",
    );
    let body = base64::engine::general_purpose::STANDARD
        .decode(&record.response.body_base64)
        .expect("the recorded body decodes");
    let executor = FaithfulExecutor(ExecutedResponse {
        status: record.response.status,
        headers: record.response.headers.clone(),
        body,
    });

    match tokio::runtime::Runtime::new()
        .expect("the walkthrough runtime")
        .block_on(runner.run("req-7f3a", &executor))
    {
        Ok(outcome) => {
            println!("stale build (RPL-003 warn fired): {}", outcome.stale_build);
            println!(
                "replayed status {} with {} divergence(s)",
                outcome.response.status,
                outcome.diff.divergences.len()
            );
            let json = serde_json::to_string_pretty(&outcome.diff).expect("the diff serializes");
            println!("--json surface:\n{json}");
        }
        Err(e) => println!("replay refused: {} {}", e.0.code, e.0.message),
    }
}

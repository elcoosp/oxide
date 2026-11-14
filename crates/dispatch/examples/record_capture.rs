//! The record-and-replay capture walkthrough (§V2-F15 recorder half): a
//! request is captured — redacted headers, loader calls through the
//! decorated runner, an outbound fetch op, cache ops and the
//! non-deterministic sequences — and stored under
//! `.oxide/records/<request-id>.json` in a temp project root.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dispatch --example record_capture
//! ```

use std::sync::{Arc, Mutex};

use oxide_fw_dispatch::loads::{LoadNode, run_load_graph};
use oxide_fw_dispatch::record::{
    DirRecordStore, RecordPolicy, RecordSink, Recording, RecordingLoaderRunner, Redactor,
};
use oxide_fw_server::{LoaderOutput, LoaderRunInput, LoaderRunner};

/// The app's loader host: the real runner the CLI wires (here a stand-in
/// that answers every load with data).
struct AppRunner;
#[async_trait::async_trait]
impl LoaderRunner for AppRunner {
    fn name(&self) -> &'static str {
        "app"
    }
    async fn run(&self, input: LoaderRunInput) -> oxide_fw_server::Result<LoaderOutput> {
        Ok(LoaderOutput::ok(serde_json::json!({ "ran": input.route })))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    futures::executor::block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    println!("records root: {}", root.path().display());

    // The §V2-F15 policy: dev records everything (prod samples; the
    // `record:` config key is the deferred CLI half).
    let policy = RecordPolicy::dev_all();
    let status = 200;
    println!(
        "policy {:?} records status {status}: {}",
        policy.mode,
        policy.should_record(status, 0.999)
    );

    // The capture scope: redacted request line-set, route verdict, locals.
    let mut rec = Recording::new(Redactor::new(false), "GET", "https://app.test/posts/42");
    rec.set_request_headers(&[
        ("authorization".to_string(), "Bearer sess-token".to_string()),
        ("accept".to_string(), "text/html".to_string()),
    ]);
    rec.set_route("routes/posts/[id]", &[("id".to_string(), "42".to_string())]);
    rec.set_locals(serde_json::json!({ "locale": "en" }));

    // The dispatch closure wraps its runner; the §9.2 load graph drives
    // both loads through the decorator and they land in the record.
    let rec = Arc::new(Mutex::new(rec));
    let runner: Arc<dyn LoaderRunner> = Arc::new(RecordingLoaderRunner::new(
        Arc::new(AppRunner),
        Arc::clone(&rec),
    ));
    let loads = run_load_graph(
        runner,
        vec![
            LoadNode {
                input: LoaderRunInput::new("/layout.ts", "routes/layout", "/posts/42"),
                needs_parent: false,
            },
            LoadNode {
                input: LoaderRunInput::new("/post.ts", "routes/posts/[id]", "/posts/42"),
                needs_parent: true,
            },
        ],
    )
    .await;
    println!("load graph: {loads:?}");

    // The request-scoped capture sites: one outbound fetch, one cache op,
    // the non-deterministic values the worker observed.
    {
        let mut guard = rec.lock().expect("recorder lock");
        guard.record_fetch(
            "GET",
            "https://api.example/posts/42",
            b"",
            200,
            &[("content-type".to_string(), "application/json".to_string())],
            br#"{"title":"Hello"}"#,
        );
        guard.record_cache_op("l0.get", "get:/posts/42", Some(false));
        guard.record_now(1_700_000_000_000);
        guard.record_random(0.42);
        guard.record_uuid("0f7b0f7e-9d7a-4c1a-b1c2-3d4e5f6a7b8c");
    }

    // Finish with the pipeline's response and persist it atomically.
    let record = {
        let guard = rec.lock().expect("recorder lock");
        guard.clone().finish(
            "req-7f3a",
            "aaaaaaaaaaaaaaaa",
            200,
            &[("set-cookie".to_string(), "sid=abc".to_string())],
            b"<html nonce=\"abcdef0123456789\">Hello</html>",
        )
    };
    let store = DirRecordStore::new(root.path());
    store.write(&record)?;

    let path = root
        .path()
        .join(".oxide")
        .join("records")
        .join("req-7f3a.json");
    let bytes = std::fs::read(&path)?;
    let back: oxide_fw_dispatch::record::RequestRecord = serde_json::from_slice(&bytes)?;
    println!(
        "record {}: {} loader call(s), {} fetch op(s), {} cache op(s), sequences ({} now / {} random / {} uuid)",
        back.request_id,
        back.loaders.len(),
        back.fetches.len(),
        back.cache_ops.len(),
        back.sequences.now_ms.len(),
        back.sequences.randoms.len(),
        back.sequences.uuids.len(),
    );
    println!(
        "the authorization header is stored as `{}`; the set-cookie as `{}`",
        back.request.headers[0].1, back.response.headers[0].1
    );
    Ok(())
}

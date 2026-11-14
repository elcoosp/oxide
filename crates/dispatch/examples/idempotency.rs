//! The idempotency-stage walkthrough (§V2-F14, the offline-first server
//! substrate): an offline submit is captured by the client outbox with
//! an `x-oxide-idem` key; when connectivity returns, the outbox's
//! `flush` replays it — and the [`stage_idempotency`] stage guarantees
//! the retry NEVER re-runs the handler: the first execution is stored
//! (2xx/4xx, the headers minus `set-cookie`, the 24 h TTL), the replay
//! is answered from the store with the same bytes, a 5xx stays unpinned
//! so the next flush really retries, and a store failure degrades
//! loud-then-run (`DSP-116`).
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dispatch --example idempotency
//! ```

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use oxide_fw_diagnostics::OxideError;
use oxide_fw_dispatch::idem::{
    ExecutedResponse, IdempotencyStore, IdempotencyVerdict, StoredResponse, stage_idempotency,
};

/// The in-memory store (the L1 fake; the production impl rides the
/// cache crate's L1 tier behind the same seam).
#[derive(Default)]
struct MemoryStore {
    entries: Mutex<std::collections::BTreeMap<String, StoredResponse>>,
}

impl IdempotencyStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<StoredResponse>, OxideError> {
        Ok(self
            .entries
            .lock()
            .expect("the walkthrough store lock")
            .get(key)
            .cloned())
    }

    fn put(&self, key: &str, response: &StoredResponse, _ttl_secs: u64) -> Result<(), OxideError> {
        self.entries
            .lock()
            .expect("the walkthrough store lock")
            .insert(key.to_string(), response.clone());
        Ok(())
    }
}

/// The signup handler the submit drives (the count is the story: the
/// retry must not re-run it).
struct SignupHandler {
    runs: AtomicUsize,
}

impl SignupHandler {
    async fn run(&self) -> Result<ExecutedResponse, OxideError> {
        let run = self.runs.fetch_add(1, Ordering::SeqCst) + 1;
        println!("    handler run #{run} (the insert)");
        Ok(ExecutedResponse {
            status: 201,
            headers: vec![
                ("content-type".to_string(), "application/json".to_string()),
                (
                    "set-cookie".to_string(),
                    "sid=new-session; Secure".to_string(),
                ),
            ],
            body: br#"{"created":true}"#.to_vec(),
        })
    }
}

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("the walkthrough runtime");
    runtime.block_on(async {
        let store = MemoryStore::default();
        let handler = Arc::new(SignupHandler {
            runs: AtomicUsize::new(0),
        });

        // The first submit: the offline outbox captured it with
        // `x-oxide-idem: submit-1`; connectivity is back, the flush
        // replays it.
        println!("=== first submit (the outbox flush replays the queued action)");
        let first = stage_idempotency(&store, Some("submit-1"), || handler.run())
            .await
            .expect("the first submit runs");
        let IdempotencyVerdict::Executed(response) = first else {
            panic!("the first submit executes");
        };
        println!(
            "    status {} (the Set-Cookie rides: the browser needs the session)",
            response.status
        );

        // The replay: the outbox retries the SAME item (the ack was
        // lost). The stage answers from the store — the handler never
        // runs, so the signup inserts exactly once.
        println!("=== the retry (same idempotency key, the ack was lost)");
        let second = stage_idempotency(&store, Some("submit-1"), || handler.run())
            .await
            .expect("the replay is answered");
        let IdempotencyVerdict::Replayed(replayed) = second else {
            panic!("the retry replays from the store");
        };
        println!(
            "    status {} (same bytes: {}), set-cookie stripped: {}",
            replayed.status,
            String::from_utf8_lossy(&replayed.body),
            !replayed
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("set-cookie")),
        );
        println!(
            "    handler ran {} time(s) — the insert happened once",
            handler.runs.load(Ordering::SeqCst)
        );

        // The 5xx rule: a failing submit is never pinned, so the next
        // flush really retries.
        println!("=== a 5xx submit (the server never pins the failure)");
        struct FlakyHandler {
            runs: AtomicUsize,
        }
        impl FlakyHandler {
            async fn run(&self) -> Result<ExecutedResponse, OxideError> {
                let run = self.runs.fetch_add(1, Ordering::SeqCst) + 1;
                Ok(ExecutedResponse {
                    status: 503,
                    headers: Vec::new(),
                    body: format!("unavailable (run {run})").into_bytes(),
                })
            }
        }
        let flaky = Arc::new(FlakyHandler {
            runs: AtomicUsize::new(0),
        });
        for _ in 0..2 {
            stage_idempotency(&store, Some("flaky"), || flaky.run())
                .await
                .expect("the flaky submits run");
        }
        println!(
            "    the handler ran {} time(s) — the 5xx was not stored, the retry really ran",
            flaky.runs.load(Ordering::SeqCst)
        );
        println!(
            "=== the store holds {} idempotency entries (24 h TTL each)",
            store
                .entries
                .lock()
                .expect("the walkthrough store lock")
                .len()
        );
    });
}

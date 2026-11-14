//! TDD pins for the §V2-F14 offline-first SERVER half — the
//! `stage_idempotency` data-plane stage the offline outbox's
//! `x-oxide-idem` replay key drives (the PWA-worker precache half and
//! the CLI emitter are the V8-linked halves, deferred with the A01
//! phase-3 wiring; the client outbox is `packages/core/src/client/
//! outbox.ts`).
//!
//! Written BEFORE the implementation; every pin imports public API only.
//!
//! The contracts pinned here (the spec's verbatim shapes):
//!
//! - A replay of the same key returns IDENTICAL bytes and runs the
//!   handler exactly once (the stage's point: an offline submit that
//!   retries on reconnect inserts server-side once).
//! - Different keys run separately.
//! - The stored shape is `(status, headers minus set-cookie, body)` —
//!   the replayed response never replays a Set-Cookie; the FIRST
//!   execution's own response keeps it (the browser needs the session).
//! - Only 2xx and 4xx outcomes are stored — a 5xx is never cached, so
//!   the next flush retries (the outbox's keep-5xx rule has a server
//!   side: the server never pinned the failure).
//! - A missing `x-oxide-idem` header means the stage is a pass-through:
//!   the handler runs, the store is untouched.
//! - A store failure degrades loud-then-run (the house convention: the
//!   offline convenience must never take online traffic down; the
//!   refusal is a `tracing::warn!` with the `DSP-116` code, the handler
//!   runs).
//! - The L1 key is the spec's `idem:<key>` spelling; the store's `put`
//!   carries the 24 h TTL the spec pins.
//! - The stored shape serializes with a base64 body (the record
//!   module's wire convention — the same bytes cross the L1 boundary).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use oxide_fw_diagnostics::OxideError;
use oxide_fw_dispatch::idem::{
    ExecutedResponse, IDEM_STORE, IDEM_TTL_SECS, IdempotencyStore, IdempotencyVerdict,
    StoredResponse, idem_key, stage_idempotency,
};

// --------------------------------------------------------------- fixtures

/// A handler that ran (the count the once-pin drives).
fn executed(status: u16, headers: &[(&str, &str)], body: &[u8]) -> ExecutedResponse {
    ExecutedResponse {
        status,
        headers: headers
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
        body: body.to_vec(),
    }
}

/// The in-memory store (the L1 fake; the production impl rides the
/// cache crate's L1 tier behind this same seam).
#[derive(Default)]
struct MemoryStore {
    entries: Mutex<std::collections::BTreeMap<String, (StoredResponse, u64)>>,
}

impl IdempotencyStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<StoredResponse>, OxideError> {
        Ok(self
            .entries
            .lock()
            .expect("the fake store lock")
            .get(key)
            .map(|(r, _)| r.clone()))
    }

    fn put(&self, key: &str, response: &StoredResponse, ttl_secs: u64) -> Result<(), OxideError> {
        self.entries
            .lock()
            .expect("the fake store lock")
            .insert(key.to_string(), (response.clone(), ttl_secs));
        Ok(())
    }
}

/// The store that always fails (the degrade pin drives it).
struct BrokenStore;

impl IdempotencyStore for BrokenStore {
    fn get(&self, _key: &str) -> Result<Option<StoredResponse>, OxideError> {
        Err(OxideError::new(IDEM_STORE, "the store is unreachable"))
    }

    fn put(
        &self,
        _key: &str,
        _response: &StoredResponse,
        _ttl_secs: u64,
    ) -> Result<(), OxideError> {
        Err(OxideError::new(IDEM_STORE, "the store is unreachable"))
    }
}

/// The recording handler: counts its runs and answers `response`.
struct CountingHandler {
    runs: AtomicUsize,
    response: ExecutedResponse,
}

impl CountingHandler {
    fn new(response: ExecutedResponse) -> Arc<Self> {
        Arc::new(Self {
            runs: AtomicUsize::new(0),
            response,
        })
    }

    async fn run(&self) -> Result<ExecutedResponse, OxideError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(self.response.clone())
    }
}

// ---------------------------------------------------------------- pins

/// The spec's named pin: a replay of the same key returns identical
/// bytes and the handler runs once.
#[tokio::test]
async fn replay_of_the_same_key_returns_identical_bytes_and_runs_once() {
    let store = MemoryStore::default();
    let handler = CountingHandler::new(executed(
        200,
        &[("content-type", "application/json")],
        br#"{"inserted":1}"#,
    ));
    let first = stage_idempotency(&store, Some("submit-1"), || handler.run())
        .await
        .expect("the first submit runs");
    let second = stage_idempotency(&store, Some("submit-1"), || handler.run())
        .await
        .expect("the replay is answered");
    assert!(
        matches!(first, IdempotencyVerdict::Executed(_)),
        "the first submit executes"
    );
    let IdempotencyVerdict::Replayed(replayed) = second else {
        panic!("the second submit replays");
    };
    assert_eq!(replayed.status, 200);
    assert_eq!(replayed.body, b"{\"inserted\":1}".to_vec());
    assert_eq!(
        handler.runs.load(Ordering::SeqCst),
        1,
        "the handler ran exactly once"
    );
}

/// Different keys run separately (two inserts, not one).
#[tokio::test]
async fn different_keys_run_separately() {
    let store = MemoryStore::default();
    let handler = CountingHandler::new(executed(200, &[], b"ok"));
    stage_idempotency(&store, Some("a"), || handler.run())
        .await
        .expect("key a runs");
    stage_idempotency(&store, Some("b"), || handler.run())
        .await
        .expect("key b runs");
    assert_eq!(handler.runs.load(Ordering::SeqCst), 2, "each key ran");
}

/// Only 2xx and 4xx are stored: a 5xx is never pinned, so the next
/// request with the same key runs the handler again (the outbox's
/// keep-5xx rule has a server side).
#[tokio::test]
async fn five_hundreds_are_never_stored() {
    let store = MemoryStore::default();
    let handler = CountingHandler::new(executed(503, &[], b"unavailable"));
    stage_idempotency(&store, Some("flaky"), || handler.run())
        .await
        .expect("the 5xx runs");
    stage_idempotency(&store, Some("flaky"), || handler.run())
        .await
        .expect("the retry runs");
    assert_eq!(
        handler.runs.load(Ordering::SeqCst),
        2,
        "the 5xx was not cached; the retry ran"
    );
}

/// A 4xx IS stored (the outbox deletes it as permanently rejected — the
/// server must answer the replay the same way, not re-run).
#[tokio::test]
async fn four_hundreds_are_stored() {
    let store = MemoryStore::default();
    let handler = CountingHandler::new(executed(422, &[], b"invalid"));
    stage_idempotency(&store, Some("bad"), || handler.run())
        .await
        .expect("the 4xx runs");
    let replay = stage_idempotency(&store, Some("bad"), || handler.run())
        .await
        .expect("the replay answers");
    assert!(
        matches!(replay, IdempotencyVerdict::Replayed(ref r) if r.status == 422),
        "the 4xx replay is pinned: {replay:?}"
    );
}

/// The stored shape strips `set-cookie` — the replayed response never
/// replays a session — while the FIRST execution keeps it.
#[tokio::test]
async fn the_stored_response_strips_set_cookie() {
    let store = MemoryStore::default();
    let handler = CountingHandler::new(executed(
        200,
        &[
            ("set-cookie", "sid=abc; Secure"),
            ("content-type", "text/html"),
        ],
        b"<html/>",
    ));
    let first = stage_idempotency(&store, Some("login"), || handler.run())
        .await
        .expect("the login runs");
    let IdempotencyVerdict::Executed(response) = first else {
        panic!("the first login executes");
    };
    assert!(
        response
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("set-cookie")),
        "the first execution keeps its Set-Cookie"
    );
    let second = stage_idempotency(&store, Some("login"), || handler.run())
        .await
        .expect("the replay answers");
    let IdempotencyVerdict::Replayed(replayed) = second else {
        panic!("the replay answers from the store");
    };
    assert!(
        !replayed
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("set-cookie")),
        "the replay never carries Set-Cookie: {:?}",
        replayed.headers
    );
    assert!(
        replayed
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("content-type")),
        "the other headers ride the replay"
    );
}

/// A missing `x-oxide-idem` header is a pass-through: the handler runs,
/// the store stays untouched.
#[tokio::test]
async fn a_missing_header_runs_the_handler() {
    let store = MemoryStore::default();
    let handler = CountingHandler::new(executed(200, &[], b"ok"));
    let verdict = stage_idempotency(&store, None, || handler.run())
        .await
        .expect("the pass-through runs");
    assert!(
        matches!(verdict, IdempotencyVerdict::Executed(_)),
        "no replay without the header"
    );
    assert_eq!(handler.runs.load(Ordering::SeqCst), 1);
    assert!(
        store.entries.lock().unwrap().is_empty(),
        "the pass-through stores nothing"
    );
}

/// A store failure degrades loud-then-run: the handler runs, the
/// request succeeds (the offline convenience never takes online traffic
/// down; the warn carries the `DSP-116` code).
#[tokio::test]
async fn a_store_failure_degrades_and_runs_the_handler() {
    let store = BrokenStore;
    let handler = CountingHandler::new(executed(200, &[], b"ok"));
    let verdict = stage_idempotency(&store, Some("degraded"), || handler.run())
        .await
        .expect("the degraded stage still serves");
    assert!(
        matches!(verdict, IdempotencyVerdict::Executed(_)),
        "the handler ran"
    );
    assert_eq!(handler.runs.load(Ordering::SeqCst), 1);
}

/// The L1 key is the spec's `idem:<key>` spelling.
#[test]
fn the_l1_key_is_the_idem_prefix() {
    assert_eq!(idem_key("submit-1"), "idem:submit-1");
}

/// The put carries the 24 h TTL the spec pins.
#[tokio::test]
async fn the_put_carries_the_day_ttl() {
    let store = MemoryStore::default();
    let handler = CountingHandler::new(executed(200, &[], b"ok"));
    stage_idempotency(&store, Some("ttl"), || handler.run())
        .await
        .expect("the stage runs");
    let (_, ttl) = store
        .entries
        .lock()
        .expect("the fake store lock")
        .get("idem:ttl")
        .expect("the entry is stored")
        .clone();
    assert_eq!(ttl, IDEM_TTL_SECS);
    assert_eq!(IDEM_TTL_SECS, 86_400, "24 h in seconds");
}

/// The stored shape round-trips through JSON with a base64 body (the
/// record module's wire convention — the same bytes cross the L1).
#[test]
fn the_stored_shape_roundtrips_through_json() {
    let stored = StoredResponse {
        status: 201,
        headers: vec![("content-type".into(), "application/json".into())],
        body: br#"{"id":7}"#.to_vec(),
    };
    let json = serde_json::to_string(&stored).expect("the stored shape serializes");
    assert!(
        json.contains("eyJpZCI6N30="),
        "the body rides base64: {json}"
    );
    let round: StoredResponse = serde_json::from_str(&json).expect("the stored shape deserializes");
    assert_eq!(round, stored);
    let _ = round.to_executed();
}

/// A handler error surfaces unchanged and stores nothing.
#[tokio::test]
async fn a_handler_error_is_not_stored() {
    struct FailingHandler;
    impl FailingHandler {
        async fn run(&self) -> Result<ExecutedResponse, OxideError> {
            Err(OxideError::new("RT-500", "the loader exploded"))
        }
    }
    let store = MemoryStore::default();
    let err = stage_idempotency(&store, Some("boom"), || FailingHandler.run())
        .await
        .expect_err("the handler error surfaces");
    assert_eq!(err.0.code, "RT-500");
    assert!(
        store.entries.lock().unwrap().is_empty(),
        "nothing is stored for a failed handler"
    );
}

/// The stored response converts back to the executed shape the pipeline
/// speaks (the CLI stage's bridge).
#[test]
fn the_stored_response_converts_to_the_executed_shape() {
    let stored = StoredResponse {
        status: 204,
        headers: Vec::new(),
        body: Vec::new(),
    };
    let executed = stored.to_executed();
    assert_eq!(executed.status, 204);
    assert!(executed.body.is_empty());
}

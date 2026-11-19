//! V2-F15 recorder wiring (the CLI half): the `record:` config key's
//! decision, the per-request capture the dispatch closure feeds, the
//! worker drain that folds the worker-side fetch/cache/sequence captures
//! into the record, and the finished-response hook that writes
//! `.oxide/records/<request-id>.json`.
//!
//! The capture follows the substrate's shape (`oxide_fw_dispatch::record`
//! — the schema, the redactor, the policy, the sink) and stays out of the
//! request path's way: a request that does not record pays one map
//! lookup, and a capture failure degrades to a structured warn, never an
//! error page (the `DSP-116` house pattern — the offline convenience
//! never takes online traffic down).
//!
//! The flow per request:
//!
//! 1. [`RequestRecording::start`] — the dispatch closure constructs the
//!    [`Recording`] accumulator and registers it under the request id.
//!    The policy decision lands at finish: `errors`/sampled modes need
//!    the response status, which does not exist yet, so every registered
//!    request accumulates and the finish applies
//!    [`RecordPolicy::should_record`] with the final status and the
//!    deterministic roll (`sample_roll_for(request_id)`).
//! 2. The matched route, the middleware locals and the loader calls
//!    (through [`RecordingLoaderRunner`], which the closure wraps its
//!    runner in) feed the accumulator as the request runs.
//! 3. Exactly one finish path closes the capture:
//!    - a buffered response: the [`RecorderObserver`] (the server
//!      crate's `ResponseObserver` seam, fired at the one
//!      `finish_response` funnel) finishes with the stamped response —
//!      the same header set a replay's HTTP client would read;
//!    - a streamed response: the observer hands the stamped status and
//!      headers to the [`RecordTee`] adapter (the stream is not drained
//!      yet), which accumulates the chunks the render streams and
//!      finishes when the stream ends.
//!
//!    Both paths run the same finisher task, which first drains the
//!    worker captures (`replay:drain` broadcast over every pool worker)
//!    so the record carries the fetch ops, cache ops and
//!    non-deterministic sequences the request actually observed, then
//!    applies the policy and writes `.oxide/records/<request-id>.json` (a
//!    write failure is a structured `RPL-001` warn — the record is a
//!    diagnostic, never a gate).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use futures::Stream;
use oxide_fw_config::RecordConfig;
use oxide_fw_dispatch::record::{
    CacheOp, DirRecordStore, FetchOp, NonDetSequences, RecordMode, RecordPolicy, RecordSink,
    Recording, RecordingLoaderRunner, Redactor,
};
use oxide_fw_jshost::{CallInput, EntryRef, Pool};
use oxide_fw_server::{BodyStream, OxideRequest, ResponseObserver};

/// The per-process capture configuration, built once per server from the
/// `record:` config key. Absent when the key is off — the pipeline is
/// byte-identical.
pub struct RecordCapture {
    /// The decision the finish applies (with the caller's sample roll).
    pub policy: RecordPolicy,
    /// Where finished records go (`.oxide/records/` under the root).
    pub sink: Arc<dyn RecordSink>,
    /// The build id the record stamps (`RPL-003`'s comparison side).
    pub build_id: String,
    /// The JS worker pool, for the end-of-request `replay:drain` that
    /// folds the worker-side captures into the record. `None` when the
    /// stack runs without a pool (no worker captures to drain).
    pub pool: Option<Arc<dyn Pool>>,
}

impl RecordCapture {
    /// The capture for one server: `record: 'all' | 'errors' | {sample,
    /// onError}` maps straight onto the substrate's policy; `false` (the
    /// default) records nothing. The dev `OXIDE_INCLUDE_SECRETS=1` toggle
    /// (the `oxide dev --include-secrets` flag's env spelling) keeps the
    /// secret header trio verbatim so a dev replay can exercise
    /// authenticated paths; it is never set by the production stack.
    #[must_use]
    pub fn from_config(
        record: &RecordConfig,
        root: &Path,
        build_id: &str,
        pool: Option<Arc<dyn Pool>>,
    ) -> Option<Arc<Self>> {
        let mode = match record {
            RecordConfig::Off(_) => return None,
            RecordConfig::All => RecordMode::All,
            RecordConfig::Errors => RecordMode::Errors,
            RecordConfig::Sampled { sample, on_error } => RecordMode::Sampled {
                sample_rate: *sample,
                on_error: *on_error,
            },
        };
        Some(Arc::new(Self {
            policy: RecordPolicy {
                mode,
                include_secrets: include_secrets_from_env(),
            },
            sink: Arc::new(DirRecordStore::new(root)),
            build_id: build_id.to_string(),
            pool,
        }))
    }
}

/// The dev `--include-secrets` toggle, read through the env the way the
/// dev-only toggles are (`OXIDE_DEV_NO_ESM`'s pattern): `cmd_dev` sets it
/// from the flag; the production stack builder never does.
#[must_use]
pub fn include_secrets_from_env() -> bool {
    std::env::var_os("OXIDE_INCLUDE_SECRETS").is_some_and(|v| v == "1")
}

/// The stamped status + headers the observer hands to a streamed
/// response's tee (the tee finishes with these, so the record carries the
/// same header set a replay's HTTP client would read).
struct StreamMeta {
    status: u16,
    headers: Vec<(String, String)>,
}

/// One in-flight capture: the accumulator, the sample roll the finish
/// needs, and the meta channel a streamed response's tee waits on.
struct InFlight {
    recording: Arc<Mutex<Recording>>,
    roll: f64,
    meta_tx: Option<tokio::sync::oneshot::Sender<StreamMeta>>,
}

/// The in-flight captures, keyed by request id. One per server process.
pub struct Recordings {
    map: DashMap<String, InFlight>,
}

impl Recordings {
    /// An empty map.
    #[must_use]
    pub fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }
}

impl Default for Recordings {
    fn default() -> Self {
        Self::new()
    }
}

/// The closure-facing per-request handle. The dispatch closure constructs
/// it once per request when the capture is on; every method is a no-op
/// cost otherwise (`None`).
pub struct RequestRecording {
    handle: Arc<Mutex<Recording>>,
    capture: Arc<RecordCapture>,
    request_id: String,
    roll: f64,
    /// The meta receiver the stream tee consumes (once). Interior
    /// mutability: the closure holds the handle shared-borrowed across the
    /// request phases.
    meta_rx: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<StreamMeta>>>,
}

impl RequestRecording {
    /// Start the capture for one request: the request line-set is
    /// redacted into the accumulator and the entry enters the in-flight
    /// map. The route and locals feed later (they exist after the match
    /// and the middleware phase).
    #[must_use]
    pub fn start(
        capture: &Arc<RecordCapture>,
        recordings: &Recordings,
        req: &OxideRequest,
    ) -> Option<Self> {
        let roll = oxide_fw_dispatch::record::sample_roll_for(&req.request_id);
        let redactor = Redactor::new(capture.policy.include_secrets);
        let handle = Arc::new(Mutex::new(Recording::new(redactor, &req.method, &req.url)));
        if let Ok(mut rec) = handle.lock() {
            let headers: Vec<(String, String)> = req
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            rec.set_request_headers(&headers);
        }
        let (meta_tx, meta_rx) = tokio::sync::oneshot::channel();
        recordings.map.insert(
            req.request_id.clone(),
            InFlight {
                recording: Arc::clone(&handle),
                roll,
                meta_tx: Some(meta_tx),
            },
        );
        Some(Self {
            handle,
            capture: Arc::clone(capture),
            request_id: req.request_id.clone(),
            roll,
            meta_rx: std::sync::Mutex::new(Some(meta_rx)),
        })
    }

    /// The matched route id and params (the router's verdict).
    pub fn set_route(&self, route_id: &str, params: &BTreeMap<String, String>) {
        if let Ok(mut rec) = self.handle.lock() {
            let owned: Vec<(String, String)> =
                params.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            rec.set_route(route_id, &owned);
        }
    }

    /// The middleware locals the request runs with.
    pub fn set_locals(&self, locals: &serde_json::Value) {
        if let Ok(mut rec) = self.handle.lock() {
            rec.set_locals(locals.clone());
        }
    }

    /// The loader runner this request's loads run through: the shared
    /// runner decorated with the [`RecordingLoaderRunner`], so every load
    /// the §9.2 graph drives lands in the record without the runner or
    /// the graph knowing.
    #[must_use]
    pub fn decorated(
        &self,
        inner: Option<Arc<dyn oxide_fw_server::LoaderRunner>>,
    ) -> Option<Arc<dyn oxide_fw_server::LoaderRunner>> {
        inner.map(|shared| {
            Arc::new(RecordingLoaderRunner::new(shared, Arc::clone(&self.handle)))
                as Arc<dyn oxide_fw_server::LoaderRunner>
        })
    }

    /// Wrap a response body stream for the recording (the streamed-response
    /// finish path). The chunks the client receives are byte-identical;
    /// the capture finishes when the stream ends, with the stamped status
    /// and headers the observer signals.
    pub fn tee(&self, inner: BodyStream, fallback_status: u16) -> BodyStream {
        // Poison recovery (micro-debt, V2-F47): the guard's critical
        // section is one plain `Option::take` — no invariant can break
        // inside it — so a poisoned lock hands the slot back and the
        // metadata still reaches the tee. The old `.ok()` shape silently
        // dropped the recorded metadata on a poisoned lock.
        let meta_rx = self
            .meta_rx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        Box::pin(RecordTee {
            inner,
            buf: Vec::new(),
            capture: Arc::clone(&self.capture),
            request_id: self.request_id.clone(),
            roll: self.roll,
            handle: Arc::clone(&self.handle),
            meta_rx,
            fallback_status,
        })
    }
}

/// The finisher's inputs: everything the spawned task needs to drain the
/// worker, close the accumulator and write (or drop) the record.
struct FinishRequest {
    request_id: String,
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// The one finish definition both paths drive: drain the worker captures,
/// close the accumulator, apply the policy, write the record. A write
/// failure is a structured warn (`RPL-001`) — the record is a diagnostic,
/// never a gate.
fn finish_capture(
    capture: Arc<RecordCapture>,
    recording: Arc<Mutex<Recording>>,
    roll: f64,
    finished: FinishRequest,
) {
    tokio::spawn(async move {
        // The worker captures: every pool worker answers the drain (the
        // pool's `call_all` is the broadcast), so the fetch ops, cache ops
        // and non-deterministic sequences this request produced land in
        // the record no matter which worker served which op. A drain
        // failure (no pool, a backend that refuses the op) degrades to an
        // empty capture set, logged at debug — the record shows the hole.
        let drained = match capture.pool.as_ref() {
            Some(pool) => drain_pool(pool).await,
            None => Drained::default(),
        };
        let mut rec = match recording.lock() {
            Ok(r) => r,
            Err(_) => return,
        };
        for f in &drained.fetches {
            rec.record_fetch(
                &f.method,
                &f.url,
                &drained.decode_request_body(f),
                f.response.status,
                &f.response.headers,
                &drained.decode_response_body(f),
            );
        }
        for c in &drained.cache_ops {
            rec.record_cache_op(&c.op, &c.key, c.hit);
        }
        for ms in &drained.sequences.now_ms {
            rec.record_now(*ms);
        }
        for r in &drained.sequences.randoms {
            rec.record_random(*r);
        }
        for u in &drained.sequences.uuids {
            rec.record_uuid(u);
        }
        let record = rec.finish_shared(
            &finished.request_id,
            &capture.build_id,
            finished.status,
            &finished.headers,
            &finished.body,
        );
        drop(rec);
        // The decision: the status now exists. A declined capture is
        // dropped silently (that IS the sampling decision).
        if capture.policy.should_record(finished.status, roll)
            && let Err(e) = capture.sink.write(&record)
        {
            tracing::warn!(
                code = %e.0.code,
                msg = %e.0.message,
                request_id = %finished.request_id,
                "the request record could not be written; the capture is dropped"
            );
        }
    });
}

/// The worker drain's reply shape (the worker's `replay:drain` payload —
/// camelCase, the record's wire convention).
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Drained {
    #[serde(default)]
    fetches: Vec<FetchOp>,
    #[serde(default)]
    cache_ops: Vec<CacheOp>,
    #[serde(default)]
    sequences: NonDetSequences,
}

impl Drained {
    fn decode_request_body(&self, f: &FetchOp) -> Vec<u8> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(&f.request_body_base64)
            .unwrap_or_default()
    }

    fn decode_response_body(&self, f: &FetchOp) -> Vec<u8> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(&f.response.body_base64)
            .unwrap_or_default()
    }
}

/// Drain every pool worker's capture buffer. A worker that refuses the op
/// (the V8 backend has no `replay:drain`) contributes an empty set, and
/// the refusal is logged — the captures that worker held are lost, and
/// the record shows it (no silent holes).
async fn drain_pool(pool: &Arc<dyn Pool>) -> Drained {
    let entry = EntryRef {
        kind: "replay:drain".to_string(),
        module: String::new(),
        export: String::new(),
    };
    let input = CallInput {
        payload: serde_json::json!({}),
        headers: BTreeMap::new(),
        has_body: false,
        request_id: String::new(),
        route: None,
    };
    let replies = pool.call_all(entry, input).await;
    let mut merged = Drained::default();
    let mut refused = 0usize;
    for mut reply in replies {
        match reply {
            Ok(ref mut stream) => match stream.collect().await {
                Ok(bytes) => match serde_json::from_slice::<Drained>(&bytes) {
                    Ok(one) => {
                        merged.fetches.extend(one.fetches);
                        merged.cache_ops.extend(one.cache_ops);
                        merged.sequences.now_ms.extend(one.sequences.now_ms);
                        merged.sequences.randoms.extend(one.sequences.randoms);
                        merged.sequences.uuids.extend(one.sequences.uuids);
                    }
                    Err(_) => refused += 1,
                },
                Err(_) => refused += 1,
            },
            Err(_) => refused += 1,
        }
    }
    if refused > 0 {
        tracing::debug!(
            refused,
            "replay:drain refused by some workers; their captures are not in the record"
        );
    }
    merged
}

/// The server's response-side hook: a buffered response finishes its
/// capture right here (the headers are the stamped ones — the same shape
/// a replay's HTTP client would read). A streamed response signals its
/// tee with the stamped meta and leaves the finish to it (the stream is
/// not drained yet).
pub struct RecorderObserver {
    capture: Arc<RecordCapture>,
    recordings: Arc<Recordings>,
}

impl RecorderObserver {
    /// The observer over one server's capture config.
    #[must_use]
    pub fn new(capture: Arc<RecordCapture>, recordings: Arc<Recordings>) -> Self {
        Self {
            capture,
            recordings,
        }
    }
}

impl ResponseObserver for RecorderObserver {
    fn observe(
        &self,
        request_id: &str,
        method: &str,
        status: u16,
        headers: &[(String, String)],
        body: Option<&[u8]>,
    ) {
        let _ = method;
        let Some(flight) = self.recordings.map.remove(request_id).map(|(_, v)| v) else {
            return;
        };
        match body {
            Some(bytes) => finish_capture(
                Arc::clone(&self.capture),
                flight.recording,
                flight.roll,
                FinishRequest {
                    request_id: request_id.to_string(),
                    status,
                    headers: headers.to_vec(),
                    body: bytes.to_vec(),
                },
            ),
            None => {
                // Streamed: the tee owns the finish — hand it the stamped
                // meta. A tee that already gave up (aborted render) leaves
                // the send failed; the entry was taken either way.
                if let Some(tx) = flight.meta_tx {
                    let _ = tx.send(StreamMeta {
                        status,
                        headers: headers.to_vec(),
                    });
                }
            }
        }
    }
}

/// The stream tee (the streamed-response finish path): every chunk the
/// render streams is passed through UNCHANGED and accumulated; when the
/// stream ends, the capture finishes with the accumulated body and the
/// observer-signaled meta. A caller that drops the tee mid-stream loses
/// the record (an aborted render was never a complete response to
/// replay).
pub struct RecordTee {
    inner: BodyStream,
    buf: Vec<u8>,
    capture: Arc<RecordCapture>,
    request_id: String,
    roll: f64,
    handle: Arc<Mutex<Recording>>,
    meta_rx: Option<tokio::sync::oneshot::Receiver<StreamMeta>>,
    fallback_status: u16,
}

impl Stream for RecordTee {
    type Item = std::result::Result<bytes::Bytes, std::io::Error>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match std::pin::Pin::new(&mut self.inner).poll_next(cx) {
            std::task::Poll::Ready(Some(Ok(chunk))) => {
                // The record's 1 MB body cap bounds the accumulation too —
                // the client still receives every chunk byte-identical.
                if self.buf.len() < oxide_fw_dispatch::record::MAX_CAPTURED_BODY {
                    self.buf.extend_from_slice(&chunk);
                }
                std::task::Poll::Ready(Some(Ok(chunk)))
            }
            std::task::Poll::Ready(Some(Err(e))) => std::task::Poll::Ready(Some(Err(e))),
            std::task::Poll::Ready(None) => {
                let capture = Arc::clone(&self.capture);
                let handle = Arc::clone(&self.handle);
                let roll = self.roll;
                let request_id = self.request_id.clone();
                let fallback_status = self.fallback_status;
                let meta_rx = self.meta_rx.take();
                let body = std::mem::take(&mut self.buf);
                tokio::spawn(async move {
                    let (status, headers) = match meta_rx {
                        Some(rx) => {
                            match tokio::time::timeout(std::time::Duration::from_secs(5), rx).await
                            {
                                Ok(Ok(meta)) => (meta.status, meta.headers),
                                // The observer never signaled (an aborted
                                // serve): finish with the stream's own
                                // status and no headers — an honest partial.
                                _ => (fallback_status, Vec::new()),
                            }
                        }
                        None => (fallback_status, Vec::new()),
                    };
                    finish_capture(
                        capture,
                        handle,
                        roll,
                        FinishRequest {
                            request_id,
                            status,
                            headers,
                            body,
                        },
                    );
                });
                std::task::Poll::Ready(None)
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

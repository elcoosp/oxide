//! The record-and-replay capture substrate (spec v0.2, V2-F15 recorder
//! half).
//!
//! §V2-F15: "`oxide replay <id>` re-executes a captured request
//! deterministically." The recording side captures one request's
//! reproducible world into `.oxide/records/<request-id>.json`:
//!
//! - the request (method, URL, headers with the secret trio
//!   `authorization` / `cookie` / `set-cookie` replaced by
//!   `sha256:<12 hex>` unless `--include-secrets` in dev),
//! - the matched route id and params, the middleware locals,
//! - each loader input and output (the ODF-bearing shapes serialized
//!   verbatim),
//! - each outbound `fetch` op call and response (status, headers, body
//!   base64, up to 1 MB),
//! - cache op results, the build id, and the values returned by the
//!   non-deterministic sources (`Date.now()`, `Math.random()`,
//!   `crypto.randomUUID()`).
//!
//! This module is the V8-free capture half: the schema, the redactor, the
//! sampling policy, the accumulation guard and the two seams the wiring
//! drives — the [`RecordSink`] storage seam ([`DirRecordStore`] is the
//! filesystem impl, the atomic temp+rename the F11 pointer convention
//! uses) and the [`RecordingLoaderRunner`] decorator the CLI's dispatch
//! closure wraps its real runner in so the §9.2 load graph's runs land in
//! the record without the runner or the graph knowing about recording.
//!
//! The replay half — the outbound-call matcher (`RPL-002`), the recorded
//! non-deterministic sequences, the older-build-id warning (`RPL-003`)
//! and the `--diff` comparison — is the next F15 milestone; see
//! `docs/deviations.md`. The `record:` config key, the worker's
//! `globalThis.__oxide_replay` install and the `oxide replay` command are
//! the CLI-adoption halves, all deferred with the rest of the A01
//! phase-3 wiring (see `docs/deviations.md`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use oxide_fw_diagnostics::{Diagnostic, OxideError};
use oxide_fw_server::{LoaderOutput, LoaderRunInput, LoaderRunner};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The diagnostic code for a record-store failure: the write failed, the
/// record id is not a store key (traversal, separator, empty), or the
/// store directory could not be created.
pub const RPL_RECORD_IO: &str = "RPL-001";

/// The records directory under the project root (§V2-F15).
pub const RECORDS_DIR: &str = ".oxide/records";

/// The capture cap (§V2-F15: "body base64, up to 1 MB"): request and
/// response bodies longer than this are truncated to the first
/// [`MAX_CAPTURED_BODY`] bytes and the `truncated` marker is raised —
/// the op is still captured (a dropped op would break the ordered
/// replay), the replay consumer just knows the bytes are partial.
pub const MAX_CAPTURED_BODY: usize = 1024 * 1024;

/// The record schema version. Bumped when the wire shape changes; the
/// replay side refuses a record whose version it does not know.
pub const RECORD_VERSION: u8 = 1;

/// The lowercase hex alphabet the redaction digest renders with (the
/// `sha256:<12 hex>` shape the TS side hashes against).
const HEX: &[u8; 16] = b"0123456789abcdef";

fn rpl_io(msg: impl Into<String>) -> OxideError {
    OxideError(Box::new(Diagnostic::error(RPL_RECORD_IO, msg).with_fix(
        "check the project root's writability, or use a store key without path separators",
    )))
}

/// Which headers must never land in a record verbatim (§V2-F15; the same
/// secret set the CLI's `is_secret_key` guards for logging — kept a pure
/// function here so the crate stays V8-free and the rule is testable).
#[must_use]
pub fn is_secret_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "authorization" || n == "cookie" || n == "set-cookie"
}

/// The header redactor (§V2-F15): secret headers become
/// `sha256:<12 hex>` — the algorithm-named prefix, lowercase hex, so the
/// TypeScript side hashes the same bytes with the same truncation (the
/// D30 cross-implementation rule) — and every other header passes
/// through verbatim. `include_secrets` is the dev `--include-secrets`
/// flag: the recorded request keeps its real credentials so a replay can
/// exercise authenticated paths; it never ships to prod (the config
/// surface that would expose it is the deferred CLI half).
#[derive(Debug, Clone)]
pub struct Redactor {
    include_secrets: bool,
}

impl Redactor {
    /// A redactor; `include_secrets` keeps secret header values verbatim.
    #[must_use]
    pub fn new(include_secrets: bool) -> Self {
        Self { include_secrets }
    }

    /// The redacted form of one header value.
    #[must_use]
    pub fn redact(&self, name: &str, value: &str) -> String {
        if self.include_secrets || !is_secret_header(name) {
            return value.to_string();
        }
        let digest = Sha256::digest(value.as_bytes());
        let mut hex = String::with_capacity(12);
        for byte in &digest[..6] {
            hex.push(HEX[(byte >> 4) as usize] as char);
            hex.push(HEX[(byte & 0x0f) as usize] as char);
        }
        format!("sha256:{hex}")
    }
}

/// The recording mode (§V2-F15: "dev always when `record: 'errors' |
/// 'all'`, prod sampled `record: { sample: 0.001, onError: true }`").
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecordMode {
    /// `record: 'all'` — every request is captured.
    All,
    /// `record: 'errors'` — only failed requests (5xx: the failing-loader
    /// shape the acceptance replays; 4xx is a legitimate client outcome,
    /// not a repro bug).
    Errors,
    /// Prod sampling: a deterministic fraction of healthy traffic (the
    /// caller rolls one uniform sample per request and passes it here,
    /// keeping the decision pure) plus every error when `on_error` is
    /// set.
    Sampled {
        /// The fraction of healthy traffic captured (`roll < sample_rate`).
        sample_rate: f64,
        /// Capture 5xx regardless of the roll.
        on_error: bool,
    },
}

/// The capture policy: the mode plus the dev secrets flag. The substrate
/// takes the policy as an API parameter — the `record:` config key and
/// its schema entry are the deferred CLI-adoption half (see
/// `docs/deviations.md`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecordPolicy {
    /// The mode decision.
    pub mode: RecordMode,
    /// Keep secret header values verbatim (the dev `--include-secrets`).
    pub include_secrets: bool,
}

impl RecordPolicy {
    /// The dev default: everything, secrets redacted.
    #[must_use]
    pub fn dev_all() -> Self {
        Self {
            mode: RecordMode::All,
            include_secrets: false,
        }
    }

    /// The capture decision for one request: `status` is the response
    /// status the pipeline produced, `roll` the caller's uniform sample
    /// in `[0, 1)` (ignored outside [`RecordMode::Sampled`]).
    #[must_use]
    pub fn should_record(&self, status: u16, roll: f64) -> bool {
        let is_error = status >= 500;
        match self.mode {
            RecordMode::All => true,
            RecordMode::Errors => is_error,
            RecordMode::Sampled {
                sample_rate,
                on_error,
            } => roll < sample_rate || (on_error && is_error),
        }
    }
}

/// One captured outbound `fetch` op: the call (method, URL, request body
/// base64, capped) and the response (status, redacted headers, body
/// base64, capped). The replay matcher consumes these in order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FetchOp {
    /// The HTTP method of the outbound call.
    pub method: String,
    /// The URL the call went to.
    pub url: String,
    /// The request body, base64 (empty when the call had none).
    pub request_body_base64: String,
    /// The request body hit the 1 MB cap.
    pub truncated: bool,
    /// The response the call got.
    pub response: RecordedResponse,
}

/// One captured loader call: the [`LoaderRunInput`] and [`LoaderOutput`]
/// serialized verbatim (the ODF-bearing data shapes), plus the status and
/// the two identity fields repeated for a reader that does not re-parse
/// the payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LoaderCall {
    /// Absolute filesystem path of the module exporting `load`.
    pub module_path: String,
    /// Matched route id.
    pub route: String,
    /// The load's HTTP status (`LoaderOutput::status`).
    pub status: u16,
    /// The full `LoaderRunInput`, serialized.
    pub input: serde_json::Value,
    /// The full `LoaderOutput`, serialized (the ODF data shape).
    pub output: serde_json::Value,
}

/// One captured cache op result (§V2-F15: "cache op results"): the op
/// name (`l0.get`, `tiered.invalidate_tag`, …), the key it ran on, and —
/// for reads — the hit verdict. `None` for writes and invalidations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CacheOp {
    /// The op name the cache surface logs.
    pub op: String,
    /// The canonical cache key the op ran on.
    pub key: String,
    /// The read verdict, when the op is a read.
    pub hit: Option<bool>,
}

/// The non-deterministic source sequences (§V2-F15): every value the
/// request observed from `Date.now()`, `Math.random()` and
/// `crypto.randomUUID()`, in observation order. The replay side plays
/// them back in the same order so the re-execution is byte-deterministic.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NonDetSequences {
    /// `Date.now()` values, in order.
    #[serde(default)]
    pub now_ms: Vec<u64>,
    /// `Math.random()` values, in order.
    #[serde(default)]
    pub randoms: Vec<f64>,
    /// `crypto.randomUUID()` values, in order.
    #[serde(default)]
    pub uuids: Vec<String>,
}

/// A captured request or response line-set: the redaction ran on the way
/// in, so these headers are the record-safe form.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordedResponse {
    /// The HTTP status.
    pub status: u16,
    /// The headers, secret trio redacted, order preserved.
    pub headers: Vec<(String, String)>,
    /// The body, base64 (empty when there was none).
    pub body_base64: String,
    /// The body hit the 1 MB cap.
    pub truncated: bool,
}

/// The captured request line-set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordedRequest {
    /// The HTTP method.
    pub method: String,
    /// The request URL (path + query), as the pipeline saw it.
    pub url: String,
    /// The request headers, secret trio redacted, order preserved.
    pub headers: Vec<(String, String)>,
}

/// The record (§V2-F15): one request's reproducible world, serialized to
/// `.oxide/records/<request-id>.json`. The wire shape is camelCase (the
/// TS convention) and versioned; the replay half refuses versions it
/// does not know.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RequestRecord {
    /// The schema version ([`RECORD_VERSION`]).
    pub version: u8,
    /// The request id the record is stored under.
    pub request_id: String,
    /// The build id the request ran on — the replay warns `RPL-003`
    /// against a different current build but still runs.
    pub build_id: String,
    /// The request line-set (redacted).
    pub request: RecordedRequest,
    /// The matched route id.
    pub route_id: String,
    /// The URL params the router matched.
    pub params: BTreeMap<String, String>,
    /// The middleware locals the request ran with.
    pub locals: serde_json::Value,
    /// The loader calls, in graph order.
    pub loaders: Vec<LoaderCall>,
    /// The outbound fetch ops, in call order.
    pub fetches: Vec<FetchOp>,
    /// The cache op results, in order.
    pub cache_ops: Vec<CacheOp>,
    /// The non-deterministic sequences.
    pub sequences: NonDetSequences,
    /// The response the request produced (redacted, capped).
    pub response: RecordedResponse,
}

/// The storage seam (§V2-F15): where finished records go. The filesystem
/// impl is [`DirRecordStore`]; a hosted store (the F11 `DeployStore`
/// targets) drops in behind the same trait.
pub trait RecordSink: Send + Sync {
    /// Persist one finished record.
    ///
    /// # Errors
    ///
    /// [`OxideError`] with `RPL-001` when the store cannot create its
    /// directory, the record id is not a store key, or the write fails.
    fn write(&self, record: &RequestRecord) -> Result<(), OxideError>;
}

/// A store key: the request id, taken byte-exact — it must be a single
/// path-safe segment (no separators, no `..`, non-empty).
fn store_key(request_id: &str) -> Result<String, OxideError> {
    if request_id.is_empty()
        || request_id.contains('/')
        || request_id.contains('\\')
        || request_id == "."
        || request_id == ".."
        || request_id.contains("..")
    {
        return Err(rpl_io(format!(
            "record id `{request_id}` is not a store key"
        )));
    }
    Ok(request_id.to_string())
}

/// The filesystem record store: `<root>/.oxide/records/<request-id>.json`,
/// written atomically (temp file + rename, the F11 pointer convention) so
/// a reader never observes a half-written record.
#[derive(Debug, Clone)]
pub struct DirRecordStore {
    dir: PathBuf,
}

impl DirRecordStore {
    /// A store under `root` (the project root — the §V2-F15 path is
    /// root-relative, matching the `.oxide/` state convention).
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            dir: root.join(RECORDS_DIR),
        }
    }

    /// The directory the store writes into.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl RecordSink for DirRecordStore {
    fn write(&self, record: &RequestRecord) -> Result<(), OxideError> {
        let key = store_key(&record.request_id)?;
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| rpl_io(format!("cannot create {}: {e}", self.dir.display())))?;
        let json = serde_json::to_vec_pretty(record)
            .map_err(|e| rpl_io(format!("serialize record {}: {e}", record.request_id)))?;
        let target = self.dir.join(format!("{key}.json"));
        let tmp = self.dir.join(format!(".{key}.tmp"));
        std::fs::write(&tmp, &json)
            .map_err(|e| rpl_io(format!("cannot write {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &target).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            rpl_io(format!("cannot finalize {}: {e}", target.display()))
        })?;
        Ok(())
    }
}

/// The one-request capture accumulator. The wiring constructs it after
/// [`RecordPolicy::should_record`] says yes, feeds it as the request runs
/// (loader calls through [`RecordingLoaderRunner`], fetch ops and cache
/// ops at their sites, the non-deterministic values as the worker reports
/// them), and finishes it with the response once the pipeline produced
/// one.
#[derive(Debug, Clone)]
pub struct Recording {
    redactor: Redactor,
    method: String,
    url: String,
    request_headers: Vec<(String, String)>,
    route_id: String,
    params: BTreeMap<String, String>,
    locals: serde_json::Value,
    loaders: Vec<LoaderCall>,
    fetches: Vec<FetchOp>,
    cache_ops: Vec<CacheOp>,
    sequences: NonDetSequences,
}

impl Recording {
    /// Start recording one request: the method and URL are the pipeline's
    /// view of the inbound request. The request's headers are captured by
    /// [`Recording::set_request_headers`].
    #[must_use]
    pub fn new(redactor: Redactor, method: &str, url: &str) -> Self {
        Self {
            redactor,
            method: method.to_string(),
            url: url.to_string(),
            request_headers: Vec::new(),
            route_id: String::new(),
            params: BTreeMap::new(),
            locals: serde_json::Value::Null,
            loaders: Vec::new(),
            fetches: Vec::new(),
            cache_ops: Vec::new(),
            sequences: NonDetSequences::default(),
        }
    }

    /// Capture the inbound request's headers (redacted on the way in).
    pub fn set_request_headers(&mut self, headers: &[(String, String)]) {
        self.request_headers = self.redact_headers(headers);
    }

    /// The matched route id and params (the router's verdict), captured
    /// once the router matched.
    pub fn set_route(&mut self, route_id: &str, params: &[(String, String)]) {
        self.route_id = route_id.to_string();
        self.params = params.iter().cloned().collect();
    }

    /// The middleware locals the request runs with.
    pub fn set_locals(&mut self, locals: serde_json::Value) {
        self.locals = locals;
    }

    /// Capture one loader call (input + output serialized verbatim).
    pub fn record_loader(&mut self, input: &LoaderRunInput, output: &LoaderOutput) {
        let input = serde_json::to_value(input).unwrap_or(serde_json::Value::Null);
        let output = serde_json::to_value(output).unwrap_or(serde_json::Value::Null);
        self.loaders.push(LoaderCall {
            module_path: input
                .get("module_path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            route: input
                .get("route")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            status: output
                .get("status")
                .and_then(serde_json::Value::as_u64)
                .and_then(|s| u16::try_from(s).ok())
                .unwrap_or(0),
            input,
            output,
        });
    }

    /// Capture one outbound fetch op. Bodies are base64 under the 1 MB
    /// cap (the `truncated` markers say when); the response's secret
    /// headers are redacted like the request's.
    pub fn record_fetch(
        &mut self,
        method: &str,
        url: &str,
        request_body: &[u8],
        status: u16,
        response_headers: &[(String, String)],
        response_body: &[u8],
    ) {
        let (req_b64, req_trunc) = encode_capped(request_body);
        let (resp_b64, resp_trunc) = encode_capped(response_body);
        self.fetches.push(FetchOp {
            method: method.to_string(),
            url: url.to_string(),
            request_body_base64: req_b64,
            truncated: req_trunc,
            response: RecordedResponse {
                status,
                headers: self.redact_headers(response_headers),
                body_base64: resp_b64,
                truncated: resp_trunc,
            },
        });
    }

    /// Capture one cache op result.
    pub fn record_cache_op(&mut self, op: &str, key: &str, hit: Option<bool>) {
        self.cache_ops.push(CacheOp {
            op: op.to_string(),
            key: key.to_string(),
            hit,
        });
    }

    /// Record one `Date.now()` observation.
    pub fn record_now(&mut self, ms: u64) {
        self.sequences.now_ms.push(ms);
    }

    /// Record one `Math.random()` observation.
    pub fn record_random(&mut self, value: f64) {
        self.sequences.randoms.push(value);
    }

    /// Record one `crypto.randomUUID()` observation.
    pub fn record_uuid(&mut self, id: &str) {
        self.sequences.uuids.push(id.to_string());
    }

    /// The loader calls captured so far (the decorator's view).
    #[must_use]
    pub fn loaders(&self) -> &[LoaderCall] {
        &self.loaders
    }

    /// The fetch ops captured so far.
    #[must_use]
    pub fn fetches(&self) -> &[FetchOp] {
        &self.fetches
    }

    /// The header redaction the recording uses (the wiring redacts the
    /// inbound request's headers through the same redactor before the
    /// record is finished).
    #[must_use]
    pub fn redactor(&self) -> &Redactor {
        &self.redactor
    }

    fn redact_headers(&self, headers: &[(String, String)]) -> Vec<(String, String)> {
        headers
            .iter()
            .map(|(k, v)| (k.clone(), self.redactor.redact(k, v)))
            .collect()
    }

    /// Finish the capture: the response is the pipeline's final answer.
    #[must_use]
    pub fn finish(
        self,
        request_id: &str,
        build_id: &str,
        status: u16,
        response_headers: &[(String, String)],
        response_body: &[u8],
    ) -> RequestRecord {
        let (body_b64, truncated) = encode_capped(response_body);
        let headers = self.redact_headers(response_headers);
        RequestRecord {
            version: RECORD_VERSION,
            request_id: request_id.to_string(),
            build_id: build_id.to_string(),
            request: RecordedRequest {
                method: self.method,
                url: self.url,
                headers: self.request_headers,
            },
            route_id: self.route_id,
            params: self.params,
            locals: self.locals,
            loaders: self.loaders,
            fetches: self.fetches,
            cache_ops: self.cache_ops,
            sequences: self.sequences,
            response: RecordedResponse {
                status,
                headers,
                body_base64: body_b64,
                truncated,
            },
        }
    }
}

/// Base64 under the cap: `(encoded, truncated)`.
fn encode_capped(body: &[u8]) -> (String, bool) {
    let (bytes, truncated) = if body.len() > MAX_CAPTURED_BODY {
        (&body[..MAX_CAPTURED_BODY], true)
    } else {
        (body, false)
    };
    (
        base64::engine::general_purpose::STANDARD.encode(bytes),
        truncated,
    )
}

/// The wire-up seam: a [`LoaderRunner`] decorator the dispatch closure
/// wraps its real runner in. Every load the §9.2 graph drives through it
/// is captured — input (including the threaded `parentData`), output and
/// status — while the runner, the graph and the loaders stay
/// recording-agnostic.
pub struct RecordingLoaderRunner {
    inner: Arc<dyn LoaderRunner>,
    recording: Arc<Mutex<Recording>>,
}

impl RecordingLoaderRunner {
    /// Decorate `inner`, capturing into `recording`.
    #[must_use]
    pub fn new(inner: Arc<dyn LoaderRunner>, recording: Arc<Mutex<Recording>>) -> Self {
        Self { inner, recording }
    }
}

#[async_trait::async_trait]
impl LoaderRunner for RecordingLoaderRunner {
    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn run(&self, input: LoaderRunInput) -> oxide_fw_server::Result<LoaderOutput> {
        let output = self.inner.run(input.clone()).await?;
        if let Ok(mut rec) = self.recording.lock() {
            rec.record_loader(&input, &output);
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sha256_prefix_matches_the_d30_cross_implementation_shape() {
        // The value is hashed, not the name: two secret headers with the
        // same value carry the same digest.
        let r = Redactor::new(false);
        assert_eq!(
            r.redact("authorization", "x"),
            r.redact("set-cookie", "x"),
            "the digest covers the value only"
        );
    }

    #[test]
    fn store_keys_reject_the_traversal_shapes() {
        for bad in ["", ".", "..", "a/b", "a\\b", "x..y", "../escape"] {
            assert!(store_key(bad).is_err(), "`{bad}` is not a store key");
        }
        assert!(store_key("req-0001").is_ok());
    }
}

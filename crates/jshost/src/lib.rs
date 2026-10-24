//! oxide-fw-jshost — JsHost trait, NodeHost backend, ops, pools (spec §4, D2).
//!
//! Rust owns everything except user code and JS-only compilers (D1). The `JsHost`
//! trait exposes render + tooling pools. Backends: `V8Host` (default, via deno_core)
//! and `NodeHost` (child workers, same op protocol) — both pass the same conformance
//! suite (§4.2). This workspace ships `NodeHost` (real child processes over a
//! length-prefixed JSON+binary protocol on stdio) plus `ScriptedHost` for tests;
//! `V8Host` runs a real `deno_core` isolate per worker thread: a
//! `FsModuleLoader` resolves and reads an ESM bundle from disk, the
//! dispatch calls a named export through a dynamic `import()` driven on
//! the V8 event loop, and the result comes back as JSON. Its request-
//! scoped op surface (streaming, `cache_*`, `fetch`) is still a subset of
//! the Node worker's, so a framework whose adapter needs a Node built-in
//! still selects `NodeHost`; the module-call path is complete.
//!
//! Streaming ops (§4.5): `stream_write`, `response_head`, `request_body_read`,
//! `serialize`, `cache_get/set/tag`, `fetch`, `ws_*`, `log/trace`. Chunks cross the
//! boundary as byte handoffs, never copied through strings.

#![forbid(unsafe_code)]

use async_trait::async_trait;
use bytes::Bytes;
use oxide_fw_diagnostics::{Diagnostic, OxideError};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

pub mod snapshot;
pub mod tooling;
pub mod v8_loader;
pub mod v8_ops;
pub use snapshot::{
    AppSnapshot, SNAPSHOT_FORMAT, SideEffect, lint_top_level_side_effects, loader_options,
    snapshot_build, snapshot_build_bundle, snapshot_build_runtime,
};
pub mod framework_host;
pub mod framework_renderer;
pub mod loader_host;
pub mod tooling_pool;
pub use framework_host::{FrameworkDescriptor, FrameworkHost};
pub use framework_renderer::{
    ComponentSource, FrameworkHostRenderer, FrameworkRenderInput, FrameworkRenderer, RenderStream,
};
pub use loader_host::{
    ActionHost, ActionOutcome, EndpointHost, EndpointOutcome, LoaderHost, MiddlewareHost,
    MiddlewareOutcome, RevalidateEvent, ServerFnError, ServerFnHost, SocketHost, SocketOutcome,
};
pub use tooling_pool::{ModuleCall, ToolingModules};

pub type Result<T> = std::result::Result<T, OxideError>;

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Diagnostic::error(code, msg))
}

/// Pool sizing (§4.3): render pool default = physical cores; each isolate interleaves
/// up to K=64 in-flight requests at await points.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolConfig {
    pub size: usize,
    pub max_in_flight_per_worker: usize,
    /// Replace an isolate after this many requests (§4.3 lifecycle).
    pub recycle_after_requests: u64,
    /// Heap limit per isolate (bytes).
    pub heap_limit_bytes: u64,
    /// §17.2: `--offline-plugins` — deny all outbound network from the
    /// worker's fetch shim. Wired via the `OXIDE_OFFLINE_PLUGINS=1` env
    /// var to each worker child; the shim refuses instead of falling
    /// back to native fetch. This is the supply-chain guard the spec's
    /// CI mode names: a plugin / tooling module cannot phone home.
    pub offline_plugins: bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            size: 2,
            max_in_flight_per_worker: 64,
            recycle_after_requests: 10_000,
            heap_limit_bytes: 512 * 1024 * 1024,
            offline_plugins: std::env::var_os("OXIDE_OFFLINE_PLUGINS").is_some(),
        }
    }
}

/// What to call in the JS host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryRef {
    /// e.g. "render", "tooling:compile", "fn:<id>".
    pub kind: String,
    pub module: String,
    pub export: String,
}

/// Input to a JS call: ODF or JSON payload + metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallInput {
    pub payload: serde_json::Value,
    pub headers: BTreeMap<String, String>,
    /// streamed request body is handed over via the ops channel
    pub has_body: bool,
    /// request id for tracing (§20)
    pub request_id: String,
    /// PERF-02: the route label this call belongs to, when known. The pool
    /// uses it for affinity: calls for the same route land on the same
    /// worker, so that worker's module cache stays warm for that route
    /// instead of every worker loading every route. `None` = no affinity
    /// (least-loaded dispatch, the pre-PERF-02 behavior).
    #[serde(default)]
    pub route: Option<String>,
}

/// A streaming result from a JS call.
pub enum CallStream {
    /// Whole payload at once (small responses, tooling results).
    Complete(Bytes),
    /// Chunked: channel of body chunks; end-of-stream = None.
    Chunked(tokio::sync::mpsc::Receiver<Result<Bytes>>),
}

impl CallStream {
    /// Drain the stream to completion.
    ///
    /// §4.3 / P0-2 / WP-r9: a chunked stream must not be allowed to grow
    /// without bound. `/__data` (and any future caller that collects a
    /// loader stream) inherited the whole-collection shape with no cap,
    /// so a worker bug or a hostile app could hold an unbounded buffer
    /// on the host. Cap matches the h3 body drain: 8 MiB, 413-class on
    /// overflow. A complete payload is not capped here — it is already a
    /// `Bytes` the worker chose to send whole; the cap is a chunked-stream
    /// defence.
    pub async fn collect(&mut self) -> Result<Bytes> {
        const MAX_COLLECT_BYTES: usize = 8 * 1024 * 1024;
        // WP-B2 / P0-2: cap the *duration* too. A worker that stops
        // emitting chunks (a stuck defer promise, a hung loader) must not
        // pin the host request forever; the serve path already bounds
        // settle delivery at 30s, so collection uses the same budget.
        const COLLECT_DEADLINE: Duration = Duration::from_secs(30);
        match self {
            CallStream::Complete(b) => Ok(b.clone()),
            CallStream::Chunked(rx) => {
                let mut buf = Vec::new();
                loop {
                    let chunk = match tokio::time::timeout(COLLECT_DEADLINE, rx.recv()).await {
                        Ok(next) => next,
                        Err(_) => {
                            return Err(crate::err(
                                "JSH-DEADLINE",
                                "chunked stream did not finish within 30s".to_string(),
                            ));
                        }
                    };
                    let Some(chunk) = chunk else {
                        break;
                    };
                    let bytes = chunk?;
                    if buf.len() + bytes.len() > MAX_COLLECT_BYTES {
                        return Err(crate::err(
                            "JSH-OVERFLOW",
                            format!(
                                "chunked stream exceeded the {} MiB collect cap",
                                MAX_COLLECT_BYTES / (1024 * 1024)
                            ),
                        ));
                    }
                    buf.extend_from_slice(&bytes);
                }
                Ok(Bytes::from(buf))
            }
        }
    }
}

/// Pool statistics (§4.3, §20): saturation, recycle count, queue wait.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct PoolStats {
    pub workers: usize,
    pub in_flight: u64,
    pub completed: u64,
    pub failed: u64,
    pub recycles: u64,
    pub crashed: u64,
    pub queue_wait_ms_p50: f64,
    pub queue_wait_ms_p99: f64,
}

/// The pool trait (§4.2).
#[async_trait]
pub trait Pool: Send + Sync {
    async fn call(&self, entry: EntryRef, req: CallInput) -> Result<CallStream>;
    fn stats(&self) -> PoolStats;
    /// Abort all in-flight calls belonging to a request id (client disconnect, §9.6).
    fn abort_request(&self, request_id: &str);
    /// Graceful drain for shutdown (§12.2).
    async fn drain(&self);
}

/// The JS host trait (§4.2, D2).
pub trait JsHost: Send + Sync {
    fn render_pool(&self, cfg: PoolConfig) -> Result<Arc<dyn Pool>>;
    fn tooling_pool(&self, cfg: PoolConfig) -> Result<Arc<dyn Pool>>;
    fn backend_name(&self) -> &'static str;
}

/// PERF-05: the HTTP fetch the `fetch` op delegates to. `oxide-fw-jshost` has no
/// HTTP client of its own; the CLI wires an implementation backed by
/// `oxide-fw-server`'s SSRF-hardened fetch (SEC-03), so a server-side `fetch()`
/// in user code goes through the same validated path as the image endpoint —
/// no separate, unguarded client.
#[async_trait::async_trait]
pub trait FetchHandler: Send + Sync {
    /// Perform an HTTP request. `body` is the raw request body (empty for GET).
    /// Returns the response status, headers, and body bytes.
    async fn fetch(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<FetchResponse>;
}

/// §6.2 / §15: a plugin `transform` hook the worker calls for every module
/// it loads. This is what makes a plugin transform reach the **prerender**
/// path — the prerender renders components through the worker's module
/// loader (`compile_module`), so a plugin that rewrites a marker in a
/// `.tsx` must run here too, not only in the bundler. `id` is the module
/// path; the return is the transformed source (or the input unchanged).
#[async_trait::async_trait]
pub trait TransformHandler: Send + Sync {
    async fn transform(&self, id: &str, source: &str) -> Result<String>;
}

/// The result of a [`FetchHandler::fetch`].
#[derive(Debug, Clone)]
pub struct FetchResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Op codes over the wire (§4.5). Both V8 and Node backends speak the same table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u16)]
pub enum Op {
    StreamWrite = 1,
    ResponseHead = 2,
    RequestBodyRead = 3,
    Serialize = 4,
    CacheGet = 5,
    CacheSet = 6,
    CacheTag = 7,
    Fetch = 8,
    WsSend = 9,
    Log = 10,
    Trace = 11,
    /// §9.3 / §6.2: run the compiler passes on one `.server.ts` source and
    /// return the rewritten source. The worker's loader path needs the
    /// `'use cache'` / server-fn rewrites applied before it evaluates a
    /// module, and the Node ESM loader hook runs on its own thread with no
    /// way to reach Rust — so the worker's main thread calls this op and
    /// imports the rewritten file.
    CompileModule = 12,
}

impl Op {
    pub fn from_u16(v: u16) -> Option<Self> {
        Some(match v {
            1 => Op::StreamWrite,
            2 => Op::ResponseHead,
            3 => Op::RequestBodyRead,
            4 => Op::Serialize,
            5 => Op::CacheGet,
            6 => Op::CacheSet,
            7 => Op::CacheTag,
            8 => Op::Fetch,
            9 => Op::WsSend,
            10 => Op::Log,
            11 => Op::Trace,
            12 => Op::CompileModule,
            _ => return None,
        })
    }
}

/// Wire name of an op (§4.5 table), for tracing and error messages.
pub fn op_name(op: Op) -> &'static str {
    match op {
        Op::StreamWrite => "stream_write",
        Op::ResponseHead => "response_head",
        Op::RequestBodyRead => "request_body_read",
        Op::Serialize => "serialize",
        Op::CacheGet => "cache_get",
        Op::CacheSet => "cache_set",
        Op::CacheTag => "cache_tag",
        Op::Fetch => "fetch",
        Op::WsSend => "ws_send",
        Op::Log => "log",
        Op::Trace => "trace",
        Op::CompileModule => "compile_module",
    }
}

/// Wire message between host and worker (length-prefixed JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum WorkerMsg {
    Call {
        id: u64,
        entry: EntryRef,
        input: CallInput,
    },
    CallOk {
        id: u64,
        chunks: Vec<String>,
        complete: bool,
    },
    CallChunk {
        id: u64,
        data_b64: String,
    },
    /// PERF-12: a chunk carried as a binary frame (no base64). The JS side
    /// writes `[len][kind=1][8-byte id][raw bytes]` for a `Uint8Array` body.
    CallChunkBin {
        id: u64,
        data: Vec<u8>,
    },
    CallEnd {
        id: u64,
    },
    CallErr {
        id: u64,
        diagnostic: Diagnostic,
    },
    OpRequest {
        id: u64,
        op: Op,
        args: serde_json::Value,
    },
    OpResponse {
        id: u64,
        ok: bool,
        data: serde_json::Value,
    },
    Ready {
        pid: u32,
    },
    Pong {
        nonce: u64,
    },
}

/// Protocol guard (§4.2 "same op protocol"): no single frame may exceed 64 MiB.
pub const MAX_FRAME_LEN: u32 = 64 * 1024 * 1024;

/// Frame a message with a 4-byte little-endian length prefix.
pub fn frame(msg: &WorkerMsg) -> Result<Bytes> {
    let json = serde_json::to_vec(msg).map_err(|e| {
        err(
            "JSH-JSON",
            format!("failed to serialize worker message: {e}"),
        )
    })?;
    // BUG-16: check before casting — `as u32` would wrap a >4GiB len.
    if json.len() + 1 > MAX_FRAME_LEN as usize {
        return Err(err(
            "JSH-FRAME",
            format!(
                "frame of {} bytes exceeds the 64 MiB protocol limit (§4.2)",
                json.len()
            ),
        ));
    }
    // PERF-12: `[4-byte len][1-byte kind][payload]`, len covers kind+payload.
    // Host→worker frames are always JSON (kind 0); the worker→host direction
    // may use kind 1 for raw chunk bytes.
    let len = (json.len() + 1) as u32;
    let mut out = Vec::with_capacity(5 + json.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.push(0); // kind 0 = JSON
    out.extend_from_slice(&json);
    Ok(Bytes::from(out))
}

/// Parse frames from a buffer; returns (messages, consumed bytes).
///
/// Incomplete trailing frames are left unconsumed (the caller keeps buffering).
/// A declared length beyond [`MAX_FRAME_LEN`] is `JSH-FRAME`; a complete frame
/// whose payload is not protocol JSON is `JSH-JSON`.
pub fn unframe(buf: &[u8]) -> Result<(Vec<WorkerMsg>, usize)> {
    let mut msgs = Vec::new();
    let mut pos = 0usize;
    loop {
        if buf.len() - pos < 5 {
            break;
        }
        let len = u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]) as usize;
        if len > MAX_FRAME_LEN as usize {
            return Err(err(
                "JSH-FRAME",
                format!("declared frame length {len} exceeds the 64 MiB protocol limit (§4.2)"),
            ));
        }
        if len == 0 {
            return Err(err("JSH-FRAME", "frame length 0 (no kind byte)"));
        }
        if buf.len() - pos < 4 + len {
            break; // partial frame: wait for more bytes
        }
        let kind = buf[pos + 4];
        let payload = &buf[pos + 5..pos + 4 + len];
        match kind {
            0 => {
                let msg: WorkerMsg = serde_json::from_slice(payload).map_err(|e| {
                    err(
                        "JSH-JSON",
                        format!("worker message is not valid protocol JSON: {e}"),
                    )
                })?;
                msgs.push(msg);
            }
            1 => {
                // PERF-12: `[8-byte id LE][raw bytes]`.
                if payload.len() < 8 {
                    return Err(err("JSH-FRAME", "binary chunk shorter than its id header"));
                }
                let id = u64::from_le_bytes(payload[..8].try_into().unwrap());
                msgs.push(WorkerMsg::CallChunkBin {
                    id,
                    data: payload[8..].to_vec(),
                });
            }
            other => {
                return Err(err(
                    "JSH-FRAME",
                    format!("unknown frame kind {other} (expected 0=json, 1=binary)"),
                ));
            }
        }
        pos += 4 + len;
    }
    Ok((msgs, pos))
}

/// Least-loaded dispatch with route-affinity (§4.3).
///
/// Returns the index of the worker with the smallest load (ties → lowest index).
/// If `affinity` names an in-bounds worker whose load is at most `min + 1`, that
/// worker is preferred to keep JIT and module caches warm for hot routes.
/// PERF-02: map a route label to a stable worker index. The same route
/// always prefers the same worker, so that worker's module cache stays warm
/// for it. FNV-1a over the route bytes → `% n`. `None` route or `n == 0`
/// yields no affinity (least-loaded dispatch).
pub fn route_affinity(route: Option<&str>, n: usize) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let route = route?;
    let mut h: u64 = 0xcbf29ce484222325;
    for b in route.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    Some((h % n as u64) as usize)
}

pub fn choose_worker(loads: &[u64], affinity: Option<usize>) -> usize {
    if loads.is_empty() {
        return 0;
    }
    let mut min = loads[0];
    let mut best = 0usize;
    for (i, &l) in loads.iter().enumerate().skip(1) {
        if l < min {
            min = l;
            best = i;
        }
    }
    match affinity {
        Some(i) if i < loads.len() && loads[i] <= min + 1 => i,
        _ => best,
    }
}

// --------------------------------------------------------------- helpers ----

/// Decode standard base64 (RFC 4648, with padding) — binary chunks ride the JSON
/// protocol base64-encoded. Implemented in-house: no base64 crate is pinned in the
/// workspace (CONVENTIONS #7).
pub(crate) fn b64_decode(s: &str) -> Result<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let raw: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let pad = raw.iter().rev().take(2).filter(|&&b| b == b'=').count();
    if pad > 2 || (raw.len() - pad) % 4 == 1 || raw.len() % 4 != 0 {
        return Err(err(
            "JSH-B64",
            format!("invalid base64 length/padding ({}) chars", s.len()),
        ));
    }
    let body = &raw[..raw.len() - pad];
    let mut out = Vec::with_capacity(body.len() / 4 * 3 + 2);
    for quad in body.chunks(4) {
        let mut acc: u32 = 0;
        for (i, &c) in quad.iter().enumerate() {
            let v = val(c).ok_or_else(|| {
                err(
                    "JSH-B64",
                    format!("invalid base64 character `{}`", c as char),
                )
            })?;
            acc |= v << (18 - 6 * i);
        }
        // acc holds the data in the low 24 bits (be_bytes[0] is always 0), so the
        // decoded bytes are be_bytes[1 .. 1+keep] regardless of quad length.
        let bytes = acc.to_be_bytes();
        let keep = quad.len() - 1; // 4→3, 3→2, 2→1 decoded bytes
        out.extend_from_slice(&bytes[1..1 + keep]);
    }
    Ok(out)
}

/// Nearest-rank percentile of a sorted sample; 0.0 for an empty sample.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (p * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn sorted_copy(waits: &Mutex<VecDeque<f64>>) -> Vec<f64> {
    let mut v: Vec<f64> = waits.lock().unwrap().iter().copied().collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v
}

fn record_wait(waits: &Mutex<VecDeque<f64>>, d: Duration) {
    let mut q = waits.lock().unwrap();
    q.push_back(d.as_secs_f64() * 1000.0);
    if q.len() > 1000 {
        q.pop_front();
    }
}

// ------------------------------------------------------ scripted backend ----

/// A user-supplied handler executed by every [`ScriptedPool`] call.
pub type ScriptedHandler = Arc<dyn Fn(EntryRef, CallInput) -> Result<CallStream> + Send + Sync>;

/// Test double pool (§4.2: the conformance suite includes a scripted backend).
/// Every `call` executes the handler; without a handler, calls fail with
/// `JSH-NOSCRIPT` and the `failed` counter increments.
pub struct ScriptedPool {
    handler: Option<ScriptedHandler>,
    cfg: PoolConfig,
    completed: AtomicU64,
    failed: AtomicU64,
    in_flight: AtomicU64,
    draining: AtomicBool,
    /// Per-call wall time (ms) for queue-wait percentiles, capped at 1000 samples.
    queue_waits: Mutex<VecDeque<f64>>,
}

impl ScriptedPool {
    /// Create a scripted pool executing `handler` for every call.
    pub fn new(handler: ScriptedHandler, cfg: PoolConfig) -> Self {
        Self {
            handler: Some(handler),
            ..Self::without_handler(cfg)
        }
    }

    /// Create a scripted pool whose calls fail with `JSH-NOSCRIPT` (default host).
    pub fn without_handler(cfg: PoolConfig) -> Self {
        Self {
            handler: None,
            cfg,
            completed: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            in_flight: AtomicU64::new(0),
            draining: AtomicBool::new(false),
            queue_waits: Mutex::new(VecDeque::new()),
        }
    }
}

#[async_trait]
impl Pool for ScriptedPool {
    async fn call(&self, entry: EntryRef, req: CallInput) -> Result<CallStream> {
        if self.draining.load(Ordering::Relaxed) {
            return Err(err(
                "JSH-DRAIN",
                "pool is draining; no new calls are accepted (§12.2)",
            ));
        }
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        let start = Instant::now();
        let res = match &self.handler {
            Some(h) => h(entry.clone(), req.clone()),
            None => Err(err(
                "JSH-NOSCRIPT",
                "ScriptedHost has no handler registered; construct it via ScriptedHost::with_handler(..)",
            )),
        };
        record_wait(&self.queue_waits, start.elapsed());
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
        match res {
            Ok(stream) => {
                self.completed.fetch_add(1, Ordering::Relaxed);
                Ok(stream)
            }
            Err(e) => {
                self.failed.fetch_add(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }

    fn stats(&self) -> PoolStats {
        let waits = sorted_copy(&self.queue_waits);
        PoolStats {
            workers: self.cfg.size,
            in_flight: self.in_flight.load(Ordering::Relaxed),
            completed: self.completed.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            recycles: 0,
            crashed: 0,
            queue_wait_ms_p50: percentile(&waits, 0.5),
            queue_wait_ms_p99: percentile(&waits, 0.99),
        }
    }

    /// Calls complete synchronously; there is never anything pending to abort.
    fn abort_request(&self, _request_id: &str) {}

    /// Calls cannot outlive their caller, so draining only latches the flag.
    async fn drain(&self) {
        self.draining.store(true, Ordering::Relaxed);
    }
}

/// Test double: executes scripted responses; used by render/server tests (§4.2:
/// "Both pass the same conformance suite" — the suite includes a scripted backend).
///
/// The default host has no handler and fails calls with `JSH-NOSCRIPT`; use
/// [`ScriptedHost::with_handler`] to install one.
pub struct ScriptedHost {
    handler: Option<ScriptedHandler>,
}

impl Default for ScriptedHost {
    fn default() -> Self {
        Self { handler: None }
    }
}

impl ScriptedHost {
    /// Build a scripted host whose pools execute `handler` for every call.
    pub fn with_handler(handler: ScriptedHandler) -> Self {
        Self {
            handler: Some(handler),
        }
    }
}

impl JsHost for ScriptedHost {
    fn render_pool(&self, cfg: PoolConfig) -> Result<Arc<dyn Pool>> {
        Ok(Arc::new(match &self.handler {
            Some(h) => ScriptedPool::new(Arc::clone(h), cfg),
            None => ScriptedPool::without_handler(cfg),
        }))
    }
    fn tooling_pool(&self, cfg: PoolConfig) -> Result<Arc<dyn Pool>> {
        self.render_pool(cfg)
    }
    fn backend_name(&self) -> &'static str {
        "scripted"
    }
}

// ----------------------------------------------------------- v8 backend ----

use deno_core::{JsRuntime, RuntimeOptions};

/// Bootstrap script executed once per V8 runtime.
///
/// Provides (a) the §4.6 tier-2 Node-compat globals a bundled SSR chunk
/// references (`process`, `Buffer`), (b) the `oxide.*` op namespace, and
/// (c) `__oxideDispatch`, which imports a module by `file://` URL and calls
/// its named export with the JSON payload, returning a `{ok, value}` /
/// `{__oxide_error}` envelope. `boom`/`die` are handled Rust-side before
/// dispatch.
pub(crate) const V8_BOOTSTRAP: &str = r#"
// Sec.4.6 tier 2: the minimal Node-compat globals a self-contained bundle needs.
// A bundled SSR chunk references `process.env.NODE_ENV` (React's dev/prod
// switch), `process.emit` (a library's existence check), and `Buffer`
// (base64/UTF-8 helpers). Bare V8 has none of these; without the prelude an
// `import()` of the real bundle fails with `process is not defined`.
// The full `deno_node` surface is a later tier; this covers what the emitted
// bundles actually touch.
if (typeof globalThis.process === 'undefined') {
  globalThis.process = {
    env: { NODE_ENV: 'production' },
    // `process.emit` is probed by some libraries; a no-op keeps the check
    // truthy without pretending to be Node's event emitter.
    emit: function () {},
    // `process.cwd` / `process.platform` show up in a few polyfills.
    cwd: function () { return '/'; },
    platform: 'oxide',
    nextTick: function (fn) { Promise.resolve().then(fn); },
  };
}
// §4.6 tier 1: Web globals bare V8 lacks. deno_core does not register them;
// a bundled framework runtime (React's ESM build, the SSR adapters) uses
// TextEncoder/TextDecoder/atob/btoa/queueMicrotask/structuredClone at module
// scope and during render. These are minimal, spec-shaped implementations.
// §4.6 tier 1: a minimal Web Streams `ReadableStream`. The §7 adapter
// contract is `render(input) -> ReadableStream<Uint8Array>`, and the framework
// host consumes it with `for await`. This covers exactly that shape: a
// `start(controller)` producer with `enqueue`/`close`/`error`, and async
// iteration. (The full Web Streams spec — tee, pipeThrough, backpressure — is
// not implemented; a framework runtime that needs it selects the Node backend.)
if (typeof globalThis.ReadableStream === 'undefined') {
  globalThis.ReadableStream = class ReadableStream {
    constructor(source = {}) {
      const queue = [];
      const waiters = [];
      let done = false;
      let failure = null;
      const settle = () => {
        while (waiters.length && (queue.length || done || failure)) {
          const w = waiters.shift();
          if (failure) w.reject(failure);
          else if (queue.length) w.resolve({ value: queue.shift(), done: false });
          else w.resolve({ value: undefined, done: true });
        }
      };
      this._controller = {
        enqueue: (chunk) => { queue.push(chunk); settle(); },
        close: () => { done = true; settle(); },
        error: (e) => { failure = e; settle(); },
        get desiredSize() { return queue.length ? 1 : 0; },
      };
      try {
        if (typeof source.start === 'function') source.start(this._controller);
        if (typeof source.pull === 'function') {
          const pull = () => {
            if (done || failure) return;
            Promise.resolve(source.pull(this._controller)).then(() => {
              if (!done && !failure && queue.length === 0) pull();
            }).catch((e) => this._controller.error(e));
          };
          pull();
        }
      } catch (e) { failure = e; }
      this._read = () => {
        if (failure) return Promise.reject(failure);
        if (queue.length) return Promise.resolve({ value: queue.shift(), done: false });
        if (done) return Promise.resolve({ value: undefined, done: true });
        return new Promise((resolve, reject) => waiters.push({ resolve, reject }));
      };
    }
    getReader() {
      return {
        read: () => this._read(),
        releaseLock: () => {},
        cancel: () => { done = true; settle?.(); return Promise.resolve(); },
      };
    }
    [Symbol.asyncIterator]() {
      return { next: () => this._read() };
    }
    async cancel() { this._controller.close(); }
  };
}

if (typeof globalThis.TextEncoder === 'undefined') {
  globalThis.TextEncoder = class TextEncoder {
    constructor() { this.encoding = 'utf-8'; }
    encode(str) {
      str = String(str);
      const out = [];
      for (let i = 0; i < str.length; i++) {
        let c = str.charCodeAt(i);
        if (c < 0x80) out.push(c);
        else if (c < 0x800) out.push(0xc0 | (c >> 6), 0x80 | (c & 0x3f));
        else if (c >= 0xd800 && c <= 0xdbff && i + 1 < str.length) {
          const c2 = str.charCodeAt(i + 1);
          if (c2 >= 0xdc00 && c2 <= 0xdfff) {
            const cp = 0x10000 + ((c - 0xd800) << 10) + (c2 - 0xdc00);
            out.push(0xf0 | (cp >> 18), 0x80 | ((cp >> 12) & 0x3f),
                     0x80 | ((cp >> 6) & 0x3f), 0x80 | (cp & 0x3f));
            i++;
          } else out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 0x3f), 0x80 | (c & 0x3f));
        } else out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 0x3f), 0x80 | (c & 0x3f));
      }
      return new Uint8Array(out);
    }
    encodeInto(str, dest) {
      const bytes = this.encode(str);
      const n = Math.min(bytes.length, dest.length);
      dest.set(bytes.subarray(0, n));
      return { read: str.length, written: n };
    }
  };
}
if (typeof globalThis.TextDecoder === 'undefined') {
  globalThis.TextDecoder = class TextDecoder {
    constructor(enc, opts) { this.encoding = enc || 'utf-8'; this.fatal = !!(opts && opts.fatal); this._buf = []; }
    decode(buf, opts) {
      // BUG-07: `{stream:true}` keeps an incomplete trailing multi-byte
      // sequence buffered for the next chunk instead of dropping it (the
      // old code `break`ed and discarded it — mojibake whenever a split
      // character crossed a stream chunk, e.g. in __oxideFrameworkRender).
      const streaming = !!(opts && opts.stream);
      let bytes;
      if (buf == null) {
        bytes = new Uint8Array(this._buf);
        this._buf = [];
      } else {
        const fresh = buf instanceof Uint8Array ? buf : new Uint8Array(buf.buffer || buf);
        if (this._buf.length) {
          bytes = new Uint8Array(this._buf.length + fresh.length);
          bytes.set(this._buf, 0);
          bytes.set(fresh, this._buf.length);
          this._buf = [];
        } else bytes = fresh;
      }
      let out = '';
      for (let i = 0; i < bytes.length;) {
        const b = bytes[i];
        let cp, len;
        if (b < 0x80) { cp = b; len = 1; }
        else if (b < 0xe0) { cp = b & 0x1f; len = 2; }
        else if (b < 0xf0) { cp = b & 0x0f; len = 3; }
        else { cp = b & 0x07; len = 4; }
        if (i + len > bytes.length) {
          if (streaming) { this._buf = Array.from(bytes.slice(i)); break; }
          out += '�';
          break;
        }
        for (let j = 1; j < len; j++) cp = (cp << 6) | (bytes[i + j] & 0x3f);
        i += len;
        if (cp > 0xffff) {
          cp -= 0x10000;
          out += String.fromCharCode(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff));
        } else out += String.fromCharCode(cp);
      }
      if (!streaming) this._buf = [];
      return out;
    }
  };
}
// §4.6 tier 1: `MessageChannel`/`MessagePort`. React's browser SSR scheduler
// (react-dom/server.browser) schedules work through a MessageChannel, which
// expects macrotask delivery: `postMessage` runs after the current task AND
// after pending microtasks, yielding to I/O. Microtask delivery
// (queueMicrotask) can starve I/O when the scheduler reposts every tick —
// so delivery goes through `op_oxide_sleep(0)` (a real tokio timer), with a
// queueMicrotask fallback only if the op is unavailable.
// BUG-07: the old code delivered via queueMicrotask despite this comment.
if (typeof globalThis.MessageChannel === 'undefined') {
  globalThis.MessagePort = class MessagePort {
    constructor() { this.onmessage = null; this._other = null; this._closed = false; }
    postMessage(data) {
      if (this._closed || !this._other) return;
      const other = this._other;
      const deliver = () => {
        if (other._closed) return;
        if (typeof other.onmessage === 'function') other.onmessage({ data });
      };
      try {
        Deno.core.ops.op_oxide_sleep(0).then(deliver);
      } catch (e) {
        queueMicrotask(deliver);
      }
    }
    start() {}
    close() { this._closed = true; }
    addEventListener(type, fn) { if (type === 'message') this.onmessage = fn; }
    removeEventListener() {}
  };
  globalThis.MessageChannel = class MessageChannel {
    constructor() {
      const a = new globalThis.MessagePort();
      const b = new globalThis.MessagePort();
      a._other = b;
      b._other = a;
      this.port1 = a;
      this.port2 = b;
    }
  };
}
// §4.6 tier 1: Web Crypto's `crypto.getRandomValues` / `randomUUID`, backed
// by REAL OS entropy through the `op_oxide_random` op (ring's SystemRandom).
//
// This MUST be a CSPRNG: a `Math.random` filler would silently produce
// predictable tokens for any code that mints a session id, CSRF nonce or
// idempotency key. If the op returns short (the OS RNG failed), the shim
// THROWS — a caller fails loudly rather than proceed with weak randomness.
// §4.6 tier 1: `setTimeout`/`setInterval`/`clearTimeout`/`clearInterval`,
// backed by the `op_oxide_sleep` op (a real tokio timer the event loop polls).
// A framework's async SSR scheduler (Solid's `renderToString`) flushes through
// a `setTimeout(…, 0)`; without this it never resolves and the render is empty.
// BUG-07: interval callbacks are error-tolerant (log-and-continue via
// `oxide.log` — a throwing interval must not break the chain or crash the
// worker with an uncaught exception); clearing cancels the pending sleep.
if (typeof globalThis.setTimeout === 'undefined') {
  const timers = new Map();
  let nextId = 1;
  globalThis.setTimeout = function (fn, ms, ...args) {
    const id = nextId++;
    const handle = { cancelled: false, interval: false, ms: ms || 0, fn, args };
    timers.set(id, handle);
    const run = () => {
      Deno.core.ops.op_oxide_sleep(handle.ms).then(() => {
        if (handle.cancelled) return;
        if (handle.interval) {
          try { fn(...args); } catch (e) { try { globalThis.oxide.log('interval-error ' + ((e && e.message) || e)); } catch (_) {} }
          run();
        } else {
          timers.delete(id);
          try { fn(...args); } catch (e) { queueMicrotask(() => { throw e; }); }
        }
      });
    };
    run();
    return id;
  };
  globalThis.setInterval = function (fn, ms, ...args) {
    const id = globalThis.setTimeout(fn, ms, ...args);
    const h = timers.get(id);
    if (h) h.interval = true;
    return id;
  };
  globalThis.clearTimeout = function (id) { const h = timers.get(id); if (h) h.cancelled = true; timers.delete(id); };
  globalThis.clearInterval = globalThis.clearTimeout;
}

if (typeof globalThis.crypto === 'undefined') {
  const entropy = (n) => {
    const b = Deno.core.ops.op_oxide_random(n);
    const view = b instanceof Uint8Array ? b : new Uint8Array(b);
    if (view.length !== n) {
      throw new Error(
        'crypto: OS entropy unavailable (op_oxide_random returned ' + view.length +
        ' of ' + n + ' bytes); refusing to return weak randomness'
      );
    }
    return view;
  };
  const fill = (arr) => {
    const view = arr instanceof Uint8Array ? arr : new Uint8Array(arr.buffer, arr.byteOffset, arr.byteLength);
    if (view.length > 65536) throw new Error('crypto.getRandomValues: quota exceeded (65536 bytes max)');
    const rnd = entropy(view.length);
    view.set(rnd);
    return arr;
  };
  globalThis.crypto = {
    getRandomValues: fill,
    randomUUID: function () {
      const b = entropy(16);
      b[6] = (b[6] & 0x0f) | 0x40;
      b[8] = (b[8] & 0x3f) | 0x80;
      const h = [];
      for (let i = 0; i < 16; i++) h.push(b[i].toString(16).padStart(2, '0'));
      return h.slice(0, 4).join('') + '-' + h.slice(4, 6).join('') + '-' +
             h.slice(6, 8).join('') + '-' + h.slice(8, 10).join('') + '-' + h.slice(10, 16).join('');
    },
  };
}

if (typeof globalThis.MessageChannel === 'undefined') {
  globalThis.MessagePort = class MessagePort {
    constructor() { this.onmessage = null; this._other = null; this._closed = false; }
    postMessage(data) {
      if (this._closed || !this._other) return;
      const other = this._other;
      queueMicrotask(() => {
        if (other._closed) return;
        const ev = { data };
        if (typeof other.onmessage === 'function') other.onmessage(ev);
      });
    }
    start() {}
    close() { this._closed = true; }
    addEventListener(type, fn) { if (type === 'message') this.onmessage = fn; }
    removeEventListener() {}
  };
  globalThis.MessageChannel = class MessageChannel {
    constructor() {
      const a = new globalThis.MessagePort();
      const b = new globalThis.MessagePort();
      a._other = b;
      b._other = a;
      this.port1 = a;
      this.port2 = b;
    }
  };
}

if (typeof globalThis.queueMicrotask === 'undefined') {
  globalThis.queueMicrotask = (fn) => Promise.resolve().then(fn);
}
if (typeof globalThis.structuredClone === 'undefined') {
  globalThis.structuredClone = (v) => JSON.parse(JSON.stringify(v));
}
if (typeof globalThis.atob === 'undefined') {
  const B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  globalThis.atob = function (s) {
    s = String(s).replace(/[^A-Za-z0-9+/]/g, '');
    let out = '';
    for (let i = 0; i < s.length; i += 4) {
      const n = (B64.indexOf(s[i]) << 18) | (B64.indexOf(s[i+1]) << 12)
              | ((B64.indexOf(s[i+2]) & 63) << 6) | (B64.indexOf(s[i+3]) & 63);
      out += String.fromCharCode((n >> 16) & 255);
      if (i + 2 < s.length) out += String.fromCharCode((n >> 8) & 255);
      if (i + 3 < s.length) out += String.fromCharCode(n & 255);
    }
    return out;
  };
}
if (typeof globalThis.btoa === 'undefined') {
  const B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  globalThis.btoa = function (s) {
    s = String(s);
    let out = '';
    for (let i = 0; i < s.length; i += 3) {
      const n = (s.charCodeAt(i) << 16)
              | ((i + 1 < s.length ? s.charCodeAt(i + 1) : 0) << 8)
              | (i + 2 < s.length ? s.charCodeAt(i + 2) : 0);
      out += B64[(n >> 18) & 63] + B64[(n >> 12) & 63]
           + (i + 1 < s.length ? B64[(n >> 6) & 63] : '=')
           + (i + 2 < s.length ? B64[n & 63] : '=');
    }
    return out;
  };
}

if (typeof globalThis.Buffer === 'undefined') {
  // A narrow `Buffer` covering the two calls the bundle makes:
  // `Buffer.from(str, encoding).toString(encoding)` and `Buffer.isBuffer`.
  // It is a subclass of Uint8Array so `instanceof` checks and byte access
  // behave; the encodings handled are utf8 and base64 (base64 via the
  // global `atob`, which deno_core provides).
  class OxideBuffer extends Uint8Array {
    static from(input, encoding) {
      if (typeof input === 'string') {
        if (encoding === 'base64') {
          const bin = globalThis.atob(input);
          const out = new OxideBuffer(bin.length);
          for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
          return out;
        }
        // utf8 (default) and latin1-ish fallback.
        const bytes = new TextEncoder().encode(input);
        const out = new OxideBuffer(bytes.length);
        out.set(bytes);
        return out;
      }
      if (input instanceof Uint8Array) {
        const out = new OxideBuffer(input.length);
        out.set(input);
        return out;
      }
      return new OxideBuffer(0);
    }
    static isBuffer(v) {
      return v instanceof OxideBuffer;
    }
    toString(encoding) {
      if (encoding === 'base64') {
        let s = '';
        for (let i = 0; i < this.length; i++) s += String.fromCharCode(this[i]);
        return globalThis.btoa(s);
      }
      return new TextDecoder().decode(this);
    }
  }
  globalThis.Buffer = OxideBuffer;
}

// Sec.4.5 op namespace. deno_core exposes ops as `Deno.core.ops.op_oxide_*`.
globalThis.oxide = {
  log: (msg) => Deno.core.ops.op_oxide_log(String(msg)),
  trace: (name) => Deno.core.ops.op_oxide_trace(String(name)),
  serialize: (value) => Deno.core.ops.op_oxide_serialize(JSON.stringify(value)),
  // Sec.11 / BUG-02: the cache tier the worker's `'use cache'` functions use.
  // Returns JSON strings (op2 string returns are the portable form).
  cacheGet: (key) => Deno.core.ops.op_oxide_cache_get(String(key)),
  cacheSet: (key, status, bodyB64, tagsCsv) =>
    Deno.core.ops.op_oxide_cache_set(
      String(key), Number(status) | 0, String(bodyB64), String(tagsCsv)),
  cacheTag: (tag) => Deno.core.ops.op_oxide_cache_tag(String(tag)),
  // PERF-05: the V8 fetch op (SSRF-hardened, host-provided).
  fetch: (method, url, headersJson, bodyB64) =>
    Deno.core.ops.op_oxide_fetch(String(method), String(url), String(headersJson), String(bodyB64)),
  // PERF-03: push a render chunk to the host mid-render. Returns "true" when
  // accepted, "false" when the consumer is gone (stop rendering).
  streamWrite: (bytes) => {
    const u8 = bytes instanceof Uint8Array ? bytes : new TextEncoder().encode(String(bytes));
    let bin = '';
    for (let i = 0; i < u8.length; i++) bin += String.fromCharCode(u8[i]);
    return Deno.core.ops.op_oxide_stream_write(btoa(bin));
  },
};

// Sec.4.5 / Sec.9.3: the bridge the oxide/server runtime ('use cache',
// cache()) calls to reach the host. Node has this as a frame+promise bridge
// over the worker pipe; the V8 backend exposes direct async ops, so this
// maps the same op names onto the ops above. Returns a promise of the op
// result (null on a cache miss). ASCII only (deno_core requires 7-bit).
globalThis.__oxide_op_request = async (op, args = {}) => {
  switch (String(op)) {
    case 'cache_get': {
      const raw = await Deno.core.ops.op_oxide_cache_get(String(args.key ?? ''));
      try { return JSON.parse(raw); } catch { return null; }
    }
    case 'cache_set': {
      const tags = Array.isArray(args.tags) ? args.tags.join(',') : '';
      const b64 = String(args.body_b64 ?? '');
      const status = Number(args.status ?? 200) | 0;
      const ok = await Deno.core.ops.op_oxide_cache_set(
        String(args.key ?? ''), status, b64, tags);
      return ok === 'true';
    }
    case 'cache_tag': {
      const raw = await Deno.core.ops.op_oxide_cache_tag(String(args.tag ?? ''));
      return Number(raw) || 0;
    }
    case 'compile_module': {
      // §6.2 / §15: same shape the Node worker's `compile_server_module`
      // expects. Returns `{ code }` or throws on `error`.
      const raw = await Deno.core.ops.op_oxide_compile_module(
        String(args.path ?? ''),
        String(args.code ?? ''),
        String(args.lang ?? 'ts'),
      );
      const v = JSON.parse(raw);
      if (v && typeof v.error === 'string') {
        throw new Error(v.error);
      }
      return v;
    }
    default:
      throw new Error(
        'oxide_op_request: op `' + String(op) + '` is not implemented on the V8 backend');
  }
};

// PERF-05: a `fetch()` for user code, routed through the op.
if (typeof globalThis.fetch === 'undefined') {
  globalThis.fetch = async function (input, init = {}) {
    const url = typeof input === 'string' ? input : (input && input.url) || String(input);
    const method = (init.method || (input && input.method) || 'GET').toUpperCase();
    const hdrs = init.headers || (input && input.headers) || {};
    const headersJson = JSON.stringify(
      hdrs && typeof hdrs.forEach === 'function'
        ? (() => { const o = {}; hdrs.forEach((v, k) => { o[k] = v; }); return o; })()
        : hdrs,
    );
    let bodyB64 = '';
    if (init.body != null) {
      const s = typeof init.body === 'string' ? init.body : String(init.body);
      bodyB64 = btoa(unescape(encodeURIComponent(s)));
    }
    const resJson = await globalThis.oxide.fetch(method, url, headersJson, bodyB64);
    const res = JSON.parse(resJson);
    const bin = atob(res.body_b64 || '');
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return new Response(bytes, { status: res.status, headers: new Headers(res.headers || []) });
  };
}

// Sec.4.2 / Sec.4.4: the real V8 dispatch. `moduleUrl` is a `file://` URL the
// runtime's `FsModuleLoader` resolves; `exportName` is the export to call
// (`default` when empty); `payload` is the JSON argument. The result is a
// JSON envelope string: `{ok, value}` on success, `{__oxide_error}` on a
// module-import or export-call failure. Returned as a string so the Rust
// side reads it back without a serde_v8 dependency.
globalThis.__oxideDispatch = async function (kind, moduleUrl, exportName, payload) {
  let mod;
  try {
    mod = await import(globalThis.__oxideVersioned(moduleUrl));
  } catch (e) {
    return JSON.stringify({
      __oxide_error: 'import failed for ' + moduleUrl + ': ' + ((e && e.message) || e),
    });
  }
  const fn = mod[exportName];
  if (typeof fn !== 'function') {
    return JSON.stringify({
      __oxide_error:
        'module ' + moduleUrl + ' has no callable export `' + exportName + '`',
    });
  }
  try {
    const value = await fn(payload);
    return JSON.stringify({ ok: true, value: value === undefined ? null : value });
  } catch (e) {
    return JSON.stringify({
      __oxide_error: exportName + ' threw: ' + ((e && e.message) || e),
    });
  }
};

// §15.2: JS plugin bridge for the V8 backend. Mirrors the Node worker's
// pluginCache / dispatchPluginLoad / dispatchPluginCall: import the module
// once by path, then call hooks positionally with the documented argument
// order. Without this the generic dispatch would call `mod[export](payload)`
// and pass one object where a plugin expects `(code, id)`.
const __oxidePlugins = new Map();
const __oxidePluginHookArgs = {
  transform: ['code', 'id'],
  resolveId: ['specifier', 'from'],
  load: ['id'],
  renderChunk: ['file', 'code'],
  generateBundle: ['files'],
  transformHtml: ['route', 'html'],
  config: ['config'],
  configResolved: ['config'],
  buildStart: [],
  buildEnd: ['result'],
  handleHotUpdate: ['ctx'],
};
globalThis.__oxidePluginLoad = async function (path) {
  try {
    const url = path.startsWith('file://')
      ? path
      : 'file://' + (path.startsWith('/') ? '' : '/') + path;
    const mod = await import(globalThis.__oxideVersioned(url));
    const plugin = mod.default ?? mod;
    const hooks = Object.keys(plugin).filter((k) => typeof plugin[k] === 'function');
    __oxidePlugins.set(path, plugin);
    return JSON.stringify({ hooks, name: plugin.name ?? null });
  } catch (e) {
    return JSON.stringify({
      __oxide_error: 'plugin import failed for ' + path + ': ' + ((e && e.message) || e),
      __oxide_code: 'PLG-LOAD',
    });
  }
};
globalThis.__oxidePluginCall = async function (path, hook, args) {
  const plugin = __oxidePlugins.get(path);
  if (!plugin) {
    return JSON.stringify({
      __oxide_error: 'plugin not loaded: ' + path + ' (call plugin:load first)',
      __oxide_code: 'PLG-LOAD',
    });
  }
  const fn = plugin[hook];
  if (typeof fn !== 'function') {
    return JSON.stringify({
      __oxide_error: 'plugin ' + path + ' has no `' + hook + '` hook',
      __oxide_code: 'PLG-HOOK',
    });
  }
  const order = __oxidePluginHookArgs[hook];
  const callArgs = order
    ? order.map((k) => (args ? args[k] : undefined))
    : Object.values(args ?? {});
  const ctx = {
    warn: (m) => console.error('[plugin]', m),
    error: (m) => { throw new Error(String(m)); },
    resolve: async (spec) => spec,
  };
  try {
    const value = await fn.call(ctx, ...callArgs);
    return JSON.stringify({ ok: true, value: value === undefined ? null : value });
  } catch (e) {
    return JSON.stringify({
      __oxide_error: 'plugin hook ' + hook + ' threw: ' + ((e && e.message) || e),
      __oxide_code: 'PLG-CALL',
    });
  }
};

// B-07: §8.3 parsed search params — mirrors the Node worker:
// `+`→space before percent-decode, repeated key → array, fragment stripped.
// Shared by the loader and action events (B-07 wired both).
globalThis.__oxideParseSearchParams = (url) => {
  const out = Object.create(null);
  if (typeof url !== 'string') return out;
  const q = url.indexOf('?');
  if (q < 0) return out;
  const hashAt = url.indexOf('#', q);
  const query = hashAt >= 0 ? url.slice(q + 1, hashAt) : url.slice(q + 1);
  if (!query) return out;
  const dec = (s) => { try { return decodeURIComponent(s); } catch { return s; } };
  for (const part of query.split('&')) {
    if (!part) continue;
    const eq = part.indexOf('=');
    const rawK = eq < 0 ? part : part.slice(0, eq);
    const rawV = eq < 0 ? '' : part.slice(eq + 1);
    const k = dec(rawK.replace(/\+/g, ' '));
    const v = dec(rawV.replace(/\+/g, ' '));
    if (!k) continue;
    if (k in out) {
      const prev = out[k];
      if (Array.isArray(prev)) prev.push(v); else out[k] = [prev, v];
    } else out[k] = v;
  }
  return out;
};

// loader:run — import a route's `page.server.ts` (transpiled by the Rust
// loader) and call its `load(event)`. Mirrors the Node worker's
// dispatchLoaderRun: a full LoadEvent (cookies, setHeaders, locals, locale,
// parent, defer) with `error()`/`redirect()` sentinels turned into status +
// location, and the defer registry awaited + returned as `deferred`.
globalThis.__oxideLoaderRun = async function (moduleUrl, input) {
  try {
    const mod = await import(globalThis.__oxideVersioned(moduleUrl));
    const load = mod.load;
    if (typeof load !== 'function') {
      return JSON.stringify({ __oxide_error: 'module has no callable `load` export' });
    }
    const registry = new Map();
    let nextId = 1;
    const defer = (p) => { const id = nextId++; registry.set(id, p); return { __deferred: id }; };

    const hdrs = Array.isArray(input.headers) ? input.headers : [];
    const inbound = hdrs.find((h) => Array.isArray(h) && String(h[0]).toLowerCase() === 'cookie');
    const jar = new Map();
    if (inbound) {
      for (const part of String(inbound[1]).split(';')) {
        const eq = part.indexOf('=');
        if (eq > 0) { const k = part.slice(0, eq).trim(); if (k) jar.set(k, decodeURIComponent(part.slice(eq + 1).trim())); }
      }
    }
    const pending = new Map();
    const ser = (n, v, o) => {
      o = o || {};
      let s = n + '=' + encodeURIComponent(v);
      if (o.path) s += '; Path=' + o.path;
      if (o.domain) s += '; Domain=' + o.domain;
      if (typeof o.maxAge === 'number') s += '; Max-Age=' + o.maxAge;
      if (o.httpOnly) s += '; HttpOnly';
      if (o.secure) s += '; Secure';
      if (o.sameSite) { const ss = String(o.sameSite); s += '; SameSite=' + ss[0].toUpperCase() + ss.slice(1).toLowerCase(); }
      return s;
    };
    const cookies = {
      get: (n) => jar.get(n),
      set: (n, v, o) => { jar.set(n, v); pending.set(n, ser(n, v, o)); },
      delete: (n) => { jar.delete(n); pending.set(n, ser(n, '', { maxAge: 0, path: '/' })); },
    };
    const extra = [];
    const setHeaders = (h) => { if (h && typeof h === 'object') for (const [k, v] of Object.entries(h)) extra.push([String(k), String(v)]); };

    // §8.1 / §9.1: locals from the payload (worker parity — this fork used to
    // hardcode `{}`).
    const locals = input.locals && typeof input.locals === 'object' ? input.locals : {};
    // §9.2: `parent()` returns the pre-resolved outer-layout data the Rust
    // whitelist forwards as `parent_data` (worker parity — was hardcoded {}).
    const parentData = input.parent_data || {};
    // SPEC-10 / §16.4: the pipeline's resolved locale (worker parity).
    const locale = typeof input.locale === 'string' ? input.locale : '';

    const event = {
      route: input.route || '',
      searchParams: globalThis.__oxideParseSearchParams(input.url || '/'),
      params: input.params || {},
      url: input.url || '/',
      request: { method: input.method || 'GET', url: input.url || '/', headers: hdrs },
      locals,
      locale,
      cookies,
      setHeaders,
      parent: async () => parentData,
      defer,
      __oxideDeferredRegistry: registry,
    };
    let result;
    try {
      result = await load(event);
    } catch (err) {
      if (err && typeof err.status === 'number') {
        return JSON.stringify({ ok: true, status: err.status, location: err.location ?? null,
          data: { message: err.message ?? '' }, setCookies: [...pending.values()], headers: extra });
      }
      throw err;
    }
    // B-03 / B2 (V8 parity): in streaming mode emit the initial payload at
    // loader speed, then one NDJSON settle line per `defer()` as it settles.
    // The host drives the returned promise (which awaits every settle) so the
    // event loop keeps running after the caller already has the stream.
    if (input && input.__oxide_stream) {
      const pendingIds = [...registry.keys()].map(String);
      Deno.core.ops.op_oxide_loader_emit(JSON.stringify({ ok: true, status: 200, location: null,
        deferred: {}, deferredPending: pendingIds, data: result ?? {},
        setCookies: [...pending.values()], headers: extra }));
      const jobs = [];
      for (const [id, p] of registry) {
        jobs.push(Promise.resolve(p).then(
          (value) => { Deno.core.ops.op_oxide_loader_emit(JSON.stringify({
            deferredSettle: { id: String(id), value: value === undefined ? null : value } }));
          },
          (e) => { Deno.core.ops.op_oxide_loader_emit(JSON.stringify({
            deferredSettle: { id: String(id), error: String(e?.message ?? e) } }));
          },
        ));
      }
      await Promise.all(jobs);
      return '';
    }
    // §7.1 / §9.2: await every `defer(promise)` marker and serialize
    // `{ id: value }`; a rejection surfaces as `{ __oxide_deferred_error }`
    // so a partially-ready loader still renders (worker parity — this fork
    // used to drop the registry entirely).
    const deferred = {};
    for (const [id, p] of registry) {
      try {
        deferred[String(id)] = await p;
      } catch (e) {
        deferred[String(id)] = { __oxide_deferred_error: String(e?.message ?? e) };
      }
    }
    return JSON.stringify({ ok: true, status: 200, location: null, deferred, data: result ?? {},
      setCookies: [...pending.values()], headers: extra });
  } catch (e) {
    return JSON.stringify({ __oxide_error: 'loader:run failed: ' + ((e && e.message) || e) });
  }
};

// framework:load — import the config (transpiled by the Rust loader), read the
// adapter descriptor off `default.framework`. Mirrors the Node worker.
globalThis.__oxideFrameworkLoad = async function (configPath) {
  try {
    const url = configPath.startsWith('file://')
      ? configPath
      : 'file://' + (configPath.startsWith('/') ? '' : '/') + configPath;
    const mod = await import(globalThis.__oxideVersioned(url));
    const cfg = (mod && mod.default) || mod;
    const fw = cfg && cfg.framework;
    if (!fw || typeof fw !== 'object') {
      return JSON.stringify({ __oxide_error: 'config has no framework export' });
    }
    return JSON.stringify({
      name: String(fw.name || ''),
      extensions: Array.isArray(fw.extensions) ? fw.extensions.map(String) : [],
      capabilities: fw.capabilities || {},
    });
  } catch (e) {
    return JSON.stringify({
      __oxide_error: 'framework:load failed: ' + ((e && e.message) || e),
    });
  }
};

// framework:render — call the adapter's `render(input)` and collect its
// ReadableStream to a string. (Streaming is a later refinement; the collected
// shape is what `FrameworkHost::render` returns.)
globalThis.__oxideFrameworkRender = async function (configPath, renderInput, components) {
  try {
    const url = configPath.startsWith('file://')
      ? configPath
      : 'file://' + (configPath.startsWith('/') ? '' : '/') + configPath;
    const mod = await import(globalThis.__oxideVersioned(url));
    const cfg = (mod && mod.default) || mod;
    const fw = cfg && cfg.framework;
    if (!fw || typeof fw.render !== 'function') {
      return JSON.stringify({ __oxide_error: 'config has no framework.render' });
    }
    // §6.4: resolve components from the build entry (prod) or the CLI's
    // source map (dev), never from a config closure. `components` arrives
    // as JSON; a malformed value degrades to the adapter's own fallback.
    let componentResolver = null;
    try {
      const cs = typeof components === 'string' ? JSON.parse(components) : components;
      if (cs && typeof cs.entry === 'string' && cs.entry.length > 0) {
        const entryUrl = cs.entry.startsWith('file://')
          ? cs.entry
          : 'file://' + (cs.entry.startsWith('/') ? '' : '/') + cs.entry;
        const entry = await import(globalThis.__oxideVersioned(entryUrl));
        const routes = entry.routes || {};
        // §6.4 / WP-A2: rolldown names entries after their sanitized file
        // basename (`[slug]` -> `_slug_`). The pipeline sends the source
        // shape. Accept both — a resolver miss on a bundled build surfaces
        // as a 500 on every dynamic route.
        const alias = (id) => {
          if (Object.prototype.hasOwnProperty.call(routes, id)) return id;
          const bracket = id.replace(/_([^_/]+)_/g, '[$1]');
          if (Object.prototype.hasOwnProperty.call(routes, bracket)) return bracket;
          const underscore = id.replace(/\[([^\]/]+)\]/g, '_$1_');
          if (Object.prototype.hasOwnProperty.call(routes, underscore)) return underscore;
          return id;
        };
        componentResolver = (id) => {
          const k = alias(id);
          if (!Object.prototype.hasOwnProperty.call(routes, k)) return undefined;
          const ns = routes[k];
          if (!ns || typeof ns !== 'object') return ns;
          return ns.default ?? ns.Page ?? ns.page ?? ns.Component ?? ns.component ?? ns;
        };
      } else if (cs && cs.sources && typeof cs.sources === 'object') {
        const routes = {};
        const rinput = renderInput || {};
        const wanted = [rinput.route, ...(Array.isArray(rinput.layouts) ? rinput.layouts : []), rinput.errorRoute]
          .filter((x) => typeof x === 'string' && x.length > 0);
        for (const rid of new Set(wanted)) {
          const src = cs.sources[rid];
          if (typeof src !== 'string' || src.length === 0) continue;
          const srcUrl = src.startsWith('file://') ? src : 'file://' + (src.startsWith('/') ? '' : '/') + src;
          routes[rid] = await import(globalThis.__oxideVersioned(srcUrl));
        }
        componentResolver = (id) => {
          if (!Object.prototype.hasOwnProperty.call(routes, id)) return undefined;
          const ns = routes[id];
          if (!ns || typeof ns !== 'object') return ns;
          return ns.default ?? ns.Page ?? ns.page ?? ns.Component ?? ns.component ?? ns;
        };
      }
    } catch (e) {
      return JSON.stringify({
        __oxide_error: 'component resolver failed: ' + ((e && e.message) || e),
      });
    }
    const rinput = renderInput || {};
    if (componentResolver) rinput.resolve = componentResolver;
    const stream = fw.render(rinput, componentResolver || undefined);
    // PERF-03: push each chunk to the host via `oxide.streamWrite`. When the
    // host is not streaming this call (no sink), streamWrite answers "false"
    // and we accumulate instead, so the buffered path still works.
    const enc = new TextEncoder();
    let html = '';
    let streamed = false;
    for await (const chunk of stream) {
      const s = typeof chunk === 'string' ? chunk : new TextDecoder().decode(chunk, { stream: true });
      const accepted = globalThis.oxide.streamWrite
        ? (await globalThis.oxide.streamWrite(enc.encode(s))) === 'true'
        : false;
      if (accepted) {
        streamed = true;
      } else {
        html += s;
      }
    }
    if (!streamed) {
      return JSON.stringify({ html });
    }
    return JSON.stringify({ streamed: true });
  } catch (e) {
    return JSON.stringify({
      __oxide_error: 'framework:render failed: ' + ((e && e.message) || e) +
        ' [reactLoadError=' + (globalThis.__oxideReactLoadError || 'none') + ']',
    });
  }
};

// BUG-04: V8 dispatch parity. The Node worker special-cases action:run,
// server-fn:run, generate-params:run and framework:invalidate (envelope
// semantics the generic mod[export](payload) arm cannot provide); these
// entry points port that logic to the V8 bootstrap so backend=v8
// mis-executes nothing.
//
// Module generations (§13.3): `__oxideFrameworkInvalidate` bumps
// `__oxideGeneration`; every entry point below imports through
// `__oxideVersioned`, and the Rust loader propagates the referrer's `?v=`
// to nested relative imports — so a bumped generation re-evaluates the
// whole graph, mirroring the Node worker's versioned() tagger.
globalThis.__oxideGeneration = 0;
globalThis.__oxideVersioned = function (url) {
  const g = globalThis.__oxideGeneration | 0;
  if (!g) return url;
  return url + (url.indexOf('?') >= 0 ? '&' : '?') + 'v=' + g;
};
globalThis.__oxideFrameworkInvalidate = async function () {
  globalThis.__oxideGeneration = (globalThis.__oxideGeneration | 0) + 1;
  return JSON.stringify({ generation: globalThis.__oxideGeneration });
};

// Minimal FormData for action dispatch when the host has none (the bootstrap
// provides Web globals piecemeal; FormData is not among them). Enough for
// `Object.fromEntries(form.entries())` and for user code reading the form.
function __oxideFormData(entries) {
  if (typeof globalThis.FormData === 'function') {
    const fd = new globalThis.FormData();
    for (const [k, v] of entries) fd.append(k, v);
    return fd;
  }
  const items = Array.from(entries);
  return {
    append: (k, v) => { items.push([String(k), String(v)]); },
    entries: function* () { for (const [k, v] of items) yield [k, v]; },
    get: (k) => { const f = items.find(([kk]) => kk === String(k)); return f ? f[1] : null; },
    has: (k) => items.some(([kk]) => kk === String(k)),
    [Symbol.iterator]: function* () { for (const [k, v] of items) yield [k, v]; },
  };
}

function __oxideCookieJar(hdrs) {
  const jar = new Map();
  const inbound = (Array.isArray(hdrs) ? hdrs : []).find(
    (h) => Array.isArray(h) && String(h[0]).toLowerCase() === 'cookie');
  if (inbound) {
    for (const part of String(inbound[1]).split(';')) {
      const eq = part.indexOf('=');
      if (eq > 0) { const k = part.slice(0, eq).trim(); if (k) jar.set(k, decodeURIComponent(part.slice(eq + 1).trim())); }
    }
  }
  const pending = new Map();
  const ser = (n, v, o) => {
    o = o || {};
    let s = n + '=' + encodeURIComponent(v);
    if (o.path) s += '; Path=' + o.path;
    if (o.domain) s += '; Domain=' + o.domain;
    if (typeof o.maxAge === 'number') s += '; Max-Age=' + o.maxAge;
    if (o.httpOnly) s += '; HttpOnly';
    if (o.secure) s += '; Secure';
    if (o.sameSite) { const ss = String(o.sameSite); s += '; SameSite=' + ss[0].toUpperCase() + ss.slice(1).toLowerCase(); }
    return s;
  };
  return {
    cookies: {
      get: (n) => jar.get(n),
      set: (n, v, o) => { jar.set(n, v); pending.set(n, ser(n, v, o)); },
      delete: (n) => { jar.delete(n); pending.set(n, ser(n, '', { maxAge: 0, path: '/' })); },
    },
    pending,
  };
}

// action:run — port of the Node worker's dispatchActionRun: cookies, Standard
// Schema input validation, revalidate collectors, error()/redirect() sentinel
// mapping. Envelope: {ok,status,location,data,setCookies,headers,revalidate}
// or {__oxide_error[, __oxide_code]} (NOT-FOUND codes survive so the pipeline
// maps them to 404, as with the Node backend).
globalThis.__oxideActionRun = async function (moduleUrl, input) {
  try {
    const mod = await import(globalThis.__oxideVersioned(moduleUrl));
    const name = input.action;
    if (typeof name !== 'string' || !name) {
      return JSON.stringify({ __oxide_error: 'action:run requires an `action` name', __oxide_code: 'JSH-ACTION' });
    }
    const handlers = mod.actions;
    if (!handlers || typeof handlers !== 'object') {
      return JSON.stringify({ __oxide_error: 'module has no `actions` export', __oxide_code: 'JSH-ACTION' });
    }
    const def = handlers[name];
    if (!def) {
      return JSON.stringify({ __oxide_error: 'no action named `' + name + '`', __oxide_code: 'JSH-ACTION-NOT-FOUND' });
    }
    const run = typeof def === 'function' ? def : def.run;
    if (typeof run !== 'function') {
      return JSON.stringify({ __oxide_error: 'action `' + name + '` has no run function', __oxide_code: 'JSH-ACTION' });
    }
    let entries;
    try {
      entries = input.form_json ? JSON.parse(input.form_json) : [];
    } catch (e) {
      return JSON.stringify({ __oxide_error: 'form body is not valid: ' + ((e && e.message) || e), __oxide_code: 'JSH-ACTION' });
    }
    const form = __oxideFormData(entries);
    const revalidateLog = [];
    const revalidate = {
      tag: (t) => revalidateLog.push({ type: 'tag', tag: String(t) }),
      path: (p) => revalidateLog.push({ type: 'path', path: String(p) }),
    };
    const { cookies, pending } = __oxideCookieJar(input.headers);
    const extra = [];
    const setHeaders = (h) => { if (h && typeof h === 'object') for (const [k, v] of Object.entries(h)) extra.push([String(k), String(v)]); };
    let validated;
    if (def && def.input && typeof def.input['~standard']?.validate === 'function') {
      const raw = Object.fromEntries(form.entries());
      const res = await def.input['~standard'].validate(raw);
      if (res.issues && res.issues.length > 0) {
        return JSON.stringify({ ok: true, status: 422, location: null,
          data: { message: res.issues.map((i) => i.message).join('; ') },
          setCookies: [...pending.values()], headers: extra, revalidate: revalidateLog });
      }
      validated = res.value;
    } else {
      validated = Object.fromEntries(form.entries());
    }
    const event = {
      params: input.params ?? {},
      searchParams: globalThis.__oxideParseSearchParams(input.url ?? '/'),
      request: { method: input.method ?? 'POST', url: input.url ?? '/', headers: Array.isArray(input.headers) ? input.headers : [] },
      locals: {},
      cookies,
      setHeaders,
      input: validated,
      form,
      revalidate,
    };
    let result;
    try {
      result = await run(event);
    } catch (err) {
      if (err && typeof err.status === 'number') {
        return JSON.stringify({ ok: true, status: err.status, location: err.location ?? null,
          data: { message: err.message ?? '' }, setCookies: [...pending.values()], headers: extra, revalidate: revalidateLog });
      }
      throw err;
    }
    return JSON.stringify({ ok: true, status: 200, location: null, data: result ?? {},
      setCookies: [...pending.values()], headers: extra, revalidate: revalidateLog });
  } catch (e) {
    return JSON.stringify({ __oxide_error: 'action:run failed: ' + ((e && e.message) || e), __oxide_code: 'JSH-ACTION' });
  }
};

// server-fn:run — port of dispatchServerFn. Imports the SSR entry graph (which
// runs `__oxide_register_fn` registrations into `globalThis.__oxide_fn_registry`
// via the imported `oxide/server` module), looks up fn_id, calls with the JSON
// args array. Codes (NOT-FOUND vs THREW) survive for the pipeline's mapping.
globalThis.__oxideServerFnRun = async function (entryUrl, fnId, argsJson) {
  try {
    if (typeof fnId !== 'string' || !fnId) {
      return JSON.stringify({ __oxide_error: 'server-fn requires a `fn_id` string', __oxide_code: 'JSH-FN' });
    }
    await import(globalThis.__oxideVersioned(entryUrl));
    const registry = globalThis.__oxide_fn_registry;
    if (!registry || typeof registry !== 'object') {
      return JSON.stringify({ __oxide_error: 'no server-fn registry: the SSR entry did not import `oxide/server`', __oxide_code: 'JSH-FN' });
    }
    const fn = registry[fnId];
    if (typeof fn !== 'function') {
      return JSON.stringify({ __oxide_error: 'no server function registered under id ' + fnId, __oxide_code: 'JSH-FN-NOT-FOUND' });
    }
    let args;
    try {
      args = argsJson ? JSON.parse(argsJson) : [];
    } catch (e) {
      return JSON.stringify({ __oxide_error: 'arguments are not valid JSON: ' + ((e && e.message) || e), __oxide_code: 'JSH-FN' });
    }
    if (!Array.isArray(args)) {
      return JSON.stringify({ __oxide_error: 'arguments must be a JSON array', __oxide_code: 'JSH-FN' });
    }
    try {
      const value = await fn(...args);
      return JSON.stringify({ value: value === undefined ? null : value });
    } catch (e) {
      return JSON.stringify({ __oxide_error: (e && e.message) || String(e), __oxide_code: 'JSH-FN-THREW' });
    }
  } catch (e) {
    return JSON.stringify({ __oxide_error: 'server-fn:run failed: ' + ((e && e.message) || e), __oxide_code: 'JSH-FN' });
  }
};

// generate-params:run — port of dispatchGenerateParamsRun. Returns the
// `{params:[...]}` envelope LoaderHost expects (never the generic
// `{ok,value}` — that mismatch is the JSH-PARAMS failure this fixes).
globalThis.__oxideGenerateParamsRun = async function (moduleUrl) {
  try {
    const mod = await import(globalThis.__oxideVersioned(moduleUrl));
    const gp = mod.generateParams;
    if (gp === undefined) {
      return JSON.stringify({ params: [] });
    }
    const value = typeof gp === 'function' ? await gp() : gp;
    if (!Array.isArray(value)) {
      return JSON.stringify({ __oxide_error: 'generateParams must return an array; got ' + (value === null ? 'null' : typeof value), __oxide_code: 'JSH-PARAMS' });
    }
    const params = value.map((item) => {
      if (item === null || typeof item !== 'object' || Array.isArray(item)) return {};
      const out = {};
      for (const [k, v] of Object.entries(item)) {
        if (v === null || v === undefined) continue;
        if (typeof v === 'object') continue;
        out[k] = typeof v === 'string' ? v : String(v);
      }
      return out;
    });
    return JSON.stringify({ params });
  } catch (e) {
    return JSON.stringify({ __oxide_error: 'generate-params:run failed: ' + ((e && e.message) || e), __oxide_code: 'JSH-PARAMS' });
  }
};
"#;

/// The single V8 extension every worker uses — and the one the startup
/// snapshot is built from.
///
/// It carries the bootstrap (`V8_BOOTSTRAP`) as a `js_file` and the oxide ops.
/// Both the snapshot build and the worker load construct it through this one
/// function, so the extension **name and op order match exactly**. deno_core
/// rejects a snapshot loaded with a differently-named extension
/// ("loaded in wrong order"), and a snapshot's op external-reference table is
/// keyed by that order.
pub(crate) fn oxide_extension() -> deno_core::Extension {
    use deno_core::ExtensionFileSource;
    use std::borrow::Cow;
    use std::sync::Arc;
    // deno_core requires extension source be 7-bit ASCII; the bootstrap's
    // comments carry `§`/`—`. Escaping each non-ASCII scalar to `\uXXXX` is
    // semantics-preserving in JS (cosmetic in a comment, identical elsewhere).
    let bootstrap_src = crate::snapshot::escape_non_ascii(V8_BOOTSTRAP);
    let bootstrap =
        ExtensionFileSource::new_computed("oxide:bootstrap", Arc::from(bootstrap_src.as_str()));
    deno_core::Extension {
        name: "oxide_runtime",
        deps: &[],
        js_files: Cow::Owned(vec![bootstrap]),
        esm_files: Cow::Borrowed(&[]),
        lazy_loaded_esm_files: Cow::Borrowed(&[]),
        lazy_loaded_js_files: Cow::Borrowed(&[]),
        synthetic_esm_modules: Cow::Borrowed(&[]),
        esm_entry_point: None,
        ops: Cow::Owned(crate::v8_ops::op_decls()),
        objects: Cow::Borrowed(&[]),
        external_references: Cow::Borrowed(&[]),
        global_template_middleware: None,
        global_object_middleware: None,
        op_state_fn: None,
        needs_lazy_init: false,
        middleware_fn: None,
        enabled: true,
    }
}

/// Messages from the pool to a V8 worker thread.
enum V8Msg {
    Call {
        id: u64,
        entry: EntryRef,
        input: CallInput,
    },
    /// Sent by the pool during warm replacement (§4.3). The worker exits its
    /// recv loop after handling any already-dequeued call.
    Shutdown,
}

/// Handle to a V8 worker thread running a pinned `JsRuntime` (§4.2 default).
struct V8WorkerHandle {
    tx: mpsc::Sender<V8Msg>,
    in_flight: AtomicU64,
    served: AtomicU64,
    /// Shared with `abort_request` so pending calls can be resolved without
    /// re-entering the worker thread.
    pending: Mutex<HashMap<u64, Pending>>,
    /// Set while this worker is being replaced (§4.3). Dispatch skips retiring
    /// workers so the fresh one takes over immediately.
    retiring: AtomicBool,
    /// BUG-03: set when a send to this worker's channel fails — the worker
    /// thread is gone. `choose_worker` treats a dead worker like a retiring
    /// one, and the failing call spawns a warm replacement, so the pool
    /// recovers instead of failing every call forever.
    dead: AtomicBool,
}

fn spawn_v8_worker(
    snapshot: Option<&'static [u8]>,
    cache: Option<Arc<dyn oxide_fw_cache::CacheHandler>>,
    fetch_handler: Option<Arc<dyn FetchHandler>>,
    transform_handler: Option<Arc<dyn crate::TransformHandler>>,
) -> Result<Arc<V8WorkerHandle>> {
    let (tx, mut rx) = mpsc::channel::<V8Msg>(64);
    let (init_tx, init_rx) = std::sync::mpsc::channel::<Result<()>>();
    let handle = Arc::new(V8WorkerHandle {
        tx,
        in_flight: AtomicU64::new(0),
        served: AtomicU64::new(0),
        pending: Mutex::new(HashMap::new()),
        retiring: AtomicBool::new(false),
        dead: AtomicBool::new(false),
    });
    let handle_for_thread = Arc::clone(&handle);

    std::thread::Builder::new()
        .name("oxide-v8-worker".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    let _ = init_tx.send(Err(err("JSH-V8", format!("tokio rt: {e}"))));
                    return;
                }
            };
            // BUG-02: install the cache handler for this thread BEFORE the
            // runtime evaluates any user module, so `oxide.cacheGet/Set/Tag`
            // see it. Thread-local because a `JsRuntime` is `!Send`.
            crate::v8_ops::set_cache_handler(cache);
            crate::v8_ops::set_fetch_handler(fetch_handler);
            crate::v8_ops::set_transform_handler(transform_handler);
            let local = tokio::task::LocalSet::new();
            local.block_on(&rt, async move {
                // The one shared extension: bootstrap js_file + oxide ops.
                // Identical to what `snapshot_build_runtime` snapshots, so the
                // name and op order match when a blob is restored.
                let ext = crate::oxide_extension();
                // §4.2: a real `ModuleLoader` so the runtime can evaluate an
                // ESM bundle. `FsModuleLoader` resolves `file://` specifiers
                // and reads them from disk — which is exactly what a
                // self-contained `dist/server` bundle needs. (`Rc` is fine:
                // `JsRuntime` is `!Send` and pinned to this thread.)
                // §4.2: a transpiling loader — reads `.ts`/`.tsx`, runs the
                // Oxc type-strip + JSX lowering, resolves bare specifiers
                // against node_modules. `FsModuleLoader` alone cannot parse a
                // `.ts` config; this can.
                let module_loader: std::rc::Rc<dyn deno_core::ModuleLoader> =
                    std::rc::Rc::new(crate::v8_loader::TranspileLoader::new(
                        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
                        None,
                    ));
                let mut js = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // §4.4: a snapshot already holds the evaluated bootstrap
                    // (process/Buffer/oxide.*/__oxideDispatch) and its module
                    // graph. Loading it means the extension list and loader
                    // are NOT re-supplied — the heap is restored verbatim.
                    // A snapshot restores the evaluated heap, but deno_core
                    // still needs the SAME extension (for its op external-
                    // reference table) and `skip_op_registration: true`, or the
                    // op count baked into the blob will not match the runtime's
                    // (`range end index N out of range`). This mirrors
                    // `create_snapshot`'s own warmup-load pattern.
                    JsRuntime::new(RuntimeOptions {
                        startup_snapshot: snapshot,
                        extensions: vec![ext],
                        skip_op_registration: snapshot.is_some(),
                        module_loader: Some(module_loader),
                        ..Default::default()
                    })
                })) {
                    Ok(r) => r,
                    Err(_) => {
                        let _ = init_tx.send(Err(err("JSH-V8", "JsRuntime::new panicked")));
                        return;
                    }
                };
                let _ = init_tx.send(Ok(()));

                while let Some(msg) = rx.recv().await {
                    match msg {
                        V8Msg::Call { id, entry, input } => {
                            let res = v8_dispatch(&mut js, &entry, &input).await;
                            // PERF-03: if this call opened a streaming render,
                            // hand the receiver to the caller FIRST, then drive
                            // the stashed promise so chunks flow while it runs.
                            let streaming =
                                crate::v8_ops::PENDING_STREAM.with(|s| s.borrow_mut().take());
                            let pending = handle_for_thread.pending.lock().unwrap().remove(&id);
                            if let Some(p) = pending {
                                let _ = p.tx.send(res);
                            }
                            if let Some(promise) = streaming {
                                let resolve = js.resolve(promise);
                                let resolved = js
                                    .with_event_loop_promise(
                                        resolve,
                                        deno_core::PollEventLoopOptions::default(),
                                    )
                                    .await;
                                if let Ok(value) = resolved {
                                    let text = {
                                        deno_core::scope!(scope, js);
                                        deno_core::v8::Local::new(scope, value)
                                            .to_rust_string_lossy(scope)
                                    };
                                    crate::v8_ops::finish_loader_stream(text);
                                }
                                // Drop the sinks: closes the chunk stream.
                                crate::v8_ops::set_stream_sink(None);
                                crate::v8_ops::set_loader_emit_sink(None);
                            }
                        }
                        V8Msg::Shutdown => {
                            // §4.3: the pool has already routed new work to a
                            // fresh worker; this thread drains and exits, which
                            // drops the JsRuntime and its OS thread.
                            break;
                        }
                    }
                }
            });
        })
        .map_err(|e| err("JSH-V8", format!("spawn thread: {e}")))?;

    match init_rx.recv_timeout(Duration::from_secs(30)) {
        Ok(Ok(())) => Ok(handle),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(err(
            "JSH-V8",
            "worker failed to initialize within 30s (first build may be downloading V8)",
        )),
    }
}

/// Build a `file://` URL for a module path so the `FsModuleLoader` can resolve
/// it. A relative path is joined onto the current directory first.
fn v8_module_url(path: &str) -> Result<String> {
    let p = std::path::Path::new(path);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| err("JSH-V8", format!("cwd: {e}")))?
            .join(p)
    };
    deno_core::ModuleSpecifier::from_file_path(&abs)
        .map(|u| u.to_string())
        .map_err(|_| err("JSH-V8", format!("not a file path: {}", abs.display())))
}

/// §4.2 / §4.4: dispatch a call into the V8 runtime. Imports `entry.module`
/// (a real ESM bundle) via the runtime's `FsModuleLoader`, calls `entry.export`
/// with `input.payload`, and returns the JSON result. The call is async — a
/// dynamic `import()` and a user async function both suspend — so the worker
/// loop drives the V8 event loop through the returned promise.
/// Execute `script` (a call into a `globalThis.__oxide*` bootstrap entry
/// point returning a JSON envelope string) and parse the envelope.
/// `site` names the call for diagnostics; `default_code` is the error code
/// when the envelope carries a message but no `__oxide_code`.
async fn exec_v8_json(
    js: &mut JsRuntime,
    site: &'static str,
    script: String,
    default_code: &'static str,
) -> Result<serde_json::Value> {
    // `execute_script` takes `impl IntoModuleName` for the name and
    // `impl IntoModuleCodeString` for the code. A `String` satisfies both
    // (`site` is a `&str` param, not `&'static str`; `script` is owned).
    let promise = js
        .execute_script(site.to_string(), script)
        .map_err(|e| err("JSH-V8", format!("{site} dispatch: {e}")))?;
    let resolve = js.resolve(promise);
    let resolved = js
        .with_event_loop_promise(resolve, deno_core::PollEventLoopOptions::default())
        .await
        .map_err(|e| err("JSH-V8", format!("{site} await: {e}")))?;
    let s = {
        deno_core::scope!(scope, js);
        let local = deno_core::v8::Local::new(scope, resolved);
        local.to_rust_string_lossy(scope)
    };
    let v: serde_json::Value = serde_json::from_str(&s)
        .map_err(|e| err("JSH-V8", format!("{site} returned non-JSON: {e}")))?;
    if let Some(m) = v.get("__oxide_error").and_then(|x| x.as_str()) {
        // Codes cross the JS boundary as strings but `err` needs
        // `&'static str` — allow-list the codes the bootstrap can emit so
        // NOT-FOUND vs THREW survives for the pipeline's HTTP mapping.
        let code: &'static str = match v.get("__oxide_code").and_then(|x| x.as_str()) {
            Some("JSH-ACTION-NOT-FOUND") => "JSH-ACTION-NOT-FOUND",
            Some("JSH-FN-NOT-FOUND") => "JSH-FN-NOT-FOUND",
            Some("JSH-FN-THREW") => "JSH-FN-THREW",
            Some("JSH-ACTION") => "JSH-ACTION",
            Some("JSH-FN") => "JSH-FN",
            Some("JSH-PARAMS") => "JSH-PARAMS",
            _ => default_code,
        };
        return Err(err(code, m.to_string()));
    }
    Ok(v)
}

async fn v8_dispatch(
    js: &mut JsRuntime,
    entry: &EntryRef,
    input: &CallInput,
) -> Result<CallStream> {
    // Kinds the Node worker special-cases are handled Rust-side so semantics
    // match without burdening the JS dispatch.
    if entry.kind == "boom" {
        return Err(err(
            "JSH-BOOM",
            format!("worker simulated failure for kind `{}`", entry.kind),
        ));
    }
    if entry.kind == "die" {
        return Err(err("JSH-CRASH", "worker simulated crash (kind `die`)"));
    }
    // §9.1: the loader op imports `entry.module` (a page.server.ts) and calls
    // its `load(event)`. The envelope is the LoaderOutput shape
    // (`{status, location, data, …}`), not the generic `{ok, value}` — handle
    // it before the generic path.
    if entry.kind == "loader:run" {
        let url = v8_module_url(&entry.module)?;
        let u = serde_json::to_string(&url).unwrap_or_else(|_| "\"\"".into());
        let inp = serde_json::to_string(&input.payload).unwrap_or_else(|_| "{}".into());
        let script = format!("globalThis.__oxideLoaderRun({u}, {inp})");
        // B-03: streaming loaders return their chunk stream immediately; the
        // worker loop drives the promise (settles are emitted as late lines).
        let streaming_loader = input
            .payload
            .get("__oxide_stream")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let promise = js
            .execute_script("oxide:loader", script)
            .map_err(|e| err("JSH-V8", format!("loader dispatch: {e}")))?;
        if streaming_loader {
            let (tx, rx) = mpsc::channel::<Result<Bytes>>(16);
            crate::v8_ops::set_loader_emit_sink(Some(tx));
            crate::v8_ops::PENDING_STREAM.with(|slot| {
                *slot.borrow_mut() = Some(promise);
            });
            return Ok(CallStream::Chunked(rx));
        }
        let resolve = js.resolve(promise);
        let resolved = js
            .with_event_loop_promise(resolve, deno_core::PollEventLoopOptions::default())
            .await
            .map_err(|e| err("JSH-V8", format!("loader await: {e}")))?;
        let s = {
            deno_core::scope!(scope, js);
            let local = deno_core::v8::Local::new(scope, resolved);
            local.to_rust_string_lossy(scope)
        };
        let v: serde_json::Value = serde_json::from_str(&s)
            .map_err(|e| err("JSH-V8", format!("loader returned non-JSON: {e}")))?;
        if let Some(m) = v.get("__oxide_error").and_then(|x| x.as_str()) {
            return Err(err("JSH-LOADER", m.to_string()));
        }
        let bytes = serde_json::to_vec(&v).unwrap_or_default();
        return Ok(CallStream::Complete(Bytes::from(bytes)));
    }

    // The framework ops take their module from the payload's `path`, not from
    // `entry.module` (the Node worker's shape). Route them to their dedicated
    // JS entry points.
    if entry.kind == "framework:load" || entry.kind == "framework:render" {
        let path = input
            .payload
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if path.is_empty() {
            return Err(err(
                "JSH-FRAMEWORK",
                format!("{} requires a `path` string", entry.kind),
            ));
        }
        let script = if entry.kind == "framework:load" {
            let p = serde_json::to_string(&path).unwrap_or_else(|_| "\"\"".into());
            format!("globalThis.__oxideFrameworkLoad({p})")
        } else {
            let p = serde_json::to_string(&path).unwrap_or_else(|_| "\"\"".into());
            let inp = serde_json::to_string(
                input
                    .payload
                    .get("input")
                    .unwrap_or(&serde_json::Value::Null),
            )
            .unwrap_or_else(|_| "null".into());
            let comps = serde_json::to_string(
                input
                    .payload
                    .get("components")
                    .unwrap_or(&serde_json::Value::Null),
            )
            .unwrap_or_else(|_| "null".into());
            format!("globalThis.__oxideFrameworkRender({p}, {inp}, {comps})")
        };
        // PERF-03: `framework:render` streams. Install a sink, execute the
        // render (which returns a promise), stash that promise, and return a
        // `Chunked` stream NOW — the worker loop drives the promise to
        // completion *after* the caller has the receiver, so chunks flow live.
        // `framework:load` stays buffered (a small descriptor).
        if entry.kind == "framework:render" {
            let promise = js
                .execute_script("oxide:framework", script)
                .map_err(|e| err("JSH-V8", format!("framework dispatch: {e}")))?;
            let (tx, rx) = mpsc::channel::<Result<Bytes>>(16);
            crate::v8_ops::set_stream_sink(Some(tx));
            crate::v8_ops::PENDING_STREAM.with(|slot| {
                *slot.borrow_mut() = Some(promise);
            });
            return Ok(CallStream::Chunked(rx));
        }

        let promise = js
            .execute_script("oxide:framework", script)
            .map_err(|e| err("JSH-V8", format!("framework dispatch: {e}")))?;
        let resolve = js.resolve(promise);
        let resolved = js
            .with_event_loop_promise(resolve, deno_core::PollEventLoopOptions::default())
            .await
            .map_err(|e| err("JSH-V8", format!("framework await: {e}")))?;
        let s = {
            deno_core::scope!(scope, js);
            let local = deno_core::v8::Local::new(scope, resolved);
            local.to_rust_string_lossy(scope)
        };
        let v: serde_json::Value = serde_json::from_str(&s)
            .map_err(|e| err("JSH-V8", format!("framework returned non-JSON: {e}")))?;
        if let Some(m) = v.get("__oxide_error").and_then(|x| x.as_str()) {
            return Err(err("JSH-FRAMEWORK", m.to_string()));
        }
        let out = serde_json::to_vec(&v).unwrap_or_default();
        return Ok(CallStream::Complete(Bytes::from(out)));
    }

    // BUG-04: V8 dispatch parity for the envelope-semantic kinds. The Node
    // worker implements these with dedicated entry points (cookies/defer/
    // revalidate for actions; {ok,value} vs {params} for generate-params;
    // generation bump for invalidate); the generic mod[export](payload) arm
    // below silently mis-executes them. Each maps to its __oxide* bootstrap
    // entry point (same logic as oxide-worker.mjs).
    if entry.kind == "action:run" {
        let url = v8_module_url(&entry.module)?;
        let u = serde_json::to_string(&url).unwrap_or_else(|_| "\"\"".into());
        let inp = serde_json::to_string(&input.payload).unwrap_or_else(|_| "{}".into());
        let v = exec_v8_json(
            js,
            "oxide:action",
            format!("globalThis.__oxideActionRun({u}, {inp})"),
            "JSH-ACTION",
        )
        .await?;
        let bytes = serde_json::to_vec(&v).unwrap_or_default();
        return Ok(CallStream::Complete(Bytes::from(bytes)));
    }
    if entry.kind == "server-fn:run" {
        let fn_id = input
            .payload
            .get("fn_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let entry_path = input
            .payload
            .get("entry_path")
            .and_then(|v| v.as_str())
            .unwrap_or(&entry.module);
        let url = v8_module_url(entry_path)?;
        let u = serde_json::to_string(&url).unwrap_or_else(|_| "\"\"".into());
        let f = serde_json::to_string(fn_id).unwrap_or_else(|_| "\"\"".into());
        // args_json arrives as a JSON *string* (the array text); pass it as
        // a JS string literal — the bootstrap JSON.parses it, as Node does.
        let args_expr = serde_json::to_string(
            input
                .payload
                .get("args_json")
                .and_then(|v| v.as_str())
                .unwrap_or("[]"),
        )
        .unwrap_or_else(|_| "\"[]\"".into());
        let v = exec_v8_json(
            js,
            "oxide:server-fn",
            format!("globalThis.__oxideServerFnRun({u}, {f}, {args_expr})"),
            "JSH-FN",
        )
        .await?;
        let bytes = serde_json::to_vec(&v).unwrap_or_default();
        return Ok(CallStream::Complete(Bytes::from(bytes)));
    }
    if entry.kind == "generate-params:run" {
        let url = v8_module_url(&entry.module)?;
        let u = serde_json::to_string(&url).unwrap_or_else(|_| "\"\"".into());
        let v = exec_v8_json(
            js,
            "oxide:generate-params",
            format!("globalThis.__oxideGenerateParamsRun({u})"),
            "JSH-PARAMS",
        )
        .await?;
        let bytes = serde_json::to_vec(&v).unwrap_or_default();
        return Ok(CallStream::Complete(Bytes::from(bytes)));
    }
    if entry.kind == "framework:invalidate" {
        let v = exec_v8_json(
            js,
            "oxide:invalidate",
            "globalThis.__oxideFrameworkInvalidate()".to_string(),
            "JSH-FRAMEWORK",
        )
        .await?;
        let bytes = serde_json::to_vec(&v).unwrap_or_default();
        return Ok(CallStream::Complete(Bytes::from(bytes)));
    }
    if entry.kind == "plugin:load" {
        let plugin_path = input
            .payload
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if plugin_path.is_empty() {
            return Err(err("PLG-LOAD", "plugin:load requires a `path`"));
        }
        let p = serde_json::to_string(plugin_path).unwrap_or_else(|_| "\"\"".into());
        let v = exec_v8_json(
            js,
            "oxide:plugin-load",
            format!("globalThis.__oxidePluginLoad({p})"),
            "PLG-LOAD",
        )
        .await?;
        let bytes = serde_json::to_vec(&v).unwrap_or_default();
        return Ok(CallStream::Complete(Bytes::from(bytes)));
    }
    if entry.kind == "plugin:call" {
        let path = input
            .payload
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let hook = input
            .payload
            .get("hook")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if path.is_empty() || hook.is_empty() {
            return Err(err("PLG-CALL", "plugin:call requires `path` and `hook`"));
        }
        let p = serde_json::to_string(path).unwrap_or_else(|_| "\"\"".into());
        let h = serde_json::to_string(hook).unwrap_or_else(|_| "\"\"".into());
        let args = input
            .payload
            .get("args")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let a = serde_json::to_string(&args).unwrap_or_else(|_| "null".into());
        let v = exec_v8_json(
            js,
            "oxide:plugin-call",
            format!("globalThis.__oxidePluginCall({p}, {h}, {a})"),
            "PLG-CALL",
        )
        .await?;
        let out = v.get("value").cloned().unwrap_or(serde_json::Value::Null);
        let bytes = serde_json::to_vec(&out).unwrap_or_default();
        return Ok(CallStream::Complete(Bytes::from(bytes)));
    }
    if entry.kind == "tooling:module" {
        // The SFC compilers are npm packages needing Node built-ins; they
        // run on the Node tooling pool even when the render backend is V8
        // (framework_shim). Reaching V8 is a wiring error — refuse loudly
        // (JSH-V8KIND) rather than mis-executing.
        return Err(err(
            "JSH-V8KIND",
            "tooling:module is not available on the V8 backend; use the Node tooling pool",
        ));
    }

    if entry.module.is_empty() {
        return Err(err(
            "JSH-V8",
            format!("kind `{}` requires a `module` path", entry.kind),
        ));
    }

    let module_url = v8_module_url(&entry.module)?;
    let export_name = if entry.export.is_empty() {
        "default"
    } else {
        entry.export.as_str()
    };

    let kind = serde_json::to_string(&entry.kind).unwrap_or_else(|_| "\"\"".into());
    let url = serde_json::to_string(&module_url).unwrap_or_else(|_| "\"\"".into());
    let exp = serde_json::to_string(export_name).unwrap_or_else(|_| "\"\"".into());
    let payload = serde_json::to_string(&input.payload).unwrap_or_else(|_| "null".into());
    let script = format!("globalThis.__oxideDispatch({kind}, {url}, {exp}, {payload})");

    // `__oxideDispatch` returns a promise; drive the event loop until it
    // settles, then read the resolved string.
    let promise = js
        .execute_script("oxide:call", script)
        .map_err(|e| err("JSH-V8", format!("dispatch failed: {e}")))?;
    // Drive the event loop until the dispatch promise settles — the same
    // two-statement shape deno_core's own `resolve_value` uses (a nested
    // `js.with_event_loop_promise(js.resolve(..), ..)` borrows `js` twice).
    let resolve = js.resolve(promise);
    let resolved = js
        .with_event_loop_promise(resolve, deno_core::PollEventLoopOptions::default())
        .await
        .map_err(|e| err("JSH-V8", format!("dispatch await failed: {e}")))?;
    let s = {
        deno_core::scope!(scope, js);
        let local = deno_core::v8::Local::new(scope, resolved);
        local.to_rust_string_lossy(scope)
    };

    // The envelope is `{ok, value}` or `{__oxide_error}`. A surfaced JS error
    // is a coded dispatch failure, not a transport failure.
    let v: serde_json::Value = serde_json::from_str(&s)
        .map_err(|e| err("JSH-V8", format!("dispatch returned non-JSON: {e}: {s}")))?;
    if let Some(msg) = v.get("__oxide_error").and_then(|x| x.as_str()) {
        return Err(err("JSH-V8", msg.to_string()));
    }
    let out = v.get("value").cloned().unwrap_or(serde_json::Value::Null);
    let bytes =
        serde_json::to_vec(&out).map_err(|e| err("JSH-V8", format!("serialize result: {e}")))?;
    Ok(CallStream::Complete(Bytes::from(bytes)))
}

/// V8 backend (§4.2 default, via deno_core).
///
/// One `JsRuntime` per worker thread, each pinned (`JsRuntime` is `!Send`);
/// calls cross the thread boundary through an mpsc channel plus a shared
/// pending map. A call imports its `entry.module` through the runtime's
/// `FsModuleLoader` and invokes `entry.export` (default `default`) with the
/// JSON payload, awaiting the result on the V8 event loop. Mirrors the Node
/// worker's `boom` / `die` sentinels.
pub struct V8Host {
    /// §4.4: an optional V8 startup snapshot blob. When set, each worker
    /// restores this heap instead of evaluating the bootstrap — skipping V8
    /// init, the bootstrap script, and the module graph's evaluation. Built by
    /// `oxide build` (`snapshot_build_bundle`) and read from disk at start.
    snapshot: Option<std::sync::Arc<Vec<u8>>>,
    /// §9.3/§11 (BUG-02): the cache tier the worker's `'use cache'` functions
    /// reach through `oxide.cacheGet/Set/Tag`. `None` means those ops answer
    /// "no cache" (a miss / a no-op write) — honest, and the pre-BUG-02
    /// behavior.
    cache_handler: Option<std::sync::Arc<dyn oxide_fw_cache::CacheHandler>>,
    /// PERF-05: the fetch handler each worker installs for the `fetch` op.
    fetch_handler: Option<std::sync::Arc<dyn FetchHandler>>,
    /// §6.2 / §15: the plugin `transform` hook applied to every module the
    /// worker loads (reaching the prerender path).
    transform_handler: Option<std::sync::Arc<dyn TransformHandler>>,
    /// §4.4 / BUG-03: the leaked `&'static` view of `snapshot`, computed
    /// **once**. Before this, `static_snapshot()` leaked a fresh copy on every
    /// call (`render_pool` + `tooling_pool` + each pool rebuild), an unbounded
    /// leak.
    leaked: std::sync::OnceLock<Option<&'static [u8]>>,
}

impl Default for V8Host {
    fn default() -> Self {
        Self {
            snapshot: None,
            cache_handler: None,
            fetch_handler: None,
            transform_handler: None,
            leaked: std::sync::OnceLock::new(),
        }
    }
}

impl V8Host {
    /// A host with no snapshot — each worker evaluates the bootstrap fresh.
    pub fn new() -> Self {
        Self::default()
    }

    /// A host that restores `blob` in every worker (§4.4). The bytes are
    /// leaked once (they must outlive every worker thread, which V8 requires
    /// be `&'static`).
    pub fn with_snapshot(blob: Vec<u8>) -> Self {
        Self {
            snapshot: Some(std::sync::Arc::new(blob)),
            cache_handler: None,
            fetch_handler: None,
            transform_handler: None,
            leaked: std::sync::OnceLock::new(),
        }
    }

    /// §9.3/§11 (BUG-02): attach the cache tier. Mirrors
    /// `NodeHost::with_cache_handler`; the handler reaches each worker's
    /// `oxide.cacheGet/Set/Tag` ops through a thread-local installed at spawn.
    pub fn with_cache_handler(
        mut self,
        handler: std::sync::Arc<dyn oxide_fw_cache::CacheHandler>,
    ) -> Self {
        self.cache_handler = Some(handler);
        self
    }

    /// PERF-05: attach the fetch handler each worker installs for the `fetch` op.
    pub fn with_fetch_handler(mut self, handler: std::sync::Arc<dyn FetchHandler>) -> Self {
        self.fetch_handler = Some(handler);
        self
    }

    /// §6.2 / §15: attach the plugin `transform` hook applied to every
    /// module the worker's `compile_module` op loads (so a plugin transform
    /// reaches the prerender path, not just the bundler).
    pub fn with_transform_handler(mut self, handler: std::sync::Arc<dyn TransformHandler>) -> Self {
        self.transform_handler = Some(handler);
        self
    }

    /// Read a versioned snapshot envelope from `path` and build a host
    /// around it (BUG-12). A missing file yields a snapshot-less host.
    /// A corrupt/mismatched envelope is LOUD (stderr warn with `JSH-SNAPVER`
    /// naming expected vs found) and downgrades to no-snapshot — the caller
    /// still works (SPEC-01 completes the code-cache fallback ladder).
    pub fn from_snapshot_file(path: &std::path::Path) -> Self {
        match Self::load_snapshot_envelope(path) {
            Ok(Some(blob)) => Self::with_snapshot(blob),
            Ok(None) => Self::default(),
            Err(e) => {
                eprintln!(
                    "warn: snapshot unavailable ({} {}); continuing without snapshot (code-cache fallback: SPEC-01)",
                    e.0.code, e.0.message
                );
                Self::default()
            }
        }
    }

    /// Parse the `AppSnapshot` envelope written by `oxide build` (BUG-12).
    /// `Ok(None)` = file missing (no snapshot, not an error).
    /// `Err` = present-but-unusable (`JSH-SNAPVER`), including old raw blobs.
    pub fn load_snapshot_envelope(path: &std::path::Path) -> Result<Option<Vec<u8>>> {
        use base64::Engine as _;
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => return Ok(None),
        };
        let env: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
            err(
                "JSH-SNAPVER",
                format!(
                    "snapshot at {} is not a versioned envelope (stale raw blob?); expected format {} / oxide {}",
                    path.display(),
                    crate::SNAPSHOT_FORMAT,
                    env!("CARGO_PKG_VERSION"),
                ),
            )
        })?;
        let format = env
            .get("format")
            .and_then(|v| v.as_u64())
            .unwrap_or(u64::MAX);
        let version = env
            .get("oxide_version")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing>");
        if format != crate::SNAPSHOT_FORMAT as u64 || version != env!("CARGO_PKG_VERSION") {
            return Err(err(
                "JSH-SNAPVER",
                format!(
                    "snapshot at {} mismatch: found format {}/oxide {version}; expected format {}/oxide {}",
                    path.display(),
                    format,
                    crate::SNAPSHOT_FORMAT,
                    env!("CARGO_PKG_VERSION"),
                ),
            ));
        }
        let b64 = env.get("blob_b64").and_then(|v| v.as_str()).unwrap_or("");
        let blob = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| err("JSH-SNAPVER", format!("snapshot blob_b64 invalid: {e}")))?;
        if blob.is_empty() {
            return Ok(None);
        }
        Ok(Some(blob))
    }

    /// The `&'static` view of the snapshot, if any. Leaks the bytes on first
    /// call; a host is built once per process, so this is bounded.
    fn static_snapshot(&self) -> Option<&'static [u8]> {
        // BUG-03: leak ONCE. `OnceLock` makes the `&'static` view stable for
        // the host's lifetime, so repeated `build_pool` calls (render pool +
        // tooling pool + every warm replacement) share one allocation.
        *self.leaked.get_or_init(|| {
            self.snapshot.as_ref().map(|b| {
                let leaked: &'static [u8] = Box::leak(b.as_slice().to_vec().into_boxed_slice());
                leaked
            })
        })
    }

    fn build_pool(
        cfg: PoolConfig,
        snapshot: Option<&'static [u8]>,
        cache: Option<std::sync::Arc<dyn oxide_fw_cache::CacheHandler>>,
        fetch_handler: Option<std::sync::Arc<dyn FetchHandler>>,
        transform_handler: Option<std::sync::Arc<dyn crate::TransformHandler>>,
    ) -> Result<Arc<V8Pool>> {
        let mut workers = Vec::with_capacity(cfg.size.max(1));
        for _ in 0..cfg.size.max(1) {
            workers.push(spawn_v8_worker(
                snapshot,
                cache.clone(),
                fetch_handler.clone(),
                transform_handler.clone(),
            )?);
        }
        Ok(Arc::new(V8Pool {
            cfg,
            workers: Arc::new(std::sync::RwLock::new(workers)),
            next_id: AtomicU64::new(0),
            counters: Arc::new(PoolCounters::default()),
            queue_waits: Mutex::new(VecDeque::new()),
            draining: AtomicBool::new(false),
            snapshot,
            cache_handler: cache,
            fetch_handler,
            transform_handler,
        }))
    }
}

impl JsHost for V8Host {
    fn render_pool(&self, cfg: PoolConfig) -> Result<Arc<dyn Pool>> {
        Self::build_pool(
            cfg,
            self.static_snapshot(),
            self.cache_handler.clone(),
            self.fetch_handler.clone(),
            self.transform_handler.clone(),
        )
        .map(|p| p as Arc<dyn Pool>)
    }
    fn tooling_pool(&self, cfg: PoolConfig) -> Result<Arc<dyn Pool>> {
        Self::build_pool(
            cfg,
            self.static_snapshot(),
            self.cache_handler.clone(),
            self.fetch_handler.clone(),
            self.transform_handler.clone(),
        )
        .map(|p| p as Arc<dyn Pool>)
    }
    fn backend_name(&self) -> &'static str {
        "v8"
    }
}

/// Pool of V8 worker threads (§4.2 default backend).
///
/// `workers` is `Arc<RwLock<...>>` so the warm-replacement background task
/// (§4.3) can hold a handle without borrowing the pool.
pub struct V8Pool {
    cfg: PoolConfig,
    workers: Arc<std::sync::RwLock<Vec<Arc<V8WorkerHandle>>>>,
    next_id: AtomicU64,
    counters: Arc<PoolCounters>,
    queue_waits: Mutex<VecDeque<f64>>,
    draining: AtomicBool,
    /// §4.4: the startup snapshot every worker (and each warm replacement)
    /// restores. `None` means each worker evaluates the bootstrap fresh.
    snapshot: Option<&'static [u8]>,
    /// BUG-02: the cache tier every worker (and each replacement) installs.
    cache_handler: Option<std::sync::Arc<dyn oxide_fw_cache::CacheHandler>>,
    /// PERF-05: the fetch handler every worker (and each replacement) installs.
    fetch_handler: Option<std::sync::Arc<dyn FetchHandler>>,
    /// §6.2 / §15: the plugin transform handler every worker's
    /// `compile_module` op applies before the Oxc passes.
    transform_handler: Option<std::sync::Arc<dyn crate::TransformHandler>>,
}

impl V8Pool {
    fn total_in_flight(&self) -> u64 {
        self.workers
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|w| w.in_flight.load(Ordering::Relaxed))
            .sum()
    }

    /// Warm replacement (§4.3): spawn a fresh worker *before* the old one
    /// drains, swap it into the worker set, then signal the old worker to
    /// exit. Idempotent — the caller must win the `retiring` CAS first.
    ///
    /// Associated function (not a method) so it can be called from a detached
    /// tokio task without holding `&self`. The task holds a clone of the
    /// `workers` Arc and the worker handle.
    async fn spawn_replacement(
        workers: Arc<std::sync::RwLock<Vec<Arc<V8WorkerHandle>>>>,
        counters: Arc<PoolCounters>,
        idx: usize,
        snapshot: Option<&'static [u8]>,
        cache: Option<std::sync::Arc<dyn oxide_fw_cache::CacheHandler>>,
        fetch_handler: Option<std::sync::Arc<dyn FetchHandler>>,
        transform_handler: Option<std::sync::Arc<dyn crate::TransformHandler>>,
    ) {
        let old = {
            let guard = workers.read().unwrap_or_else(|p| p.into_inner());
            match guard.get(idx) {
                Some(w) => Arc::clone(w),
                None => return,
            }
        };

        // Wait for the old worker to finish any in-flight call. The worker is
        // synchronous, so this completes in a couple of ms in the common case.
        let deadline = Instant::now() + Duration::from_secs(5);
        while old.in_flight.load(Ordering::Relaxed) > 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }

        // Spawn fresh worker BEFORE signalling the old one (§4.3).
        let fresh = match spawn_v8_worker(snapshot, cache, fetch_handler, transform_handler) {
            Ok(w) => w,
            Err(_) => {
                // Could not replace — clear the retire flag so the pool can
                // dispatch to the old worker again. Not fatal.
                old.retiring.store(false, Ordering::Release);
                return;
            }
        };

        // Swap it in.
        {
            let mut guard = workers.write().unwrap_or_else(|p| p.into_inner());
            if idx < guard.len() {
                guard[idx] = fresh;
            }
        }
        counters.recycles.fetch_add(1, Ordering::Relaxed);

        // Signal the old worker to exit.
        let _ = old.tx.send(V8Msg::Shutdown).await;
    }
}

#[async_trait]
impl Pool for V8Pool {
    async fn call(&self, entry: EntryRef, req: CallInput) -> Result<CallStream> {
        if self.draining.load(Ordering::Relaxed) {
            return Err(err(
                "JSH-DRAIN",
                "pool is draining; no new calls are accepted (§12.2)",
            ));
        }
        let start = Instant::now();

        // Snapshot the current worker set. Retiring workers report `u64::MAX`
        // load so `choose_worker` skips them in favour of the fresh replacement
        // (§4.3 warm replacement).
        let (loads, worker, worker_idx) = {
            let guard = self.workers.read().unwrap_or_else(|p| p.into_inner());
            if guard.is_empty() {
                return Err(err("JSH-POOL", "V8 pool has no workers"));
            }
            let loads: Vec<u64> = guard
                .iter()
                .map(|w| {
                    // BUG-03: a dead worker is skipped exactly like a retiring
                    // one (report max load).
                    if w.retiring.load(Ordering::Acquire) || w.dead.load(Ordering::Acquire) {
                        u64::MAX
                    } else {
                        w.in_flight.load(Ordering::Relaxed)
                    }
                })
                .collect();
            let affinity = route_affinity(req.route.as_deref(), loads.len());
            let idx = choose_worker(&loads, affinity);
            if loads[idx] == u64::MAX {
                // BUG-03: every worker is retiring or dead; a warm
                // replacement is in flight. Fail this call honestly rather
                // than dispatch into a dead channel.
                return Err(err(
                    "JSH-POOL",
                    "no live V8 worker (all retiring/dead); a replacement is in flight",
                ));
            }
            (loads, Arc::clone(&guard[idx]), idx)
        };
        let _ = &loads;

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        worker.pending.lock().unwrap().insert(
            id,
            Pending {
                tx,
                request_id: req.request_id.clone(),
            },
        );
        worker.in_flight.fetch_add(1, Ordering::Relaxed);
        worker.served.fetch_add(1, Ordering::Relaxed);

        if let Err(e) = worker
            .tx
            .send(V8Msg::Call {
                id,
                entry: entry.clone(),
                input: req.clone(),
            })
            .await
        {
            worker.pending.lock().unwrap().remove(&id);
            worker.in_flight.fetch_sub(1, Ordering::Relaxed);
            self.counters.failed.fetch_add(1, Ordering::Relaxed);
            // BUG-03: the worker's thread is gone. Mark it dead (so
            // `choose_worker` skips it) and spawn a warm replacement — a dead
            // worker left in the set would fail every call forever. CAS so
            // concurrent failures trigger one replacement.
            if !worker.dead.swap(true, Ordering::AcqRel) {
                let workers = Arc::clone(&self.workers);
                let counters = Arc::clone(&self.counters);
                let snapshot = self.snapshot;
                let cache = self.cache_handler.clone();
                let fetch = self.fetch_handler.clone();
                let transform = self.transform_handler.clone();
                tokio::spawn(async move {
                    V8Pool::spawn_replacement(
                        workers, counters, worker_idx, snapshot, cache, fetch, transform,
                    )
                    .await
                });
            }
            return Err(err(
                "JSH-CRASH",
                format!("worker channel closed; replacement spawned: {e}"),
            ));
        }

        let res = tokio::time::timeout(CALL_TIMEOUT, rx).await;
        worker.in_flight.fetch_sub(1, Ordering::Relaxed);
        match res {
            Err(_) => {
                worker.pending.lock().unwrap().remove(&id);
                self.counters.failed.fetch_add(1, Ordering::Relaxed);
                record_wait(&self.queue_waits, start.elapsed());
                Err(err(
                    "JSH-TIMEOUT",
                    format!(
                        "no response from worker within 10s for call id {id} (kind `{}`)",
                        entry.kind
                    ),
                ))
            }
            Ok(Err(_dropped)) => {
                self.counters.failed.fetch_add(1, Ordering::Relaxed);
                record_wait(&self.queue_waits, start.elapsed());
                Err(err("JSH-ABORT", "call aborted before completion"))
            }
            Ok(Ok(Err(e))) => {
                self.counters.failed.fetch_add(1, Ordering::Relaxed);
                record_wait(&self.queue_waits, start.elapsed());
                Err(e)
            }
            Ok(Ok(Ok(stream))) => {
                self.counters.completed.fetch_add(1, Ordering::Relaxed);
                record_wait(&self.queue_waits, start.elapsed());
                // §4.3 lifecycle: after a successful call, if this worker has
                // reached its recycle threshold, spawn a fresh one and retire
                // it. CAS so concurrent callers trigger it at most once.
                let served = worker.served.load(Ordering::Relaxed);
                if self.cfg.recycle_after_requests > 0
                    && served >= self.cfg.recycle_after_requests
                    && !worker.retiring.swap(true, Ordering::AcqRel)
                {
                    let workers = Arc::clone(&self.workers);
                    let counters = Arc::clone(&self.counters);
                    let snapshot = self.snapshot; // Copy (&'static [u8])
                    let cache = self.cache_handler.clone();
                    let fetch = self.fetch_handler.clone();
                    let transform = self.transform_handler.clone();
                    tokio::spawn(async move {
                        V8Pool::spawn_replacement(
                            workers, counters, worker_idx, snapshot, cache, fetch, transform,
                        )
                        .await
                    });
                }
                Ok(stream)
            }
        }
    }

    fn stats(&self) -> PoolStats {
        let waits = sorted_copy(&self.queue_waits);
        PoolStats {
            workers: self.workers.read().unwrap_or_else(|p| p.into_inner()).len(),
            in_flight: self.total_in_flight(),
            completed: self.counters.completed.load(Ordering::Relaxed),
            failed: self.counters.failed.load(Ordering::Relaxed),
            recycles: self.counters.recycles.load(Ordering::Relaxed),
            crashed: self.counters.crashed.load(Ordering::Relaxed),
            queue_wait_ms_p50: percentile(&waits, 0.5),
            queue_wait_ms_p99: percentile(&waits, 0.99),
        }
    }

    fn abort_request(&self, request_id: &str) {
        let guard = self.workers.read().unwrap_or_else(|p| p.into_inner());
        for w in guard.iter() {
            let mut pending = w.pending.lock().unwrap();
            let ids: Vec<u64> = pending
                .iter()
                .filter(|(_, p)| p.request_id == request_id)
                .map(|(id, _)| *id)
                .collect();
            for id in ids {
                if let Some(p) = pending.remove(&id) {
                    let _ = p.tx.send(Err(err(
                        "JSH-ABORT",
                        format!("request `{request_id}` aborted by client disconnect (§9.6)"),
                    )));
                }
            }
        }
    }

    async fn drain(&self) {
        self.draining.store(true, Ordering::Relaxed);
        let deadline = Instant::now() + DRAIN_BUDGET;
        while self.total_in_flight() > 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

// ---------------------------------------------------------- node backend ----

/// Shared pool counters (also reachable from the per-worker reader tasks).
#[derive(Debug, Default)]
struct PoolCounters {
    completed: AtomicU64,
    failed: AtomicU64,
    recycles: AtomicU64,
    crashed: AtomicU64,
}

/// A pending call awaiting its worker's response.
struct Pending {
    tx: oneshot::Sender<Result<CallStream>>,
    request_id: String,
}

/// One spawned `node oxide-worker.mjs` child process (§4.6 tier 3). The child and
/// its pipes are swapped in place when the worker is recycled or replaced, so
/// dispatch tables and pending maps survive across generations.
struct Worker {
    /// PERF-03: in-flight streaming senders, keyed by call id. Shared between
    /// the stdout reader (which feeds chunks) and any spawned op task (a
    /// user `stream_write` pushes into the matching call's sender).
    streams: Arc<std::sync::Mutex<HashMap<u64, mpsc::Sender<Result<Bytes>>>>>,
    stdin: tokio::sync::Mutex<ChildStdin>,
    child: tokio::sync::Mutex<Child>,
    /// Reader task for the current process generation; aborted on replacement.
    reader_handle: Mutex<Option<JoinHandle<()>>>,
    pending: Mutex<HashMap<u64, Pending>>,
    in_flight: AtomicU64,
    /// Requests served since the last (re)spawn — drives §4.3 recycling.
    served: AtomicU64,
    /// Set by the reader when the worker process exited (stdout EOF).
    dead: AtomicBool,
    /// PID reported by the worker's `Ready{pid}` greeting.
    pid: Mutex<Option<u32>>,
    /// Serializes recycle/respawn replacement on this worker.
    recycle_lock: tokio::sync::Mutex<()>,
    /// §9.3: the Rust cache tier a `cache_get` / `cache_set` / `cache_tag`
    /// request from user code dispatches to. `None` means the runtime's
    /// in-process Map is the only cache. A `Mutex` because the pool builds
    /// workers before the caller attaches a handler; the read happens once
    /// per op request, not on the hot path.
    cache: Mutex<Option<Arc<dyn oxide_fw_cache::CacheHandler>>>,
    /// PERF-05: the fetch handler this worker's `fetch` op delegates to.
    fetch: Mutex<Option<Arc<dyn FetchHandler>>>,
    /// §6.2 / §15: the plugin `transform` hook this worker's `compile_module`
    /// op runs before the Oxc passes (so a plugin transform reaches the
    /// prerender path).
    transform: Mutex<Option<Arc<dyn TransformHandler>>>,
}

/// Spawn `node <worker_script>` with piped stdio; returns (child, stdin, stdout).
/// Uses `tokio::process::Command` directly (tokio 1.53 has no `Child::from(std)`);
/// `spawn()` is sync but requires the caller to run inside a tokio runtime — pool
/// creation always happens there (§4.2 pools are built from async host code).
fn spawn_child(
    node_bin: &str,
    worker_script: &Path,
    offline_plugins: bool,
) -> Result<(Child, ChildStdin, ChildStdout)> {
    let mut cmd = tokio::process::Command::new(node_bin);
    // §17.2: --offline-plugins. Passed as an env var to the child; the
    // worker's fetch shim reads it and refuses all outbound network.
    cmd.env(
        "OXIDE_OFFLINE_PLUGINS",
        if offline_plugins { "1" } else { "0" },
    );
    // Register the TSX loader hook so `import './page.tsx'` resolves
    // via sucrase (JSX lowering). Node's built-in type stripper handles
    // .ts but not .tsx; the loader bridges the gap.
    //
    // We use `--import <register-hooks.mjs>` (which calls `register()`)
    // rather than `--experimental-loader`, because the latter is
    // deprecated as of Node 22 and emits a warning on every worker
    // spawn. The shim and the loader are siblings of the worker script,
    // so all three ship together with the jshost crate.
    let register_hooks = worker_script
        .parent()
        .map(|p| p.join("register-hooks.mjs"))
        .unwrap_or_else(|| std::path::PathBuf::from("register-hooks.mjs"));
    cmd.arg("--import").arg(&register_hooks);

    cmd.arg(worker_script)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());
    // Bind the child's stdin/stdout to the reader runtime (C), NOT the
    // caller's runtime. `tokio::process::Command::spawn()` registers the
    // pipes with whichever reactor is current at spawn time; a caller whose
    // runtime is later blocked — the sync plugin-hook bridge blocks its
    // runtime with `std::thread::scope` — would then starve that reactor,
    // and neither the write to stdin nor the read from stdout would make
    // progress. Entering C's context first makes every pipe operation wait
    // on C's reactor, which has its own thread and is never blocked.
    let handle = reader_runtime_handle();
    let _enter = handle.enter();
    let mut child = cmd.spawn().map_err(|e| {
        err(
            "JSH-NODE",
            format!("failed to spawn node worker `{node_bin}`: {e}"),
        )
    })?;
    let stdin = child.stdin.take().expect("worker stdin is piped");
    let stdout = child.stdout.take().expect("worker stdout is piped");
    Ok((child, stdin, stdout))
}

/// Pool of real Node child workers speaking the framed JSON protocol on stdio
/// (§4.2, §4.6 tier 3). Requests are dispatched least-loaded (§4.3), workers are
/// recycled after `PoolConfig::recycle_after_requests` requests, a crash fails
/// only its in-flight requests (`JSH-CRASH`) and the worker is replaced on the
/// next dispatch.
pub struct NodePool {
    cfg: PoolConfig,
    node_bin: String,
    worker_script: PathBuf,
    workers: Vec<Arc<Worker>>,
    next_id: AtomicU64,
    counters: Arc<PoolCounters>,
    /// Per-call wall time (ms) for queue-wait percentiles, capped at 1000 samples.
    queue_waits: Mutex<VecDeque<f64>>,
    draining: AtomicBool,
}

impl NodePool {
    /// §9.3: attach the Rust cache tier. Also propagates to workers spawned
    /// later by `replace_worker`, so a recycled worker keeps the handler.
    pub fn with_cache_handler(self: &Arc<Self>, handler: Arc<dyn oxide_fw_cache::CacheHandler>) {
        for w in &self.workers {
            *w.cache.lock().unwrap() = Some(Arc::clone(&handler));
        }
        // `replace_worker` swaps a worker's *pipes*, not the `Worker`
        // struct, so the handler on each worker survives a recycle without
        // any extra bookkeeping.
    }

    /// PERF-05: attach the fetch handler the worker's `fetch` op delegates to.
    /// Mirrors [`Self::with_cache_handler`].
    pub fn with_fetch_handler(self: &Arc<Self>, handler: Arc<dyn FetchHandler>) {
        for w in &self.workers {
            *w.fetch.lock().unwrap() = Some(Arc::clone(&handler));
        }
    }

    /// §6.2 / §15: attach the plugin `transform` hook for the worker's
    /// `compile_module` op (prerender-path transforms). Mirrors
    /// [`Self::with_fetch_handler`].
    pub fn with_transform_handler(self: &Arc<Self>, handler: Arc<dyn TransformHandler>) {
        for w in &self.workers {
            *w.transform.lock().unwrap() = Some(Arc::clone(&handler));
        }
    }
}

/// Response timeout per call (§4.3 watchdog).
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// Drain waits at most this long for in-flight calls (§12.2).
const DRAIN_BUDGET: Duration = Duration::from_secs(1);

impl NodePool {
    /// Probe the node binary and worker script, then spawn `cfg.size` children.
    ///
    /// Synchronous (runs `node --version` and forks via `std::process::Command`,
    /// converted with `tokio::process::Child::from`); must run inside a runtime.
    pub fn spawn(
        node_bin: &str,
        worker_script: impl Into<PathBuf>,
        cfg: PoolConfig,
    ) -> Result<Arc<NodePool>> {
        let worker_script = worker_script.into();
        Self::probe(node_bin, &worker_script)?;
        let counters = Arc::new(PoolCounters::default());
        let mut workers = Vec::with_capacity(cfg.size.max(1));
        for _ in 0..cfg.size.max(1) {
            workers.push(Self::start_worker(
                node_bin,
                &worker_script,
                counters.clone(),
                cfg.offline_plugins,
            )?);
        }
        Ok(Arc::new(NodePool {
            cfg,
            node_bin: node_bin.to_string(),
            worker_script,
            workers,
            next_id: AtomicU64::new(0),
            counters,
            queue_waits: Mutex::new(VecDeque::new()),
            draining: AtomicBool::new(false),
        }))
    }

    /// Fail fast at pool creation when `node` or the worker script is unusable.
    fn probe(node_bin: &str, worker_script: &Path) -> Result<()> {
        if let Err(e) = std::process::Command::new(node_bin)
            .arg("--version")
            .output()
        {
            return Err(err(
                "JSH-NODE",
                format!(
                    "node binary `{node_bin}` is not available: {e} — install Node.js or pick another runtime (§4.6 tier 3)"
                ),
            ));
        }
        if !worker_script.is_file() {
            return Err(err(
                "JSH-NODE",
                format!(
                    "worker script `{}` does not exist (§4.2)",
                    worker_script.display()
                ),
            ));
        }
        Ok(())
    }

    fn start_worker(
        node_bin: &str,
        worker_script: &Path,
        counters: Arc<PoolCounters>,
        offline_plugins: bool,
    ) -> Result<Arc<Worker>> {
        let (child, stdin, stdout) = spawn_child(node_bin, worker_script, offline_plugins)?;
        let worker = Arc::new(Worker {
            stdin: tokio::sync::Mutex::new(stdin),
            child: tokio::sync::Mutex::new(child),
            reader_handle: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            in_flight: AtomicU64::new(0),
            served: AtomicU64::new(0),
            dead: AtomicBool::new(false),
            pid: Mutex::new(None),
            streams: Arc::new(std::sync::Mutex::new(HashMap::new())),
            recycle_lock: tokio::sync::Mutex::new(()),
            cache: Mutex::new(None),
            fetch: Mutex::new(None),
            transform: Mutex::new(None),
        });
        let reader = spawn_reader(stdout, Arc::clone(&worker), counters);
        *worker.reader_handle.lock().unwrap() = Some(reader);
        Ok(worker)
    }

    /// PIDs reported via `Ready{pid}` per worker (None until the greeting arrives).
    pub fn worker_pids(&self) -> Vec<Option<u32>> {
        self.workers
            .iter()
            .map(|w| *w.pid.lock().unwrap())
            .collect()
    }

    fn total_in_flight(&self) -> u64 {
        self.workers
            .iter()
            .map(|w| w.in_flight.load(Ordering::Relaxed))
            .sum()
    }

    /// Replace the worker process (§4.3 lifecycle) when it is dead or due for
    /// recycling and currently idle. `crashed` distinguishes crash-replacement
    /// (already counted in `crashed` at EOF) from threshold recycling (counted
    /// in `recycles` here).
    async fn replace_worker(&self, worker: &Arc<Worker>, crashed: bool) -> Result<()> {
        if let Some(h) = worker.reader_handle.lock().unwrap().take() {
            h.abort(); // cancel before the reader can observe EOF
        }
        {
            let mut c = worker.child.lock().await;
            let _ = c.kill().await;
        }
        let (child, stdin, stdout) = spawn_child(
            &self.node_bin,
            &self.worker_script,
            self.cfg.offline_plugins,
        )?;
        *worker.child.lock().await = child;
        *worker.stdin.lock().await = stdin;
        worker.pending.lock().unwrap().clear();
        worker.served.store(0, Ordering::Relaxed);
        worker.dead.store(false, Ordering::Relaxed);
        *worker.pid.lock().unwrap() = None;
        let reader = spawn_reader(stdout, Arc::clone(worker), Arc::clone(&self.counters));
        *worker.reader_handle.lock().unwrap() = Some(reader);
        if !crashed {
            self.counters.recycles.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Ensure the picked worker is alive and not due for recycling (§4.3).
    async fn ensure_ready(&self, worker: &Arc<Worker>) -> Result<()> {
        fn due_for_recycle(pool: &NodePool, w: &Worker) -> bool {
            pool.cfg.recycle_after_requests > 0
                && w.served.load(Ordering::Relaxed) >= pool.cfg.recycle_after_requests
                && w.in_flight.load(Ordering::Relaxed) == 0
        }
        if !worker.dead.load(Ordering::Relaxed) && !due_for_recycle(self, worker) {
            return Ok(());
        }
        let _guard = worker.recycle_lock.lock().await;
        if worker.dead.load(Ordering::Relaxed) {
            self.replace_worker(worker, true).await?;
        } else if due_for_recycle(self, worker) {
            self.replace_worker(worker, false).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl Pool for NodePool {
    async fn call(&self, entry: EntryRef, req: CallInput) -> Result<CallStream> {
        if self.draining.load(Ordering::Relaxed) {
            return Err(err(
                "JSH-DRAIN",
                "pool is draining; no new calls are accepted (§12.2)",
            ));
        }
        let start = Instant::now();
        // PERF-02: least-loaded dispatch with route affinity — calls for the
        // same route prefer the same worker so its module cache stays warm.
        let loads: Vec<u64> = self
            .workers
            .iter()
            .map(|w| w.in_flight.load(Ordering::Relaxed))
            .collect();
        let affinity = route_affinity(req.route.as_deref(), loads.len());
        let worker = Arc::clone(&self.workers[choose_worker(&loads, affinity)]);
        self.ensure_ready(&worker).await?;

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        worker.pending.lock().unwrap().insert(
            id,
            Pending {
                tx,
                request_id: req.request_id.clone(),
            },
        );
        worker.in_flight.fetch_add(1, Ordering::Relaxed);
        worker.served.fetch_add(1, Ordering::Relaxed);

        let framed = frame(&WorkerMsg::Call {
            id,
            entry: entry.clone(),
            input: req.clone(),
        })?;
        let write_res = worker.stdin.lock().await.write_all(&framed).await;
        if let Err(e) = write_res {
            worker.pending.lock().unwrap().remove(&id);
            worker.in_flight.fetch_sub(1, Ordering::Relaxed);
            self.counters.failed.fetch_add(1, Ordering::Relaxed);
            return Err(err(
                "JSH-CRASH",
                format!("write to worker failed: {e} (§4.3 failure isolation)"),
            ));
        }

        let res = tokio::time::timeout(CALL_TIMEOUT, rx).await;
        worker.in_flight.fetch_sub(1, Ordering::Relaxed);
        match res {
            Err(_) => {
                // release the slot so a late reply is dropped, not misattributed
                worker.pending.lock().unwrap().remove(&id);
                self.counters.failed.fetch_add(1, Ordering::Relaxed);
                record_wait(&self.queue_waits, start.elapsed());
                Err(err(
                    "JSH-TIMEOUT",
                    format!(
                        "no response from worker within 10s for call id {id} (kind `{}`)",
                        entry.kind
                    ),
                ))
            }
            Ok(Err(_dropped)) => {
                self.counters.failed.fetch_add(1, Ordering::Relaxed);
                record_wait(&self.queue_waits, start.elapsed());
                Err(err(
                    "JSH-CRASH",
                    "worker exited before responding (§4.3 failure isolation)",
                ))
            }
            Ok(Ok(Err(e))) => {
                self.counters.failed.fetch_add(1, Ordering::Relaxed);
                record_wait(&self.queue_waits, start.elapsed());
                Err(e)
            }
            Ok(Ok(Ok(stream))) => {
                self.counters.completed.fetch_add(1, Ordering::Relaxed);
                record_wait(&self.queue_waits, start.elapsed());
                Ok(stream)
            }
        }
    }

    fn stats(&self) -> PoolStats {
        let waits = sorted_copy(&self.queue_waits);
        PoolStats {
            workers: self.workers.len(),
            in_flight: self.total_in_flight(),
            completed: self.counters.completed.load(Ordering::Relaxed),
            failed: self.counters.failed.load(Ordering::Relaxed),
            recycles: self.counters.recycles.load(Ordering::Relaxed),
            crashed: self.counters.crashed.load(Ordering::Relaxed),
            queue_wait_ms_p50: percentile(&waits, 0.5),
            queue_wait_ms_p99: percentile(&waits, 0.99),
        }
    }

    /// Resolve every pending call belonging to `request_id` with `JSH-ABORT`
    /// (client disconnect, §9.6). Late worker replies are dropped.
    fn abort_request(&self, request_id: &str) {
        for w in &self.workers {
            let mut pending = w.pending.lock().unwrap();
            let ids: Vec<u64> = pending
                .iter()
                .filter(|(_, p)| p.request_id == request_id)
                .map(|(id, _)| *id)
                .collect();
            for id in ids {
                if let Some(p) = pending.remove(&id) {
                    let _ = p.tx.send(Err(err(
                        "JSH-ABORT",
                        format!("request `{request_id}` aborted by client disconnect (§9.6)"),
                    )));
                }
            }
        }
    }

    /// Mark the pool draining (new calls → `JSH-DRAIN`) and wait — bounded by
    /// [`DRAIN_BUDGET`] — for in-flight calls to finish (§12.2).
    async fn drain(&self) {
        self.draining.store(true, Ordering::Relaxed);
        let deadline = Instant::now() + DRAIN_BUDGET;
        while self.total_in_flight() > 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// Node backend (§4.2, §4.6 tier 3): spawns `node worker/oxide-worker.mjs` children,
/// each pinned to a tokio task; requests dispatched least-loaded with route-affinity
/// preference (§4.3). A crash kills one worker and fails only its in-flight requests
/// (§4.3 failure isolation); the supervisor retries once if idempotent.
pub struct NodeHost {
    pub node_bin: String,
    pub worker_script: std::path::PathBuf,
    pub request_counter: AtomicU64,
    /// §9.3: the Rust cache tier a worker's `op_request` frames dispatch to.
    /// `None` means the runtime's in-process Map is the only cache.
    pub cache_handler: Option<Arc<dyn oxide_fw_cache::CacheHandler>>,
    /// PERF-05: the fetch handler a worker's `fetch` op dispatches to.
    pub fetch_handler: Option<Arc<dyn FetchHandler>>,
    /// §6.2 / §15: the plugin `transform` hook a worker's `compile_module`
    /// op applies (reaching the prerender path).
    pub transform_handler: Option<Arc<dyn TransformHandler>>,
}

impl NodeHost {
    pub fn new(worker_script: impl Into<std::path::PathBuf>) -> Self {
        Self {
            node_bin: "node".into(),
            worker_script: worker_script.into(),
            request_counter: AtomicU64::new(0),
            cache_handler: None,
            fetch_handler: None,
            transform_handler: None,
        }
    }

    /// PERF-05: attach the `fetch` op handler.
    pub fn with_fetch_handler(mut self, handler: Arc<dyn FetchHandler>) -> Self {
        self.fetch_handler = Some(handler);
        self
    }

    /// §9.3: attach a cache tier. Workers spawned by this host dispatch
    /// `cache_get` / `cache_set` / `cache_tag` to it.
    pub fn with_cache_handler(mut self, handler: Arc<dyn oxide_fw_cache::CacheHandler>) -> Self {
        self.cache_handler = Some(handler);
        self
    }

    /// §6.2 / §15: attach the plugin `transform` hook a worker's
    /// `compile_module` op applies (reaching the prerender path).
    pub fn with_transform_handler(mut self, handler: Arc<dyn TransformHandler>) -> Self {
        self.transform_handler = Some(handler);
        self
    }

    /// Locate the bundled worker script relative to this crate.
    pub fn worker_script_default() -> std::path::PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("worker/oxide-worker.mjs")
    }
}

impl JsHost for NodeHost {
    fn render_pool(&self, cfg: PoolConfig) -> Result<Arc<dyn Pool>> {
        let p = NodePool::spawn(&self.node_bin, self.worker_script.clone(), cfg)?;
        if let Some(h) = &self.cache_handler {
            p.with_cache_handler(Arc::clone(h));
        }
        if let Some(h) = &self.fetch_handler {
            p.with_fetch_handler(Arc::clone(h));
        }
        if let Some(h) = &self.transform_handler {
            p.with_transform_handler(Arc::clone(h));
        }
        Ok(p as Arc<dyn Pool>)
    }
    fn tooling_pool(&self, cfg: PoolConfig) -> Result<Arc<dyn Pool>> {
        let p = NodePool::spawn(&self.node_bin, self.worker_script.clone(), cfg)?;
        if let Some(h) = &self.cache_handler {
            p.with_cache_handler(Arc::clone(h));
        }
        if let Some(h) = &self.fetch_handler {
            p.with_fetch_handler(Arc::clone(h));
        }
        if let Some(h) = &self.transform_handler {
            p.with_transform_handler(Arc::clone(h));
        }
        Ok(p as Arc<dyn Pool>)
    }
    fn backend_name(&self) -> &'static str {
        "node"
    }
}

// ------------------------------------------------------------ reader task --

/// Read worker stdout, decode frames and resolve pending calls (§4.2 protocol).
///
/// - `Ready{pid}` records the worker pid.
/// - `CallOk{complete:true}` resolves `Complete` (base64 chunks concatenated).
/// - `CallChunk*`/`CallEnd` resolve `Chunked` with live per-id senders.
/// - `CallErr` resolves the pending call with the worker's diagnostic.
/// - stdout EOF (worker exit) fails all pending calls with `JSH-CRASH` and bumps
///   the `crashed` counter (§4.3 failure isolation).
/// The runtime the worker's child process (and its reader task) live on.
///
/// A **dedicated** runtime, separate from whatever the caller is using. A
/// worker's stdin/stdout pipes are bound to this runtime at spawn time
/// (`spawn_child` enters this context before `Command::spawn`), and its
/// stdout reader is spawned on it. The reason is the sync plugin-hook
/// bridge: `build_js_plugin_hook` (and its five siblings) call
/// `std::thread::scope`, which blocks the calling thread and therefore the
/// caller's runtime. If the child's I/O lived on that runtime, no reactor
/// would be polled while the build is inside a hook — the write to stdin
/// would stall, no response would arrive, and `oxide build` would hang.
///
/// With the child bound here, every pipe operation waits on this runtime's
/// reactor, which has its own thread that is never blocked. A
/// current-thread caller is therefore fine.
///
/// One runtime per process, created lazily on first use. A fresh
/// `std::thread` builds it because `Builder::build()` panics if called from
/// within another async context (which `spawn_child` often is).
fn reader_runtime_handle() -> tokio::runtime::Handle {
    use std::sync::OnceLock;
    static HANDLE: OnceLock<tokio::runtime::Handle> = OnceLock::new();
    HANDLE
        .get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .name("oxide-reader-rt-init".to_string())
                .spawn(move || {
                    let rt = tokio::runtime::Builder::new_multi_thread()
                        .worker_threads(1)
                        .enable_all()
                        .thread_name("oxide-reader-rt")
                        .build()
                        .expect("reader runtime");
                    let handle = rt.handle().clone();
                    let _ = tx.send(handle);
                    // Leak the runtime: it lives for the process lifetime.
                    // Dropping it would abort every reader task on it.
                    std::mem::forget(rt);
                })
                .expect("spawn reader runtime init");
            rx.recv().expect("reader runtime handle")
        })
        .clone()
}

fn spawn_reader(
    mut stdout: ChildStdout,
    worker: Arc<Worker>,
    counters: Arc<PoolCounters>,
) -> JoinHandle<()> {
    reader_runtime_handle().spawn(async move {
        let mut buf: Vec<u8> = Vec::with_capacity(8192);
        let mut chunk = [0u8; 8192];
        loop {
            let (msgs, consumed) = match unframe(&buf) {
                Ok(x) => x,
                Err(e) => {
                    worker.dead.store(true, Ordering::Relaxed);
                    let code = e.0.code.clone();
                    let message = e.0.message.clone();
                    fail_all(&worker, code, message);
                    return;
                }
            };
            buf.drain(..consumed);
            for m in msgs {
                handle_worker_msg(m, &worker);
            }
            match stdout.read(&mut chunk).await {
                Ok(0) | Err(_) => {
                    worker.dead.store(true, Ordering::Relaxed);
                    fail_all(
                        &worker,
                        "JSH-CRASH".to_string(),
                        "worker process exited before responding (§4.3 failure isolation)"
                            .to_string(),
                    );
                    counters.crashed.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
    })
}

/// Fail every pending call (worker exit / protocol error). `streams` senders are
/// dropped, which closes any chunked collector's stream.
fn fail_all(worker: &Worker, code: String, message: String) {
    // §4.3 / P0-2(b): a worker that crashes mid-stream must produce an
    // **error**, not a clean end-of-stream with truncated bytes. Before
    // this fix, clearing the `streams` map dropped every `Sender` without
    // an error frame: the consumer's `CallStream::Chunked(rx)` saw a
    // graceful channel close (`recv() == None`) and `collect()` returned
    // `Ok(partial_bytes)` — a 200 with a truncated body. Now each open
    // stream gets an `Err(JSH-CRASH)` pushed first, so `collect()`'s
    // `chunk?` surfaces the crash to the caller.
    {
        let mut streams = worker.streams.lock().unwrap();
        for (_, tx) in streams.drain() {
            // Best-effort: a consumer that already went away dropped the
            // receiver; that stream is closed and has no one to notify.
            let _ = tx.try_send(Err(OxideError(Diagnostic::error(
                code.clone(),
                message.clone(),
            ))));
        }
    }
    let mut pending = worker.pending.lock().unwrap();
    for (_, p) in pending.drain() {
        let _ = p.tx.send(Err(OxideError(Diagnostic::error(
            code.clone(),
            message.clone(),
        ))));
    }
}

fn resolve(worker: &Worker, id: u64, res: Result<CallStream>) {
    if let Some(p) = worker.pending.lock().unwrap().remove(&id) {
        let _ = p.tx.send(res);
    }
}

/// Open (or reuse) the chunked stream for `id`, resolving the pending call on
/// first use so bytes flow while the worker is still sending.
fn stream_sender(worker: &Worker, id: u64) -> Option<mpsc::Sender<Result<Bytes>>> {
    let mut guard = worker.streams.lock().unwrap();
    if let Some(tx) = guard.get(&id) {
        return Some(tx.clone());
    }
    let (tx, rx) = mpsc::channel(16);
    guard.insert(id, tx.clone());
    drop(guard);
    resolve(worker, id, Ok(CallStream::Chunked(rx)));
    Some(tx)
}

/// PERF-03: drop a call's stream sender (consumer gone or call ended).
fn drop_stream(worker: &Worker, id: u64) {
    worker.streams.lock().unwrap().remove(&id);
}

/// §4.5: run a worker-originated op against the host. Only the three cache
/// ops are implemented — a `fetch` / `stream_write` / `ws_*` request from a
/// worker answers with an "unimplemented" error, which the runtime sees as
/// an op failure and can fall back on.
async fn op_dispatch(
    cache: Option<&dyn oxide_fw_cache::CacheHandler>,
    fetch: Option<&dyn FetchHandler>,
    transform: Option<&dyn TransformHandler>,
    memo: Option<&oxide_fw_cache::FetchMemo>,
    streams: Option<Arc<std::sync::Mutex<HashMap<u64, mpsc::Sender<Result<Bytes>>>>>>,
    op: Op,
    args: &serde_json::Value,
) -> Result<serde_json::Value> {
    match op {
        Op::CacheGet => {
            let Some(cache) = cache else {
                // No handler: answer `null`, which the runtime reads as a
                // miss and covers with its in-process Map.
                return Ok(serde_json::Value::Null);
            };
            let Some(key) = args.get("key").and_then(|v| v.as_str()) else {
                return Err(err("JSH-OP", "cache_get requires a string `key`"));
            };
            match cache.get(key).await? {
                Some(entry) => {
                    // §9.5: the entry body is ODF bytes (the same format the
                    // wire uses). Return them base64-encoded — the JS runtime
                    // decodes with `decodeOdf` so rich types (Date, Map, Set,
                    // BigInt, shared refs) survive the cache round-trip. Raw
                    // bytes are opaque to the tier, so ODF is the honest
                    // representation; JSON was lossy (it flattened every rich
                    // type).
                    use base64::Engine as _;
                    let body_b64 = base64::engine::general_purpose::STANDARD.encode(&entry.body);
                    Ok(serde_json::json!({
                        "body_b64": body_b64,
                        // Keep the tags on the response for a caller that wants
                        // to re-derive policy without a second round-trip.
                        "tags": entry.tags,
                    }))
                }
                None => Ok(serde_json::Value::Null),
            }
        }
        Op::CacheSet => {
            let Some(cache) = cache else {
                return Ok(serde_json::json!({ "stored": false }));
            };
            let Some(key) = args.get("key").and_then(|v| v.as_str()) else {
                return Err(err("JSH-OP", "cache_set requires a string `key`"));
            };
            // §9.5: `body_b64` is base64 ODF — the primary path (rich types
            // survive). A `body` JSON value is the fallback for a runtime
            // that could not ODF-encode; it is serialized as JSON bytes.
            let bytes = if let Some(b64) = args.get("body_b64").and_then(|v| v.as_str()) {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|e| err("JSH-OP", format!("cache_set body_b64 invalid: {e}")))?
            } else if let Some(body) = args.get("body") {
                serde_json::to_vec(body).map_err(|e| {
                    err("JSH-OP", format!("cache_set body is not serializable: {e}"))
                })?
            } else {
                return Err(err(
                    "JSH-OP",
                    "cache_set requires a `body_b64` (ODF) or `body` (JSON) field",
                ));
            };
            let tags: Vec<String> = args
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let ttl_secs = args.get("ttl").and_then(|v| v.as_u64()).unwrap_or(0);
            let swr_secs = args.get("swr").and_then(|v| v.as_u64());
            let entry = oxide_fw_cache::Entry {
                body: bytes,
                status: 200,
                headers: Vec::new(),
                tags,
                policy: oxide_fw_cache::Policy {
                    ttl: std::time::Duration::from_secs(ttl_secs),
                    swr: swr_secs.map(std::time::Duration::from_secs),
                    stale_if_error: None,
                    soft_tags: true,
                },
                stored_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
                etag: String::new(),
            };
            cache.set(key, entry).await?;
            Ok(serde_json::json!({ "stored": true }))
        }
        Op::CacheTag => {
            let Some(cache) = cache else {
                return Ok(serde_json::json!({ "invalidated": 0 }));
            };
            let Some(tag) = args.get("tag").and_then(|v| v.as_str()) else {
                return Err(err("JSH-OP", "cache_tag requires a string `tag`"));
            };
            let soft = args.get("soft").and_then(|v| v.as_bool()).unwrap_or(true);
            let n = cache.invalidate_tag(tag, soft).await?;
            Ok(serde_json::json!({ "invalidated": n }))
        }
        Op::CompileModule => {
            let Some(code) = args.get("code").and_then(|v| v.as_str()) else {
                return Err(err("JSH-OP", "compile_module requires a string `code`"));
            };
            let Some(path) = args.get("path").and_then(|v| v.as_str()) else {
                return Err(err("JSH-OP", "compile_module requires a string `path`"));
            };
            let lang = match args.get("lang").and_then(|v| v.as_str()) {
                Some("tsx") => "tsx",
                Some("jsx") => "jsx",
                Some("js") | Some("mjs") => "mjs",
                _ => "ts",
            };
            // §6.2 / §15: run the plugin `transform` hook FIRST, before the
            // Oxc passes. This is what makes a plugin transform reach the
            // prerender path — the prerender compiles modules through this
            // op. A hook error fails the module's compile with the hook's
            // diagnostic.
            let source_owned: String = if let Some(h) = transform {
                h.transform(path, code).await.map_err(|e| {
                    err(
                        "JSH-COMPILE",
                        format!("plugin transform failed for {path}: {}", e.0.message),
                    )
                })?
            } else {
                code.to_string()
            };
            let input = oxide_fw_compiler::TransformInput {
                path: path.to_string(),
                code: source_owned,
                // The loader path is server-side: the passes that matter
                // here (`'use cache'` on a `.server.ts`) run on `Ssr`.
                env: oxide_fw_compiler::Env::Ssr,
                lang,
                // No bundle is being emitted on this path, so the id a
                // server-fn stub would compute is unused. `'use cache'`
                // emits `__oxide_cache(fn, module, export, opts)` — no
                // secret involved.
                build_secret: String::new(),
                dev: true,
                jsx_import_source: None,
            };
            match oxide_fw_compiler::OxcTransformer.transform_passes_only(&input) {
                Ok(code) => Ok(serde_json::json!({ "code": code })),
                Err(e) => Err(err(
                    "JSH-COMPILE",
                    format!("compile_module failed for {path}: {}", e.0.message),
                )),
            }
        }
        // §9.5 / PERF-01: encode a JSON value to ODF bytes, base64 for the
        // JSON op protocol. The worker's `oxide.serializeProps` uses this so
        // island props ride the ODF wire format (the client decodes ODF,
        // falling back to JSON for older bundles).
        Op::Serialize => {
            let Some(value) = args.get("value") else {
                return Err(err("JSH-OP", "serialize requires a `value` field"));
            };
            let odf = oxide_fw_odf::Value::from_json(value);
            let bytes = oxide_fw_odf::encode(&odf)
                .map_err(|e| err("JSH-OP", format!("serialize: {}", e.0.message)))?;
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            Ok(serde_json::json!({ "b64": b64 }))
        }
        // PERF-05: `fetch` from user code. Delegates to the host's
        // SSRF-hardened handler; memoized per request when a `FetchMemo` is
        // wired (idempotent GETs dedupe).
        Op::Fetch => {
            let Some(handler) = fetch else {
                return Err(err(
                    "JSH-OP",
                    "fetch is unavailable: no FetchHandler is wired on this host",
                ));
            };
            let method = args
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("GET")
                .to_uppercase();
            let url = args
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| err("JSH-OP", "fetch requires a string `url`"))?;
            let body = args
                .get("body_b64")
                .and_then(|v| v.as_str())
                .and_then(|s| {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD.decode(s).ok()
                })
                .unwrap_or_default();
            let headers: Vec<(String, String)> = args
                .get("headers")
                .and_then(|v| v.as_object())
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                        .collect()
                })
                .unwrap_or_default();
            // Memoize GETs only (a POST with a body is already keyed by the
            // body hash, but a side-effecting request should not be replayed).
            let memo_key = (method == "GET").then(|| oxide_fw_cache::memo_key(&method, url, &body));
            if let (Some(m), Some(k)) = (memo, memo_key.as_deref()) {
                if let Some(hit) = m.get(k) {
                    let v: serde_json::Value =
                        serde_json::from_slice(&hit).unwrap_or(serde_json::Value::Null);
                    return Ok(v);
                }
            }
            let resp = handler.fetch(&method, url, &headers, &body).await?;
            use base64::Engine as _;
            let out = serde_json::json!({
                "status": resp.status,
                "headers": resp.headers,
                "body_b64": base64::engine::general_purpose::STANDARD.encode(&resp.body),
            });
            if let (Some(m), Some(k)) = (memo, memo_key.as_deref()) {
                m.insert(
                    k,
                    bytes::Bytes::from(serde_json::to_vec(&out).unwrap_or_default()),
                );
            }
            Ok(out)
        }
        // PERF-03: user-code streaming. `stream_write` pushes a chunk to the
        // in-flight call whose id the args name; the reader's `streams` map
        // holds that call's sender. `response_head` is accepted and ignored on
        // this host (the status is set when the call resolves).
        Op::StreamWrite => {
            let Some(streams) = streams else {
                return Err(err("JSH-OP", "stream_write is only valid during a call"));
            };
            let call_id = args
                .get("call_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| err("JSH-OP", "stream_write requires a numeric `call_id`"))?;
            let data = args
                .get("data_b64")
                .and_then(|v| v.as_str())
                .and_then(|s| {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD.decode(s).ok()
                })
                .unwrap_or_default();
            let mut guard = streams.lock().unwrap_or_else(|p| p.into_inner());
            match guard.get(&call_id) {
                Some(tx) => {
                    if tx.try_send(Ok(Bytes::from(data))).is_err() {
                        guard.remove(&call_id); // consumer gone
                        return Ok(serde_json::json!({ "written": false }));
                    }
                    Ok(serde_json::json!({ "written": true }))
                }
                None => Ok(serde_json::json!({ "written": false })),
            }
        }
        Op::ResponseHead => {
            // The status/headers ride the final response envelope; accept the
            // op so a user streaming API can announce the head early.
            Ok(serde_json::json!({ "accepted": true }))
        }
        Op::RequestBodyRead => Ok(serde_json::json!({ "done": true })),
        _ => Err(err(
            "JSH-OP",
            format!(
                "worker op `{}` is not implemented on this host",
                op_name(op)
            ),
        )),
    }
}

fn handle_worker_msg(msg: WorkerMsg, worker: &Arc<Worker>) {
    match msg {
        WorkerMsg::Ready { pid } => {
            *worker.pid.lock().unwrap() = Some(pid);
        }
        WorkerMsg::CallOk {
            id,
            chunks,
            complete,
        } => {
            let mut body: Vec<u8> = Vec::new();
            for c in &chunks {
                match b64_decode(c) {
                    Ok(b) => body.extend_from_slice(&b),
                    Err(e) => {
                        drop_stream(worker, id);
                        resolve(worker, id, Err(e));
                        return;
                    }
                }
            }
            if complete {
                drop_stream(worker, id);
                resolve(worker, id, Ok(CallStream::Complete(Bytes::from(body))));
            } else if let Some(tx) = stream_sender(worker, id) {
                if tx.try_send(Ok(Bytes::from(body))).is_err() {
                    drop_stream(worker, id); // collector gone
                }
            }
        }
        WorkerMsg::CallChunkBin { id, data } => {
            if let Some(tx) = stream_sender(worker, id) {
                if tx.try_send(Ok(Bytes::from(data))).is_err() {
                    drop_stream(worker, id);
                }
            }
        }
        WorkerMsg::CallChunk { id, data_b64 } => match b64_decode(&data_b64) {
            Ok(bytes) => {
                if let Some(tx) = stream_sender(worker, id) {
                    if tx.try_send(Ok(Bytes::from(bytes))).is_err() {
                        drop_stream(worker, id); // collector gone: stop buffering
                    }
                }
            }
            Err(e) => {
                drop_stream(worker, id);
                resolve(worker, id, Err(e));
            }
        },
        WorkerMsg::CallEnd { id } => {
            drop_stream(worker, id); // dropping the sender closes the collector's stream
        }
        WorkerMsg::CallErr { id, diagnostic } => {
            drop_stream(worker, id);
            resolve(worker, id, Err(OxideError(diagnostic)));
        }
        // §4.5 / §9.3: a `cache_get` / `cache_set` / `cache_tag` request
        // from user code. The reader owns stdout; the response goes back on
        // the worker's stdin. Dispatch to the handler and write an
        // `op_response`. When no handler is configured, answer `null` so
        // the runtime's in-process fallback runs.
        WorkerMsg::OpRequest { id, op, args } => {
            let cache = worker.cache.lock().unwrap().clone();
            let fetch = worker.fetch.lock().unwrap().clone();
            let transform = worker.transform.lock().unwrap().clone();
            let streams = Arc::clone(&worker.streams);
            let worker = Arc::clone(worker);
            tokio::spawn(async move {
                let (ok, data) = match op_dispatch(
                    cache.as_deref(),
                    fetch.as_deref(),
                    transform.as_deref(),
                    None,
                    Some(Arc::clone(&streams)),
                    op,
                    &args,
                )
                .await
                {
                    Ok(v) => (true, v),
                    Err(e) => (
                        false,
                        serde_json::json!({ "code": e.0.code, "message": e.0.message }),
                    ),
                };
                let reply = WorkerMsg::OpResponse { id, ok, data };
                if let Ok(bytes) = frame(&reply) {
                    let mut stdin = worker.stdin.lock().await;
                    use tokio::io::AsyncWriteExt as _;
                    let _ = stdin.write_all(&bytes).await;
                    let _ = stdin.flush().await;
                }
            });
        }
        // Host→worker directions (Call; OpResponse per §4.5) and pongs are
        // not produced by workers on this channel; ignore defensively.
        WorkerMsg::Call { .. } | WorkerMsg::OpResponse { .. } | WorkerMsg::Pong { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_decode_vectors() {
        assert_eq!(b64_decode("").unwrap(), b"");
        assert_eq!(b64_decode("aGk=").unwrap(), b"hi");
        assert_eq!(b64_decode("YWJj").unwrap(), b"abc");
        assert_eq!(b64_decode("YWJjZA==").unwrap(), b"abcd");
        assert_eq!(b64_decode("0JLQv9Cw").unwrap(), "Впа".as_bytes());
        assert_eq!(b64_decode("IQ==").unwrap(), b"!");
        assert!(
            b64_decode("a").is_err(),
            "dangling single character is invalid"
        );
        assert!(b64_decode("a!c=").is_err(), "invalid character rejected");
        assert!(
            b64_decode("abc==").is_err(),
            "length not a multiple of four rejected"
        );
        assert!(
            b64_decode("a===").is_err(),
            "three padding characters rejected"
        );
    }

    #[test]
    fn percentile_nearest_rank() {
        assert_eq!(percentile(&[], 0.5), 0.0);
        assert_eq!(percentile(&[7.0], 0.99), 7.0);
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0], 0.5), 2.0);
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0], 0.99), 4.0);
    }
}

#[cfg(test)]
mod bug12_tests {
    use super::*;

    fn write_envelope(
        dir: &std::path::Path,
        name: &str,
        val: &serde_json::Value,
    ) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, serde_json::to_vec(val).unwrap()).unwrap();
        p
    }

    fn envelope_of(blob: &[u8]) -> serde_json::Value {
        use base64::Engine as _;
        serde_json::json!({
            "format": crate::SNAPSHOT_FORMAT,
            "oxide_version": env!("CARGO_PKG_VERSION"),
            "source_hash": "abc",
            "blob_b64": base64::engine::general_purpose::STANDARD.encode(blob),
        })
    }

    #[test]
    fn envelope_round_trip() {
        let dir = std::env::temp_dir().join(format!("ox-bug12-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = write_envelope(&dir, "snap.bin", &envelope_of(b"hello-blob"));
        let out = V8Host::load_snapshot_envelope(&p)
            .unwrap()
            .expect("blob present");
        assert_eq!(out, b"hello-blob");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_and_stale_blobs_rejected_with_snapver() {
        let dir = std::env::temp_dir().join(format!("ox-bug12-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Raw (pre-BUG-12) blob bytes.
        let raw = dir.join("raw.bin");
        std::fs::write(&raw, b"\x00\x01\x02not-json").unwrap();
        let e = V8Host::load_snapshot_envelope(&raw).unwrap_err();
        assert_eq!(e.0.code, "JSH-SNAPVER");
        // Wrong version.
        let mut bad = envelope_of(b"x");
        bad["oxide_version"] = serde_json::Value::String("0.0.0-stale".into());
        let p = write_envelope(&dir, "stale.bin", &bad);
        let e = V8Host::load_snapshot_envelope(&p).unwrap_err();
        assert_eq!(e.0.code, "JSH-SNAPVER");
        // Missing file is Ok(None), not an error — server still boots.
        let missing = dir.join("nope.bin");
        assert!(V8Host::load_snapshot_envelope(&missing).unwrap().is_none());
        // And from_snapshot_file downgrades loudly instead of failing.
        let host = V8Host::from_snapshot_file(&raw);
        assert!(host.snapshot.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod perf01_tests {
    use super::*;

    /// PERF-01: the `serialize` op encodes a JSON value to ODF and returns
    /// it base64-encoded; the bytes start with the ODF magic.
    #[tokio::test]
    async fn serialize_op_encodes_odf() {
        let args = serde_json::json!({ "value": { "a": 1, "b": [true, null] } });
        let out = op_dispatch(None, None, None, None, None, Op::Serialize, &args)
            .await
            .expect("serialize ok");
        let b64 = out["b64"].as_str().expect("b64 field");
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64)
            .expect("valid base64");
        assert_eq!(&bytes[..4], b"ODF1", "ODF magic present");
        // Round-trip through the decoder to prove it is real ODF.
        let decoded = oxide_fw_odf::decode(&bytes).expect("decodes");
        assert_eq!(
            decoded.to_json(),
            serde_json::json!({ "a": 1, "b": [true, null] })
        );
    }

    #[tokio::test]
    async fn serialize_op_requires_a_value() {
        let out = op_dispatch(
            None,
            None,
            None,
            None,
            None,
            Op::Serialize,
            &serde_json::json!({}),
        )
        .await;
        assert!(out.is_err(), "missing value is an error");
    }
}

mod perf05_tests {
    use super::*;
    use async_trait::async_trait;

    struct Echo;
    #[async_trait]
    impl FetchHandler for Echo {
        async fn fetch(
            &self,
            method: &str,
            url: &str,
            _h: &[(String, String)],
            _b: &[u8],
        ) -> Result<FetchResponse> {
            Ok(FetchResponse {
                status: 200,
                headers: vec![("x-method".into(), method.into())],
                body: url.as_bytes().to_vec(),
            })
        }
    }

    #[tokio::test]
    async fn fetch_op_delegates_to_the_handler() {
        let h = Echo;
        let args = serde_json::json!({ "url": "https://x.test/p", "method": "get" });
        let out = op_dispatch(None, Some(&h), None, None, None, Op::Fetch, &args)
            .await
            .expect("fetch ok");
        assert_eq!(out["status"], 200);
        assert_eq!(out["headers"][0][1], "GET");
        use base64::Engine as _;
        let body = base64::engine::general_purpose::STANDARD
            .decode(out["body_b64"].as_str().unwrap())
            .unwrap();
        assert_eq!(body, b"https://x.test/p");
    }

    #[tokio::test]
    async fn fetch_op_without_a_handler_errors() {
        let args = serde_json::json!({ "url": "https://x.test/" });
        assert!(
            op_dispatch(None, None, None, None, None, Op::Fetch, &args)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn fetch_op_memoizes_get() {
        let h = Echo;
        let memo = oxide_fw_cache::FetchMemo::default();
        let args = serde_json::json!({ "url": "https://x.test/memo", "method": "GET" });
        let _ = op_dispatch(None, Some(&h), None, Some(&memo), None, Op::Fetch, &args)
            .await
            .unwrap();
        // A second call for the same GET hits the memo (same body).
        let out = op_dispatch(None, Some(&h), None, Some(&memo), None, Op::Fetch, &args)
            .await
            .unwrap();
        assert_eq!(out["status"], 200);
    }
}

mod perf02_tests {
    use super::*;

    /// PERF-02: the same route always maps to the same worker index.
    #[test]
    fn route_affinity_is_stable_and_bounded() {
        let a1 = route_affinity(Some("routes/blog/[slug]"), 4);
        let a2 = route_affinity(Some("routes/blog/[slug]"), 4);
        assert_eq!(a1, a2, "stable across calls");
        assert!(a1.unwrap() < 4, "in bounds");
        // Distinct routes generally differ (not a guarantee, but a sanity
        // check that the hash spreads).
        let b = route_affinity(Some("routes/index"), 4);
        let c = route_affinity(Some("routes/about"), 4);
        assert!(b != c || a1 != b, "hash spreads across routes");
    }

    #[test]
    fn route_affinity_none_and_zero() {
        assert_eq!(route_affinity(None, 4), None);
        assert_eq!(route_affinity(Some("x"), 0), None);
    }
}

mod bug03_tests {
    use super::*;

    #[test]
    fn static_snapshot_leaks_once_and_is_stable() {
        let host = V8Host::with_snapshot(vec![1, 2, 3, 4]);
        let a = host.static_snapshot().expect("snapshot present");
        let b = host.static_snapshot().expect("snapshot present");
        assert_eq!(
            a.as_ptr(),
            b.as_ptr(),
            "the leaked snapshot must be stable across calls (BUG-03: leak once)"
        );
        assert_eq!(a, [1u8, 2, 3, 4].as_slice());
    }

    #[test]
    fn snapshot_less_host_returns_none() {
        let host = V8Host::new();
        assert!(host.static_snapshot().is_none());
        assert!(host.static_snapshot().is_none());
    }
}

#[cfg(test)]
mod p0_2_overflow_tests {
    use super::*;

    /// §4.3 / P0-2 / WP-r9 / WP-r10-3: the chunked-stream collect is
    /// capped at 8 MiB; an overflow returns `JSH-OVERFLOW` before the
    /// buffer grows past the cap. The `Complete` variant is exempt
    /// (already a `Bytes` the worker chose to send whole).
    #[tokio::test]
    async fn collect_chunked_overflow_returns_jsh_overflow() {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let mut stream = CallStream::Chunked(rx);

        // Spawn a producer that sends 9 chunks of 1 MiB each — 9 MiB total,
        // 1 MiB over the 8 MiB cap. The overflow check fires on the 9th.
        tokio::spawn(async move {
            let chunk = bytes::Bytes::from(vec![0u8; 1024 * 1024]);
            for _ in 0..9 {
                if tx.send(Ok(chunk.clone())).await.is_err() {
                    break;
                }
            }
        });

        let err = stream.collect().await.expect_err("overflow must fail");
        assert_eq!(err.0.code, "JSH-OVERFLOW", "got: {}", err.0.code);
        assert!(
            err.0.message.contains("8 MiB"),
            "message names the cap: {}",
            err.0.message
        );
    }

    /// The `Complete` variant is not capped — it's already a `Bytes` the
    /// worker chose to send whole, so the cap is a chunked-stream defence.
    #[tokio::test]
    async fn collect_complete_is_not_capped() {
        let big = bytes::Bytes::from(vec![0u8; 20 * 1024 * 1024]);
        let mut stream = CallStream::Complete(big.clone());
        let out = stream.collect().await.expect("Complete is exempt");
        assert_eq!(out.len(), big.len());
    }

    /// Under the cap: a chunked stream collects normally.
    #[tokio::test]
    async fn collect_chunked_under_cap_succeeds() {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let mut stream = CallStream::Chunked(rx);

        tokio::spawn(async move {
            let chunk = bytes::Bytes::from_static(b"hello");
            for _ in 0..3 {
                if tx.send(Ok(chunk.clone())).await.is_err() {
                    break;
                }
            }
        });

        let out = stream.collect().await.expect("under cap");
        assert_eq!(&out[..], b"hellohellohello");
    }
}

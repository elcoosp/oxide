//! oxide-jshost — JsHost trait, NodeHost backend, ops, pools (spec §4, D2).
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
use oxide_diagnostics::{Diagnostic, OxideError};
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

pub mod tooling;
pub mod v8_ops;
pub mod snapshot;
pub mod v8_loader;
pub use snapshot::{lint_top_level_side_effects, loader_options, snapshot_build, snapshot_build_bundle, snapshot_build_runtime, AppSnapshot, SideEffect, SNAPSHOT_FORMAT};
pub mod tooling_pool;
pub mod framework_host;
pub mod framework_renderer;
pub mod loader_host;
pub use framework_host::{FrameworkDescriptor, FrameworkHost};
pub use framework_renderer::{FrameworkHostRenderer, FrameworkRenderInput, FrameworkRenderer, RenderStream};
pub use loader_host::{ActionHost, ActionOutcome, LoaderHost, RevalidateEvent, ServerFnError, ServerFnHost};
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
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            size: 2,
            max_in_flight_per_worker: 64,
            recycle_after_requests: 10_000,
            heap_limit_bytes: 512 * 1024 * 1024,
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
}

/// A streaming result from a JS call.
pub enum CallStream {
    /// Whole payload at once (small responses, tooling results).
    Complete(Bytes),
    /// Chunked: channel of body chunks; end-of-stream = None.
    Chunked(tokio::sync::mpsc::Receiver<Result<Bytes>>),
}

impl CallStream {
    pub async fn collect(&mut self) -> Result<Bytes> {
        match self {
            CallStream::Complete(b) => Ok(b.clone()),
            CallStream::Chunked(rx) => {
                let mut buf = Vec::new();
                while let Some(chunk) = rx.recv().await {
                    buf.extend_from_slice(&chunk?);
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
    let len = json.len() as u32;
    if len > MAX_FRAME_LEN {
        return Err(err(
            "JSH-FRAME",
            format!("frame of {len} bytes exceeds the 64 MiB protocol limit (§4.2)"),
        ));
    }
    let mut out = Vec::with_capacity(4 + json.len());
    out.extend_from_slice(&len.to_le_bytes());
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
        if buf.len() - pos < 4 {
            break;
        }
        let len = u32::from_le_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]) as usize;
        if len > MAX_FRAME_LEN as usize {
            return Err(err(
                "JSH-FRAME",
                format!("declared frame length {len} exceeds the 64 MiB protocol limit (§4.2)"),
            ));
        }
        if buf.len() - pos < 4 + len {
            break; // partial frame: wait for more bytes
        }
        let msg: WorkerMsg = serde_json::from_slice(&buf[pos + 4..pos + 4 + len]).map_err(|e| {
            err(
                "JSH-JSON",
                format!("worker message is not valid protocol JSON: {e}"),
            )
        })?;
        msgs.push(msg);
        pos += 4 + len;
    }
    Ok((msgs, pos))
}

/// Least-loaded dispatch with route-affinity (§4.3).
///
/// Returns the index of the worker with the smallest load (ties → lowest index).
/// If `affinity` names an in-bounds worker whose load is at most `min + 1`, that
/// worker is preferred to keep JIT and module caches warm for hot routes.
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
    constructor(enc, opts) { this.encoding = enc || 'utf-8'; this.fatal = !!(opts && opts.fatal); }
    decode(buf) {
      if (buf == null) return '';
      const bytes = buf instanceof Uint8Array ? buf : new Uint8Array(buf.buffer || buf);
      let out = '';
      for (let i = 0; i < bytes.length;) {
        const b = bytes[i];
        let cp, len;
        if (b < 0x80) { cp = b; len = 1; }
        else if (b < 0xe0) { cp = b & 0x1f; len = 2; }
        else if (b < 0xf0) { cp = b & 0x0f; len = 3; }
        else { cp = b & 0x07; len = 4; }
        if (i + len > bytes.length) break;
        for (let j = 1; j < len; j++) cp = (cp << 6) | (bytes[i + j] & 0x3f);
        i += len;
        if (cp > 0xffff) {
          cp -= 0x10000;
          out += String.fromCharCode(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff));
        } else out += String.fromCharCode(cp);
      }
      return out;
    }
  };
}
// §4.6 tier 1: `MessageChannel`/`MessagePort`. React's browser SSR scheduler
// (react-dom/server.browser) schedules work through a MessageChannel. A
// macrotask-based port is enough: `postMessage` queues delivery after the
// current task, which is what the scheduler expects.
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
          try { fn(...args); } catch (e) { queueMicrotask(() => { throw e; }); }
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
};

// Sec.4.2 / Sec.4.4: the real V8 dispatch. `moduleUrl` is a `file://` URL the
// runtime's `FsModuleLoader` resolves; `exportName` is the export to call
// (`default` when empty); `payload` is the JSON argument. The result is a
// JSON envelope string: `{ok, value}` on success, `{__oxide_error}` on a
// module-import or export-call failure. Returned as a string so the Rust
// side reads it back without a serde_v8 dependency.
globalThis.__oxideDispatch = async function (kind, moduleUrl, exportName, payload) {
  let mod;
  try {
    mod = await import(moduleUrl);
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

// loader:run — import a route's `page.server.ts` (transpiled by the Rust
// loader) and call its `load(event)`. Mirrors the Node worker's
// dispatchLoaderRun: a full LoadEvent (cookies, setHeaders, parent, defer),
// with `error()`/`redirect()` sentinels turned into status + location.
globalThis.__oxideLoaderRun = async function (moduleUrl, input) {
  try {
    const mod = await import(moduleUrl);
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

    const event = {
      route: input.route || '',
      params: input.params || {},
      url: input.url || '/',
      request: { method: input.method || 'GET', url: input.url || '/', headers: hdrs },
      locals: {},
      cookies,
      setHeaders,
      parent: async () => ({}),
      defer,
      __oxideDeferredRegistry: registry,
    };
    let result;
    try {
      result = await load(event);
    } catch (err) {
      if (err && typeof err.status === 'number') {
        return JSON.stringify({ status: err.status, location: err.location ?? null,
          data: { message: err.message ?? '' }, setCookies: [...pending.values()], headers: extra });
      }
      throw err;
    }
    return JSON.stringify({ status: 200, location: null, data: result ?? {},
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
    const mod = await import(url);
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
globalThis.__oxideFrameworkRender = async function (configPath, renderInput) {
  try {
    const url = configPath.startsWith('file://')
      ? configPath
      : 'file://' + (configPath.startsWith('/') ? '' : '/') + configPath;
    const mod = await import(url);
    const cfg = (mod && mod.default) || mod;
    const fw = cfg && cfg.framework;
    if (!fw || typeof fw.render !== 'function') {
      return JSON.stringify({ __oxide_error: 'config has no framework.render' });
    }
    const stream = fw.render(renderInput || {});
    let html = '';
    const dec = new TextDecoder();
    for await (const chunk of stream) {
      html += typeof chunk === 'string' ? chunk : dec.decode(chunk, { stream: true });
    }
    html += dec.decode();
    return JSON.stringify({ html });
  } catch (e) {
    return JSON.stringify({
      __oxide_error: 'framework:render failed: ' + ((e && e.message) || e) +
        ' [reactLoadError=' + (globalThis.__oxideReactLoadError || 'none') + ']',
    });
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
}

fn spawn_v8_worker(
    snapshot: Option<&'static [u8]>,
    cache: Option<Arc<dyn oxide_cache::CacheHandler>>,
) -> Result<Arc<V8WorkerHandle>> {
    let (tx, mut rx) = mpsc::channel::<V8Msg>(64);
    let (init_tx, init_rx) = std::sync::mpsc::channel::<Result<()>>();
    let handle = Arc::new(V8WorkerHandle {
        tx,
        in_flight: AtomicU64::new(0),
        served: AtomicU64::new(0),
        pending: Mutex::new(HashMap::new()),
        retiring: AtomicBool::new(false),
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
                            let pending =
                                handle_for_thread.pending.lock().unwrap().remove(&id);
                            if let Some(p) = pending {
                                let _ = p.tx.send(res);
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
        let promise = js
            .execute_script("oxide:loader", script)
            .map_err(|e| err("JSH-V8", format!("loader dispatch: {e}")))?;
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
                input.payload.get("input").unwrap_or(&serde_json::Value::Null),
            )
            .unwrap_or_else(|_| "null".into());
            format!("globalThis.__oxideFrameworkRender({p}, {inp})")
        };
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
        // `framework:load` returns the descriptor object; `framework:render`
        // returns `{html}` and the caller wants the raw HTML bytes.
        let out = if entry.kind == "framework:render" {
            v.get("html")
                .and_then(|h| h.as_str())
                .map(|h| h.as_bytes().to_vec())
                .unwrap_or_default()
        } else {
            serde_json::to_vec(&v).unwrap_or_default()
        };
        return Ok(CallStream::Complete(Bytes::from(out)));
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
    let script =
        format!("globalThis.__oxideDispatch({kind}, {url}, {exp}, {payload})");

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
    let bytes = serde_json::to_vec(&out)
        .map_err(|e| err("JSH-V8", format!("serialize result: {e}")))?;
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
    cache_handler: Option<std::sync::Arc<dyn oxide_cache::CacheHandler>>,
}

impl Default for V8Host {
    fn default() -> Self {
        Self { snapshot: None, cache_handler: None }
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
        }
    }

    /// §9.3/§11 (BUG-02): attach the cache tier. Mirrors
    /// `NodeHost::with_cache_handler`; the handler reaches each worker's
    /// `oxide.cacheGet/Set/Tag` ops through a thread-local installed at spawn.
    pub fn with_cache_handler(
        mut self,
        handler: std::sync::Arc<dyn oxide_cache::CacheHandler>,
    ) -> Self {
        self.cache_handler = Some(handler);
        self
    }

    /// Read a snapshot blob from `path` and build a host around it. A missing
    /// or empty file yields a snapshot-less host — the caller still works.
    pub fn from_snapshot_file(path: &std::path::Path) -> Self {
        match std::fs::read(path) {
            Ok(b) if !b.is_empty() => Self::with_snapshot(b),
            _ => Self::default(),
        }
    }

    /// The `&'static` view of the snapshot, if any. Leaks the bytes on first
    /// call; a host is built once per process, so this is bounded.
    fn static_snapshot(&self) -> Option<&'static [u8]> {
        self.snapshot.as_ref().map(|b| {
            let leaked: &'static [u8] = Box::leak(b.as_slice().to_vec().into_boxed_slice());
            leaked
        })
    }

    fn build_pool(
        cfg: PoolConfig,
        snapshot: Option<&'static [u8]>,
        cache: Option<std::sync::Arc<dyn oxide_cache::CacheHandler>>,
    ) -> Result<Arc<V8Pool>> {
        let mut workers = Vec::with_capacity(cfg.size.max(1));
        for _ in 0..cfg.size.max(1) {
            workers.push(spawn_v8_worker(snapshot, cache.clone())?);
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
        }))
    }
}

impl JsHost for V8Host {
    fn render_pool(&self, cfg: PoolConfig) -> Result<Arc<dyn Pool>> {
        Self::build_pool(cfg, self.static_snapshot(), self.cache_handler.clone())
            .map(|p| p as Arc<dyn Pool>)
    }
    fn tooling_pool(&self, cfg: PoolConfig) -> Result<Arc<dyn Pool>> {
        Self::build_pool(cfg, self.static_snapshot(), self.cache_handler.clone())
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
    cache_handler: Option<std::sync::Arc<dyn oxide_cache::CacheHandler>>,
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
        cache: Option<std::sync::Arc<dyn oxide_cache::CacheHandler>>,
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
        let fresh = match spawn_v8_worker(snapshot, cache) {
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
                    if w.retiring.load(Ordering::Acquire) {
                        u64::MAX
                    } else {
                        w.in_flight.load(Ordering::Relaxed)
                    }
                })
                .collect();
            let idx = choose_worker(&loads, None);
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
            return Err(err("JSH-CRASH", format!("worker channel closed: {e}")));
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
                    tokio::spawn(async move {
                        V8Pool::spawn_replacement(workers, counters, worker_idx, snapshot, cache)
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
    cache: Mutex<Option<Arc<dyn oxide_cache::CacheHandler>>>,
}

/// Spawn `node <worker_script>` with piped stdio; returns (child, stdin, stdout).
/// Uses `tokio::process::Command` directly (tokio 1.53 has no `Child::from(std)`);
/// `spawn()` is sync but requires the caller to run inside a tokio runtime — pool
/// creation always happens there (§4.2 pools are built from async host code).
fn spawn_child(node_bin: &str, worker_script: &Path) -> Result<(Child, ChildStdin, ChildStdout)> {
    let mut cmd = tokio::process::Command::new(node_bin);
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
    pub fn with_cache_handler(
        self: &Arc<Self>,
        handler: Arc<dyn oxide_cache::CacheHandler>,
    ) {
        for w in &self.workers {
            *w.cache.lock().unwrap() = Some(Arc::clone(&handler));
        }
        // `replace_worker` swaps a worker's *pipes*, not the `Worker`
        // struct, so the handler on each worker survives a recycle without
        // any extra bookkeeping.
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
                format!("node binary `{node_bin}` is not available: {e} — install Node.js or pick another runtime (§4.6 tier 3)"),
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
    ) -> Result<Arc<Worker>> {
        let (child, stdin, stdout) = spawn_child(node_bin, worker_script)?;
        let worker = Arc::new(Worker {
            stdin: tokio::sync::Mutex::new(stdin),
            child: tokio::sync::Mutex::new(child),
            reader_handle: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            in_flight: AtomicU64::new(0),
            served: AtomicU64::new(0),
            dead: AtomicBool::new(false),
            pid: Mutex::new(None),
            recycle_lock: tokio::sync::Mutex::new(()),
            cache: Mutex::new(None),
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
        let (child, stdin, stdout) = spawn_child(&self.node_bin, &self.worker_script)?;
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
        // Least-loaded dispatch; no route-affinity for child workers (§4.3).
        let loads: Vec<u64> = self
            .workers
            .iter()
            .map(|w| w.in_flight.load(Ordering::Relaxed))
            .collect();
        let worker = Arc::clone(&self.workers[choose_worker(&loads, None)]);
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
    pub cache_handler: Option<Arc<dyn oxide_cache::CacheHandler>>,
}

impl NodeHost {
    pub fn new(worker_script: impl Into<std::path::PathBuf>) -> Self {
        Self {
            node_bin: "node".into(),
            worker_script: worker_script.into(),
            request_counter: AtomicU64::new(0),
            cache_handler: None,
        }
    }

    /// §9.3: attach a cache tier. Workers spawned by this host dispatch
    /// `cache_get` / `cache_set` / `cache_tag` to it.
    pub fn with_cache_handler(
        mut self,
        handler: Arc<dyn oxide_cache::CacheHandler>,
    ) -> Self {
        self.cache_handler = Some(handler);
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
        Ok(p as Arc<dyn Pool>)
    }
    fn tooling_pool(&self, cfg: PoolConfig) -> Result<Arc<dyn Pool>> {
        let p = NodePool::spawn(&self.node_bin, self.worker_script.clone(), cfg)?;
        if let Some(h) = &self.cache_handler {
            p.with_cache_handler(Arc::clone(h));
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
fn spawn_reader(
    mut stdout: ChildStdout,
    worker: Arc<Worker>,
    counters: Arc<PoolCounters>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf: Vec<u8> = Vec::with_capacity(8192);
        let mut chunk = [0u8; 8192];
        let mut streams: HashMap<u64, mpsc::Sender<Result<Bytes>>> = HashMap::new();
        loop {
            let (msgs, consumed) = match unframe(&buf) {
                Ok(x) => x,
                Err(e) => {
                    worker.dead.store(true, Ordering::Relaxed);
                    let code = e.0.code.clone();
                    let message = e.0.message.clone();
                    fail_all(&worker, &mut streams, code, message);
                    return;
                }
            };
            buf.drain(..consumed);
            for m in msgs {
                handle_worker_msg(m, &worker, &mut streams);
            }
            match stdout.read(&mut chunk).await {
                Ok(0) | Err(_) => {
                    worker.dead.store(true, Ordering::Relaxed);
                    fail_all(
                        &worker,
                        &mut streams,
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
fn fail_all(
    worker: &Worker,
    streams: &mut HashMap<u64, mpsc::Sender<Result<Bytes>>>,
    code: String,
    message: String,
) {
    streams.clear();
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
fn stream_sender(
    worker: &Worker,
    streams: &mut HashMap<u64, mpsc::Sender<Result<Bytes>>>,
    id: u64,
) -> Option<mpsc::Sender<Result<Bytes>>> {
    if let Some(tx) = streams.get(&id) {
        return Some(tx.clone());
    }
    let (tx, rx) = mpsc::channel(16);
    streams.insert(id, tx.clone());
    resolve(worker, id, Ok(CallStream::Chunked(rx)));
    Some(tx)
}

/// §4.5: run a worker-originated op against the host. Only the three cache
/// ops are implemented — a `fetch` / `stream_write` / `ws_*` request from a
/// worker answers with an "unimplemented" error, which the runtime sees as
/// an op failure and can fall back on.
async fn op_dispatch(
    cache: Option<&dyn oxide_cache::CacheHandler>,
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
                    // `cache_set` stored the value as JSON bytes. Decode
                    // them so the runtime receives the value itself, not the
                    // entry envelope — a caller does `const v = await
                    // cacheGet(key)` and expects `v` to be what it stored.
                    // A body that is not valid JSON is returned as the raw
                    // UTF-8 string, which is the honest best-effort for a
                    // value the tier round-trips as opaque bytes.
                    let value: serde_json::Value = serde_json::from_slice(&entry.body)
                        .unwrap_or_else(|_| {
                            serde_json::Value::String(
                                String::from_utf8_lossy(&entry.body).into_owned(),
                            )
                        });
                    Ok(value)
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
            let Some(body) = args.get("body") else {
                return Err(err("JSH-OP", "cache_set requires a `body` field"));
            };
            // `body` is a JSON value; serialize it to bytes.
            let bytes = serde_json::to_vec(body).map_err(|e| {
                err("JSH-OP", format!("cache_set body is not serializable: {e}"))
            })?;
            let tags: Vec<String> = args
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            let ttl_secs = args.get("ttl").and_then(|v| v.as_u64()).unwrap_or(0);
            let swr_secs = args.get("swr").and_then(|v| v.as_u64());
            let entry = oxide_cache::Entry {
                body: bytes,
                status: 200,
                headers: Vec::new(),
                tags,
                policy: oxide_cache::Policy {
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
            let input = oxide_compiler::TransformInput {
                path: path.to_string(),
                code: code.to_string(),
                // The loader path is server-side: the passes that matter
                // here (`'use cache'` on a `.server.ts`) run on `Ssr`.
                env: oxide_compiler::Env::Ssr,
                lang,
                // No bundle is being emitted on this path, so the id a
                // server-fn stub would compute is unused. `'use cache'`
                // emits `__oxide_cache(fn, module, export, opts)` — no
                // secret involved.
                build_secret: String::new(),
                dev: true,
                jsx_import_source: None,
            };
            match oxide_compiler::OxcTransformer.transform_passes_only(&input) {
                Ok(code) => Ok(serde_json::json!({ "code": code })),
                Err(e) => Err(err(
                    "JSH-COMPILE",
                    format!("compile_module failed for {path}: {}", e.0.message),
                )),
            }
        }
        _ => Err(err(
            "JSH-OP",
            format!("worker op `{}` is not implemented on this host", op_name(op)),
        )),
    }
}

fn handle_worker_msg(
    msg: WorkerMsg,
    worker: &Arc<Worker>,
    streams: &mut HashMap<u64, mpsc::Sender<Result<Bytes>>>,
) {
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
                        streams.remove(&id);
                        resolve(worker, id, Err(e));
                        return;
                    }
                }
            }
            if complete {
                streams.remove(&id);
                resolve(worker, id, Ok(CallStream::Complete(Bytes::from(body))));
            } else if let Some(tx) = stream_sender(worker, streams, id) {
                if tx.try_send(Ok(Bytes::from(body))).is_err() {
                    streams.remove(&id); // collector gone
                }
            }
        }
        WorkerMsg::CallChunk { id, data_b64 } => match b64_decode(&data_b64) {
            Ok(bytes) => {
                if let Some(tx) = stream_sender(worker, streams, id) {
                    if tx.try_send(Ok(Bytes::from(bytes))).is_err() {
                        streams.remove(&id); // collector gone: stop buffering
                    }
                }
            }
            Err(e) => {
                streams.remove(&id);
                resolve(worker, id, Err(e));
            }
        },
        WorkerMsg::CallEnd { id } => {
            streams.remove(&id); // dropping the sender closes the collector's stream
        }
        WorkerMsg::CallErr { id, diagnostic } => {
            streams.remove(&id);
            resolve(worker, id, Err(OxideError(diagnostic)));
        }
        // §4.5 / §9.3: a `cache_get` / `cache_set` / `cache_tag` request
        // from user code. The reader owns stdout; the response goes back on
        // the worker's stdin. Dispatch to the handler and write an
        // `op_response`. When no handler is configured, answer `null` so
        // the runtime's in-process fallback runs.
        WorkerMsg::OpRequest { id, op, args } => {
            let cache = worker.cache.lock().unwrap().clone();
            let worker = Arc::clone(worker);
            tokio::spawn(async move {
                let (ok, data) = match op_dispatch(cache.as_deref(), op, &args).await {
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

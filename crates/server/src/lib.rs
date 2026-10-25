//! oxide-fw-server — hyper pipeline, static assets, compression, TLS, rules (spec §12, §17, D7, D13).
//!
//! Request pipeline (§12.1): accept → limits + request id → static asset match
//! (immutable hashed, precompressed zstd/br/gzip, range + If-None-Match) → native rules →
//! route match + L0 → JS middleware only if present → dispatch (render/action/endpoint/
//! server-fn) → streaming response with dynamic compression + Server-Timing → cache fill.
//! Security defaults (§17.1) enforced in Rust before JS.

#![forbid(unsafe_code)]

pub mod loader;
pub mod metrics;
pub mod page_renderer;
/// §12.1: the QUIC server config the CLI passes to `serve_http3`. Re-exported
/// so the CLI does not depend on `quinn` directly.
pub use quinn::ServerConfig as H3ServerConfig;

pub use loader::{LoaderOutput, LoaderRunInput, LoaderRunner};
pub use page_renderer::{
    EndpointDispatcher, EndpointRunError, EndpointRunResult, MiddlewareDispatcher,
    MiddlewareRunError, MiddlewareRunResult, PageRenderInput, PageRenderer, RenderStream,
    SocketDispatcher, SocketRunError, SocketRunResult,
};
pub mod session;
pub use session::{Session, SessionCodec};
pub mod trace_context;

use bytes::Bytes;
use flate2::write::GzEncoder;
use futures::Stream;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, Limited};
use std::pin::Pin;

use hyper::body::Incoming;
use hyper::{Request as HyperRequest, Response as HyperResponse};
use oxide_fw_diagnostics::{Diagnostic, OxideError};
use oxide_fw_render::RequestTimings;
use oxide_fw_router::RulesEngine;
use std::collections::BTreeMap;
use std::io::Write as IoWrite;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

pub type Result<T> = std::result::Result<T, OxideError>;
pub use metrics::Metrics;

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Diagnostic::error(code, msg))
}

/// A platform-agnostic request handed to the pipeline (web-standard shape, G5).
#[derive(Debug)]
pub struct OxideRequest {
    pub method: String,
    pub url: String,
    /// P-14: shared across the dispatch clones (loader/action/trace) instead
    /// of a deep `Vec` clone per hop.
    pub headers: std::sync::Arc<Vec<(String, String)>>,
    /// P-14: lazily-built lowercased name → first index, so `header()` is
    /// O(1) after the first lookup instead of a linear scan per call.
    /// Derived header index (P-14). Public only so struct literals in tests
    /// can initialize it; treat it as an implementation detail.
    #[doc(hidden)]
    pub header_index: OnceLock<std::collections::HashMap<Box<str>, usize>>,
    pub body: Bytes,
    pub request_id: String,
    /// §20: the parsed W3C trace context from the incoming `traceparent`
    /// header, when present. Threaded into the fetch op so an outgoing
    /// request carries `traceparent` and a downstream service joins the
    /// same trace. `None` when the header is absent or malformed.
    pub trace: Option<crate::trace_context::TraceContext>,
    /// S-10: the socket peer address. Set by the accept loop (h1/h2 from
    /// `io.peer_addr()`, h3 from `conn.remote_address()`); `None` for a
    /// synthetic/in-process request. Used to derive an unspoofable rate-limit
    /// key instead of trusting `X-Forwarded-For`.
    pub peer: Option<std::net::IpAddr>,
    /// S-20: how many times this request has been re-dispatched by a
    /// middleware rewrite. Internal (never read from an inbound header), so a
    /// client cannot pre-set it to exhaust the rewrite budget.
    pub rewrite_hops: u8,
    /// B-22: the middleware module that issued the rewrite that produced this
    /// request. On re-dispatch the chain skips every layer up to (and
    /// including) that one, so outer middleware does not run twice. Internal,
    /// like `rewrite_hops` — never read from an inbound header.
    pub mw_resume_after: Option<String>,
}

impl Clone for OxideRequest {
    fn clone(&self) -> Self {
        Self {
            method: self.method.clone(),
            url: self.url.clone(),
            headers: std::sync::Arc::clone(&self.headers),
            // The index is derived from `headers`; rebuild lazily on the clone.
            header_index: OnceLock::new(),
            body: self.body.clone(),
            request_id: self.request_id.clone(),
            trace: self.trace.clone(),
            peer: self.peer,
            rewrite_hops: self.rewrite_hops,
            mw_resume_after: self.mw_resume_after.clone(),
        }
    }
}

impl OxideRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        let index = self.header_index.get_or_init(|| {
            let mut map = std::collections::HashMap::with_capacity(self.headers.len());
            for (i, (k, _)) in self.headers.iter().enumerate() {
                map.entry(k.to_ascii_lowercase().into_boxed_str())
                    .or_insert(i);
            }
            map
        });
        // Fast path: HTTP/2 and hyper's h1 parser hand us lowercase names.
        if let Some(i) = index.get(name) {
            return self.headers.get(*i).map(|(_, v)| v.as_str());
        }
        index
            .get(name.to_ascii_lowercase().as_str())
            .and_then(|i| self.headers.get(*i))
            .map(|(_, v)| v.as_str())
    }
}

/// SPEC-24 / §20: a header key that must never reach a log line.
pub fn is_secret_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    matches!(
        k.as_str(),
        "cookie" | "authorization" | "set-cookie" | "x-oxide-fw-dev-token"
    ) || k.contains("secret")
        || k.contains("token")
        || k.contains("password")
        || k.contains("api_key")
}

/// SPEC-24 / §20: one structured request log line with secret headers
/// redacted. Emitted by the response finisher; under `OXIDE_LOG=json` the
/// formatter renders it as a JSON object.
fn log_request(request_id: &str, method: &str, status: u16, headers: &[(String, String)]) {
    // P-13: gate before doing any formatting. With no INFO subscriber (the
    // common production case) this is a single relaxed check.
    if !tracing::enabled!(target: "oxide::request", tracing::Level::INFO) {
        return;
    }
    let mut redacted = String::with_capacity(headers.len() * 24);
    for (k, v) in headers {
        let val = if is_secret_key(k) {
            "[redacted]"
        } else {
            v.as_str()
        };
        redacted.push_str(k);
        redacted.push('=');
        redacted.push_str(val);
        redacted.push(' ');
    }
    tracing::info!(
        target: "oxide::request",
        request_id = request_id,
        method = method,
        status = status,
        headers = redacted.trim_end(),
        "request"
    );
}

/// A chunked response body stream. Each item is either a chunk of bytes
/// or an I/O error that aborts the response (hyper closes the
/// connection — a truncated 200 is never silently delivered).
pub type BodyStream =
    Pin<Box<dyn Stream<Item = std::result::Result<Bytes, std::io::Error>> + Send + Sync + 'static>>;

/// HTTP response produced by the pipeline. `body` is the collected form
/// (used by tests, by `json()` / `html()` constructors, and by any
/// response that isn't streamed). `stream`, when `Some`, is the
/// streaming form: `to_hyper_response` drives the response from it,
/// ignoring `body` and using chunked transfer encoding.
pub struct OxideResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
    pub stream: Option<BodyStream>,
}

impl Clone for OxideResponse {
    fn clone(&self) -> Self {
        // Streams cannot be cloned. Cloning a streaming response yields a
        // clone without the stream — the original still owns it. Documented
        // limitation: tests and callers that want a fresh copy use `.body`
        // or re-render.
        Self {
            status: self.status,
            headers: self.headers.clone(),
            body: self.body.clone(),
            stream: None,
        }
    }
}

impl std::fmt::Debug for OxideResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OxideResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .field("body_len", &self.body.len())
            .field("has_stream", &self.stream.is_some())
            .finish()
    }
}

impl OxideResponse {
    pub fn new(status: u16, body: impl Into<Bytes>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
            stream: None,
        }
    }
    /// Construct a streaming response. `to_hyper_response` drives the body
    /// from `stream` and does NOT set `content-length` (the transport uses
    /// chunked transfer encoding). Errors from `stream` abort the response.
    pub fn stream(status: u16, stream: BodyStream) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Bytes::new(),
            stream: Some(stream),
        }
    }

    pub fn with_header(mut self, k: &str, v: impl Into<String>) -> Self {
        self.headers.push((k.to_string(), v.into()));
        self
    }
    pub fn html(status: u16, body: impl Into<String>) -> Self {
        Self::new(status, body.into()).with_header("content-type", "text/html; charset=utf-8")
    }
    pub fn json(status: u16, body: serde_json::Value) -> Self {
        Self::new(status, serde_json::to_string(&body).unwrap())
            .with_header("content-type", "application/json")
    }
    /// Case-insensitive single-header lookup (test/inspection helper).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Request limits (§12.1 step 2, §17.1): enforced in Rust before JS.
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_body: usize,
    pub max_headers: usize,
    pub max_url_len: usize,
    pub multipart_parts: usize,
    pub slow_request_timeout_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_body: 32 * 1024 * 1024,
            max_headers: 200,
            max_url_len: 8 * 1024,
            multipart_parts: 1000,
            slow_request_timeout_ms: 30_000,
        }
    }
}

/// Static asset store (§12.1 step 3): immutable hashed assets, memory-mapped or
/// embedded, pre-compressed (zstd, br, gzip), range + If-None-Match support.
#[derive(Debug)]
pub struct StaticAssets {
    pub root: std::path::PathBuf,
    /// prebuilt map: url path → (file, etag, size, mtime).
    pub index: BTreeMap<String, AssetEntry>,
}

#[derive(Debug, Clone)]
pub struct AssetEntry {
    pub path: std::path::PathBuf,
    pub etag: String,
    pub size: u64,
    pub mtime: u64,
    /// Content type inferred by extension.
    pub content_type: &'static str,
    /// Available pre-compressed variants: encoding → file path.
    pub precompressed: BTreeMap<String, std::path::PathBuf>,
}

impl StaticAssets {
    /// Walk `root` recursively (hidden entries skipped) and index every file by its
    /// URL path `/<rel path>` (backslashes normalized). The etag is the first 16 hex
    /// chars of the blake3 content hash; precompressed siblings `<f>.zst|.br|.gz` map
    /// to `zstd`/`br`/`gzip`.
    pub fn scan(root: impl Into<std::path::PathBuf>) -> Result<Self> {
        let root = root.into();
        if !root.is_dir() {
            return Err(err(
                "SRV-ASSET",
                format!("asset root {} is not a directory", root.display()),
            ));
        }
        let mut index = BTreeMap::new();
        walk_assets(&root, &root, &mut index)?;
        Ok(Self { root, index })
    }

    /// Serve a request for an asset: 404/304/range/content-encoding negotiation.
    ///
    /// Semantics (§12.1 step 3, §11.3): `If-None-Match` → 304 with the etag +
    /// immutable cache-control; `Accept-Encoding` selects a precompressed variant
    /// (server preference zstd > br > gzip); single `bytes=a-b|a-|-n` ranges → 206
    /// (+ 416 for invalid/unsatisfiable). Documented simplification: a Range on a
    /// *compressed* variant serves the full negotiated representation with 200 —
    /// no partial decode. `HEAD` returns headers only.
    pub fn serve(&self, req: &OxideRequest, path: &str) -> Option<OxideResponse> {
        let entry = self.index.get(path)?;
        let etag_raw = entry.etag.as_str();
        let etag_h = format!("\"{etag_raw}\"");
        // §16.6: a service worker script must REVALIDATE on every fetch. A
        // year-long immutable `sw.js` keeps serving the old precache list —
        // an updated app never activates (the spec's PWA update story is
        // dead on arrival). `pwa-register.js` is the registration shim and
        // has the same requirement. Hashed build assets keep `immutable`
        // (their names change with their content).
        const SW_CACHE_CTL: &str = "public, max-age=0, must-revalidate";
        const CACHE_CTL: &str = "public, max-age=31536000, immutable";
        // §16.5 / WP-r9-7: the search index is an unhashed,
        // rebuilt-every-build file. `immutable` would pin a stale index in
        // every visitor's browser for a year. Same must-revalidate class
        // as the SW: cheap to re-check, correct when the build changes.
        let is_service_worker = path == "/sw.js"
            || path == "/pwa-register.js"
            || path == "/client/search-index.json"
            || path == "/search-index.json";
        let cache_ctl: &str = if is_service_worker {
            SW_CACHE_CTL
        } else {
            CACHE_CTL
        };

        if let Some(inm) = req.header("if-none-match") {
            let matches = inm.split(',').map(|c| c.trim()).any(|cand| {
                let cand = cand.strip_prefix("W/").unwrap_or(cand);
                cand == "*" || cand.trim_matches('"') == etag_raw
            });
            if matches {
                return Some(
                    OxideResponse::new(304, Bytes::new())
                        .with_header("ETag", etag_h)
                        .with_header("Cache-Control", cache_ctl)
                        .with_header("Vary", "Accept-Encoding"),
                );
            }
        }

        let is_get = req.method.eq_ignore_ascii_case("GET");
        let range = if is_get {
            req.header("range").map(|s| s.to_string())
        } else {
            None
        };
        let available: Vec<&str> = entry.precompressed.keys().map(|k| k.as_str()).collect();
        let negotiated = if is_get {
            negotiate_encoding(req.header("accept-encoding").unwrap_or(""), &available)
        } else {
            None
        };

        if let Some(enc) = negotiated.as_deref() {
            if let Some(variant) = entry.precompressed.get(enc) {
                if let Some(body) = cached_asset_body(variant) {
                    // Range on a compressed variant → full 200 (no partial decode).
                    let len = body.len().to_string();
                    let mut resp = OxideResponse::new(200, body);
                    resp = with_asset_headers(resp, entry, &etag_h, cache_ctl);
                    resp.headers
                        .push(("Content-Encoding".into(), enc.to_string()));
                    resp.headers.push(("Content-Length".into(), len));
                    return Some(resp);
                }
                // variant unreadable → fall through to the identity body
            }
        }

        let Some(full) = cached_asset_body(&entry.path) else {
            return None;
        };

        if req.method.eq_ignore_ascii_case("HEAD") {
            let mut resp = OxideResponse::new(200, Bytes::new());
            resp = with_asset_headers(resp, entry, &etag_h, cache_ctl);
            resp.headers
                .push(("Content-Length".into(), entry.size.to_string()));
            return Some(resp);
        }

        let mut resp = match &range {
            Some(spec) => match parse_range(spec, full.len() as u64) {
                Some((a, b)) => {
                    let slice = &full[a as usize..=(b as usize)];
                    OxideResponse::new(206, Bytes::copy_from_slice(slice))
                        .with_header("Content-Range", format!("bytes {a}-{b}/{}", full.len()))
                }
                None => OxideResponse::new(416, Bytes::new())
                    .with_header("Content-Range", format!("bytes */{}", full.len())),
            },
            None => OxideResponse::new(200, full.clone()),
        };
        resp = with_asset_headers(resp, entry, &etag_h, cache_ctl);
        Some(resp)
    }
}

/// P-12: process-wide cache of asset bodies, keyed by file path. The asset
/// index is content-addressed and immutable for a build, so a cached body can
/// never go stale within a process. Bounded by total bytes.
/// A5: the asset-body cache TTL. 300s by default; `OXIDE_ASSET_CACHE_TTL_SECS`
/// overrides it (a test sets it short to prove expiry re-reads).
pub fn asset_cache_ttl() -> std::time::Duration {
    std::env::var("OXIDE_ASSET_CACHE_TTL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or_else(|| std::time::Duration::from_secs(300))
}

fn asset_body_cache() -> &'static moka::sync::Cache<std::path::PathBuf, Bytes> {
    static CACHE: std::sync::OnceLock<moka::sync::Cache<std::path::PathBuf, Bytes>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| {
        moka::sync::Cache::builder()
            .max_capacity(256 * 1024 * 1024)
            .weigher(|_k, v: &Bytes| v.len().min(u32::MAX as usize) as u32)
            // A5: a dev rebuild replaces dist/client in place; without a TTL
            // the cache served the previous body indefinitely.
            .time_to_live(asset_cache_ttl())
            .build()
    })
}

/// P-12: return the asset body from the cache, reading it once on a miss.
fn cached_asset_body(path: &std::path::Path) -> Option<Bytes> {
    let cache = asset_body_cache();
    if let Some(b) = cache.get(path) {
        return Some(b);
    }
    let bytes = std::fs::read(path).ok()?;
    let b = Bytes::from(bytes);
    cache.insert(path.to_path_buf(), b.clone());
    Some(b)
}

fn with_asset_headers(
    mut resp: OxideResponse,
    entry: &AssetEntry,
    etag_h: &str,
    cache_ctl: &str,
) -> OxideResponse {
    resp.headers
        .push(("Content-Type".into(), entry.content_type.to_string()));
    resp.headers.push(("ETag".into(), etag_h.to_string()));
    resp.headers
        .push(("Cache-Control".into(), cache_ctl.to_string()));
    resp.headers.push(("Vary".into(), "Accept-Encoding".into()));
    resp.headers.push(("Accept-Ranges".into(), "bytes".into()));
    resp
}

/// Recursive asset walk (hidden files and directories are skipped).
fn walk_assets(
    root: &std::path::Path,
    dir: &std::path::Path,
    index: &mut BTreeMap<String, AssetEntry>,
) -> Result<()> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| err("SRV-ASSET", format!("read {}: {e}", dir.display())))?;
    for entry in entries {
        let entry = entry.map_err(|e| err("SRV-ASSET", format!("dir entry: {e}")))?;
        let path = entry.path();
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if ft.is_dir() {
            walk_assets(root, &path, index)?;
            continue;
        }
        if !ft.is_file() {
            continue;
        }
        let content = match std::fs::read(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let rel = path.strip_prefix(root).unwrap_or(&path);
        let url = format!("/{}", rel.to_string_lossy().replace('\\', "/"));
        let etag = blake3::hash(&content).to_hex()[..16].to_string();
        let md = std::fs::metadata(&path).ok();
        let mtime = md
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut precompressed = BTreeMap::new();
        for (ext, enc) in [("zst", "zstd"), ("br", "br"), ("gz", "gzip")] {
            let mut sibling = path.clone().into_os_string();
            sibling.push(format!(".{ext}"));
            let sibling = std::path::PathBuf::from(sibling);
            if sibling.is_file() {
                precompressed.insert(enc.to_string(), sibling);
            }
        }
        index.insert(
            url,
            AssetEntry {
                path,
                etag,
                size: content.len() as u64,
                mtime,
                content_type: content_type_for(&entry.file_name().to_string_lossy()),
                precompressed,
            },
        );
    }
    Ok(())
}

fn content_type_for(file_name: &str) -> &'static str {
    let ext = file_name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Content negotiation for compression (§12.1 step 8): zstd preferred, then br, gzip.
///
/// Parses q-values (`br;q=0.8, gzip`); returns the first available encoding (in server
/// preference order) whose client q-value > 0 (exact token wins over `*`, which carries
/// a default q of 1). Returns `None` for identity-only clients.
/// PERF-06: is a content type worth compressing? Text-ish types (HTML, CSS,
/// JS, JSON, SVG, XML) shrink well; already-compressed types (images, fonts,
/// video, wasm) do not, and compressing them wastes CPU for no gain.
fn is_compressible(content_type: &str) -> bool {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    ct.starts_with("text/")
        || ct == "application/json"
        || ct == "application/javascript"
        || ct == "application/oxide+odf"
        || ct == "application/xml"
        || ct == "image/svg+xml"
        || ct.ends_with("+json")
        || ct.ends_with("+xml")
}

pub fn negotiate_encoding(accept_encoding: &str, available: &[&str]) -> Option<String> {
    let mut entries: Vec<(String, f64)> = Vec::new();
    for part in accept_encoding.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (token, q) = match part.split_once(';') {
            Some((t, params)) => {
                let q = params
                    .split(';')
                    .find_map(|p| {
                        p.trim()
                            .strip_prefix("q=")
                            .or_else(|| p.trim().strip_prefix("Q="))
                    })
                    .and_then(|v| v.trim().parse::<f64>().ok())
                    .unwrap_or(0.0);
                (t.trim().to_ascii_lowercase(), q)
            }
            None => (part.to_ascii_lowercase(), 1.0),
        };
        entries.push((token, q));
    }
    let q_of = |token: &str| -> Option<f64> {
        entries
            .iter()
            .find(|(t, _)| t == token)
            .map(|(_, q)| *q)
            .or_else(|| entries.iter().find(|(t, _)| t == "*").map(|(_, q)| *q))
    };
    for candidate in ["zstd", "br", "gzip"] {
        if !available.iter().any(|a| a.eq_ignore_ascii_case(candidate)) {
            continue;
        }
        if q_of(candidate).is_some_and(|q| q > 0.0) {
            return Some(candidate.to_string());
        }
    }
    None
}

/// P-15: a `std::io::Write` sink that forwards compressed bytes to the
/// response stream as they are produced, so the first chunk is flushed at
/// shell speed instead of after the whole page has rendered.
struct ChunkSink {
    tx: tokio::sync::mpsc::Sender<std::result::Result<Bytes, std::io::Error>>,
}

impl std::io::Write for ChunkSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if !buf.is_empty()
            && self
                .tx
                .blocking_send(Ok(Bytes::copy_from_slice(buf)))
                .is_err()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "response stream dropped",
            ));
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

enum StreamEncoder {
    Zstd(Box<zstd::stream::write::Encoder<'static, ChunkSink>>),
    Gzip(flate2::write::GzEncoder<ChunkSink>),
}

impl StreamEncoder {
    fn new(encoding: &str, sink: ChunkSink) -> std::io::Result<Self> {
        match encoding.to_ascii_lowercase().as_str() {
            "zstd" => Ok(StreamEncoder::Zstd(Box::new(
                zstd::stream::write::Encoder::new(sink, 3)?,
            ))),
            _ => Ok(StreamEncoder::Gzip(flate2::write::GzEncoder::new(
                sink,
                flate2::Compression::new(3),
            ))),
        }
    }
}

impl std::io::Write for StreamEncoder {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            StreamEncoder::Zstd(w) => w.write(buf),
            StreamEncoder::Gzip(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            StreamEncoder::Zstd(w) => w.flush(),
            StreamEncoder::Gzip(w) => w.flush(),
        }
    }
}

impl StreamEncoder {
    fn finish(self) -> std::io::Result<()> {
        match self {
            StreamEncoder::Zstd(w) => w.finish().map(|_| ()),
            StreamEncoder::Gzip(w) => w.finish().map(|_| ()),
        }
    }
}

/// P-15: wrap a streaming body in a background compressor. The blocking
/// encoder runs on the blocking pool (zstd/gzip are not async) and is fed
/// chunk-by-chunk; each `flush` surfaces bytes immediately so TTFB is the
/// shell's, not the page's.
fn spawn_stream_compressor(encoding: &str, mut stream: BodyStream) -> BodyStream {
    use futures::StreamExt as _;
    let (in_tx, in_rx) =
        tokio::sync::mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(4);
    let (out_tx, out_rx) =
        tokio::sync::mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(8);
    let err_tx = out_tx.clone();
    let enc = encoding.to_string();
    tokio::task::spawn_blocking(move || {
        let mut writer = match StreamEncoder::new(&enc, ChunkSink { tx: out_tx }) {
            Ok(w) => w,
            Err(e) => {
                let _ = err_tx.blocking_send(Err(e));
                return;
            }
        };
        let mut rx = in_rx;
        while let Some(item) = rx.blocking_recv() {
            match item {
                Ok(chunk) => {
                    if writer.write_all(&chunk).is_err() {
                        break;
                    }
                    let _ = writer.flush();
                }
                Err(e) => {
                    let _ = err_tx.blocking_send(Err(e));
                    break;
                }
            }
        }
        let _ = writer.finish();
    });
    tokio::spawn(async move {
        while let Some(item) = stream.next().await {
            if in_tx.send(item).await.is_err() {
                break;
            }
        }
    });
    Box::pin(tokio_stream::wrappers::ReceiverStream::new(out_rx))
}

/// Dynamic compression of a body chunk stream, flushed for first byte (§12.1 step 8).
///
/// Deviation (documented): brotli is not compiled into this crate, so `"br"` fails with
/// `SRV-NOENC` naming the `brotli` feature flag; precompressed `.br` assets are still
/// served via [`StaticAssets::serve`] without runtime compression.
pub fn compress_body(encoding: &str, body: &[u8], level: i32) -> Result<Bytes> {
    match encoding.to_ascii_lowercase().as_str() {
        "identity" | "" => Ok(Bytes::copy_from_slice(body)),
        "gzip" => {
            let lvl = level.clamp(1, 9) as u32;
            let mut enc = GzEncoder::new(Vec::new(), flate2::Compression::new(lvl));
            enc.write_all(body)
                .map_err(|e| err("SRV-COMP", format!("gzip failed: {e}")))?;
            let out = enc
                .finish()
                .map_err(|e| err("SRV-COMP", format!("gzip failed: {e}")))?;
            Ok(Bytes::from(out))
        }
        "zstd" => {
            let lvl = level.clamp(1, 19);
            let out = zstd::bulk::compress(body, lvl)
                .map_err(|e| err("SRV-COMP", format!("zstd failed: {e}")))?;
            Ok(Bytes::from(out))
        }
        "br" => {
            // brotli's quality range is 0..=11; we clamp to the pragmatic band and
            // map level 0 → quality 4 (the fastest sane setting for a per-request
            // dynamic path — the spec's `zstd`-preferred ordering is honoured by
            // the caller, brotli is the second choice).
            let q = if level <= 0 {
                4
            } else {
                level.clamp(1, 11) as u32
            };
            let mut out = Vec::with_capacity(body.len() / 2);
            {
                let mut w = brotli::CompressorWriter::new(&mut out, 4096, q, 22);
                w.write_all(body)
                    .map_err(|e| err("SRV-COMP", format!("brotli failed: {e}")))?;
            }
            Ok(Bytes::from(out))
        }
        other => Err(err(
            "SRV-NOENC",
            format!("unsupported content encoding `{other}`"),
        )),
    }
}

/// Security headers (§17.1): X-Content-Type-Options, Referrer-Policy,
/// Permissions-Policy, COOP, HSTS on TLS, frame protections. Applied unless disabled.
///
/// The CSP header is appended only when a nonce is given (strict mode, via
/// [`csp_header`]); pass `None` for `off`/`hash` modes and append the hash policy
/// separately.
pub fn security_headers(tls: bool, csp_nonce: Option<&str>) -> Vec<(String, String)> {
    let mut h: Vec<(String, String)> = [
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "strict-origin-when-cross-origin"),
        (
            "Permissions-Policy",
            "camera=(), microphone=(), geolocation=()",
        ),
        ("Cross-Origin-Opener-Policy", "same-origin"),
        ("X-Frame-Options", "DENY"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    if tls {
        h.push((
            "Strict-Transport-Security".into(),
            "max-age=63072000; includeSubDomains".into(),
        ));
    }
    if let Some(n) = csp_nonce {
        h.push(("Content-Security-Policy".into(), csp_header("strict", n)));
    }
    h
}

/// P-16: cached `Alt-Svc` value keyed by the (fixed) h3 port. The port never
/// changes for a running server, so the `format!` runs once.
fn alt_svc_for(port: u16) -> &'static str {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<std::collections::HashMap<u16, &'static str>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(v) = guard.get(&port) {
            return v;
        }
    }
    let leaked: &'static str = Box::leak(format!("h3=\":{port}\"; ma=86400").into_boxed_str());
    if let Ok(mut guard) = cache.lock() {
        guard.insert(port, leaked);
    }
    leaked
}

/// Strict CSP with nonce + strict-dynamic (§17.1); hash mode for static pages.
/// `off` (and any unknown mode) yields an empty string — the header is absent.
pub fn csp_header(mode: &str, nonce: &str) -> String {
    match mode {
        "strict" => format!(
            "default-src 'self'; script-src 'self' 'nonce-{nonce}' 'strict-dynamic'; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'self'"
        ),
        "hash" => "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'self'".into(),
        _ => String::new(),
    }
}

/// CSRF checks (§17.1): POST-only actions/server-fns; Origin/Sec-Fetch-Site verification;
/// SameSite cookies; optional double-submit token.
///
/// Safe methods (`GET`/`HEAD`/`OPTIONS`) and `mode: "off"` always pass. In `origin`
/// mode: `Sec-Fetch-Site: same-origin|none` passes; an explicit `cross-site` signal
/// fails; a present `Origin` must match the `Host` authority; a **missing Origin** on a
/// same-origin fetch is allowed (fail-open for non-browser clients — documented, since
/// every cross-origin browser fetch carries `Origin`, and explicit cross-site signals
/// are caught above). In `token` mode a failing origin check is rescued by the
/// double-submit pair `x-oxide-csrf` header == `oxide_csrf` cookie.
pub fn csrf_check(req: &OxideRequest, mode: &str) -> std::result::Result<(), Diagnostic> {
    let method = req.method.to_ascii_uppercase();
    if matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS") {
        return Ok(());
    }
    if mode.eq_ignore_ascii_case("off") {
        return Ok(());
    }
    let site = req
        .header("sec-fetch-site")
        .map(|s| s.trim().to_ascii_lowercase());
    if let Some(site) = &site {
        if site == "same-origin" || site == "none" {
            return Ok(());
        }
        if site == "cross-site" {
            return csrf_reject(req, mode);
        }
        // "same-site" and unknown values fall through to the Origin comparison
    }
    if let Some(origin) = req.header("origin") {
        if origin_authority(origin) == host_authority(req.header("host").unwrap_or("")) {
            return Ok(());
        }
        return csrf_reject(req, mode);
    }
    // Missing Origin on a same-origin fetch → allow (documented).
    Ok(())
}

fn csrf_reject(req: &OxideRequest, mode: &str) -> std::result::Result<(), Diagnostic> {
    if mode.eq_ignore_ascii_case("token") {
        let header = req.header("x-oxide-csrf");
        let cookie = cookie_value(req.header("cookie").unwrap_or(""), "oxide_csrf");
        if let (Some(h), Some(c)) = (header, cookie) {
            // SEC-04: constant-time compare (the `==` rescue leaked the
            // token prefix via timing).
            if constant_time_eq(h.as_bytes(), c.as_bytes()) {
                return Ok(());
            }
        }
    }
    Err(Diagnostic::error(
        "SRV-CSRF",
        "cross-origin request rejected by Origin/Sec-Fetch-Site verification",
    ))
}

pub fn cookie_value<'a>(cookie_header: &'a str, name: &str) -> Option<&'a str> {
    for pair in cookie_header.split(';') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=') {
            if k.trim() == name {
                return Some(v.trim());
            }
        }
    }
    None
}

/// Authority (host[:port]) of an `Origin` header value.
fn origin_authority(origin: &str) -> String {
    let rest = origin.split_once("://").map(|(_, r)| r).unwrap_or(origin);
    let rest = rest.split('/').next().unwrap_or(rest);
    host_authority(rest)
}

/// Lowercased authority with a trailing `:port` stripped only when it is the port form.
fn host_authority(h: &str) -> String {
    strip_port(h.trim()).to_ascii_lowercase()
}

/// Strip a `:port` suffix (bracketed IPv6 aware); IPv6 literals without brackets pass.
fn strip_port(h: &str) -> &str {
    if let Some(rest) = h.strip_prefix('[') {
        return match rest.find(']') {
            Some(end) => &rest[..end],
            None => h,
        };
    }
    match h.rfind(':') {
        Some(i)
            if !h[..i].contains(':')
                && !h[i + 1..].is_empty()
                && h[i + 1..].bytes().all(|b| b.is_ascii_digit()) =>
        {
            &h[..i]
        }
        _ => h,
    }
}

/// SSRF protection (§17.1): block private/loopback/link-local ranges unless allow-listed,
/// re-validate after redirects + DNS resolution. Used by image optimizer + fetch op.
///
/// Literal IPs in the blocked ranges fail; hostnames `localhost` (any case),
/// `*.local` and `metadata.google.internal` fail. DNS re-resolution after redirects is
/// the caller's responsibility (the fetch op re-checks the resolved address).
pub fn ssrf_allowed(host: &str, allow_list: &[String]) -> bool {
    let host = strip_port(host.trim());
    if allow_list
        .iter()
        .any(|a| a.as_str().eq_ignore_ascii_case(host))
    {
        return true;
    }
    let lower = host.to_ascii_lowercase();
    if let Ok(ip) = lower.parse::<std::net::IpAddr>() {
        return !is_blocked_ip(&ip);
    }
    if lower == "localhost" || lower.ends_with(".localhost") {
        return false;
    }
    if lower.ends_with(".local") {
        return false;
    }
    if lower == "metadata.google.internal" || lower.ends_with(".metadata.google.internal") {
        return false;
    }
    true
}

fn is_blocked_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_unspecified()
                || o[0] == 10
                || o[0] == 127
                || (o[0] == 172 && (16..=31).contains(&o[1]))
                || (o[0] == 192 && o[1] == 168)
                || (o[0] == 169 && o[1] == 254)
        }
        std::net::IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_blocked_ip(&std::net::IpAddr::V4(mapped));
            }
            let seg = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || (seg[0] & 0xfe00) == 0xfc00 // fc00::/7 unique local
                || (seg[0] & 0xffc0) == 0xfe80 // fe80::/10 link local
        }
    }
}

/// SEC-05: parse a CIDR (`10.0.0.0/8`, `127.0.0.1/32`, `::1/128`) into
/// (network, prefix_len). A bare IP (no `/`) is treated as a /32 or /128.
pub fn parse_cidr(s: &str) -> Option<(std::net::IpAddr, u8)> {
    let (ip_s, len_s) = match s.split_once('/') {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };
    let ip: std::net::IpAddr = ip_s.trim().parse().ok()?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let len: u8 = match len_s {
        Some(l) => l.trim().parse().ok()?,
        None => max,
    };
    (len <= max).then_some((ip, len))
}

pub fn ip_in_cidr(ip: std::net::IpAddr, net: std::net::IpAddr, len: u8) -> bool {
    match (ip, net) {
        (std::net::IpAddr::V4(a), std::net::IpAddr::V4(n)) => {
            let mask = if len == 0 { 0 } else { u32::MAX << (32 - len) };
            (u32::from(a) & mask) == (u32::from(n) & mask)
        }
        (std::net::IpAddr::V6(a), std::net::IpAddr::V6(n)) => {
            let a = u128::from(a);
            let n = u128::from(n);
            let mask = if len == 0 {
                0
            } else {
                u128::MAX << (128 - len)
            };
            (a & mask) == (n & mask)
        }
        _ => false,
    }
}

/// SEC-05: is `peer` inside any of the `trusted_proxies` CIDRs?
pub fn peer_is_trusted(peer: std::net::IpAddr, trusted: &[String]) -> bool {
    trusted
        .iter()
        .any(|c| parse_cidr(c).is_some_and(|(net, len)| ip_in_cidr(peer, net, len)))
}

/// SEC-05: derive the effective client IP / scheme / host. A trusted peer has
/// its `X-Forwarded-For` (rightmost-untrusted — we take the leftmost entry
/// here, the common single-proxy case), `X-Forwarded-Proto`, and
/// `X-Forwarded-Host` honoured; an untrusted peer has them **ignored**
/// entirely (the header values are never trusted from an arbitrary client).
pub fn effective_client<'a>(
    peer: std::net::IpAddr,
    trusted: &[String],
    xff: Option<&'a str>,
    xf_proto: Option<&'a str>,
    xf_host: Option<&'a str>,
    real_host: Option<&'a str>,
) -> (Option<&'a str>, Option<&'a str>, Option<&'a str>) {
    if peer_is_trusted(peer, trusted) {
        let ip = xff.and_then(|v| v.split(',').next()).map(|s| s.trim());
        (ip, xf_proto, xf_host.or(real_host))
    } else {
        (None, None, real_host)
    }
}

/// SEC-09: mint a dev session token — 16 bytes from the OS CSPRNG, hex. The
/// CLI prints it in the dev banner; the pipeline requires it for `/__oxide`.
pub fn dev_token() -> String {
    use ring::rand::SecureRandom as _;
    let rng = ring::rand::SystemRandom::new();
    let mut bytes = [0u8; 16];
    if rng.fill(&mut bytes).is_ok() {
        let mut s = String::with_capacity(32);
        for b in bytes {
            s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
            s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
        }
        return s;
    }
    // Fallback (never on hosted platforms): the request-id counter path.
    format!(
        "{:016x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    )
}

/// SEC-09: does the request carry the dev session token? Accepts it as a
/// `token=` query parameter or the `x-oxide-fw-dev-token` header. Constant-time
/// compare (the token is a secret).
fn dev_token_ok(req: &OxideRequest, want: &str) -> bool {
    let from_header = req.header("x-oxide-fw-dev-token");
    let from_query = req.url.split_once('?').map(|(_, q)| q).and_then(|q| {
        q.split('&')
            .find_map(|kv| kv.strip_prefix("token=").map(|v| v.to_string()))
    });
    let given = from_header.map(str::to_string).or(from_query);
    match given {
        Some(g) => {
            let a = g.as_bytes();
            let b = want.as_bytes();
            a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
        }
        None => false,
    }
}

/// SEC-09: is `host` acceptable for a dev server bound to `bind_ip`? Dev
/// accepts `localhost`, the loopback literals, and the bind IP itself. A
/// request for any other Host (a DNS-rebinding attempt against a dev server)
/// is rejected.
pub fn dev_host_allowed(host_header: &str, bind_ip: std::net::IpAddr) -> bool {
    let host = host_authority(host_header);
    if host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]" {
        return true;
    }
    if let Ok(ip) = strip_port(host_header.trim()).parse::<std::net::IpAddr>() {
        return ip == bind_ip;
    }
    // A non-local hostname is only acceptable when it is the bind IP's text.
    host == bind_ip.to_string()
}

/// S-03 / S-04: does an `Origin` header match the request authority (or, when
/// configured, the Host allow-list)? A missing/malformed Origin is rejected
/// unless `allow_missing` (non-browser clients in dev). Browsers always send
/// `Origin` on a WebSocket handshake, so this blocks cross-site WS hijacking.
pub fn ws_origin_ok(
    origin: Option<&str>,
    host_header: Option<&str>,
    allow_list: &[String],
    allow_missing: bool,
) -> bool {
    let Some(origin) = origin else {
        return allow_missing;
    };
    // Extract the authority from `scheme://authority[/...]` without pulling
    // the `http` crate as a direct dependency.
    let after_scheme = match origin.split_once("://") {
        Some((_, rest)) => rest,
        None => origin,
    };
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    let o_host = strip_port(authority.trim());
    if o_host.is_empty() {
        return false;
    }
    if let Some(h) = host_header.map(host_authority) {
        if o_host.eq_ignore_ascii_case(&h) {
            return true;
        }
    }
    allow_list.iter().any(|a| a.eq_ignore_ascii_case(o_host))
}

/// Host validation (§17.1): Host allow-list + trusted-proxy + X-Forwarded-* handling.
/// An empty allow-list allows everything; entries match exactly (any case, port
/// stripped) or as leading-dot suffixes (`.example.com` matches `sub.example.com`).
pub fn host_allowed(host_header: &str, allow_list: &[String]) -> bool {
    if allow_list.is_empty() {
        return true;
    }
    let host = strip_port(host_header.trim()).to_ascii_lowercase();
    allow_list.iter().any(|entry| {
        let entry = entry.trim().to_ascii_lowercase();
        if let Some(dot) = entry.strip_prefix('.') {
            host.ends_with(&format!(".{dot}"))
        } else {
            entry == host
        }
    })
}

fn cookie_mac(key: &[u8], name: &str, value: &str) -> String {
    let mut h = blake3::Hasher::new();
    h.update(key);
    h.update(b".");
    h.update(name.as_bytes());
    h.update(b".");
    h.update(value.as_bytes());
    h.finalize().to_hex()[..24].to_string()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Cookie primitive (§16.6): signed cookies with key rotation.
///
/// Wire format: `name.value.mac` where `mac = blake3(key.name.value)` truncated to 24
/// hex chars (hmac-style, keyed by the secret). The first key signs; verification tries
/// every key in order so rotation is seamless. An empty key list uses the empty key.
///
/// S-21.2: not yet wired to the user-facing `cookie` API (that lands with the
/// session read/write-back work). Hidden so no downstream crate treats it as a
/// stable surface before it is.
#[doc(hidden)]
pub fn sign_cookie(name: &str, value: &str, keys: &[&[u8]]) -> String {
    let key = keys.first().copied().unwrap_or(&[]);
    format!("{name}.{value}.{}", cookie_mac(key, name, value))
}

#[doc(hidden)]
pub fn verify_cookie(signed: &str, keys: &[&[u8]]) -> Option<(String, String)> {
    let first = signed.find('.')?;
    let last = signed.rfind('.')?;
    if last <= first {
        return None;
    }
    let name = &signed[..first];
    let value = &signed[first + 1..last];
    let mac = &signed[last + 1..];
    let empty_keys: &[&[u8]] = &[&[]];
    let keys = if keys.is_empty() { empty_keys } else { keys };
    for key in keys {
        if constant_time_eq(cookie_mac(key, name, value).as_bytes(), mac.as_bytes()) {
            return Some((name.to_string(), value.to_string()));
        }
    }
    None
}

/// Token-bucket rate limiter (§17.1 native rule type, usable without JS).
#[derive(Default)]
pub struct RateLimiter {
    pub buckets: dashmap_like::Buckets,
}

pub mod dashmap_like {
    pub type Buckets =
        std::sync::Mutex<std::collections::BTreeMap<String, (f64, std::time::Instant)>>;
}

impl RateLimiter {
    /// Returns true if allowed under `max` per `window_secs` for `key`.
    ///
    /// Token bucket: capacity `max`, starts full, refills continuously at
    /// `max / window_secs` tokens per second (`window_secs` clamped to ≥ 1).
    pub fn allow(&self, key: &str, max: u32, window_secs: u64) -> bool {
        let mut map = self.buckets.lock().unwrap();
        let now = Instant::now();
        // A3 / S-10: bound the bucket map. Keys are attacker-influenced (the
        // peer IP or a forwarded address), so without eviction a long-lived
        // process accumulates one entry per address forever. Drop buckets
        // idle longer than the largest window we can observe (one hour) once
        // the map grows past a sane bound. A bucket that is evicted while
        // still active simply starts full again — the same coarse limit the
        // token bucket already has between processes.
        if map.len() > 4096 {
            map.retain(|_, (_, last)| now.duration_since(*last).as_secs() < 3600);
        }
        let rate = max as f64 / window_secs.max(1) as f64;
        let bucket = map.entry(key.to_string()).or_insert((max as f64, now));
        let elapsed = now.duration_since(bucket.1).as_secs_f64();
        bucket.0 = (bucket.0 + elapsed * rate).min(max as f64);
        bucket.1 = now;
        if bucket.0 >= 1.0 {
            bucket.0 -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Parse a single-range `Range` header value against a body of `size` bytes.
/// Returns the inclusive `(start, end)` byte pair, or `None` when the header is
/// malformed, multi-range, or unsatisfiable (all mapped to 416 — §12.1 step 3).
fn parse_range(spec: &str, size: u64) -> Option<(u64, u64)> {
    if size == 0 {
        return None;
    }
    let rest = spec.trim().strip_prefix("bytes=")?;
    if rest.contains(',') {
        return None; // single range only
    }
    let (a, b) = rest.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() {
        // suffix form: last N bytes
        let n: u64 = b.parse().ok()?;
        if n == 0 {
            return None;
        }
        return Some((size.saturating_sub(n), size - 1));
    }
    let start: u64 = a.parse().ok()?;
    let end = if b.is_empty() {
        size - 1
    } else {
        b.parse::<u64>().ok()?.min(size - 1)
    };
    if start > end || start >= size {
        return None;
    }
    Some((start, end))
}

/// Inspector snapshot provider (§13.4). The server never interprets the
/// payload — it renders it at `/__oxide` (HTML) and `/__oxide/state` (JSON).
/// Wired by the CLI's dev command; absent in production.
pub trait InspectorSnapshot: Send + Sync + std::fmt::Debug {
    /// Machine-readable state (schema is the dev crate's `InspectorState`).
    fn state_json(&self) -> serde_json::Value;

    /// Human-friendly HTML for `GET /__oxide`. Default wraps pretty JSON.
    fn render_html(&self) -> String {
        let json = serde_json::to_string_pretty(&self.state_json()).unwrap_or_else(|_| "{}".into());
        format!(
            "<!doctype html><html><head><title>Oxide Inspector</title>\
             <style>body{{font:13px/1.5 ui-monospace,monospace;background:#0a0a0a;color:#e6e6e6;padding:24px}}\
             pre{{white-space:pre-wrap;word-break:break-word}}</style></head>\
             <body><h1>Oxide Inspector</h1><pre>{}</pre></body></html>",
            json.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        )
    }
}

/// Forward a request to `url` and convert the upstream response into an
/// `OxideResponse` (§13.5 dev proxy; also usable as a generic reverse-proxy
/// helper). Uses `hyper-util`'s pooled client so repeated hits to the same
/// upstream share a connection. Hop-by-hop headers are stripped per
/// RFC 7230 §6.1.
pub async fn forward_proxy(url: &str, req: OxideRequest) -> Result<OxideResponse> {
    use http_body_util::{BodyExt, Full};
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;
    use std::sync::OnceLock;

    static CLIENT: OnceLock<
        Client<hyper_util::client::legacy::connect::HttpConnector, Full<Bytes>>,
    > = OnceLock::new();
    let client = CLIENT.get_or_init(|| Client::builder(TokioExecutor::new()).build_http());

    let mut builder = hyper::Request::builder()
        .method(req.method.as_str())
        .uri(url);
    for (k, v) in req.headers.iter() {
        builder = builder.header(k.as_str(), v.as_str());
    }
    let outbound = builder
        .body(Full::new(req.body))
        .map_err(|e| err("SRV-PROXY", format!("build request failed: {e}")))?;

    let resp = client
        .request(outbound)
        .await
        .map_err(|e| err("SRV-PROXY", format!("upstream request failed: {e}")))?;

    let status = resp.status().as_u16();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    let body = resp
        .into_body()
        .collect()
        .await
        .map_err(|e| err("SRV-PROXY", format!("reading upstream body failed: {e}")))?
        .to_bytes();

    let mut out = OxideResponse::new(status, body);
    for (k, v) in headers {
        if matches!(
            k.to_ascii_lowercase().as_str(),
            "connection"
                | "keep-alive"
                | "transfer-encoding"
                | "upgrade"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "te"
                | "trailers"
        ) {
            continue;
        }
        out = out.with_header(&k, v);
    }
    Ok(out)
}

/// The pipeline service: rules → static → L0 → (render dispatch is injected by cli).
/// Constructed from config + manifests; testable without a listener via `serve_request`.
/// SPEC-10 / §16.4: the i18n policy the pipeline applies to an incoming
/// path — a locale prefix selects the locale; a missing prefix on a
/// non-default request is redirected to the negotiated locale.
#[derive(Debug, Clone, Default)]
pub struct I18nPolicy {
    pub locales: Vec<String>,
    pub default: String,
    /// `prefix-except-default` (default) | `prefix-always` | `prefix-never`.
    pub strategy: String,
}

impl I18nPolicy {
    fn is_enabled(&self) -> bool {
        !self.locales.is_empty()
    }
    /// The locale a path explicitly prefixes, plus the remainder.
    fn explicit<'a>(&self, path: &'a str) -> Option<(String, &'a str)> {
        for loc in &self.locales {
            let pref = format!("/{loc}");
            if let Some(rest) = path.strip_prefix(&pref) {
                if rest.is_empty() || rest.starts_with('/') {
                    return Some((loc.clone(), if rest.is_empty() { "/" } else { rest }));
                }
            }
        }
        None
    }
    /// Should this locale carry a path prefix under the strategy?
    fn prefixes(&self, locale: &str) -> bool {
        match self.strategy.as_str() {
            "prefix-always" => true,
            "prefix-never" => false,
            _ => locale != self.default,
        }
    }
    /// Negotiate `Accept-Language` against the supported set.
    fn negotiate(&self, accept_language: &str) -> String {
        for part in accept_language.split(',') {
            let tag = part.split(';').next().unwrap_or("").trim();
            if tag.is_empty() || tag == "*" {
                continue;
            }
            for loc in &self.locales {
                if loc.eq_ignore_ascii_case(tag) {
                    return loc.clone();
                }
            }
            let lang = tag.split('-').next().unwrap_or(tag);
            for loc in &self.locales {
                if loc
                    .split('-')
                    .next()
                    .unwrap_or(loc)
                    .eq_ignore_ascii_case(lang)
                {
                    return loc.clone();
                }
            }
        }
        self.default.clone()
    }
}

pub struct Pipeline {
    /// A3: per-peer token bucket for the OG endpoint. `resvg` rasterization
    /// is CPU-bound and unauthenticated; without a limit one client can pin a
    /// core.
    pub og_rate_limiter: RateLimiter,
    /// Max OG renders per peer per window. Default 10.
    pub og_rate_max: u32,
    /// The OG rate-limit window, seconds. Default 60.
    pub og_rate_window_secs: u64,

    pub rules: RulesEngine,
    pub assets: Option<StaticAssets>,
    pub limits: Limits,
    pub security: crate::SecuritySettings,
    /// SPEC-10 / §16.4: i18n locale policy. `None` disables locale routing.
    pub i18n: Option<crate::I18nPolicy>,
    pub rate_limiter: Arc<RateLimiter>,
    pub health_path: String,
    /// §6.4 / Wave M: readiness endpoint (`/_oxide/ready`). Liveness
    /// (`/_oxide/health`) answers the moment the socket binds; readiness
    /// answers once the renderer pool is warm. A single-node server keeps
    /// serving on `/health` — this is a signal for LBs and benchmarks,
    /// not a traffic gate. Empty disables the endpoint (tests).
    pub ready_path: String,
    /// §6.4 / Wave M: `false` while the renderer is warming; flipped by
    /// the CLI's `tokio::spawn` after `PageRenderer::warm()` returns.
    /// Defaults to `true` so tests and cache-only servers need no changes.
    pub render_ready: Arc<std::sync::atomic::AtomicBool>,
    /// Prometheus metrics endpoint path (§20). `/_oxide/metrics` default;
    /// empty string disables.
    pub metrics_path: String,
    /// Optional metrics registry. When `Some`, `/metrics_path` renders it.
    pub metrics: Option<Arc<Metrics>>,
    /// Optional inspector provider (§13.4). When `Some`, `/__oxide` and
    /// `/__oxide/state` return the inspector snapshot. Never set in production.
    pub inspector: Option<Arc<dyn InspectorSnapshot>>,
    /// SPEC-14 / §13.3: the dev HMR message source. `Some` only in dev; the
    /// `/_oxide/hmr` WebSocket subscribes to it. `oxide-fw-server` does not
    /// depend on `oxide-fw-dev`, so this is a broadcast of pre-serialized JSON
    /// strings the CLI wires from its `HmrHub`.
    pub hmr: Option<Arc<tokio::sync::broadcast::Sender<String>>>,
    /// §13.3: the client manifest's `build_id` (`"dev"` when no build).
    /// Advertised in the `Connected` HMR handshake so a stale tab reloads.
    pub hmr_build_id: String,
    /// Optional page renderer (§7). When `Some`, `serve_request` dispatches
    /// page renders to it instead of the neutral-IR path. Attached via
    /// `Pipeline::with_page_renderer` (see the CLI wiring). `None` preserves
    /// the legacy behavior for builds without a framework adapter.
    pub page_renderer: Option<Arc<dyn crate::PageRenderer>>,
    /// Optional image endpoint (§16.1). When `Some`, `/_oxide/image`
    /// requests are served natively — parse, validate, transform, respond.
    /// The transform never enters JS. `None` disables the endpoint (a
    /// 404), which is what a project with images turned off wants.
    pub image_endpoint: Option<Arc<crate::ImageEndpoint>>,
    /// §16.3: the OG-image endpoint. `Some` enables `GET /_oxide/og`.
    /// The rasterization lives in `oxide-fw-assets` behind `OgEndpointBackend`.
    pub og_endpoint: Option<Arc<OgEndpoint>>,
    /// Optional server-function dispatcher (§9.4). When `Some`,
    /// `POST /_oxide/fn/<id>` routes through it. The dispatcher owns the
    /// id → handler map, the input validation, and the encode/decode of
    /// the arguments; the pipeline owns only the CSRF check, the method
    /// and content-type guards, and the response envelope.
    ///
    /// Like `image_endpoint`, this is a trait seam: the pipeline does not
    /// depend on the runtime that actually executes a user function.
    pub server_fns: Option<Arc<dyn ServerFnDispatcher>>,
    /// Optional action dispatcher (§9.3). When `Some`, a form POST to an
    /// action URL routes through it. Like `server_fns`, this is a trait seam.
    pub actions: Option<Arc<dyn ActionDispatcher>>,
    /// SEC-02: whether the listener serves TLS. Gates HSTS emission
    /// (§17.1 "HSTS on TLS") — set where the listener is built.
    pub tls: bool,
    /// SEC-09: a dev-only session token. When `Some`, the `/__oxide`
    /// inspector endpoints require it (via `?token=` or the
    /// `x-oxide-fw-dev-token` header). A dev server on a shared LAN must not
    /// expose its inspector to anyone who can reach the port. `None`
    /// disables the guard (production never sets an inspector at all).
    pub dev_token: Option<String>,
    /// S-03/S-04/S-21.1: when `Some`, the listener is a dev server bound to
    /// this IP and `dev_host_allowed` gates the Host header on the HMR/socket
    /// upgrade arms (DNS-rebinding protection). `None` in production.
    pub dev_bind_ip: Option<std::net::IpAddr>,
    /// §9.6: WebSocket route dispatch. `Some` when the project has any
    /// `socket.ts` routes and a pool to run them. The service function
    /// upgrades a matching request and pumps phases through this.
    pub sockets: Option<Arc<SocketService>>,
    /// §12.1: the UDP port the HTTP/3 listener binds, when enabled. A
    /// non-zero value makes every HTTP/1.1+2 response advertise it via
    /// `Alt-Svc`, so a browser upgrades to h3 on the next request.
    pub http3_port: u16,
    /// C3: whether to emit `Alt-Svc: h3=":port"`. The h3 listener runs
    /// either way; this gates only the advertisement a browser uses to
    /// upgrade.
    pub http3_advertise: bool,
}

/// §9.6: the WebSocket route service. Holds the dispatcher and a lookup from
/// URL path → `socket.ts` module path (matched against the route tree at
/// boot, so the hot path is a map probe, not a re-scan).
#[derive(Clone)]
pub struct SocketService {
    pub dispatcher: Arc<dyn SocketDispatcher>,
    /// URL path → on-disk `socket.ts` path. Keyed by the route's URL segments
    /// joined by `/` (a `:param` segment matches any value — see the lookup
    /// in `serve_request`).
    pub routes: Arc<std::collections::BTreeMap<String, std::path::PathBuf>>,
    /// Live connections, keyed by socket id. `module_path` scopes fan-out to
    /// sockets served by the same `socket.ts`; `tx` is that connection's
    /// outbound queue, pumped by its writer task.
    pub registry: Arc<std::sync::Mutex<std::collections::HashMap<String, SocketClient>>>,
}

/// One live socket. Public so tests and embedders can inspect presence.
pub struct SocketClient {
    pub module_path: String,
    pub tx: tokio::sync::mpsc::UnboundedSender<String>,
}

/// RAII registration: dropping the guard removes the socket from the
/// registry, including every early-return path in the upgrade task.
pub struct SocketRegistration {
    registry: Arc<std::sync::Mutex<std::collections::HashMap<String, SocketClient>>>,
    id: String,
}

impl Drop for SocketRegistration {
    fn drop(&mut self) {
        if let Ok(mut reg) = self.registry.lock() {
            reg.remove(&self.id);
        }
    }
}

impl SocketService {
    /// Register a connection and return its drop guard.
    pub fn register(
        &self,
        id: &str,
        module_path: &str,
        tx: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> SocketRegistration {
        if let Ok(mut reg) = self.registry.lock() {
            reg.insert(
                id.to_string(),
                SocketClient {
                    module_path: module_path.to_string(),
                    tx,
                },
            );
        }
        SocketRegistration {
            registry: Arc::clone(&self.registry),
            id: id.to_string(),
        }
    }

    /// Presence: every live socket id served by the same module, including
    /// `id` itself.
    pub fn client_ids(&self, module_path: &str) -> Vec<String> {
        let Ok(reg) = self.registry.lock() else {
            return Vec::new();
        };
        let mut ids: Vec<String> = reg
            .iter()
            .filter(|(_, c)| c.module_path == module_path)
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// Fan out to every other socket on `module_path` (targeted frames are
    /// delivered with [`Self::send_to`]).
    pub fn broadcast(&self, module_path: &str, except: &str, data: &str) {
        let Ok(reg) = self.registry.lock() else {
            return;
        };
        for (id, client) in reg.iter() {
            if id != except && client.module_path == module_path {
                let _ = client.tx.send(data.to_string());
            }
        }
    }

    /// Deliver one frame to a specific live socket id.
    pub fn send_to(&self, id: &str, data: &str) {
        let Ok(reg) = self.registry.lock() else {
            return;
        };
        if let Some(client) = reg.get(id) {
            let _ = client.tx.send(data.to_string());
        }
    }
}

impl std::fmt::Debug for SocketService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SocketService")
            .field("routes", &self.routes.len())
            .field(
                "connections",
                &self.registry.lock().map(|r| r.len()).unwrap_or(0),
            )
            .finish()
    }
}

/// Find the socket route for a URL path: exact match first, then a
/// segment-count match where any `:param` / `*rest` segment matches.
pub fn socket_route_for<'a>(
    routes: &'a std::collections::BTreeMap<String, std::path::PathBuf>,
    path: &str,
) -> Option<&'a std::path::PathBuf> {
    if let Some(p) = routes.get(path) {
        return Some(p);
    }
    let want: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    for (pattern, module) in routes {
        let pat: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
        if pat.len() != want.len() {
            continue;
        }
        let mut ok = true;
        for (p, w) in pat.iter().zip(want.iter()) {
            if p.starts_with(':') || p.starts_with('*') {
                continue;
            }
            if p != w {
                ok = false;
                break;
            }
        }
        if ok {
            return Some(module);
        }
    }
    None
}

/// §9.3: the seam between the HTTP pipeline and whichever runtime runs a
/// user's form action. Mirrors `ServerFnDispatcher`: the pipeline owns the
/// request parsing, the CSRF check, and the response envelope; the CLI wires
/// a dispatcher backed by the render pool.
#[async_trait::async_trait]
pub trait ActionDispatcher: Send + Sync {
    /// Run the named action from `module_path` with the form body. `Ok`
    /// carries the action's result (status, location, data, cookies,
    /// headers, revalidations); `Err` is a coded transport/runner failure.
    async fn dispatch(
        &self,
        module_path: &str,
        action: &str,
        form_json: &str,
        params: &serde_json::Map<String, serde_json::Value>,
        url: &str,
        method: &str,
        headers: &[(String, String)],
        // §8.1 middleware.ts / §9.1: request-scoped locals injected by
        // middleware. An action's `event.locals` reads this; absent (empty
        // object) when no middleware ran.
        locals: &serde_json::Value,
        // §20: W3C trace context for outgoing fetches inside the action.
        trace: Option<&serde_json::Value>,
    ) -> std::result::Result<ActionRunResult, DispatchError>;
}

/// The result of an action run, in the shape the pipeline builds a response
/// from. Field names match the worker's `action:run` reply.
#[derive(Debug, Clone, Default)]
pub struct ActionRunResult {
    pub status: u16,
    pub location: Option<String>,
    pub data: serde_json::Value,
    pub set_cookies: Vec<String>,
    pub headers: Vec<(String, String)>,
    pub revalidate: Vec<(String, String)>,
}

/// §9.4: the seam between the HTTP pipeline and whichever runtime executes
/// a user's server function. The CLI wires a dispatcher backed by the
/// render pool (`oxide-fw-jshost`); tests can wire a table of closures.
#[async_trait::async_trait]
pub trait ServerFnDispatcher: Send + Sync {
    /// Look up the function registered under `id` and run it with `args`.
    /// `Ok(bytes)` is the response body — the *encoded* return value, in
    /// whatever wire format the dispatcher and client agreed on. `Err` is
    /// a coded failure the pipeline turns into a 4xx/5xx envelope.
    async fn dispatch(
        &self,
        id: &str,
        args_json: &[u8],
    ) -> std::result::Result<Vec<u8>, DispatchError>;
}

/// A server-fn dispatch failure. The pipeline maps `status` to the HTTP
/// response and `code`/`message` into the JSON error envelope.
#[derive(Debug, Clone)]
pub struct DispatchError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl DispatchError {
    pub fn not_found(id: &str) -> Self {
        Self {
            status: 404,
            code: "FN-NOT-FOUND".into(),
            message: format!("no server function registered under id {id:?}"),
        }
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            code: "FN-BAD-REQUEST".into(),
            message: message.into(),
        }
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            status: 500,
            code: "FN-INTERNAL".into(),
            message: message.into(),
        }
    }
}

/// §16.1 image endpoint, host-side. Owns the backend choice, the source
/// resolution policy (a local path under the site root, or a URL matched
/// against `ImagesConfig.remotePatterns`), and the signing secret.
///
/// The transform itself is `oxide_fw_assets::ImageBackend`, but the server
/// crate must not depend on `oxide-fw-assets` — that crate pulls ravif,
/// resvg, skrifa and lightningcss, and `oxide-fw-server` is meant to build
/// lean. `oxide-fw-server` names only the trait seams below; the CLI builds
/// the concrete endpoint from config and hands it in through
/// `Pipeline::image_endpoint`.
pub struct ImageEndpoint {
    /// The pure-Rust backend (or a future libvips one).
    pub backend: Arc<dyn ImageEndpointBackend>,
    /// Root under which a `/`-prefixed source path is resolved.
    pub asset_root: std::path::PathBuf,
    /// Signing secret. `None` means the endpoint is open (dev only); `Some`
    /// requires a valid `?s=` on every request.
    pub signing_secret: Option<String>,
    /// §16.1: hosts an `https://` source may be fetched from. Empty means
    /// remote fetch is off — only site-relative sources work, which is the
    /// safe default. A pattern is `host` (matches the exact hostname) or
    /// `*.host` (matches any subdomain). A `protocol` on the pattern is
    /// enforced: `Some("https")` rejects an `http://` source.
    pub remote_patterns: Vec<RemoteImagePattern>,
    /// §17.1: hosts the SSRF check treats as allowed even in a blocked
    /// range. This is the escape hatch for an image CDN on a private
    /// network. Empty in every normal deployment.
    pub ssrf_allow_list: Vec<String>,
    /// Byte ceiling on a single remote fetch. Protects the server from a
    /// source that streams an unbounded body.
    pub remote_max_bytes: usize,
    /// §16.1: transformed-result cache keyed by source bytes + transform
    /// params. Shared across clones of the endpoint (the remote-image arm
    /// rebuilds the struct per request), so it is an `Arc`.
    pub result_cache: Arc<std::sync::Mutex<std::collections::HashMap<String, ImageCacheEntry>>>,
    /// §16.1: concurrent-transform ceiling. `None` means unlimited (tests).
    pub max_concurrency: usize,
    /// Live transforms. Incremented before `process`, decremented after.
    pub in_flight: Arc<std::sync::atomic::AtomicUsize>,
    /// §16.1: largest accepted width/height.
    pub max_dimension: u32,
}

/// §16.1: attach an LQIP data URI (if the backend produced one) as a
/// response header. Repeating a 2 KB data URI in every `<img>` body is not
/// the protocol here; the client reads it from the response metadata.
trait ImagePlaceholderExt {
    fn pipe_image_placeholder(self, placeholder: Option<String>) -> Self;
}

impl ImagePlaceholderExt for OxideResponse {
    fn pipe_image_placeholder(self, placeholder: Option<String>) -> Self {
        match placeholder {
            Some(p) => self.with_header("x-oxide-lqip", p),
            None => self,
        }
    }
}

/// §16.1: one cached transformed image.
#[derive(Debug, Clone)]
pub struct ImageCacheEntry {
    pub bytes: bytes::Bytes,
    pub content_type: String,
    pub etag: String,
    pub placeholder: Option<String>,
}

/// §16.1: one allow-listed remote image host.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteImagePattern {
    /// Exact hostname, or `*.suffix` for any subdomain of `suffix`.
    pub host: String,
    /// `None` accepts either scheme; `Some("https")` restricts to it.
    pub protocol: Option<String>,
}

/// `ImageEndpoint` has trait-object fields and is not `Clone`. The remote
/// image arm needs an owned copy that outlives the pipeline borrow; this
/// rebuilds the struct, sharing the backend `Arc` (the one field that is
/// already `Arc`) and copying the plain values.
fn clone_image_endpoint(e: &ImageEndpoint) -> ImageEndpoint {
    ImageEndpoint {
        backend: Arc::clone(&e.backend),
        asset_root: e.asset_root.clone(),
        signing_secret: e.signing_secret.clone(),
        remote_patterns: e.remote_patterns.clone(),
        ssrf_allow_list: e.ssrf_allow_list.clone(),
        remote_max_bytes: e.remote_max_bytes,
        // Shared, not copied: the cache and the in-flight counter must be
        // the same across per-request endpoint clones.
        result_cache: Arc::clone(&e.result_cache),
        max_concurrency: e.max_concurrency,
        in_flight: Arc::clone(&e.in_flight),
        max_dimension: e.max_dimension,
    }
}

/// The backend seam. `oxide_fw_assets::ImageBackend` is adapted to this in the
/// CLI, so the server has no compile-time tie to the asset crate.
pub trait ImageEndpointBackend: Send + Sync {
    /// Transform the source bytes per the request. The error string is
    /// surfaced as an `AST-*`-coded diagnostic in the HTTP response.
    fn process(
        &self,
        req: ImageEndpointRequest,
    ) -> std::result::Result<ImageEndpointResult, String>;
}

/// The subset of `oxide_fw_assets::ImageRequest` the endpoint passes through.
#[derive(Debug, Clone)]
pub struct ImageEndpointRequest {
    pub source: Vec<u8>,
    pub format: Option<&'static str>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub quality: Option<u8>,
    /// §16.1: `blur` / `pixelate` LQIP request, passed through to the backend.
    pub placeholder: Option<String>,
}

/// Bytes plus the chosen content type and a cache key.
#[derive(Debug, Clone)]
pub struct ImageEndpointResult {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    pub cache_key: String,
    /// §16.1: optional LQIP data URI when the request asked for one.
    pub placeholder: Option<String>,
}

// ---------------------------------------------------------------------------
// OG image endpoint (§16.3)
// ---------------------------------------------------------------------------

/// §16.3: the `/_oxide/og` endpoint. Renders an Open Graph image from a
/// neutral element tree through a pluggable backend (the pure-Rust
/// `resvg`/`taffy`/`parley` pipeline in `oxide-fw-assets`). Like the image
/// endpoint, this is a trait seam so `oxide-fw-server` does not depend on
/// `oxide-fw-assets`.
pub struct OgEndpoint {
    pub backend: Arc<dyn OgEndpointBackend>,
}

impl std::fmt::Debug for OgEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OgEndpoint")
    }
}

/// §16.3: rasterize a neutral OG element tree to PNG bytes.
pub trait OgEndpointBackend: Send + Sync {
    fn render(
        &self,
        tree: &serde_json::Value,
        width: u32,
        height: u32,
    ) -> std::result::Result<Vec<u8>, String>;
}

/// Parse an `/_oxide/og?…` query into the element tree the backend renders.
///
/// Kept deliberately small: the query carries the *content* (a title, a
/// subtitle, an optional background), and this builds the neutral tree from
/// it. A caller that needs a richer template renders it at build time and
/// serves the PNG as an asset — the endpoint is for the dynamic case.
///
/// Query keys: `title` (required), `subtitle` (optional), `bg` (optional CSS
/// color, default `#0a0a0a`), `fg` (optional, default `#ffffff`), `w`/`h`
/// (optional pixel size, default 1200×630).
pub fn og_tree_from_query(
    query: &str,
) -> std::result::Result<(serde_json::Value, u32, u32), String> {
    let mut title: Option<String> = None;
    let mut subtitle: Option<String> = None;
    let mut bg = "#0a0a0a".to_string();
    let mut fg = "#ffffff".to_string();
    let mut width: u32 = 1200;
    let mut height: u32 = 630;
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = match pair.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        let v = percent_decode_query(v);
        match k {
            "title" => title = Some(v),
            "subtitle" => subtitle = Some(v),
            "bg" => bg = v,
            "fg" => fg = v,
            "w" => {
                if let Ok(n) = v.parse::<u32>() {
                    width = n.clamp(16, 4096);
                }
            }
            "h" => {
                if let Ok(n) = v.parse::<u32>() {
                    height = n.clamp(16, 4096);
                }
            }
            _ => {}
        }
    }
    // S-09: cap the rasterized *area*, not just each dimension — 4096×4096 is
    // ~100 ms of resvg CPU per request (an unauth'd CPU-exhaustion vector). A
    // 1 MP budget keeps worst-case ~40 ms and is ample for an OG card.
    const OG_MAX_AREA: u64 = 1_000_000;
    if (width as u64) * (height as u64) > OG_MAX_AREA {
        let capped_h = (OG_MAX_AREA / width.max(1) as u64).max(16) as u32;
        height = capped_h.min(height);
    }
    let Some(mut title) = title else {
        return Err("og endpoint requires a `title` query parameter".to_string());
    };
    // S-09: bound the interpolated text so a huge title cannot blow up layout.
    title.truncate(256);
    if let Some(s) = subtitle.as_mut() {
        s.truncate(256);
    }
    let mut children = vec![serde_json::json!({
        "type": "text",
        "text": title,
        "x": 64,
        "y": 240,
        "size": 72,
        "fill": fg,
        "weight": "bold",
    })];
    if let Some(sub) = subtitle {
        children.push(serde_json::json!({
            "type": "text",
            "text": sub,
            "x": 64,
            "y": 340,
            "size": 36,
            "fill": fg,
        }));
    }
    Ok((
        serde_json::json!({ "background": bg, "children": children }),
        width,
        height,
    ))
}

/// Minimal `+`/`%XX` query-value decode (no `+`-to-space: a title with a `+`
/// means a literal plus, and a caller encodes a space as `%20`).
fn percent_decode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(h) = hex {
                if let Ok(b) = u8::from_str_radix(h, 16) {
                    out.push(b as char);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// §16.3: handle `GET /_oxide/og?title=…`. Synchronous: the backend rasterizes
/// in-process, no fetch. The response is a PNG with a public cache header.
fn serve_og_endpoint(endpoint: &OgEndpoint, raw_url: &str) -> OxideResponse {
    let query = raw_url.split_once('?').map(|(_, q)| q).unwrap_or("");
    let (tree, w, h) = match og_tree_from_query(query) {
        Ok(v) => v,
        Err(msg) => {
            return OxideResponse::new(400, format!("bad og request: {msg}"))
                .with_header("content-type", "text/plain; charset=utf-8");
        }
    };
    match endpoint.backend.render(&tree, w, h) {
        Ok(png) => OxideResponse::new(200, Bytes::from(png))
            .with_header("content-type", "image/png")
            .with_header("cache-control", "public, max-age=3600"),
        Err(msg) => OxideResponse::new(500, format!("og render failed: {msg}"))
            .with_header("content-type", "text/plain; charset=utf-8"),
    }
}

impl Default for Pipeline {
    /// A no-op pipeline: no rules, no assets, no metrics, no renderer.
    /// Used as a starting point before chaining `with_*` builders and in
    /// tests that only care about a specific layer of the pipeline.
    fn default() -> Self {
        Self {
            rules: Default::default(),
            assets: None,
            limits: Default::default(),
            security: Default::default(),
            i18n: None,
            rate_limiter: std::sync::Arc::new(Default::default()),
            og_rate_limiter: RateLimiter::default(),
            og_rate_max: 10,
            og_rate_window_secs: 60,
            health_path: String::new(),
            ready_path: String::new(),
            render_ready: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            metrics_path: String::new(),
            metrics: None,
            inspector: None,
            hmr: None,
            hmr_build_id: String::new(),
            page_renderer: None,
            image_endpoint: None,
            og_endpoint: None,
            server_fns: None,
            actions: None,
            tls: false,
            dev_token: None,
            dev_bind_ip: None,
            sockets: None,
            http3_port: 0,
            http3_advertise: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SecuritySettings {
    pub csp: String,
    pub csrf: String,
    pub host_allow_list: Vec<String>,
    pub ssrf_allow_list: Vec<String>,
    pub cookie_keys: Vec<Vec<u8>>,
    /// §16.6: encryption keys for the session codec (`SessionCodec`), first
    /// key seals, every key tried on open (rotation without logout). Empty
    /// when the project configures none — session cookies then fall back to
    /// the signed-only `sign_cookie` primitive.
    pub session_keys: Vec<Vec<u8>>,
    /// SEC-05 / S-10: CIDRs whose `X-Forwarded-For` is trusted. Empty = trust
    /// no proxy (the rate-limit key uses the socket peer directly).
    pub trusted_proxies: Vec<String>,
    /// S-21.3: emit the `x-oxide-fw-cache` status header. Default `true`.
    pub expose_cache_header: bool,
}

impl Default for SecuritySettings {
    fn default() -> Self {
        Self {
            csp: "strict".into(),
            csrf: "origin".into(),
            host_allow_list: Vec::new(),
            ssrf_allow_list: Vec::new(),
            cookie_keys: Vec::new(),
            session_keys: Vec::new(),
            trusted_proxies: Vec::new(),
            expose_cache_header: true,
        }
    }
}

/// The request path of an OxideRequest URL (absolute URLs stripped to origin-form,
/// query/fragment dropped).
/// S-18: normalize a client-reported vitals `route` into a low-cardinality
/// label. Only `[A-Za-z0-9_/\-.:]` survives (query removed, capped at 128);
/// anything else — or an empty/garbage value — folds to `_other`, so the
/// public beacon cannot mint unbounded metrics keys.
fn sanitize_route_label(raw: &str) -> String {
    let path = raw.split(['?', '#']).next().unwrap_or("").trim();
    if path.is_empty() {
        return "_other".to_string();
    }
    let ok = path.len() <= 128
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'.' | b':'));
    if ok {
        path.to_string()
    } else {
        "_other".to_string()
    }
}

fn path_of(url: &str) -> String {
    let mut u = url;
    if let Some(rest) = u
        .strip_prefix("http://")
        .or_else(|| u.strip_prefix("https://"))
    {
        u = match rest.find('/') {
            Some(i) => &rest[i..],
            None => "/",
        };
    }
    let path = u.split(['?', '#']).next().unwrap_or("");
    if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    }
}

/// Replace the path of a URL, keeping any query/fragment.
fn url_with_path(url: &str, new_path: &str) -> String {
    match url.find(['?', '#']) {
        Some(i) => format!("{new_path}{}", &url[i..]),
        None => new_path.to_string(),
    }
}

fn path_has_extension(path: &str) -> bool {
    let last = path.rsplit('/').next().unwrap_or("");
    match last.rfind('.') {
        Some(i) => i > 0 && i + 1 < last.len(),
        None => false,
    }
}

/// Per-request CSP nonce derived from the request id (deterministic, unique
/// per request). Public because the framework dispatch injected by the CLI
/// must stamp the same nonce on any script it adds to the response — a script
/// tag without the nonce is blocked by `strict-dynamic` (§17.1).
pub fn nonce_for(request_id: &str) -> String {
    blake3::hash(request_id.as_bytes()).to_hex()[..32].to_string()
}

/// §16.1: handle an `/_oxide/image?url=…&w=…&format=…` request.
///
/// The signature check runs first when a secret is configured, so a
/// tampered or unsigned request never reaches the backend. Errors carry a
/// stable message so a client can tell "no signing secret configured" from
/// "signature mismatch" from "unknown format".
fn serve_image_endpoint(endpoint: Option<&ImageEndpoint>, raw_url: &str) -> NativePrefix {
    let Some(endpoint) = endpoint else {
        return NativePrefix::Raw(
            OxideResponse::new(404, "image endpoint not configured")
                .with_header("content-type", "text/plain; charset=utf-8"),
        );
    };

    // The signing check needs the `s=` value and the *rest* of the query
    // in the exact form the signer hashed. `sign_image_url` signs
    // `path?query` where query excludes `s=`, so re-derive that here.
    let (path, params_no_sig) = split_sig(raw_url);

    let parsed = match crate::parse_image_query(&params_no_sig) {
        Ok(p) => p,
        Err(msg) => {
            return NativePrefix::Raw(
                OxideResponse::new(400, format!("bad image request: {msg}"))
                    .with_header("content-type", "text/plain; charset=utf-8"),
            );
        }
    };

    if let Some(secret) = endpoint.signing_secret.as_deref() {
        let Some(sig) = parsed.signature.as_deref() else {
            return NativePrefix::Raw(
                OxideResponse::new(403, "image request not signed")
                    .with_header("content-type", "text/plain; charset=utf-8"),
            );
        };
        // Recompute exactly what the signer hashed: `path?params` with the
        // `s=` param stripped. `sign_image_url` takes the URL *before* the
        // signature is appended, so the reconstruction must match that.
        let signed_form = if params_no_sig.is_empty() {
            path.to_string()
        } else {
            format!("{path}?{params_no_sig}")
        };
        if !crate::verify_image_sig(&signed_form, secret, sig) {
            return NativePrefix::Raw(
                OxideResponse::new(403, "image signature mismatch")
                    .with_header("content-type", "text/plain; charset=utf-8"),
            );
        }
    }

    // §16.1: resolve the source. A `/`-prefixed path reads under the asset
    // root; an `http(s)://` URL must match a `remotePatterns` entry and
    // pass the §17.1 SSRF check. The route is `GET` only, so the handler
    // cannot await — remote fetches are the one case that needs async, and
    // they are handled below by short-circuiting into a future. Every
    // other path is synchronous and returns immediately.
    let is_remote = parsed.url.starts_with("http://") || parsed.url.starts_with("https://");
    if is_remote {
        return match plan_remote_image(endpoint, &parsed) {
            Ok((mut request, format)) => {
                // Stash the URL in `source` — the async arm has no other
                // channel to receive it through (see `fetch_remote_image`).
                request.source = parsed.url.clone().into_bytes();
                // `endpoint` is a `&ImageEndpoint` (borrowed from the
                // pipeline's `Arc<ImageEndpoint>`); the async arm outlives
                // that borrow, so build a fresh `Arc` from a clone.
                NativePrefix::RemoteImage {
                    endpoint: Arc::new(clone_image_endpoint(endpoint)),
                    request,
                    format,
                    rule_headers: Vec::new(),
                }
            }
            Err(resp) => NativePrefix::Raw(resp),
        };
    }
    let path_only = parsed.url.split(['?', '#']).next().unwrap_or("");
    if !path_only.starts_with('/') {
        return NativePrefix::Raw(
            OxideResponse::new(
                400,
                "image source must be a site-relative path or an https URL",
            )
            .with_header("content-type", "text/plain; charset=utf-8"),
        );
    }
    let rel = path_only.trim_start_matches('/');
    let abs = endpoint.asset_root.join(rel);
    // Reject path traversal: the resolved path must stay under the root.
    let canon_root = match endpoint.asset_root.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            return NativePrefix::Raw(
                OxideResponse::new(500, "image asset root missing")
                    .with_header("content-type", "text/plain; charset=utf-8"),
            );
        }
    };
    let canon_abs = match abs.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            return NativePrefix::Raw(
                OxideResponse::new(404, "image source not found")
                    .with_header("content-type", "text/plain; charset=utf-8"),
            );
        }
    };
    if !canon_abs.starts_with(&canon_root) {
        return NativePrefix::Raw(
            OxideResponse::new(403, "image source escapes the asset root")
                .with_header("content-type", "text/plain; charset=utf-8"),
        );
    }

    let source = match std::fs::read(&canon_abs) {
        Ok(b) => b,
        Err(_) => {
            return NativePrefix::Raw(
                OxideResponse::new(404, "image source not readable")
                    .with_header("content-type", "text/plain; charset=utf-8"),
            );
        }
    };

    let format_static = parsed.format;
    // §16.1: never allocate a canvas larger than the configured ceiling.
    for (name, dim) in [("w", parsed.width), ("h", parsed.height)] {
        if let Some(d) = dim {
            if d == 0 || d > endpoint.max_dimension {
                return NativePrefix::Raw(
                    OxideResponse::new(
                        400,
                        format!(
                            "image `{name}` out of range (max {})",
                            endpoint.max_dimension
                        ),
                    )
                    .with_header("content-type", "text/plain; charset=utf-8"),
                );
            }
        }
    }
    // §16.1: cache the transformed bytes by (source, format, w, h, q). The
    // source bytes are hashed, so a changed file naturally misses. The key
    // is also the ETag, independent of the backend's own cache key.
    let mut key_hasher = blake3::Hasher::new();
    key_hasher.update(&source);
    key_hasher.update(format_static.unwrap_or("").as_bytes());
    key_hasher.update(&parsed.width.unwrap_or(0).to_le_bytes());
    key_hasher.update(&parsed.height.unwrap_or(0).to_le_bytes());
    key_hasher.update(&[parsed.quality.unwrap_or(0)]);
    let key = key_hasher.finalize().to_hex().to_string();
    if let Some(cached) = endpoint
        .result_cache
        .lock()
        .ok()
        .and_then(|c| c.get(&key).cloned())
    {
        return NativePrefix::Raw(
            OxideResponse::new(200, cached.bytes)
                .with_header("content-type", cached.content_type)
                .with_header("Cache-Control", "public, max-age=31536000, immutable")
                .with_header("ETag", cached.etag)
                .with_header("Vary", "Accept")
                .pipe_image_placeholder(cached.placeholder),
        );
    }
    // §16.1: bound concurrent CPU-bound transforms. A request that arrives
    // over the ceiling is shed with 503 + Retry-After rather than queueing
    // behind every other transform (the work is synchronous, so queueing
    // would pin a request thread).
    if endpoint.max_concurrency > 0 {
        let live = endpoint
            .in_flight
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if live >= endpoint.max_concurrency {
            endpoint
                .in_flight
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            return NativePrefix::Raw(
                OxideResponse::new(503, "image transform concurrency limit reached")
                    .with_header("retry-after", "1")
                    .with_header("content-type", "text/plain; charset=utf-8"),
            );
        }
    }
    let req = ImageEndpointRequest {
        source,
        format: format_static,
        width: parsed.width,
        height: parsed.height,
        quality: parsed.quality,
        placeholder: parsed.placeholder.clone(),
    };

    let result = endpoint.backend.process(req);
    if endpoint.max_concurrency > 0 {
        endpoint
            .in_flight
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
    match result {
        Ok(result) => {
            let etag = format!(
                "\"{}\"",
                &result.cache_key[..16.min(result.cache_key.len())]
            );
            if let Ok(mut c) = endpoint.result_cache.lock() {
                // Bound the cache; entries are immutable, so dropping the
                // oldest-inserted half is safe and cheap.
                if c.len() > 256 {
                    let keep: Vec<String> = c.keys().skip(128).cloned().collect();
                    c.retain(|k, _| keep.contains(k));
                }
                c.insert(
                    key,
                    ImageCacheEntry {
                        bytes: Bytes::from(result.bytes.clone()),
                        content_type: result.content_type.to_string(),
                        etag: etag.clone(),
                        placeholder: result.placeholder.clone(),
                    },
                );
            }
            NativePrefix::Raw(
                OxideResponse::new(200, Bytes::from(result.bytes))
                    .with_header("content-type", result.content_type)
                    // §16.1: transformed images are content-addressed by the
                    // backend's cache key, so they are safe to cache immutably.
                    .with_header("Cache-Control", "public, max-age=31536000, immutable")
                    .with_header("ETag", etag)
                    .with_header("Vary", "Accept")
                    // §16.1: the LQIP was only piped on the cache-hit branch,
                    // so a client saw `x-oxide-lqip` on the second request and
                    // never on the first. The fresh render carries it too.
                    .pipe_image_placeholder(result.placeholder.clone()),
            )
        }
        Err(msg) => NativePrefix::Raw(
            OxideResponse::new(400, format!("image transform failed: {msg}"))
                .with_header("content-type", "text/plain; charset=utf-8"),
        ),
    }
}

/// Split a request URL into `(path, params-before-s=)`. The signer signs
/// `path?params` with `s=` appended last, so removing the `s=` pair (in any
/// position) reconstructs the signed form.
/// Parsed `/_oxide/image` query. Mirrors `oxide_fw_assets`'s type but lives
/// here so the server has no dependency on the asset crate.
struct ImageQuery {
    url: String,
    width: Option<u32>,
    height: Option<u32>,
    format: Option<&'static str>,
    quality: Option<u8>,
    placeholder: Option<String>,
    signature: Option<String>,
}

fn parse_image_query(query: &str) -> std::result::Result<ImageQuery, String> {
    let mut url: Option<String> = None;
    let mut width = None;
    let mut height = None;
    let mut format: Option<&'static str> = None;
    let mut quality = None;
    let mut placeholder = None;
    let mut signature = None;
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "url" => {
                if v.is_empty() {
                    return Err("`url` is empty".into());
                }
                url = Some(percent_decode(v));
            }
            "w" => {
                width = Some(
                    v.parse::<u32>()
                        .map_err(|_| format!("`w` not an integer: {v:?}"))?,
                )
            }
            "h" => {
                height = Some(
                    v.parse::<u32>()
                        .map_err(|_| format!("`h` not an integer: {v:?}"))?,
                )
            }
            "format" => {
                format = Some(match v.to_ascii_lowercase().as_str() {
                    "avif" => "image/avif",
                    "webp" => "image/webp",
                    "jpeg" | "jpg" => "image/jpeg",
                    "png" => "image/png",
                    other => return Err(format!("unknown `format`: {other:?}")),
                });
            }
            "q" | "quality" => {
                let q = v
                    .parse::<u32>()
                    .map_err(|_| format!("`q` not an integer: {v:?}"))?;
                if q == 0 || q > 100 {
                    return Err(format!("`q` out of range: {q}"));
                }
                quality = Some(q as u8);
            }
            "placeholder" => {
                let p = v.to_ascii_lowercase();
                if p != "blur" && p != "pixelate" {
                    return Err(format!("unknown `placeholder`: {v:?}"));
                }
                placeholder = Some(p);
            }
            "s" => signature = Some(v.to_string()),
            _ => {}
        }
    }
    Ok(ImageQuery {
        url: url.ok_or("`url` is required")?,
        width,
        height,
        format,
        quality,
        placeholder,
        signature,
    })
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_nib(bytes[i + 1]), hex_nib(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_nib(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Recompute the signature `oxide_fw_assets::sign_image_url` would have
/// produced for `signed_form`, and compare it to `sig`.
pub fn verify_image_sig(signed_form: &str, secret: &str, sig: &str) -> bool {
    let mut h = blake3::Hasher::new();
    h.update(signed_form.as_bytes());
    h.update(secret.as_bytes());
    // SEC-08: full 64-hex digest, constant-time compare (was a 16-hex
    // prefix with a short-circuiting byte compare).
    let expected = h.finalize().to_hex().to_string();
    if expected.len() != sig.len() {
        return false;
    }
    expected
        .bytes()
        .zip(sig.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// §16.1 + §17.1: check a remote image URL against the pattern list and
/// the SSRF allow-list, then fetch it. Split from the sync handler because
/// the fetch is the one async operation the endpoint performs.
///
/// Because `serve_image_endpoint` returns an `OxideResponse` synchronously
/// (the pipeline's native prefix is sync), the remote case is *refused*
/// here in this slice and the actual fetch is deferred to a follow-up that
/// makes the endpoint async. The checks below run first so a bad URL is
/// rejected with the right code regardless.
/// §16.1 + §17.1: validate a remote image URL and, if it passes, return the
/// `ImageEndpointRequest` the async arm will fetch. The signature, the
/// pattern match, and the SSRF check all run here — synchronously — so a
/// bad URL is rejected before any network call.
/// §16.1 test helper: a URL-only query the test can build without
/// reaching into `ImageQuery`, which is private to the pipeline.
#[doc(hidden)]
pub struct ImageQueryForTest {
    pub url: String,
}

/// §16.1 test helper: run `plan_remote_image` and return
/// `(status, message)` on rejection, so a test can assert both without
/// reaching into `OxideResponse`'s body bytes.
#[doc(hidden)]
pub fn plan_remote_image_for_test(
    endpoint: &ImageEndpoint,
    query: &ImageQueryForTest,
) -> std::result::Result<(ImageEndpointRequest, Option<&'static str>), (u16, String)> {
    let parsed = ImageQuery {
        url: query.url.clone(),
        width: None,
        height: None,
        format: None,
        quality: None,
        placeholder: None,
        signature: None,
    };
    plan_remote_image(endpoint, &parsed)
        .map_err(|r| (r.status, String::from_utf8_lossy(&r.body).into_owned()))
}

fn plan_remote_image(
    endpoint: &ImageEndpoint,
    parsed: &ImageQuery,
) -> std::result::Result<(ImageEndpointRequest, Option<&'static str>), OxideResponse> {
    // Takes an owned `String` — `OxideResponse::new` needs `Into<String>`
    // and a `&str` borrowed from a temporary inside the closure body does
    // not outlive the call.
    let err = |status: u16, msg: String| -> OxideResponse {
        OxideResponse::new(status, msg).with_header("content-type", "text/plain; charset=utf-8")
    };

    let url =
        url::Url::parse(&parsed.url).map_err(|e| err(400, format!("invalid source URL: {e}")))?;
    let scheme = url.scheme().to_string();
    if scheme != "http" && scheme != "https" {
        return Err(err(400, "image URL scheme must be http or https".into()));
    }
    let host = url
        .host_str()
        .ok_or_else(|| err(400, "image URL has no host".into()))?
        .to_string();

    if endpoint.remote_patterns.is_empty() {
        return Err(err(403, "remote image sources are not allowed".into()));
    }
    if !remote_pattern_allows(&endpoint.remote_patterns, &host, &scheme) {
        return Err(err(
            403,
            format!("host {host:?} is not an allowed image source"),
        ));
    }
    if !ssrf_allowed(&host, &endpoint.ssrf_allow_list) {
        return Err(err(
            403,
            "image source resolves to a blocked network range".into(),
        ));
    }

    // The request carries the resolved format; the fetch arm fills `source`.
    Ok((
        ImageEndpointRequest {
            source: Vec::new(),
            format: parsed.format,
            width: parsed.width,
            height: parsed.height,
            quality: parsed.quality,
            placeholder: parsed.placeholder.clone(),
        },
        parsed.format,
    ))
}

/// §16.1: fetch a validated remote image and transform it.
///
/// SEC-03: `fetch_remote_checked` resolves the host, validates **every**
/// resolved address against the SSRF block ranges, connects to a validated
/// address (connect-by-IP, defeating DNS rebinding between check and
/// connect), and re-validates the host pattern + SSRF ranges at every
/// redirect hop. `reqwest` is not a dependency of this crate, so the fetch
/// uses the same `hyper-util` client `forward_proxy` does.
async fn fetch_remote_image(
    endpoint: Arc<ImageEndpoint>,
    mut request: ImageEndpointRequest,
    format: Option<&'static str>,
) -> OxideResponse {
    let err = |status: u16, msg: String| -> OxideResponse {
        OxideResponse::new(status, msg).with_header("content-type", "text/plain; charset=utf-8")
    };

    // Recover the URL from the request we built: `plan_remote_image` set
    // `format`, not `source`, so the URL travels in a side channel. Rather
    // than thread it separately, the plan fn stores it in `source` as the
    // URL's bytes and the fetch reads it back. That keeps the async arm's
    // signature to `(endpoint, request, format)`.
    let url = match String::from_utf8(std::mem::take(&mut request.source)) {
        Ok(u) => u,
        Err(_) => return err(400, "remote image URL was not valid UTF-8".to_string()),
    };

    // Fetch with a byte ceiling. `MAX` is the manifest's `remote_max_bytes`.
    let max = endpoint.remote_max_bytes;
    let bytes = match fetch_remote_checked(
        &url,
        max,
        &endpoint.remote_patterns,
        &endpoint.ssrf_allow_list,
    )
    .await
    {
        Ok(b) => b,
        Err(e) => return err(502, format!("remote image fetch failed: {e}")),
    };

    // The transform is synchronous.
    let ireq = ImageEndpointRequest {
        source: bytes,
        format,
        width: request.width,
        height: request.height,
        quality: request.quality,
        placeholder: request.placeholder.clone(),
    };
    match endpoint.backend.process(ireq) {
        Ok(result) => OxideResponse::new(200, Bytes::from(result.bytes))
            .with_header("content-type", result.content_type)
            .with_header("Cache-Control", "public, max-age=31536000, immutable")
            .with_header(
                "ETag",
                format!(
                    "\"{}\"",
                    &result.cache_key[..16.min(result.cache_key.len())]
                ),
            ),
        Err(msg) => err(400, format!("image transform failed: {msg}")),
    }
}

/// Fetch a URL with a byte ceiling. Uses the pooled `hyper-util` client the
/// proxy path already depends on. Returns the body bytes.
/// SEC-03: which of `addrs` are blocked? Returns the first blocked address,
/// or `None` when every address passes. Pure, so it is unit-tested.
pub fn first_blocked_addr(
    addrs: &[std::net::IpAddr],
    ssrf_allow_list: &[String],
) -> Option<std::net::IpAddr> {
    addrs
        .iter()
        .copied()
        .find(|a| !ssrf_allowed(&a.to_string(), ssrf_allow_list))
}

/// SEC-03: resolve `host:port`, reject any resolved address in a blocked
/// range, then connect to a **validated** address (connect-by-IP, so DNS
/// cannot rebind to a blocked address between the check and the connect).
/// Follows up to 5 redirects, re-validating the image host pattern and the
/// SSRF range at every hop. This is what the doc comment always claimed;
/// before this fix the code was a plain GET with default redirect-following
/// and zero re-checks.
/// PERF-05: a general SSRF-safe HTTP fetch for the `fetch` op (user code's
/// server-side `fetch()`). Unlike [`fetch_remote_checked`] (image endpoint,
/// GET-only, host-pattern-gated), this supports any method + headers + body
/// and is gated only by the SSRF block ranges. Returns
/// `(status, headers, body)`.
/// A6: the only headers an SSRF-guarded fetch forwards.
///
/// A caller-supplied `Authorization` / `Cookie` must never ride a request to
/// a host the app does not own — that is a credential leak. The allowlist is
/// the cross-origin-safe set: content negotiation and identification, nothing
/// that authenticates.
pub fn ssrf_forwardable_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    const ALLOW: &[&str] = &["accept", "content-type", "user-agent"];
    headers
        .iter()
        .filter(|(k, _)| ALLOW.iter().any(|a| k.eq_ignore_ascii_case(a)))
        .cloned()
        .collect()
}

pub async fn ssrf_safe_fetch(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
    ssrf_allow_list: &[String],
) -> std::result::Result<(u16, Vec<(String, String)>, Vec<u8>), String> {
    // A6: forward only headers that are safe cross-origin. A caller-supplied
    // Authorization/Cookie must never ride an SSRF-guarded request to a host
    // the app does not own - that is a credential leak, not a feature.
    let headers: Vec<(String, String)> = ssrf_forwardable_headers(headers);

    use http_body_util::{BodyExt, Full};
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;
    use std::sync::OnceLock;

    static CLIENT: OnceLock<
        Client<hyper_util::client::legacy::connect::HttpConnector, Full<Bytes>>,
    > = OnceLock::new();
    let client = CLIENT.get_or_init(|| Client::builder(TokioExecutor::new()).build_http());

    let current = url::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    let scheme = current.scheme().to_string();
    if scheme != "http" && scheme != "https" {
        return Err(format!("scheme {scheme} not allowed"));
    }
    let host = current
        .host_str()
        .ok_or_else(|| "no host".to_string())?
        .to_string();
    let port = current
        .port_or_known_default()
        .ok_or_else(|| "no port".to_string())?;

    // Validate every resolved address before connecting (connect-by-IP).
    let addrs: Vec<std::net::IpAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| format!("dns: {e}"))?
        .map(|s| s.ip())
        .collect();
    if addrs.is_empty() {
        return Err("dns: no addresses".into());
    }
    if let Some(bad) = first_blocked_addr(&addrs, ssrf_allow_list) {
        return Err(format!("resolves to a blocked address {bad}"));
    }
    let ip = addrs[0];
    let ip_lit = if ip.is_ipv6() {
        format!("[{ip}]")
    } else {
        ip.to_string()
    };
    let path = if current.path().is_empty() {
        "/"
    } else {
        current.path()
    };
    let query = current.query().map(|q| format!("?{q}")).unwrap_or_default();
    let connect_url = format!("{scheme}://{ip_lit}:{port}{path}{query}");
    let host_header = if (scheme == "http" && port == 80) || (scheme == "https" && port == 443) {
        host.clone()
    } else {
        format!("{host}:{port}")
    };

    let mut builder = hyper::Request::builder()
        .method(method)
        .uri(&connect_url)
        .header("host", host_header);
    for (k, v) in headers {
        // Skip a caller-supplied Host (we set it) and hop-by-hop headers.
        if k.eq_ignore_ascii_case("host") || k.eq_ignore_ascii_case("connection") {
            continue;
        }
        builder = builder.header(k, v);
    }
    let req = builder
        .body(Full::new(Bytes::copy_from_slice(body)))
        .map_err(|e| format!("build request: {e}"))?;
    let resp = client
        .request(req)
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status().as_u16();
    let resp_headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();
    // Cap the body so a hostile upstream cannot exhaust memory.
    const MAX: usize = 8 * 1024 * 1024;
    let mut out: Vec<u8> = Vec::new();
    let mut b = resp.into_body();
    while let Some(frame) = b.frame().await {
        let frame = frame.map_err(|e| format!("body read failed: {e}"))?;
        if let Ok(chunk) = frame.into_data() {
            if out.len() + chunk.len() > MAX {
                return Err(format!("body exceeds the {MAX}-byte ceiling"));
            }
            out.extend_from_slice(&chunk);
        }
    }
    Ok((status, resp_headers, out))
}

async fn fetch_remote_checked(
    url: &str,
    max: usize,
    patterns: &[RemoteImagePattern],
    ssrf_allow_list: &[String],
) -> std::result::Result<Vec<u8>, String> {
    use http_body_util::{BodyExt, Full};
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;
    use std::sync::OnceLock;

    static CLIENT: OnceLock<
        Client<hyper_util::client::legacy::connect::HttpConnector, Full<Bytes>>,
    > = OnceLock::new();
    let client = CLIENT.get_or_init(|| Client::builder(TokioExecutor::new()).build_http());

    let mut current = url::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    for _hop in 0..=5 {
        let scheme = current.scheme().to_string();
        if scheme != "http" && scheme != "https" {
            return Err(format!("scheme {scheme} not allowed"));
        }
        let host = current
            .host_str()
            .ok_or_else(|| "no host".to_string())?
            .to_string();
        if !remote_pattern_allows(patterns, &host, &scheme) {
            return Err(format!("host {host:?} is not an allowed image source"));
        }
        let port = current
            .port_or_known_default()
            .ok_or_else(|| "no port".to_string())?;

        // Resolve and validate every address before connecting.
        let addrs: Vec<std::net::IpAddr> = tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|e| format!("dns: {e}"))?
            .map(|s| s.ip())
            .collect();
        if addrs.is_empty() {
            return Err("dns: no addresses".into());
        }
        if let Some(bad) = first_blocked_addr(&addrs, ssrf_allow_list) {
            return Err(format!("resolves to a blocked address {bad}"));
        }

        // Connect to the validated address; carry the real Host header so
        // name-based virtual hosting still works.
        let ip = addrs[0];
        let ip_lit = if ip.is_ipv6() {
            format!("[{ip}]")
        } else {
            ip.to_string()
        };
        let path = if current.path().is_empty() {
            "/"
        } else {
            current.path()
        };
        let query = current.query().map(|q| format!("?{q}")).unwrap_or_default();
        let connect_url = format!("{scheme}://{ip_lit}:{port}{path}{query}");
        let host_header = if (scheme == "http" && port == 80) || (scheme == "https" && port == 443)
        {
            host.clone()
        } else {
            format!("{host}:{port}")
        };

        let req = hyper::Request::builder()
            .method("GET")
            .uri(&connect_url)
            .header("host", host_header)
            .header("accept", "image/*")
            .body(Full::new(Bytes::new()))
            .map_err(|e| format!("build request: {e}"))?;
        let resp = client
            .request(req)
            .await
            .map_err(|e| format!("request failed: {e}"))?;

        if resp.status().is_redirection() {
            // Re-validate the redirect target on the next loop iteration
            // (host pattern + DNS + range); a 302 to `169.254.169.254` or to
            // a non-allowed host must not be followed blindly.
            let location = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| "redirect without a Location header".to_string())?;
            current = current
                .join(location)
                .map_err(|e| format!("bad redirect target: {e}"))?;
            continue;
        }
        if !resp.status().is_success() {
            return Err(format!("upstream returned {}", resp.status()));
        }
        let mut body = resp.into_body();
        let mut out: Vec<u8> = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|e| format!("body read failed: {e}"))?;
            if let Ok(chunk) = frame.into_data() {
                if out.len() + chunk.len() > max {
                    return Err(format!("body exceeds the {max}-byte ceiling"));
                }
                out.extend_from_slice(&chunk);
            }
        }
        return Ok(out);
    }
    Err("too many redirects (max 5)".into())
}

/// Match a remote host against `remotePatterns`. A pattern `host` matches
/// the exact hostname (case-insensitive); `*.suffix` matches `x.suffix` and
/// deeper. A pattern's `protocol` restricts the URL scheme.
pub fn remote_pattern_allows(patterns: &[RemoteImagePattern], host: &str, scheme: &str) -> bool {
    let host = host.to_ascii_lowercase();
    patterns.iter().any(|p| {
        if let Some(proto) = &p.protocol {
            if !proto.eq_ignore_ascii_case(scheme) {
                return false;
            }
        }
        let pat = p.host.to_ascii_lowercase();
        if let Some(suffix) = pat.strip_prefix("*.") {
            host.ends_with(&format!(".{suffix}"))
        } else {
            host == pat
        }
    })
}

fn split_sig(raw_url: &str) -> (&str, String) {
    let (path, query) = match raw_url.split_once('?') {
        Some((p, q)) => (p, q),
        None => (raw_url, ""),
    };
    let filtered: Vec<&str> = query
        .split('&')
        .filter(|p| !p.is_empty() && !p.starts_with("s="))
        .collect();
    (path, filtered.join("&"))
}

/// §9.4: run a server function through the dispatcher and build the JSON
/// response envelope. `{"ok":true,"data":…}` on success, `{"ok":false,
/// "code":…,"message":…}` on failure — the shape `oxide/fn`'s
/// `__oxide_call_fn` decodes.
async fn run_server_fn(
    dispatcher: Option<&dyn ServerFnDispatcher>,
    id: &str,
    args: &Bytes,
) -> OxideResponse {
    let Some(dispatcher) = dispatcher else {
        return OxideResponse::json(
            404,
            serde_json::json!({
                "ok": false,
                "code": "FN-NO-DISPATCHER",
                "message": "server functions are not enabled in this build",
            }),
        );
    };
    match dispatcher.dispatch(id, args).await {
        Ok(body) => {
            // The dispatcher returns the *encoded* return value; the
            // envelope wraps it. If the dispatcher already produced a
            // full envelope (a future optimization), the double-wrap is
            // harmless because the client unwraps `data` anyway.
            let data: serde_json::Value = serde_json::from_slice(&body).unwrap_or_else(|_| {
                serde_json::Value::String(String::from_utf8_lossy(&body).into_owned())
            });
            OxideResponse::json(200, serde_json::json!({ "ok": true, "data": data }))
        }
        Err(e) => OxideResponse::json(
            e.status,
            serde_json::json!({ "ok": false, "code": e.code, "message": e.message }),
        ),
    }
}

/// §9.3: run a form action through the dispatcher and build the response.
/// A not-found action is a 404, a transport failure a 500; a validated
/// failure (422) and the action's own status pass through. Cookies and
/// headers the action set ride the response; revalidations are applied to
/// nothing here (the pipeline has no cache handle) — they are surfaced on
/// the response as a header the cache layer can read.
#[allow(clippy::too_many_arguments)]
pub async fn run_action(
    dispatcher: Option<&dyn ActionDispatcher>,
    module_path: &str,
    route: &str,
    action: &str,
    form_json: &str,
    params: &serde_json::Map<String, serde_json::Value>,
    url: &str,
    method: &str,
    headers: &[(String, String)],
    // §8.1 middleware.ts / §9.1: request-scoped locals injected by middleware.
    locals: &serde_json::Value,
    // §20: W3C trace context for outgoing fetches inside the action.
    trace: Option<&serde_json::Value>,
) -> OxideResponse {
    let Some(dispatcher) = dispatcher else {
        return OxideResponse::json(
            500,
            serde_json::json!({"error": {"code": "ACT-NO-DISPATCHER", "message": "no action dispatcher configured"}}),
        );
    };
    match dispatcher
        .dispatch(
            module_path,
            action,
            form_json,
            params,
            url,
            method,
            headers,
            locals,
            trace,
        )
        .await
    {
        Ok(out) => {
            let mut resp = if out.status == 200 {
                OxideResponse::json(200, out.data.clone())
            } else {
                let mut r = OxideResponse::new(out.status, out.data.to_string())
                    .with_header("content-type", "application/json");
                if let Some(loc) = out.location.clone() {
                    r = r.with_header("location", loc);
                }
                r
            };
            for c in &out.set_cookies {
                resp = resp.with_header("set-cookie", c.clone());
            }
            for (k, v) in &out.headers {
                resp = resp.with_header(k, v.clone());
            }
            // §11.3: tag/path invalidations the action requested. The CLI
            // dispatch reads this header and applies it to the L0/L1 cache.
            if !out.revalidate.is_empty() {
                let encoded: Vec<String> = out
                    .revalidate
                    .iter()
                    .map(|(k, v)| format!("{k}:{v}"))
                    .collect();
                resp = resp.with_header("x-oxide-revalidate", encoded.join(","));
            }
            resp
        }
        Err(e) => {
            // The dispatcher already mapped the worker's error code to an
            // HTTP status; honour it rather than re-deriving from the code
            // string (the CLI rewrites `JSH-ACTION*` to `ACT*`, so a string
            // compare here would always miss and fall through to 500).
            let status = e.status;
            OxideResponse::json(
                status,
                serde_json::json!({"error": {
                    "code": e.code,
                    "message": e.message,
                    "route": route,
                    "action": action,
                }}),
            )
        }
    }
}

/// Result of the native (no-JS) prefix of the pipeline ([`Pipeline::native_prefix`]).
enum NativePrefix {
    /// Response returned as-is (limits, host, health, rules, csrf failures).
    Raw(OxideResponse),
    /// Static asset hit — finished with security + rule headers.
    Static(OxideResponse, Vec<(String, String)>),
    /// A server-function call (§9.4): the pipeline has already checked the
    /// method, the content type, and CSRF; the async arm runs the
    /// dispatcher and builds the JSON envelope.
    ServerFn {
        id: String,
        args: Bytes,
        rule_headers: Vec<(String, String)>,
    },
    /// A remote image transform (§16.1). The sync prefix validated the URL;
    /// the async arm fetches it and runs the backend's transform.
    RemoteImage {
        endpoint: Arc<ImageEndpoint>,
        request: ImageEndpointRequest,
        format: Option<&'static str>,
        rule_headers: Vec<(String, String)>,
    },
    /// Proceed to dispatch carrying the merged rule headers.
    Dispatch(Vec<(String, String)>, Option<String>),
}

impl Pipeline {
    /// Attach a metrics registry (§20). The pipeline observes its own native
    /// branches (`_native`, `_static`); the JS delegate observes the dispatch
    /// path. Idempotent — replacing an existing registry is fine.
    pub fn with_metrics(mut self, m: Arc<Metrics>) -> Self {
        self.metrics = Some(m);
        self
    }

    /// The attached metrics registry, if any.
    pub fn metrics(&self) -> Option<&Arc<Metrics>> {
        self.metrics.as_ref()
    }

    /// Attach an inspector (§13.4). Only the dev command calls this.
    pub fn with_inspector(mut self, insp: Arc<dyn InspectorSnapshot>) -> Self {
        self.inspector = Some(insp);
        self
    }

    /// Attach a page renderer (§7). When set, `serve_request` dispatches page
    /// renders to it. When `None` (default), the pipeline preserves the
    /// legacy neutral-IR behavior. Chainable.
    pub fn with_page_renderer(mut self, r: Arc<dyn crate::PageRenderer>) -> Self {
        self.page_renderer = Some(r);
        self
    }

    /// The attached page renderer, if any. Used by the inspector and by
    /// `serve_request` to decide the render path.
    pub fn page_renderer(&self) -> Option<&Arc<dyn crate::PageRenderer>> {
        self.page_renderer.as_ref()
    }

    /// Handle one request through the native (no-JS) prefix of the pipeline.
    /// Dispatch to JS/render is delegated to `dispatch` when the rules+static+L0
    /// layers don't produce a response (§4.1: hot path completes without JS when
    /// cached/redirected/rewritten/static).
    ///
    /// Returns a boxed future instead of being an `async fn`: rustc holds *all*
    /// async-fn arguments across *every* await point, so a `&dyn Fn` parameter
    /// (whose referent is not `Sync`) would make this future non-`Send` and
    /// unusable inside hyper connection tasks. Here `dispatch` is only *called*
    /// synchronously — the returned `'static` BoxFuture is itself `Send` — and
    /// the await sees only `Send` values. Parameter types are unchanged.
    pub fn serve_request(
        &self,
        mut req: OxideRequest,
        dispatch: &dyn Fn(OxideRequest) -> futures::future::BoxFuture<'static, OxideResponse>,
    ) -> futures::future::BoxFuture<'_, OxideResponse> {
        // §20 tracing: one span per request, propagated through the W3C trace
        // context when an OTLP endpoint is configured. The span carries the
        // request id so it can be correlated with `Server-Timing` and logs.
        let span = tracing::info_span!(
            "oxide.request",
            request_id = %req.request_id,
            method = %req.method,
            path = %path_of(&req.url),
        );
        let _enter = span.enter();

        // §20 metrics: time the request, observe the response with a bounded
        // route label. The native prefix is entirely synchronous, so Raw and
        // Static are observed here. The Dispatch branch observes nothing —
        // the JS delegate (which owns routing) is responsible for
        // per-route labels on that path.
        let started = std::time::Instant::now();
        let metrics = self.metrics.clone();

        // SPEC-24: stage spans. `match` covers routing + native rules; the
        // render/load happen inside the dispatch delegate (which emits its own
        // `total`); these spans make the native stages observable.
        let _match = tracing::info_span!("oxide.match").entered();
        let native = self.native_prefix(&mut req);
        drop(_match);
        match native {
            NativePrefix::Raw(resp) => {
                if let Some(m) = &metrics {
                    m.observe("_native", started.elapsed(), resp.status);
                }
                Box::pin(std::future::ready(resp))
            }
            NativePrefix::Static(resp, rule_headers) => {
                let request_id = req.request_id.clone();
                let finished = self.finish_response_with_request(
                    resp,
                    &rule_headers,
                    &request_id,
                    Some(&req.method),
                    req.header("cookie"),
                );
                if let Some(m) = &metrics {
                    m.observe("_static", started.elapsed(), finished.status);
                }
                Box::pin(std::future::ready(finished))
            }
            NativePrefix::RemoteImage {
                endpoint,
                request,
                format,
                rule_headers,
            } => {
                let request_id = req.request_id.clone();
                let method = req.method.clone();
                let cookie = req.header("cookie").map(str::to_string);
                Box::pin(async move {
                    let resp = fetch_remote_image(endpoint, request, format).await;
                    self.finish_response_with_request(
                        resp,
                        &rule_headers,
                        &request_id,
                        Some(&method),
                        cookie.as_deref(),
                    )
                })
            }
            NativePrefix::ServerFn {
                id,
                args,
                rule_headers,
            } => {
                // §9.4: the pipeline already checked method + content type
                // + CSRF in `native_prefix`; this arm runs the user's
                // function and builds the response envelope.
                let request_id = req.request_id.clone();
                let method = req.method.clone();
                let cookie = req.header("cookie").map(str::to_string);
                let dispatch = self.server_fns.clone();
                Box::pin(async move {
                    let resp = run_server_fn(dispatch.as_deref(), &id, &args).await;
                    self.finish_response_with_request(
                        resp,
                        &rule_headers,
                        &request_id,
                        Some(&method),
                        cookie.as_deref(),
                    )
                })
            }
            NativePrefix::Dispatch(rule_headers, content_language) => {
                // Capture the request id before `req` moves into `dispatch` —
                // `finish_response` needs it to derive the per-request CSP
                // nonce, and the framework dispatch derives the same value
                // from the same id to stamp the boot script tag (§17.1).
                let request_id = req.request_id.clone();
                let method = req.method.clone();
                let cookie = req.header("cookie").map(str::to_string);
                let accept_encoding = req.header("accept-encoding").map(str::to_string);
                // PERF-06: a real total duration for the framework render path.
                // `started` was captured at `serve_request` entry, so this
                // covers match + middleware + load + render. Reported as
                // `total` — one honest number rather than the previous
                // all-zero `RequestTimings::default()`. Per-stage breakdown
                // (match/cache/load/render/inject) needs the pipeline to
                // expose stage instants; scoped.
                let started_render = started;
                // Call dispatch synchronously; only the (Send) boxed future crosses the await.
                let fut = dispatch(req);
                Box::pin(async move {
                    let mut resp = fut.await;
                    if !resp
                        .headers
                        .iter()
                        .any(|(k, _)| k.eq_ignore_ascii_case("server-timing"))
                    {
                        let ms = started_render.elapsed().as_secs_f64() * 1000.0;
                        resp = resp.with_header("Server-Timing", format!("total;dur={ms:.2}"));
                    }
                    // SPEC-10: the resolved locale rides the page response.
                    if let Some(loc) = &content_language {
                        if !resp
                            .headers
                            .iter()
                            .any(|(k, _)| k.eq_ignore_ascii_case("content-language"))
                        {
                            resp = resp.with_header("Content-Language", loc.clone());
                        }
                    }
                    self.finish_response_full_async(
                        resp,
                        &rule_headers,
                        &request_id,
                        Some(&method),
                        cookie.as_deref(),
                        accept_encoding.as_deref(),
                    )
                    .await
                })
            }
        }
    }

    /// Synchronous native prefix (§12.1 steps 2-6): limits → host allow-list →
    /// health → rules → static → csrf. Mutates `req.url` on rewrites.
    /// S-10: the rate-limit bucket key. Uses the socket peer; an
    /// `X-Forwarded-For` leftmost entry is honoured only when the peer is a
    /// trusted proxy. Falls back to a constant when no peer is known
    /// (in-process tests).
    fn client_key(&self, req: &OxideRequest) -> String {
        match req.peer {
            Some(peer) => {
                let xff = req.header("x-forwarded-for");
                let (ip, _, _) =
                    effective_client(peer, &self.security.trusted_proxies, xff, None, None, None);
                ip.unwrap_or("local").to_string()
            }
            None => "local".to_string(),
        }
    }

    fn native_prefix(&self, req: &mut OxideRequest) -> NativePrefix {
        let rid = ("x-oxide-request-id", req.request_id.clone());

        // (1) limits (§12.1 step 2) — request id echoed on every violation.
        if req.body.len() > self.limits.max_body {
            return NativePrefix::Raw(
                OxideResponse::new(413, "payload too large").with_header(rid.0, rid.1),
            );
        }
        if req.url.len() > self.limits.max_url_len {
            return NativePrefix::Raw(
                OxideResponse::new(414, "url too long").with_header(rid.0, rid.1),
            );
        }
        if req.headers.len() > self.limits.max_headers {
            return NativePrefix::Raw(
                OxideResponse::new(431, "too many headers").with_header(rid.0, rid.1),
            );
        }

        // (2) Host allow-list (§17.1) — only enforced when configured.
        if !self.security.host_allow_list.is_empty() {
            let ok = req
                .header("host")
                .is_some_and(|h| host_allowed(h, &self.security.host_allow_list));
            if !ok {
                return NativePrefix::Raw(OxideResponse::new(421, "misdirected request"));
            }
        }

        let mut path = path_of(&req.url);
        // SPEC-10 / §16.4: locale routing. A locale prefix selects the locale
        // and is stripped (the route table is locale-free); a missing prefix
        // on a non-default `Accept-Language` redirects to the prefixed URL
        // under the strategy. `Content-Language` rides the response.
        let mut content_language: Option<String> = None;
        if let Some(pol) = self.i18n.as_ref().filter(|p| p.is_enabled()) {
            if let Some((loc, rest)) = pol.explicit(&path) {
                content_language = Some(loc);
                path = rest.to_string();
                req.url = url_with_path(&req.url, &path);
            } else {
                let neg = pol.negotiate(req.header("accept-language").unwrap_or(""));
                if pol.prefixes(&neg) {
                    let loc = format!("/{neg}");
                    let dest = if path == "/" {
                        loc
                    } else {
                        format!("{loc}{path}")
                    };
                    return NativePrefix::Raw(
                        OxideResponse::new(307, Bytes::new())
                            .with_header("Location", dest)
                            .with_header("Content-Language", neg),
                    );
                }
                content_language = Some(neg);
            }
        }

        // (3a) metrics endpoint (§20) — Prometheus text exposition.
        if req.method.eq_ignore_ascii_case("GET")
            && !self.metrics_path.is_empty()
            && path == self.metrics_path
        {
            return NativePrefix::Raw(match &self.metrics {
                Some(m) => OxideResponse::new(200, Bytes::from(m.render()))
                    .with_header("content-type", "text/plain; version=0.0.4; charset=utf-8"),
                None => OxideResponse::new(404, "metrics endpoint not configured")
                    .with_header("content-type", "text/plain; charset=utf-8"),
            });
        }

        // (3a.1) web-vitals beacon (§16.6 / §20). The client's reporting hook
        // POSTs `navigator.sendBeacon('/_oxide/vitals', JSON)`; we record each
        // sample into the metrics registry. A beacon must be cheap and never
        // error in a way the page sees — a malformed body is a 204, not a 4xx,
        // because the browser does not read the response anyway. Recording
        // never enters JS.
        if req.method.eq_ignore_ascii_case("POST") && path == "/_oxide/vitals" {
            if let Some(m) = &self.metrics {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&req.body) {
                    // Accept one sample `{name, value, route?}` or an array.
                    let items: Vec<&serde_json::Value> = match &v {
                        serde_json::Value::Array(a) => a.iter().collect(),
                        other => vec![other],
                    };
                    for it in items {
                        let name = it.get("name").and_then(|x| x.as_str()).unwrap_or("");
                        let value = it.get("value").and_then(|x| x.as_f64());
                        let raw_route = it.get("route").and_then(|x| x.as_str()).unwrap_or("");
                        // S-18: the endpoint is public, so an attacker could
                        // mint a metrics key per unique `route`. Normalize to a
                        // route-id shape and fold anything else into `_other`.
                        let route = sanitize_route_label(raw_route);
                        if !name.is_empty() {
                            if let Some(val) = value {
                                m.observe_vital(name, &route, val);
                            }
                        }
                    }
                }
            }
            return NativePrefix::Raw(OxideResponse::new(204, Bytes::new()));
        }

        // (3a.2) inspector endpoints (§13.4) — only when configured (dev only).
        if let Some(insp) = &self.inspector {
            let is_inspector = (req.method.eq_ignore_ascii_case("GET") && path == "/__oxide")
                || (req.method.eq_ignore_ascii_case("GET") && path == "/__oxide/state");
            if is_inspector {
                // SEC-09: gate on the dev session token when one is set.
                if let Some(want) = &self.dev_token {
                    if !dev_token_ok(req, want) {
                        return NativePrefix::Raw(
                            OxideResponse::new(403, "inspector requires the dev token")
                                .with_header("content-type", "text/plain; charset=utf-8"),
                        );
                    }
                }
                if path == "/__oxide" {
                    return NativePrefix::Raw(
                        OxideResponse::new(200, insp.render_html())
                            .with_header("content-type", "text/html; charset=utf-8"),
                    );
                }
                return NativePrefix::Raw(OxideResponse::json(200, insp.state_json()));
            }
        }

        // (3b) health endpoint (§12.2).
        if req.method.eq_ignore_ascii_case("GET") && path == self.health_path {
            return NativePrefix::Raw(
                OxideResponse::new(200, "ok")
                    .with_header("content-type", "text/plain; charset=utf-8"),
            );
        }

        // (3b') readiness endpoint (§6.4 / Wave M). 200 when the renderer
        // is warm; 503 with retry-after while warming. Distinct from
        // health — traffic can flow as soon as health is 200; readiness
        // is what an LB or a benchmark polls to avoid measuring warm-up.
        if !self.ready_path.is_empty()
            && req.method.eq_ignore_ascii_case("GET")
            && path == self.ready_path
        {
            let ready = self.render_ready.load(std::sync::atomic::Ordering::Relaxed);
            let mut r = if ready {
                OxideResponse::new(200, "ok")
            } else {
                OxideResponse::new(503, "warming")
            };
            r = r.with_header("content-type", "text/plain; charset=utf-8");
            if !ready {
                r = r.with_header("retry-after", "1");
            }
            return NativePrefix::Raw(r);
        }

        // (3c) image endpoint (§16.1). GET/HEAD only. The transform runs
        // in Rust, never entering JS; a missing endpoint or an invalid
        // request yields a coded 4xx that names the reason.
        // §16.3: the OG-image endpoint. Rendered synchronously through the
        // backend (resvg/taffy/parley in oxide-fw-assets); a PNG response.
        if req.method.eq_ignore_ascii_case("GET") && path == "/_oxide/og" {
            if let Some(ep) = self.og_endpoint.as_deref() {
                // A3: resvg rasterization is CPU-bound and the endpoint is
                // unauthenticated. Rate-limit per peer before the work, not
                // after. `og_rate_limiter` is shared across requests.
                // Use the same trusted-proxy-aware key as the general
                // rate-limit rule; a raw leftmost `X-Forwarded-For` is
                // spoofable and let one client mint unbounded buckets.
                let peer = self.client_key(req);
                if !self
                    .og_rate_limiter
                    .allow(&peer, self.og_rate_max, self.og_rate_window_secs)
                {
                    return NativePrefix::Raw(
                        OxideResponse::new(429, "og rate limit exceeded")
                            .with_header("retry-after", self.og_rate_window_secs.to_string()),
                    );
                }
                return NativePrefix::Raw(serve_og_endpoint(ep, &req.url));
            }
            return NativePrefix::Raw(
                OxideResponse::new(404, "og endpoint not configured")
                    .with_header("content-type", "text/plain; charset=utf-8"),
            );
        }

        if req.method.eq_ignore_ascii_case("GET") && path == "/_oxide/image" {
            return serve_image_endpoint(self.image_endpoint.as_deref(), &req.url);
        }

        // (3d) server functions (§9.4): `POST /_oxide/fn/<id>`.
        //
        // The route is POST-only, requires the `application/oxide-fn`
        // content type (so a form action POST is not mistaken for one), and
        // passes CSRF before the dispatcher is reached. The id is taken
        // from the path; the body is the encoded argument list.
        if let Some(id) = path.strip_prefix("/_oxide/fn/") {
            if !req.method.eq_ignore_ascii_case("POST") {
                return NativePrefix::Raw(
                    OxideResponse::new(405, "server functions are POST-only")
                        .with_header("allow", "POST")
                        .with_header("content-type", "text/plain; charset=utf-8"),
                );
            }
            let ct = req
                .header("content-type")
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim();
            if ct != "application/oxide-fn" {
                return NativePrefix::Raw(
                    OxideResponse::new(
                        415,
                        "server functions require content-type application/oxide-fn",
                    )
                    .with_header("content-type", "text/plain; charset=utf-8"),
                );
            }
            if let Err(d) = crate::csrf_check(req, &self.security.csrf) {
                return NativePrefix::Raw(OxideResponse::new(403, d.message));
            }
            let id = percent_decode(id);
            // Merge rule headers so a matching header rule still applies.
            let mut rule_headers: Vec<(String, String)> = Vec::new();
            let fn_client_key = self.client_key(req);
            // SEC-11: the server-fn route evaluates the FULL rules engine —
            // redirect, basic-auth, rate-limit, and header rules — not just
            // rate-limit + headers. A fn behind an auth gate 401s; one under a
            // redirect rule redirects.
            let fn_auth = req.header("authorization");
            match self
                .rules
                .evaluate_with_auth(&path, &fn_client_key, fn_auth.as_deref())
            {
                oxide_fw_router::RuleOutcome::Headers(map) => {
                    rule_headers = map.into_iter().collect();
                }
                oxide_fw_router::RuleOutcome::RateLimited { retry_after_secs } => {
                    return NativePrefix::Raw(
                        OxideResponse::new(429, "too many requests")
                            .with_header("retry-after", retry_after_secs.to_string()),
                    );
                }
                oxide_fw_router::RuleOutcome::AuthRequired => {
                    return NativePrefix::Raw(OxideResponse::new(401, "unauthorized").with_header(
                        "WWW-Authenticate",
                        "Basic realm=\"oxide\", charset=\"UTF-8\"",
                    ));
                }
                oxide_fw_router::RuleOutcome::Redirect {
                    destination,
                    status,
                } => {
                    return NativePrefix::Raw(
                        OxideResponse::new(status, Bytes::new())
                            .with_header("Location", destination),
                    );
                }
                oxide_fw_router::RuleOutcome::Rewrite(_) | oxide_fw_router::RuleOutcome::None => {}
            }
            let args = req.body.clone();
            return NativePrefix::ServerFn {
                id,
                args,
                rule_headers,
            };
        }

        // (4) static assets (§12.1 step 3 — BEFORE rules, per the spec order).
        // A static file under a rule source is served, not redirected: the
        // asset store answers first. `rule_headers` is populated by step (5)
        // for the dispatch path; a static hit returns before rules run.
        let mut rule_headers: Vec<(String, String)> = Vec::new();
        let is_get_head = matches!(req.method.to_ascii_uppercase().as_str(), "GET" | "HEAD");
        if is_get_head && (path_has_extension(&path) || path.starts_with("/_oxide/static")) {
            if let Some(assets) = &self.assets {
                if let Some(resp) = assets.serve(req, &path) {
                    return NativePrefix::Static(resp, rule_headers);
                }
            }
        }

        // (5) native rules (§12.1 step 4).
        // SEC-01 / S-10: bucket per client from the *socket peer*; an
        // `X-Forwarded-For` is honoured only when the peer is a trusted proxy
        // (config `security.trustedProxies`), so a spoofed header cannot mint
        // a fresh bucket.
        let client_key = self.client_key(req);
        let req_auth = req.header("authorization");
        match self
            .rules
            .evaluate_with_auth(&path, &client_key, req_auth.as_deref())
        {
            oxide_fw_router::RuleOutcome::Redirect {
                destination,
                status,
            } => {
                return NativePrefix::Raw(
                    OxideResponse::new(status, Bytes::new()).with_header("Location", destination),
                );
            }
            oxide_fw_router::RuleOutcome::Rewrite(dest) => {
                req.url = url_with_path(&req.url, &dest);
            }
            oxide_fw_router::RuleOutcome::Headers(map) => {
                rule_headers = map.into_iter().collect();
            }
            oxide_fw_router::RuleOutcome::AuthRequired => {
                return NativePrefix::Raw(OxideResponse::new(401, "unauthorized").with_header(
                    "WWW-Authenticate",
                    "Basic realm=\"oxide\", charset=\"UTF-8\"",
                ));
            }
            oxide_fw_router::RuleOutcome::RateLimited { retry_after_secs } => {
                // SEC-01: a 429 with the seconds until a token refills.
                return NativePrefix::Raw(
                    OxideResponse::new(429, "too many requests")
                        .with_header("retry-after", retry_after_secs.to_string()),
                );
            }
            oxide_fw_router::RuleOutcome::None => {}
        }

        // (6) dispatch (§12.1 step 7) with CSRF verification (§17.1) first.
        if let Err(d) = csrf_check(req, &self.security.csrf) {
            return NativePrefix::Raw(OxideResponse::new(403, Bytes::from(d.message)));
        }
        NativePrefix::Dispatch(rule_headers, content_language)
    }

    /// Adds the CSRF token issuer (SEC-04): on a GET/HEAD
    /// response in `token` mode with no `oxide_csrf` cookie on the request,
    /// set a random 32-hex cookie (SameSite=Lax, Path=/). `method`/`cookie`
    /// are `None` on paths where the request is no longer available (all
    /// current call sites pass them; `None` skips issuance, never fails).
    fn finish_response_with_request(
        &self,
        resp: OxideResponse,
        rule_headers: &[(String, String)],
        request_id: &str,
        method: Option<&str>,
        cookie_header: Option<&str>,
    ) -> OxideResponse {
        self.finish_response_full(resp, rule_headers, request_id, method, cookie_header, None)
    }

    /// PERF-06: `finish_response_with_request` plus dynamic compression when
    /// the caller passes the request's `Accept-Encoding`. Kept separate so the
    /// many existing call sites (which have no body worth compressing, or a
    /// pre-compressed static asset) do not need the extra argument.
    /// PERF-06: the sync form finalizes headers/security only. Dynamic
    /// compression lives in [`Self::finish_response_full_async`] so the CPU
    /// work runs on the blocking pool, not a Tokio worker.
    fn finish_response_full(
        &self,
        resp: OxideResponse,
        rule_headers: &[(String, String)],
        request_id: &str,
        method: Option<&str>,
        cookie_header: Option<&str>,
        accept_encoding: Option<&str>,
    ) -> OxideResponse {
        let _ = accept_encoding;
        // §12.1: advertise HTTP/3 so a browser upgrades on the next request.
        // A 0 port means the h3 listener is disabled.
        let mut resp = resp;
        if self.http3_port != 0
            && !resp
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("alt-svc"))
        {
            let port = self.http3_port;
            // C3: advert only when `http3_advertise` is set on the pipeline.
            // The listener still runs with it off — a client that already
            // knows the endpoint keeps working; a fresh browser stays on
            // h1/h2.
            if self.http3_advertise {
                // P-16: the port is fixed for the server's lifetime, so the
                // Alt-Svc value is formatted once and reused.
                let value = alt_svc_for(port);
                resp = resp.with_header("alt-svc", value);
            }
        }

        self.finish_response_inner(resp, rule_headers, request_id, method, cookie_header)
    }

    /// PERF-06 / §12.1 step 8: dynamic compression off the reactor.
    /// `zstd`/`gzip` at level 3 on a 50 KB page is ~0.2-0.5 ms of pure CPU;
    /// inline on a Tokio worker it blocks every other task on that worker.
    /// `spawn_blocking` moves it to the blocking pool. Only a non-streaming
    /// body with a compressible content type and no existing encoding is
    /// considered — a static asset arrives pre-compressed.
    async fn finish_response_full_async(
        &self,
        mut resp: OxideResponse,
        rule_headers: &[(String, String)],
        request_id: &str,
        method: Option<&str>,
        cookie_header: Option<&str>,
        accept_encoding: Option<&str>,
    ) -> OxideResponse {
        if let Some(ae) = accept_encoding {
            let already_encoded = resp
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-encoding"));
            // P-15: streamed bodies are compressed too. The streaming path
            // takes precedence; buffered bodies keep the whole-body branch.
            if !already_encoded && resp.stream.is_some() {
                let ct_owned = resp
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default();
                if is_compressible(&ct_owned) {
                    if let Some(enc) = negotiate_encoding(ae, &["zstd", "gzip"]) {
                        if let Some(stream) = resp.stream.take() {
                            resp.stream = Some(spawn_stream_compressor(&enc, stream));
                            resp.headers
                                .retain(|(k, _)| !k.eq_ignore_ascii_case("content-length"));
                            resp.headers.push(("Content-Encoding".to_string(), enc));
                            resp.headers
                                .push(("Vary".to_string(), "Accept-Encoding".to_string()));
                        }
                    }
                }
            }
            let compressible = resp.stream.is_none() && !resp.body.is_empty() && !already_encoded;
            if compressible {
                let ct_owned = resp
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default();
                if is_compressible(&ct_owned) {
                    if let Some(enc) = negotiate_encoding(ae, &["zstd", "gzip"]) {
                        let body = resp.body.clone();
                        let orig_len = body.len();
                        let enc_for_task = enc.clone();
                        let joined = tokio::task::spawn_blocking(move || {
                            compress_body(&enc_for_task, &body, 3).ok()
                        })
                        .await;
                        if let Ok(Some(compressed)) = joined {
                            if compressed.len() < orig_len {
                                resp.body = compressed;
                                resp.headers
                                    .retain(|(k, _)| !k.eq_ignore_ascii_case("content-length"));
                                resp.headers.push(("Content-Encoding".to_string(), enc));
                                resp.headers
                                    .push(("Vary".to_string(), "Accept-Encoding".to_string()));
                            }
                        }
                    }
                }
            }
        }
        self.finish_response_full(resp, rule_headers, request_id, method, cookie_header, None)
    }

    fn finish_response_inner(
        &self,
        resp: OxideResponse,
        rule_headers: &[(String, String)],
        request_id: &str,
        method: Option<&str>,
        cookie_header: Option<&str>,
    ) -> OxideResponse {
        // §17.1: the CSP nonce is derived from the request id. The id is a
        // property of the *request*, not the response; hashing an empty
        // string when the response carries no `x-oxide-request-id` header
        // (which is the normal path — the header is only added on native
        // error responses) yields the same nonce for every response, which
        // is both a security hole and a mismatch against the framework
        // dispatch's per-request nonce.
        let nonce = nonce_for(request_id);
        let mut r = resp;

        let csp_nonce = match self.security.csp.as_str() {
            "off" => None,
            "hash" => None, // hash policy appended explicitly below
            _ => Some(nonce.as_str()),
        };
        for (k, v) in security_headers(self.tls, csp_nonce) {
            r = r.with_header(k.as_str(), v);
        }
        if self.security.csp == "hash" {
            r = r.with_header("Content-Security-Policy", csp_header("hash", &nonce));
        }
        for (k, v) in rule_headers {
            r = r.with_header(k.as_str(), v.clone());
        }
        // S-21.3: a strict deployment hides whether a hit was served from
        // cache. Drop the header (set by the L0 layer) when disabled.
        if !self.security.expose_cache_header {
            r.headers
                .retain(|(k, _)| !k.eq_ignore_ascii_case("x-oxide-fw-cache"));
        }
        if !r
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("server-timing"))
        {
            r = r.with_header("Server-Timing", RequestTimings::default().server_timing());
        }
        // SEC-04 issuer: token mode needs a token to exist. Issue on safe
        // methods only; the double-submit check compares this cookie against
        // the `x-oxide-csrf` header on unsafe methods.
        // SPEC-24: one redacted request log line per finished response.
        log_request(request_id, method.unwrap_or(""), r.status, &r.headers);
        if self.security.csrf.eq_ignore_ascii_case("token")
            && matches!(
                method.map(|m| m.to_ascii_uppercase()).as_deref(),
                Some("GET") | Some("HEAD")
            )
            && cookie_value(cookie_header.unwrap_or(""), "oxide_csrf").is_none()
        {
            use ring::rand::SecureRandom as _;
            let rng = ring::rand::SystemRandom::new();
            let mut bytes = [0u8; 16];
            if rng.fill(&mut bytes).is_ok() {
                let hex: String = bytes
                    .iter()
                    .flat_map(|b| {
                        [
                            char::from_digit((b >> 4) as u32, 16).unwrap(),
                            char::from_digit((b & 0xf) as u32, 16).unwrap(),
                        ]
                    })
                    .collect();
                // S-21.4: `Secure` whenever the listener serves TLS.
                let secure = if self.tls { "; Secure" } else { "" };
                r = r.with_header(
                    "Set-Cookie",
                    &format!("oxide_csrf={hex}; Path=/; SameSite=Lax{secure}"),
                );
            }
        }
        r
    }
}

/// Splice `tag` into a chunked HTML body stream immediately before the first
/// `</body>` (matched across chunk boundaries via a small rolling buffer).
///
/// Used by the framework pipeline to inject the island boot script into a
/// live SSR response. The transform is generic — any tag, any close marker —
/// because it's just a byte-stream rewrite and there's no reason to special-
/// case HTML here.
///
/// Buffer bound: the rolling tail keeps at most `close.len() - 1` bytes
/// between iterations, so a large response does not accumulate in memory.
/// If the stream ends without a matching `close`, the tag is appended at the
/// end (the shell is malformed HTML at that point anyway; the boot script
/// still has to load).
pub fn inject_before_close(src: BodyStream, close: &[u8], tag: &[u8]) -> BodyStream {
    use bytes::Bytes;
    use futures::StreamExt as _;

    if close.is_empty() {
        // No anchor: pass through unchanged. A caller passing an empty
        // needle is a bug, but returning the source unmodified is safer
        // than panicking.
        return src;
    }

    let close_owned: Vec<u8> = close.to_vec();
    let tag_owned: Vec<u8> = tag.to_vec();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(8);

    tokio::spawn(async move {
        let close = close_owned;
        let tag = tag_owned;
        let mut tail: Vec<u8> = Vec::new();
        let mut injected = false;
        let mut src = src;

        while let Some(item) = src.next().await {
            let chunk = match item {
                Ok(b) => b,
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            };
            if injected {
                if tx.send(Ok(chunk)).await.is_err() {
                    return;
                }
                continue;
            }

            let mut buf: Vec<u8> = std::mem::take(&mut tail);
            buf.extend_from_slice(&chunk);

            if let Some(idx) = find_subslice(&buf, &close) {
                let before = Bytes::from(buf[..idx].to_vec());
                let after = Bytes::from(buf[idx..].to_vec());
                if tx.send(Ok(before)).await.is_err() {
                    return;
                }
                if tx.send(Ok(Bytes::from(tag.clone()))).await.is_err() {
                    return;
                }
                if !after.is_empty() && tx.send(Ok(after)).await.is_err() {
                    return;
                }
                injected = true;
                continue;
            }

            let keep = close.len() - 1;
            if buf.len() > keep {
                let flush_len = buf.len() - keep;
                let flush = Bytes::from(buf[..flush_len].to_vec());
                if tx.send(Ok(flush)).await.is_err() {
                    return;
                }
                tail = buf[flush_len..].to_vec();
            } else {
                tail = buf;
            }
        }

        if !injected {
            if !tail.is_empty() {
                let _ = tx.send(Ok(Bytes::from(tail))).await;
            }
            let _ = tx.send(Ok(Bytes::from(tag))).await;
        }
    });

    Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)))
}

/// First index of `needle` in `haystack`. Naive window scan; the needles we
/// search for here are tiny (`</body>` is 7 bytes) so any preprocessing would
/// cost more than it saves.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Convert between hyper and Oxide request/response (streaming bodies preserved via BoxBody).
pub fn to_hyper_response(resp: OxideResponse) -> HyperResponse<BoxBody<Bytes, std::io::Error>> {
    let has_len = resp
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("content-length"));
    let body_len = resp.body.len();
    let is_streaming = resp.stream.is_some();

    // Build the boxed body. Two branches:
    //   * streaming: `StreamBody` over the `BodyStream`, each chunk in a
    //     `Frame::data`. Item error type is `io::Error`.
    //   * collected: `Full<Bytes>` (single frame) with the item error type
    //     lifted from `Infallible` to `io::Error` via `map_err(|n| match n {})`.
    let boxed: BoxBody<Bytes, std::io::Error> = if let Some(stream) = resp.stream {
        use futures::StreamExt as _;
        let framed = stream.map(|r| r.map(hyper::body::Frame::data));
        // `.boxed()` is ambiguous between BodyExt and StreamExt; call
        // BodyExt's explicitly. The resulting type is
        // `BoxBody<Bytes, io::Error>` which matches the alias above.
        BodyExt::boxed(http_body_util::StreamBody::new(framed))
    } else {
        BodyExt::boxed(Full::new(resp.body).map_err(|never| match never {}))
    };

    let mut out: HyperResponse<BoxBody<Bytes, std::io::Error>> = HyperResponse::new(boxed);
    *out.status_mut() = hyper::StatusCode::from_u16(resp.status)
        .unwrap_or(hyper::StatusCode::INTERNAL_SERVER_ERROR);
    for (k, v) in resp.headers {
        if let (Ok(name), Ok(val)) = (
            hyper::header::HeaderName::from_bytes(k.as_bytes()),
            hyper::header::HeaderValue::from_str(&v),
        ) {
            out.headers_mut().append(name, val);
        } else {
            tracing::warn!(header = %k, "dropping invalid response header");
        }
    }
    let no_len_status = matches!(
        out.status(),
        hyper::StatusCode::NO_CONTENT | hyper::StatusCode::NOT_MODIFIED
    );
    // Set content-length only for the collected branch. For streams we
    // don't know the length; hyper uses chunked transfer encoding.
    if !has_len && !no_len_status && !is_streaming {
        if let Ok(val) = hyper::header::HeaderValue::from_str(&body_len.to_string()) {
            out.headers_mut().insert(hyper::header::CONTENT_LENGTH, val);
        }
    }
    out
}

pub async fn from_hyper_request(
    req: HyperRequest<Incoming>,
    request_id: String,
    limits: &Limits,
) -> Result<OxideRequest> {
    let method = req.method().as_str().to_string();
    let url = req.uri().to_string();
    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .filter(|(k, _)| !k.as_str().eq_ignore_ascii_case("x-oxide-rewrite-hop"))
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    // Bounded read (§12.1 step 2): any overflow/read failure maps to SRV-413.
    // S-11: a body read also gets a hard deadline — `header_read_timeout` only
    // covers headers, so a client that drips a large body at 1 B/s would
    // otherwise hold the task + buffer indefinitely (a slowloris variant).
    let body_deadline =
        std::time::Duration::from_millis(limits.slow_request_timeout_ms.max(1000) * 4);
    let collected = tokio::time::timeout(
        body_deadline,
        Limited::new(req.into_body(), limits.max_body).collect(),
    )
    .await
    .map_err(|_| err("SRV-408", "request body read timed out".to_string()))?;
    let body = collected
        .map_err(|_| {
            err(
                "SRV-413",
                format!(
                    "request body exceeds the limit of {} bytes",
                    limits.max_body
                ),
            )
        })?
        .to_bytes();
    // §20: parse the incoming W3C trace context so the fetch op can
    // propagate it. A malformed header is a no-trace request, not an error.
    let trace = {
        let mut map: BTreeMap<String, String> = BTreeMap::new();
        for (k, v) in &headers {
            if k.eq_ignore_ascii_case("traceparent") || k.eq_ignore_ascii_case("tracestate") {
                map.insert(k.clone(), v.clone());
            }
        }
        crate::trace_context::TraceContext::from_headers(&map)
    };
    Ok(OxideRequest {
        method,
        url,
        headers: Arc::new(headers),
        header_index: OnceLock::new(),
        body,
        request_id,
        trace,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    })
}

/// Server runner: binds (SO_REUSEPORT when supported), optional TLS (rustls),
/// graceful shutdown with connection draining (§12.2), health endpoints.
pub struct Server {
    pub addr: std::net::SocketAddr,
}

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Unpredictable per-request id (SEC-02): 16 CSPRNG bytes, hex.
/// The old counter-derived id reset every process → the same CSP nonce for
/// request #1 after every restart (strict-dynamic bypass risk). The nonce
/// (`nonce_for`), L0 sentinel substitution and boot-tag stamping all key off
/// this id, so one change point fixes all three consumers. Falls back to the
/// counter only if the OS RNG fails (never on hosted platforms).
fn next_request_id() -> String {
    use ring::rand::SecureRandom as _;
    // P-13: one SystemRandom for the process, not one per request.
    static RNG: std::sync::OnceLock<ring::rand::SystemRandom> = std::sync::OnceLock::new();
    let rng = RNG.get_or_init(ring::rand::SystemRandom::new);
    let mut bytes = [0u8; 16];
    if rng.fill(&mut bytes).is_ok() {
        let mut s = String::with_capacity(36);
        s.push_str("req-");
        for b in bytes {
            s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
            s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
        }
        return s;
    }
    let n = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "req-c{:016x}",
        n.wrapping_mul(0x9E3779B97F4A7C15) ^ 0xA5A5_0000_0000_0000
    )
}

/// Bind a listener with `SO_REUSEPORT` (§12.2 zero-downtime reload). Falls
/// back to a plain tokio bind when the socket option is unavailable (older
/// kernels, restricted sandboxes).
fn bind_reusable(addr: std::net::SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};

    let domain = if addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    // SO_REUSEPORT exists on Linux, macOS, and the BSDs. On Windows it does
    // not exist — the call below is a no-op on those builds because the
    // `reuse_port` feature is enabled by the socket2 crate for supported
    // platforms only.
    #[cfg(all(unix, not(target_os = "solaris")))]
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    tokio::net::TcpListener::from_std(socket.into())
}

impl Server {
    /// Bind `addr`, accept connections and serve them through the pipeline with the
    /// given dispatch (§12.1). Graceful shutdown: on the watch signal the listener
    /// stops accepting and in-flight connections get up to 5 s to drain (§12.2).
    pub async fn serve(
        addr: std::net::SocketAddr,
        pipeline: Arc<Pipeline>,
        dispatch: Arc<
            dyn Fn(OxideRequest) -> futures::future::BoxFuture<'static, OxideResponse>
                + Send
                + Sync,
        >,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<()> {
        Self::serve_with_tls(addr, pipeline, dispatch, shutdown, None).await
    }

    /// Same as [`Server::serve`] but with an optional rustls server config
    /// (§13.5 dev HTTPS). When `tls` is `Some`, every accepted connection is
    /// handshaked before being handed to hyper. A failed handshake logs and
    /// the connection is dropped — it never reaches the pipeline.
    pub async fn serve_with_tls(
        addr: std::net::SocketAddr,
        pipeline: Arc<Pipeline>,
        dispatch: Arc<
            dyn Fn(OxideRequest) -> futures::future::BoxFuture<'static, OxideResponse>
                + Send
                + Sync,
        >,
        shutdown: tokio::sync::watch::Receiver<bool>,
        tls: Option<Arc<tokio_rustls::rustls::ServerConfig>>,
    ) -> Result<()> {
        // §12.2: SO_REUSEPORT lets a new process bind the same port while the
        // old one is still draining, so restarts are zero-downtime. On
        // platforms where the option is unsupported, fall back to a plain bind.
        let listener =
            bind_reusable(addr).map_err(|e| err("SRV-BIND", format!("cannot bind {addr}: {e}")))?;
        let tls_acceptor = tls.map(tokio_rustls::TlsAcceptor::from);
        let mut shutdown = shutdown;
        // SEC-06: enforce the header-read timeout so a slowloris (a client
        // that drips headers) is dropped instead of holding a connection.
        // `header_read_timeout` needs a `Timer` — configure both once.
        let mut builder =
            hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());
        let header_timeout =
            std::time::Duration::from_millis(pipeline.limits.slow_request_timeout_ms);
        // S-11: `header_read_timeout` also reaps an idle HTTP/1 keep-alive
        // connection (the next request's headers must arrive within it). For
        // HTTP/2 we additionally enable keep-alive pings so a dead peer is
        // detected and the connection closed instead of held open.
        builder
            .http1()
            .timer(hyper_util::rt::TokioTimer::new())
            .header_read_timeout(Some(header_timeout));
        builder
            .http2()
            .timer(hyper_util::rt::TokioTimer::new())
            .keep_alive_interval(Some(std::time::Duration::from_secs(30)))
            .keep_alive_timeout(std::time::Duration::from_secs(20));
        let graceful = hyper_util::server::graceful::GracefulShutdown::new();
        // Bound on a TLS handshake: a peer that opens a socket and never
        // completes the handshake must not pin a task forever.
        const ACCEPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                accepted = listener.accept() => {
                    let (stream, peer) = match accepted {
                        Ok(x) => x,
                        Err(e) => {
                            tracing::warn!(error = %e, "accept failed");
                            continue;
                        }
                    };
                    let peer_ip = Some(peer.ip());
                    let pipeline_for_service = Arc::clone(&pipeline);
                    let dispatch_for_service = Arc::clone(&dispatch);
                    let service = hyper::service::service_fn(move |req: HyperRequest<Incoming>| {
                        let pipeline = Arc::clone(&pipeline_for_service);
                        let dispatch = Arc::clone(&dispatch_for_service);
                        async move {
                            // SPEC-14: the `/_oxide/hmr` WebSocket upgrade.
                            // Handled here (not in `native_prefix`) because the
                            // `hyper::upgrade::OnUpgrade` handle lives on the
                            // hyper request. Dev only (`pipeline.hmr` is None in
                            // production).
                            if let Some(hub) = pipeline.hmr.as_ref() {
                                if req.uri().path() == "/_oxide/hmr" {
                                    // S-04: dev WS — enforce the dev Host gate
                                    // (DNS rebinding) and a same-origin check.
                                    let origin = req
                                        .headers()
                                        .get(hyper::header::ORIGIN)
                                        .and_then(|v| v.to_str().ok())
                                        .map(|s| s.to_string());
                                    let host_hdr = req
                                        .headers()
                                        .get(hyper::header::HOST)
                                        .and_then(|v| v.to_str().ok())
                                        .map(|s| s.to_string());
                                    if let Some(bind_ip) = pipeline.dev_bind_ip {
                                        let host_ok = host_hdr
                                            .as_deref()
                                            .is_some_and(|h| crate::dev_host_allowed(h, bind_ip));
                                        // A browser always sends Origin; a
                                        // non-browser dev client may omit it.
                                        let origin_ok = crate::ws_origin_ok(
                                            origin.as_deref(),
                                            host_hdr.as_deref(),
                                            &pipeline.security.host_allow_list,
                                            true,
                                        );
                                        if !host_ok || !origin_ok {
                                            return Ok::<_, std::io::Error>(
                                                to_hyper_response(OxideResponse::new(
                                                    403,
                                                    "hmr origin rejected",
                                                )),
                                            );
                                        }
                                    }
                                    return Ok::<_, std::io::Error>(
                                        crate::ws_hmr_upgrade(
                                            req,
                                            Arc::clone(hub),
                                            pipeline.hmr_build_id.clone(),
                                        ),
                                    );
                                }
                            }
                            // §9.6: a WebSocket upgrade for a matched
                            // `socket.ts` route. Checked before the normal
                            // pipeline so the `hyper::upgrade::OnUpgrade`
                            // handle is available (the pipeline consumes it).
                            if let Some(svc) = pipeline.sockets.as_ref() {
                                let is_ws = req
                                    .headers()
                                    .get(hyper::header::UPGRADE)
                                    .and_then(|v| v.to_str().ok())
                                    .map(|v| v.eq_ignore_ascii_case("websocket"))
                                    .unwrap_or(false);
                                if is_ws {
                                    let path = req.uri().path().to_string();
                                    if let Some(module) =
                                        crate::socket_route_for(&svc.routes, &path)
                                    {
                                        // S-03: reject a cross-site upgrade.
                                        // A browser always sends `Origin`; a
                                        // non-browser client with no Origin is
                                        // allowed only in dev.
                                        let origin = req
                                            .headers()
                                            .get(hyper::header::ORIGIN)
                                            .and_then(|v| v.to_str().ok())
                                            .map(|s| s.to_string());
                                        let host_hdr = req
                                            .headers()
                                            .get(hyper::header::HOST)
                                            .and_then(|v| v.to_str().ok())
                                            .map(|s| s.to_string());
                                        let origin_ok = crate::ws_origin_ok(
                                            origin.as_deref(),
                                            host_hdr.as_deref(),
                                            &pipeline.security.host_allow_list,
                                            pipeline.dev_bind_ip.is_some(),
                                        );
                                        if !origin_ok {
                                            return Ok::<_, std::io::Error>(
                                                to_hyper_response(OxideResponse::new(
                                                    403,
                                                    "socket origin rejected",
                                                )),
                                            );
                                        }
                                        // S-03: hand the app an auth channel —
                                        // cookies/authorization reach `open`.
                                        let hdrs: Vec<(String, String)> = req
                                            .headers()
                                            .iter()
                                            .filter_map(|(k, v)| {
                                                v.to_str().ok().map(|vs| {
                                                    (k.as_str().to_string(), vs.to_string())
                                                })
                                            })
                                            .collect();
                                        return Ok::<_, std::io::Error>(
                                            crate::ws_socket_upgrade(
                                                req,
                                                Arc::clone(svc),
                                                module.clone(),
                                                hdrs,
                                            ),
                                        );
                                    }
                                }
                            }
                            let request_id = next_request_id();
                            match from_hyper_request(req, request_id, &pipeline.limits).await {
                                Ok(mut oreq) => {
                                    oreq.peer = peer_ip;
                                    let resp = pipeline.serve_request(oreq, dispatch.as_ref()).await;
                                    Ok(to_hyper_response(resp))
                                }
                                Err(e) => Ok(to_hyper_response(OxideResponse::new(413, e.0.message))),
                            }
                        }
                    });

                    match &tls_acceptor {
                        Some(acceptor) => {
                            // SEC-06: the handshake runs in a spawned task with
                            // a timeout, so a stuck handshake does NOT block the
                            // accept loop (the pre-fix code awaited it inline).
                            let acceptor = acceptor.clone();
                            let builder = builder.clone();
                            // `Watcher` takes `self` in `watch` (a deliberate
                            // footgun-guard); `graceful.watcher()` yields a fresh
                            // owned, `Send` handle each call, so we mint one per
                            // connection and move it into the task.
                            let watcher = graceful.watcher();
                            tokio::spawn(async move {
                                let tls_stream = match tokio::time::timeout(
                                    ACCEPT_TIMEOUT,
                                    acceptor.accept(stream),
                                )
                                .await
                                {
                                    Ok(Ok(s)) => s,
                                    Ok(Err(e)) => {
                                        tracing::debug!(error = %e, "TLS handshake failed");
                                        return;
                                    }
                                    Err(_) => {
                                        tracing::debug!("TLS handshake timed out");
                                        return;
                                    }
                                };
                                let conn = builder.serve_connection_with_upgrades(
                                    hyper_util::rt::TokioIo::new(tls_stream),
                                    service,
                                );
                                let guard = watcher.watch(conn.into_owned());
                                let _ = guard.await;
                            });
                        }
                        None => {
                            let conn = builder.serve_connection_with_upgrades(
                                hyper_util::rt::TokioIo::new(stream),
                                service,
                            );
                            let guard = graceful.watch(conn.into_owned());
                            tokio::spawn(async move {
                                let _ = guard.await;
                            });
                        }
                    }
                }
            }
        }

        drop(listener);
        // Give in-flight connections up to 5 s to drain, then close (§12.2).
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), graceful.shutdown()).await;
        Ok(())
    }

    /// Load a PEM certificate chain + private key into a rustls `ServerConfig`.
    /// Only used by the dev server (§13.5); production TLS terminates in front
    /// of the binary or via ACME (post-MLP).
    pub fn tls_config_from_pem(
        cert_pem: &[u8],
        key_pem: &[u8],
    ) -> Result<Arc<tokio_rustls::rustls::ServerConfig>> {
        use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

        let mut cert_reader = std::io::BufReader::new(cert_pem);
        let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| err("SRV-TLS", format!("cert parse failed: {e}")))?;
        if certs.is_empty() {
            return Err(err(
                "SRV-TLS",
                "cert PEM contains no CERTIFICATE blocks".to_string(),
            ));
        }

        let mut key_reader = std::io::BufReader::new(key_pem);
        let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut key_reader)
            .map_err(|e| err("SRV-TLS", format!("key parse failed: {e}")))?
            .ok_or_else(|| {
                err(
                    "SRV-TLS",
                    "key PEM contains no PRIVATE KEY block".to_string(),
                )
            })?;

        let config = tokio_rustls::rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| err("SRV-TLS", format!("rustls config failed: {e}")))?;
        Ok(Arc::new(config))
    }
}

/// SPEC-14 / §13.3: accept a `/_oxide/hmr` WebSocket upgrade and pump HMR
/// messages to the client. The 101 handshake is built by hand (the ws codec
/// in this module owns framing); each broadcast message becomes one text
/// frame. Dev only — the caller checks `pipeline.hmr` is `Some`.
fn ws_hmr_upgrade(
    mut req: HyperRequest<hyper::body::Incoming>,
    hub: Arc<tokio::sync::broadcast::Sender<String>>,
    build_id: String,
) -> HyperResponse<BoxBody<Bytes, std::io::Error>> {
    use http_body_util::Full;
    // The client's `Sec-WebSocket-Key` is echoed back as
    // base64(sha1(key + GUID)) per RFC6455 §4.2.2.
    let key = req
        .headers()
        .get("sec-websocket-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let accept = {
        use base64::Engine as _;
        let mut h = <sha1_shim::Sha1>::new();
        h.update(key.as_bytes());
        h.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
        base64::engine::general_purpose::STANDARD.encode(h.finalize())
    };
    let on_upgrade = hyper::upgrade::on(&mut req);
    tokio::spawn(async move {
        let Ok(upgraded) = on_upgrade.await else {
            return;
        };
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let io = hyper_util::rt::TokioIo::new(upgraded);
        let (mut rd, mut wr) = tokio::io::split(io);
        let mut rx = hub.subscribe();
        // The first message tells the client it is connected.
        // §13.3: advertise the current build id so a stale tab can reload.
        let hello = format!("{{\"type\":\"connected\",\"build_id\":\"{}\"}}", build_id,);
        let _ = wr
            .write_all(&crate::ws::encode(&crate::ws::Frame::Text(hello), false))
            .await;
        let mut buf = [0u8; 4096];
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Ok(json) => {
                            if wr.write_all(&crate::ws::encode(&crate::ws::Frame::Text(json), false)).await.is_err() {
                                return;
                            }
                        }
                        Err(_) => return,
                    }
                }
                n = rd.read(&mut buf) => {
                    // Client frames (ping/close) — answer a close.
                    match n {
                        Ok(0) | Err(_) => return,
                        Ok(len) => {
                            if let Ok((frames, _)) = crate::ws::decode(&buf[..len]) {
                                for f in frames {
                                    if matches!(f, crate::ws::Frame::Close(_)) {
                                        let _ = wr.write_all(&crate::ws::encode(&crate::ws::Frame::Close(Some(1000)), false)).await;
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    });
    let mut resp = HyperResponse::new(BodyExt::boxed(
        Full::new(Bytes::new()).map_err(|never| match never {}),
    ));
    *resp.status_mut() = hyper::StatusCode::SWITCHING_PROTOCOLS;
    let h = resp.headers_mut();
    h.insert(
        hyper::header::CONNECTION,
        hyper::header::HeaderValue::from_static("Upgrade"),
    );
    h.insert(
        hyper::header::UPGRADE,
        hyper::header::HeaderValue::from_static("websocket"),
    );
    if let Ok(v) = hyper::header::HeaderValue::from_str(&accept) {
        h.insert(
            hyper::header::HeaderName::from_static("sec-websocket-accept"),
            v,
        );
    }
    resp
}

/// §9.6: accept a WebSocket upgrade for a matched `socket.ts` route and pump
/// the three user phases (`open`, `message`, `close`) through the
/// `SocketService` dispatcher. The handshake is the same RFC6455 one the HMR
/// endpoint uses; the difference is that here every inbound data frame runs
/// the user's `message` handler and every outbound `send` becomes a server
/// text frame (unmasked, per RFC6455 §5.1).
fn ws_socket_upgrade(
    mut req: HyperRequest<hyper::body::Incoming>,
    service: Arc<SocketService>,
    module_path: std::path::PathBuf,
    headers: Vec<(String, String)>,
) -> HyperResponse<BoxBody<Bytes, std::io::Error>> {
    use http_body_util::Full;
    let key = req
        .headers()
        .get("sec-websocket-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let accept = {
        use base64::Engine as _;
        let mut h = <sha1_shim::Sha1>::new();
        h.update(key.as_bytes());
        h.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
        base64::engine::general_purpose::STANDARD.encode(h.finalize())
    };
    let on_upgrade = hyper::upgrade::on(&mut req);
    // One id per connection, so the worker can key per-socket state (a user
    // id stashed in `open` and read in `message`).
    static NEXT_SOCKET_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let socket_id = format!(
        "sock-{}",
        NEXT_SOCKET_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    tokio::spawn(async move {
        let Ok(upgraded) = on_upgrade.await else {
            return;
        };
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let io = hyper_util::rt::TokioIo::new(upgraded);
        let (mut rd, mut wr) = tokio::io::split(io);
        let mp = module_path.to_string_lossy().to_string();
        let dispatcher = Arc::clone(&service.dispatcher);
        // §9.6 / H1: register this connection so another socket's
        // `ctx.broadcast` / `ctx.sendTo` can reach it, and so `ctx.clients`
        // is a live presence list. Dropping the guard on every exit removes
        // it.
        let (push_tx, mut push_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let _registration = service.register(&socket_id, &mp, push_tx);

        // `open` phase. S-03: include the handshake headers (cookies,
        // authorization) so the handler can authenticate — the socket.ts
        // `open(ctx)` contract documents `{ headers }`.
        let open_payload = serde_json::json!({
            "headers": headers,
            "clients": service.client_ids(&mp),
        });
        match dispatcher.run(&mp, "open", open_payload, &socket_id).await {
            Ok(out) => {
                for text in &out.sends {
                    if wr
                        .write_all(&crate::ws::encode(
                            &crate::ws::Frame::Text(text.clone()),
                            false,
                        ))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                for text in &out.broadcasts {
                    service.broadcast(&mp, &socket_id, text);
                }
                for (id, text) in &out.targets {
                    service.send_to(id, text);
                }
                if let Some((code, _)) = out.close {
                    let _ = wr
                        .write_all(&crate::ws::encode(
                            &crate::ws::Frame::Close(Some(code)),
                            false,
                        ))
                        .await;
                    return;
                }
            }
            Err(_) => {
                let _ = wr
                    .write_all(&crate::ws::encode(
                        &crate::ws::Frame::Close(Some(1011)),
                        false,
                    ))
                    .await;
                return;
            }
        }

        let mut buf: Vec<u8> = Vec::with_capacity(8192);
        let mut chunk = [0u8; 8192];
        // S-12: cap the pending-frame buffer. A peer declaring a huge frame and
        // dribbling bytes would otherwise grow `buf` toward the 64 MiB max;
        // many such connections exhaust memory. 1 MiB pending is ~32× a normal
        // frame, so a well-behaved client never trips it.
        const MAX_PENDING_WS: usize = 1024 * 1024;
        loop {
            tokio::select! {
                // A frame another socket broadcast (or sent directly to this
                // id) is queued by the registry; write it just like a local
                // `send`.
                pushed = push_rx.recv() => {
                    match pushed {
                        Some(text) => {
                            if wr
                                .write_all(&crate::ws::encode(
                                    &crate::ws::Frame::Text(text),
                                    false,
                                ))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        None => break,
                    }
                }
                read = rd.read(&mut chunk) => {
                    let n = match read {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
            if buf.len() + n > MAX_PENDING_WS {
                tracing::warn!(%socket_id, "ws pending buffer exceeded; closing");
                let _ = wr
                    .write_all(&crate::ws::encode(
                        &crate::ws::Frame::Close(Some(1009)),
                        false,
                    ))
                    .await;
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            let (frames, consumed) = match crate::ws::decode(&buf) {
                Ok(v) => v,
                Err(_e) => {
                    let _ = wr
                        .write_all(&crate::ws::encode(
                            &crate::ws::Frame::Close(Some(1002)),
                            false,
                        ))
                        .await;
                    return;
                }
            };
            buf.drain(..consumed);
            let mut close_code: Option<u16> = None;
            for frame in frames {
                match frame {
                    crate::ws::Frame::Text(text) => {
                        let payload = serde_json::json!({
                            "data": text,
                            "headers": headers,
                            "clients": service.client_ids(&mp),
                        });
                        match dispatcher.run(&mp, "message", payload, &socket_id).await {
                            Ok(out) => {
                                for send in &out.sends {
                                    if wr
                                        .write_all(&crate::ws::encode(
                                            &crate::ws::Frame::Text(send.clone()),
                                            false,
                                        ))
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                                }
                                for send in &out.broadcasts {
                                    service.broadcast(&mp, &socket_id, send);
                                }
                                for (id, send) in &out.targets {
                                    service.send_to(id, send);
                                }
                                if let Some((code, _)) = out.close {
                                    close_code = Some(code);
                                }
                            }
                            Err(_e) => {
                                close_code = Some(1011);
                            }
                        }
                    }
                    crate::ws::Frame::Binary(bytes) => {
                        // A binary message is base64-encoded so the JSON bridge
                        // carries it without a second wire format.
                        use base64::Engine as _;
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        let payload = serde_json::json!({
                            "data": b64,
                            "binary": true,
                            "headers": headers,
                            "clients": service.client_ids(&mp),
                        });
                        if let Ok(out) = dispatcher.run(&mp, "message", payload, &socket_id).await {
                            for send in &out.sends {
                                if wr
                                    .write_all(&crate::ws::encode(
                                        &crate::ws::Frame::Text(send.clone()),
                                        false,
                                    ))
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            for send in &out.broadcasts {
                                service.broadcast(&mp, &socket_id, send);
                            }
                            for (id, send) in &out.targets {
                                service.send_to(id, send);
                            }
                            if let Some((code, _)) = out.close {
                                close_code = Some(code);
                            }
                        }
                    }
                    crate::ws::Frame::Ping(p) => {
                        let _ = wr
                            .write_all(&crate::ws::encode(&crate::ws::Frame::Pong(p), false))
                            .await;
                    }
                    crate::ws::Frame::Close(code) => {
                        close_code = Some(code.unwrap_or(1000));
                    }
                    crate::ws::Frame::Pong(_) => {}
                }
                if close_code.is_some() {
                    break;
                }
            }
            if let Some(code) = close_code {
                // `close` phase before the socket goes away.
                let payload = serde_json::json!({ "code": code, "reason": "" });
                let _ = dispatcher.run(&mp, "close", payload, &socket_id).await;
                let _ = wr
                    .write_all(&crate::ws::encode(
                        &crate::ws::Frame::Close(Some(code)),
                        false,
                    ))
                    .await;
                return;
            }
                }
            }
        }
        // The peer dropped without a close frame — still run the `close` phase
        // so the handler can clean up its own state.
        let payload = serde_json::json!({ "code": 1006, "reason": "peer disconnected" });
        let _ = dispatcher.run(&mp, "close", payload, &socket_id).await;
    });
    let mut resp = HyperResponse::new(BodyExt::boxed(
        Full::new(Bytes::new()).map_err(|never| match never {}),
    ));
    *resp.status_mut() = hyper::StatusCode::SWITCHING_PROTOCOLS;
    let h = resp.headers_mut();
    h.insert(
        hyper::header::CONNECTION,
        hyper::header::HeaderValue::from_static("Upgrade"),
    );
    h.insert(
        hyper::header::UPGRADE,
        hyper::header::HeaderValue::from_static("websocket"),
    );
    if let Ok(v) = hyper::header::HeaderValue::from_str(&accept) {
        h.insert(
            hyper::header::HeaderName::from_static("sec-websocket-accept"),
            v,
        );
    }
    resp
}

/// §12.1: build a QUIC server config (quinn) from the same PEM certificate
/// and key the TLS listener uses. HTTP/3 rides QUIC with TLS 1.3, so the
/// cert/key are shared with the TCP/TLS listener.
pub fn h3_config_from_pem(cert_pem: &[u8], key_pem: &[u8]) -> Result<quinn::ServerConfig> {
    use quinn::crypto::rustls::QuicServerConfig;
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

    let mut cert_reader = std::io::BufReader::new(cert_pem);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| err("SRV-TLS", format!("h3 cert parse failed: {e}")))?;
    if certs.is_empty() {
        return Err(err(
            "SRV-TLS",
            "h3 cert PEM contains no CERTIFICATE blocks".to_string(),
        ));
    }
    let mut key_reader = std::io::BufReader::new(key_pem);
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|e| err("SRV-TLS", format!("h3 key parse failed: {e}")))?
        .ok_or_else(|| {
            err(
                "SRV-TLS",
                "h3 key PEM contains no PRIVATE KEY block".to_string(),
            )
        })?;

    // ALPN must advertise h3 for a QUIC client to speak HTTP/3.
    let mut tls = tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| err("SRV-TLS", format!("h3 rustls config failed: {e}")))?;
    tls.alpn_protocols = vec![b"h3".to_vec()];
    let quic = QuicServerConfig::try_from(tls)
        .map_err(|e| err("SRV-TLS", format!("h3 QUIC config failed: {e}")))?;
    let mut server = quinn::ServerConfig::with_crypto(std::sync::Arc::new(quic));
    // Idle timeout + keep-alive: a QUIC connection that stalls is dropped,
    // matching the TCP slowloris protection.
    let mut transport = quinn::TransportConfig::default();
    transport.max_idle_timeout(Some(std::time::Duration::from_secs(30).try_into().unwrap()));
    server.transport_config(std::sync::Arc::new(transport));
    Ok(server)
}

/// §12.1: build a QUIC server config from an existing rustls `ServerConfig`
/// (the same one the TCP/TLS listener uses). Adds the `h3` ALPN, which the
/// TCP config does not carry.
pub fn h3_config_from_tls(tls: &tokio_rustls::rustls::ServerConfig) -> Result<quinn::ServerConfig> {
    use quinn::crypto::rustls::QuicServerConfig;
    let mut tls = tls.clone();
    tls.alpn_protocols = vec![b"h3".to_vec()];
    let quic = QuicServerConfig::try_from(tls)
        .map_err(|e| err("SRV-TLS", format!("h3 QUIC config failed: {e}")))?;
    let mut server = quinn::ServerConfig::with_crypto(std::sync::Arc::new(quic));
    let mut transport = quinn::TransportConfig::default();
    transport.max_idle_timeout(Some(std::time::Duration::from_secs(30).try_into().unwrap()));
    server.transport_config(std::sync::Arc::new(transport));
    Ok(server)
}

/// §12.1: the HTTP/3 listener. Binds a UDP socket on the same port as the
/// TCP listener, accepts QUIC connections, and drives each through `h3`.
/// Requests are converted to the same `OxideRequest` shape the HTTP/1+2 path
/// uses, so the pipeline is identical. `http3: true` in config + TLS enabled
/// is what turns this on; without TLS there is no QUIC.
pub async fn serve_http3(
    addr: std::net::SocketAddr,
    pipeline: Arc<Pipeline>,
    dispatch: Arc<
        dyn Fn(OxideRequest) -> futures::future::BoxFuture<'static, OxideResponse> + Send + Sync,
    >,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    server_config: quinn::ServerConfig,
) -> Result<()> {
    let endpoint = quinn::Endpoint::server(server_config, addr)
        .map_err(|e| err("SRV-BIND", format!("cannot bind UDP {addr}: {e}")))?;
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            incoming = endpoint.accept() => {
                let Some(connecting) = incoming else { break };
                let pipeline = Arc::clone(&pipeline);
                let dispatch = Arc::clone(&dispatch);
                tokio::spawn(async move {
                    let Ok(conn) = connecting.await else { return };
                    let peer = Some(conn.remote_address().ip());
                    let h3_conn = h3_quinn::Connection::new(conn);
                    let mut h3 = match h3::server::Connection::new(h3_conn).await {
                        Ok(c) => c,
                        Err(_) => return,
                    };
                    loop {
                        match h3.accept().await {
                            Ok(Some(resolver)) => {
                                let (req, mut stream) = match resolver.resolve_request().await {
                                    Ok(v) => v,
                                    Err(_) => continue,
                                };
                                let method = req.method().to_string();
                                let url = req.uri().to_string();
                                let headers: Vec<(String, String)> = req
                                    .headers()
                                    .iter()
                                    .filter_map(|(k, v)| {
                                        v.to_str().ok().map(|vs| (k.to_string(), vs.to_string()))
                                    })
                                    .collect();
                                // B-04: drain the request body (DATA frames).
                                // Without this, POST/action bodies were empty
                                // over h3. The cap mirrors the h1/h2 `Limited`
                                // guard; exceeding it answers 413 and moves on.
                                const H3_MAX_BODY: usize = 8 * 1024 * 1024;
                                let mut body: Vec<u8> = Vec::new();
                                let mut too_large = false;
                                loop {
                                    match stream.recv_data().await {
                                        Ok(Some(mut chunk)) => {
                                            use bytes::Buf as _;
                                            let n = chunk.remaining();
                                            if body.len() + n > H3_MAX_BODY {
                                                too_large = true;
                                                break;
                                            }
                                            body.extend_from_slice(&chunk.copy_to_bytes(n));
                                        }
                                        Ok(None) => break,
                                        Err(_) => break,
                                    }
                                }
                                if too_large {
                                    if let Ok(r) = hyper::http::Response::builder()
                                        .status(413)
                                        .body(())
                                    {
                                        let _ = stream.send_response(r).await;
                                    }
                                    let _ = stream.finish().await;
                                    continue;
                                }
                                let oreq = OxideRequest {
                                    method,
                                    url,
                                    headers: Arc::new(headers),
                                    header_index: OnceLock::new(),
                                    body: bytes::Bytes::from(body),
                                    request_id: next_request_id(),
                                    trace: None,
                                    peer,
                                    rewrite_hops: 0,
                                    mw_resume_after: None,
                                };
                                let resp = pipeline.serve_request(oreq, dispatch.as_ref()).await;
                                let mut builder = hyper::http::Response::builder().status(resp.status);
                                for (k, v) in &resp.headers {
                                    builder = builder.header(k.as_str(), v.as_str());
                                }
                                let response = match builder.body(()) {
                                    Ok(r) => r,
                                    Err(_) => {
                                        let _ = stream.finish().await;
                                        continue;
                                    }
                                };
                                if stream.send_response(response).await.is_err() {
                                    continue;
                                }
                                // B-04: streaming responses (the live SSR
                                // path) carry their bytes in `stream`, not
                                // `body`; h3 used to drop them entirely.
                                if let Some(mut st) = resp.stream {
                                    use futures::StreamExt as _;
                                    while let Some(item) = st.next().await {
                                        let Ok(chunk) = item else { break };
                                        if stream.send_data(chunk).await.is_err() {
                                            break;
                                        }
                                    }
                                } else if !resp.body.is_empty() {
                                    let _ = stream.send_data(resp.body).await;
                                }
                                let _ = stream.finish().await;
                            }
                            Ok(None) => break,
                            Err(_) => break,
                        }
                    }
                    let _ = h3.shutdown(0).await;
                });
            }
        }
    }
    endpoint.wait_idle().await;
    Ok(())
}

/// Minimal SHA-1 for the WebSocket handshake (RFC6455 requires it; SHA-1 is
/// not a security primitive here — it is a fixed protocol constant).
mod sha1_shim {
    pub struct Sha1 {
        h: [u32; 5],
        buf: Vec<u8>,
        len: u64,
    }
    impl Sha1 {
        pub fn new() -> Self {
            Self {
                h: [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0],
                buf: Vec::new(),
                len: 0,
            }
        }
        pub fn update(&mut self, data: &[u8]) {
            self.len += data.len() as u64;
            self.buf.extend_from_slice(data);
        }
        pub fn finalize(mut self) -> Vec<u8> {
            let bit_len = self.len * 8;
            self.buf.push(0x80);
            while self.buf.len() % 64 != 56 {
                self.buf.push(0);
            }
            self.buf.extend_from_slice(&bit_len.to_be_bytes());
            let blocks: Vec<[u8; 64]> = self
                .buf
                .chunks_exact(64)
                .map(|c| {
                    let mut b = [0u8; 64];
                    b.copy_from_slice(c);
                    b
                })
                .collect();
            for block in blocks {
                let mut w = [0u32; 80];
                for i in 0..16 {
                    w[i] = u32::from_be_bytes([
                        block[i * 4],
                        block[i * 4 + 1],
                        block[i * 4 + 2],
                        block[i * 4 + 3],
                    ]);
                }
                for i in 16..80 {
                    w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
                }
                let (mut a, mut b, mut c, mut d, mut e) =
                    (self.h[0], self.h[1], self.h[2], self.h[3], self.h[4]);
                for i in 0..80 {
                    let (f, k) = match i {
                        0..=19 => ((b & c) | ((!b) & d), 0x5A827999),
                        20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                        40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                        _ => (b ^ c ^ d, 0xCA62C1D6),
                    };
                    let temp = a
                        .rotate_left(5)
                        .wrapping_add(f)
                        .wrapping_add(e)
                        .wrapping_add(k)
                        .wrapping_add(w[i]);
                    e = d;
                    d = c;
                    c = b.rotate_left(30);
                    b = a;
                    a = temp;
                }
                self.h[0] = self.h[0].wrapping_add(a);
                self.h[1] = self.h[1].wrapping_add(b);
                self.h[2] = self.h[2].wrapping_add(c);
                self.h[3] = self.h[3].wrapping_add(d);
                self.h[4] = self.h[4].wrapping_add(e);
            }
            self.h.iter().flat_map(|x| x.to_be_bytes()).collect()
        }
    }
}

/// Minimal RFC6455 frame codec (§9.6 WebSockets: Rust owns upgrade, framing,
/// ping/pong and backpressure; JS handlers receive messages via ops).
///
/// `decode` implements the **server-side** decoder: client frames MUST be masked
/// (RFC6455 §5.1); server frames (`encode(frame, mask=false)`) are unmasked and are not
/// decodable with this function. Fragmentation (continuation opcodes) is rejected for
/// now — single-frame messages only (documented limitation). The client mask key comes
/// from a deterministic counter PRNG ([`ws::next_mask_key`]) so tests can predict it.
pub mod ws {
    use oxide_fw_diagnostics::{Diagnostic, OxideError};
    use std::sync::atomic::{AtomicU64, Ordering};

    pub type Result<T> = std::result::Result<T, OxideError>;

    fn ws_err(msg: impl Into<String>) -> OxideError {
        OxideError(Diagnostic::error("SRV-WS", msg))
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Frame {
        Text(String),
        Binary(Vec<u8>),
        Ping(Vec<u8>),
        Pong(Vec<u8>),
        Close(Option<u16>),
    }

    static MASK_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Deterministic client mask key (counter PRNG — predictable for tests).
    pub fn next_mask_key() -> [u8; 4] {
        let n = MASK_COUNTER.fetch_add(1, Ordering::Relaxed);
        let s = n.wrapping_mul(0x9E3779B97F4A7C15) ^ 0xB5_B5_B5_B5_9E37_79B9;
        [(s >> 24) as u8, (s >> 16) as u8, (s >> 8) as u8, s as u8]
    }

    fn opcode_and_payload(frame: &Frame) -> (u8, Vec<u8>) {
        match frame {
            Frame::Text(s) => (0x1, s.as_bytes().to_vec()),
            Frame::Binary(b) => (0x2, b.clone()),
            Frame::Close(c) => (
                0x8,
                match c {
                    Some(code) => code.to_be_bytes().to_vec(),
                    None => Vec::new(),
                },
            ),
            Frame::Ping(p) => (0x9, p.clone()),
            Frame::Pong(p) => (0xA, p.clone()),
        }
    }

    /// Encode a client→server frame (masked, RFC6455 §5.3) when `mask` is true;
    /// server frames use `mask = false` (RFC6455 §5.1: servers must not mask).
    pub fn encode(frame: &Frame, mask: bool) -> Vec<u8> {
        let (opcode, mut payload) = opcode_and_payload(frame);
        let mut out = vec![0x80 | opcode]; // FIN + opcode
        let mask_bit = if mask { 0x80u8 } else { 0 };
        let len = payload.len();
        if len < 126 {
            out.push(mask_bit | len as u8);
        } else if len <= 0xFFFF {
            out.push(mask_bit | 126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            out.push(mask_bit | 127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
        if mask {
            let key = next_mask_key();
            out.extend_from_slice(&key);
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= key[i % 4];
            }
        }
        out.extend_from_slice(&payload);
        out
    }

    /// Decode frames; returns (frames, consumed). Errors on protocol violations.
    ///
    /// Incomplete trailing data is not consumed (an empty `frames` with `consumed = 0`
    /// means "need more bytes"). Violations → `SRV-WS`: reserved opcodes (3-7, 11-15),
    /// RSV bits set, unmasked client frames, fragmented (continuation) frames,
    /// non-FIN or oversized control frames, 1-byte close payloads, invalid UTF-8 text.
    pub fn decode(buf: &[u8]) -> Result<(Vec<Frame>, usize)> {
        let mut frames = Vec::new();
        let mut pos = 0usize;
        while pos < buf.len() {
            match parse_one(&buf[pos..])? {
                Some((frame, used)) => {
                    frames.push(frame);
                    pos += used;
                }
                None => break, // incomplete
            }
        }
        Ok((frames, pos))
    }

    /// Parse one frame from the front of `buf`; `None` = need more bytes.
    fn parse_one(buf: &[u8]) -> Result<Option<(Frame, usize)>> {
        if buf.len() < 2 {
            return Ok(None);
        }
        let b0 = buf[0];
        let b1 = buf[1];
        let fin = b0 & 0x80 != 0;
        let opcode = b0 & 0x0F;
        let masked = b1 & 0x80 != 0;
        let len7 = (b1 & 0x7F) as usize;

        if b0 & 0x70 != 0 {
            return Err(ws_err("reserved RSV bits set"));
        }
        if (3..=7).contains(&opcode) {
            return Err(ws_err(format!("reserved data opcode {opcode}")));
        }
        if opcode >= 0xB {
            return Err(ws_err(format!("reserved control opcode {opcode}")));
        }
        if opcode == 0 {
            return Err(ws_err("continuation frames (fragmentation) not supported"));
        }
        let is_control = opcode >= 0x8;
        if is_control && (!fin || len7 > 125) {
            return Err(ws_err(
                "control frames must be FIN and carry at most 125 bytes",
            ));
        }
        if !masked {
            return Err(ws_err("client frames must be masked (RFC6455 §5.1)"));
        }

        let (len, header_len) = if len7 < 126 {
            (len7, 2usize)
        } else if len7 == 126 {
            if buf.len() < 4 {
                return Ok(None);
            }
            (u16::from_be_bytes([buf[2], buf[3]]) as usize, 4)
        } else {
            if buf.len() < 10 {
                return Ok(None);
            }
            (
                u64::from_be_bytes(buf[2..10].try_into().unwrap()) as usize,
                10,
            )
        };
        const MAX_FRAME: usize = 64 * 1024 * 1024;
        if len > MAX_FRAME {
            return Err(ws_err(format!(
                "frame of {len} bytes exceeds the 64 MiB guard"
            )));
        }
        if buf.len() < header_len + 4 + len {
            return Ok(None);
        }
        let key = &buf[header_len..header_len + 4];
        let payload: Vec<u8> = buf[header_len + 4..header_len + 4 + len]
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ key[i % 4])
            .collect();
        let used = header_len + 4 + len;

        let frame = match opcode {
            0x1 => Frame::Text(
                String::from_utf8(payload)
                    .map_err(|_| ws_err("text frame payload is not valid UTF-8"))?,
            ),
            0x2 => Frame::Binary(payload),
            0x8 => match payload.len() {
                0 => Frame::Close(None),
                1 => return Err(ws_err("close frame with a 1-byte payload")),
                _ => Frame::Close(Some(u16::from_be_bytes([payload[0], payload[1]]))),
            },
            0x9 => Frame::Ping(payload),
            0xA => Frame::Pong(payload),
            _ => unreachable!("reserved opcodes rejected above"),
        };
        Ok(Some((frame, used)))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn encode_uses_two_byte_length_for_126() {
            let bytes = encode(&Frame::Binary(vec![0; 126]), false);
            assert_eq!(bytes[1], 126);
            assert_eq!(u16::from_be_bytes([bytes[2], bytes[3]]), 126);
        }

        #[test]
        fn request_ids_are_unique_csprng() {
            // SEC-02: two fresh ids must differ (counter restarts made the
            // old first-request nonce predictable across restarts).
            let a = crate::next_request_id();
            let b = crate::next_request_id();
            assert_ne!(a, b);
            assert_ne!(crate::nonce_for(&a), crate::nonce_for(&b));
        }
    }
}

#[cfg(test)]
mod streaming_response_tests {
    use super::*;
    use bytes::Bytes;
    use futures::stream;

    #[tokio::test]
    async fn streaming_body_sends_chunks() {
        // Build a body stream of three chunks; verify they survive
        // `to_hyper_response` in order.
        let items: Vec<std::result::Result<Bytes, std::io::Error>> = vec![
            Ok(Bytes::from_static(b"AAA")),
            Ok(Bytes::from_static(b"BBB")),
            Ok(Bytes::from_static(b"CCC")),
        ];
        let stream: BodyStream = Box::pin(stream::iter(items));
        let resp = OxideResponse::stream(200, stream)
            .with_header("content-type", "text/html; charset=utf-8");

        let hyper_resp = to_hyper_response(resp);
        assert_eq!(hyper_resp.status(), hyper::StatusCode::OK);
        // Streaming responses skip content-length; hyper uses chunked
        // transfer encoding.
        assert!(
            hyper_resp
                .headers()
                .get(hyper::header::CONTENT_LENGTH)
                .is_none(),
            "streaming response must not set content-length"
        );

        use http_body_util::BodyExt as _;
        let body = hyper_resp.into_body();
        let collected = body.collect().await.expect("collect body").to_bytes();
        assert_eq!(collected.as_ref(), b"AAABBBCCC");
    }

    #[tokio::test]
    async fn streaming_body_error_aborts() {
        // A body stream that errors mid-way must surface the error to
        // hyper (which closes the connection) rather than truncating
        // silently.
        let items: Vec<std::result::Result<Bytes, std::io::Error>> = vec![
            Ok(Bytes::from_static(b"partial")),
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "worker crashed",
            )),
        ];
        let stream: BodyStream = Box::pin(stream::iter(items));
        let resp = OxideResponse::stream(200, stream);
        let hyper_resp = to_hyper_response(resp);

        use http_body_util::BodyExt as _;
        let body = hyper_resp.into_body();
        let result = body.collect().await;
        assert!(result.is_err(), "streaming error must abort the body");
    }

    #[test]
    fn streaming_response_clone_drops_stream() {
        let items: Vec<std::result::Result<Bytes, std::io::Error>> =
            vec![Ok(Bytes::from_static(b"x"))];
        let stream: BodyStream = Box::pin(stream::iter(items));
        let resp = OxideResponse::stream(200, stream);
        let cloned = resp.clone();
        assert!(cloned.stream.is_none(), "clone drops the stream");
        // Original still owns it.
        assert!(resp.stream.is_some());
    }
}

// ------------------------------ §16.1: image endpoint behavior

#[cfg(test)]
mod image_endpoint_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Counts `process()` calls and returns a deterministic body, so the
    /// test can tell a cache hit (0 extra calls) from a miss (1 per request).
    struct CountingBackend {
        calls: Arc<AtomicUsize>,
        concurrency_seen: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        placeholder: Option<String>,
    }

    impl ImageEndpointBackend for CountingBackend {
        fn process(
            &self,
            req: ImageEndpointRequest,
        ) -> std::result::Result<ImageEndpointResult, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let live = self.concurrency_seen.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(live, Ordering::SeqCst);
            // Keep the transform observable as "in flight" for a moment is
            // not needed here; the peak check is a single-threaded assertion.
            self.concurrency_seen.fetch_sub(1, Ordering::SeqCst);
            Ok(ImageEndpointResult {
                bytes: format!(
                    "W{}H{}Q{}",
                    req.width.unwrap_or(0),
                    req.height.unwrap_or(0),
                    req.quality.unwrap_or(0)
                )
                .into_bytes(),
                content_type: "image/png",
                cache_key: format!(
                    "k{}-{}-{}",
                    req.width.unwrap_or(0),
                    req.height.unwrap_or(0),
                    req.format.unwrap_or("png")
                ),
                placeholder: self.placeholder.clone(),
            })
        }
    }

    fn endpoint_with(
        backend: Arc<dyn ImageEndpointBackend>,
        max_concurrency: usize,
        max_dimension: u32,
    ) -> (tempfile::TempDir, ImageEndpoint) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.png"), b"not-a-real-png-but-bytes")
            .expect("write source");
        let ep = ImageEndpoint {
            asset_root: dir.path().to_path_buf(),
            signing_secret: None,
            backend,
            remote_patterns: Vec::new(),
            ssrf_allow_list: Vec::new(),
            remote_max_bytes: 1024 * 1024,
            result_cache: Arc::new(Mutex::new(std::collections::HashMap::new())),
            max_concurrency,
            in_flight: Arc::new(AtomicUsize::new(0)),
            max_dimension,
        };
        (dir, ep)
    }

    fn body_of(p: NativePrefix) -> (u16, Vec<(String, String)>, Vec<u8>) {
        match p {
            NativePrefix::Raw(r) => (r.status, r.headers, r.body.to_vec()),
            _ => panic!("expected a Raw response from the image endpoint"),
        }
    }

    /// The second identical request must not reach the backend — the
    /// transformed-result cache is content+params keyed, and it replays the
    /// bytes, the ETag, the immutable Cache-Control and any LQIP header.
    #[test]
    fn identical_requests_replay_from_the_result_cache() {
        let calls = Arc::new(AtomicUsize::new(0));
        let backend = Arc::new(CountingBackend {
            calls: Arc::clone(&calls),
            concurrency_seen: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
            placeholder: Some("data:image/png;base64,AAAA".to_string()),
        });
        let (_dir, ep) = endpoint_with(backend, 0, 8192);

        let url = "/_oxide/image?url=/a.png&w=400&h=300&q=75";
        let (s1, h1, b1) = body_of(serve_image_endpoint(Some(&ep), url));
        assert_eq!(s1, 200);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "first request renders");
        assert_eq!(String::from_utf8_lossy(&b1), "W400H300Q75");
        let etag1 = h1
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("etag"))
            .map(|(_, v)| v.clone());
        assert!(etag1.is_some(), "ETag on the first response: {h1:?}");
        assert!(
            h1.iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("cache-control") && v.contains("immutable")),
            "immutable Cache-Control: {h1:?}"
        );
        assert!(
            h1.iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("x-oxide-lqip")),
            "LQIP header piped: {h1:?}"
        );

        // Second identical request: cache hit, backend untouched.
        let (s2, h2, b2) = body_of(serve_image_endpoint(Some(&ep), url));
        assert_eq!(s2, 200);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "cache hit must not call the backend"
        );
        assert_eq!(b1, b2, "replayed bytes are identical");
        assert_eq!(
            h2.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("etag"))
                .map(|(_, v)| v.clone()),
            etag1,
            "replayed ETag is identical"
        );
        assert!(
            h2.iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("x-oxide-lqip")),
            "replayed LQIP header: {h2:?}"
        );
    }

    /// A different `w` is a different cache key — the backend runs again.
    #[test]
    fn a_different_width_misses_the_cache() {
        let calls = Arc::new(AtomicUsize::new(0));
        let backend = Arc::new(CountingBackend {
            calls: Arc::clone(&calls),
            concurrency_seen: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
            placeholder: None,
        });
        let (_dir, ep) = endpoint_with(backend, 0, 8192);

        let _ = body_of(serve_image_endpoint(
            Some(&ep),
            "/_oxide/image?url=/a.png&w=400",
        ));
        let _ = body_of(serve_image_endpoint(
            Some(&ep),
            "/_oxide/image?url=/a.png&w=800",
        ));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "a different width is a different transform"
        );
    }

    /// §16.1: a size over `max_dimension` is refused before the backend ever
    /// allocates a canvas.
    #[test]
    fn oversize_dimension_is_refused_before_rasterizing() {
        let calls = Arc::new(AtomicUsize::new(0));
        let backend = Arc::new(CountingBackend {
            calls: Arc::clone(&calls),
            concurrency_seen: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
            placeholder: None,
        });
        let (_dir, ep) = endpoint_with(backend, 0, 1000);

        let (status, _, body) = body_of(serve_image_endpoint(
            Some(&ep),
            "/_oxide/image?url=/a.png&w=1001",
        ));
        assert_eq!(status, 400, "oversize width refused");
        assert!(
            String::from_utf8_lossy(&body).contains("out of range"),
            "message names the ceiling: {}",
            String::from_utf8_lossy(&body)
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "the backend must never run for an oversize request"
        );
    }

    /// §16.1: over the concurrency ceiling the endpoint sheds with 503 +
    /// `Retry-After` instead of queueing a CPU-bound transform.
    #[test]
    fn concurrency_ceiling_sheds_with_retry_after() {
        let calls = Arc::new(AtomicUsize::new(0));
        let backend = Arc::new(CountingBackend {
            calls: Arc::clone(&calls),
            concurrency_seen: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
            placeholder: None,
        });
        let (_dir, ep) = endpoint_with(backend, 1, 8192);
        // Simulate one transform already in flight.
        ep.in_flight.store(1, Ordering::SeqCst);

        let (status, headers, _) = body_of(serve_image_endpoint(
            Some(&ep),
            "/_oxide/image?url=/a.png&w=400",
        ));
        assert_eq!(status, 503, "over the ceiling is a shed, not a queue");
        assert!(
            headers
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("retry-after") && v == "1"),
            "Retry-After present: {headers:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0, "no transform ran");
        // The counter is restored so the next request is admitted.
        assert_eq!(
            ep.in_flight.load(Ordering::SeqCst),
            1,
            "the shed decremented its own slot"
        );
    }
}

#[cfg(test)]
mod socket_fanout_tests {
    use super::*;

    struct NoopDispatcher;

    #[async_trait::async_trait]
    impl SocketDispatcher for NoopDispatcher {
        async fn run(
            &self,
            _module_path: &str,
            _phase: &str,
            _payload: serde_json::Value,
            _socket_id: &str,
        ) -> std::result::Result<SocketRunResult, SocketRunError> {
            Ok(SocketRunResult::default())
        }
    }

    fn service() -> SocketService {
        SocketService {
            dispatcher: Arc::new(NoopDispatcher),
            routes: Arc::new(BTreeMap::new()),
            registry: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    #[tokio::test]
    async fn broadcast_skips_sender_and_other_modules() {
        let svc = service();
        let (a_tx, mut a_rx) = tokio::sync::mpsc::unbounded_channel();
        let (b_tx, mut b_rx) = tokio::sync::mpsc::unbounded_channel();
        let (c_tx, mut c_rx) = tokio::sync::mpsc::unbounded_channel();
        let _a = svc.register("a", "/chat", a_tx);
        let _b = svc.register("b", "/chat", b_tx);
        let _c = svc.register("c", "/other", c_tx);

        assert_eq!(svc.client_ids("/chat").len(), 2, "presence is per-module");
        svc.broadcast("/chat", "a", "hello");
        assert!(a_rx.try_recv().is_err(), "sender is excluded");
        assert_eq!(b_rx.try_recv().unwrap(), "hello");
        assert!(c_rx.try_recv().is_err(), "other modules are excluded");

        svc.send_to("c", "direct");
        assert_eq!(c_rx.try_recv().unwrap(), "direct");
    }

    #[test]
    fn drop_unregisters_the_socket() {
        let svc = service();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let guard = svc.register("a", "/chat", tx);
        assert_eq!(svc.registry.lock().unwrap().len(), 1);
        drop(guard);
        assert!(svc.registry.lock().unwrap().is_empty());
    }
}

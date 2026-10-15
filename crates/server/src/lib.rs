//! oxide-server — hyper pipeline, static assets, compression, TLS, rules (spec §12, §17, D7, D13).
//!
//! Request pipeline (§12.1): accept → limits + request id → static asset match
//! (immutable hashed, precompressed zstd/br/gzip, range + If-None-Match) → native rules →
//! route match + L0 → JS middleware only if present → dispatch (render/action/endpoint/
//! server-fn) → streaming response with dynamic compression + Server-Timing → cache fill.
//! Security defaults (§17.1) enforced in Rust before JS.

#![forbid(unsafe_code)]

pub mod metrics;
pub mod page_renderer;
pub mod loader;
pub use page_renderer::{PageRenderInput, PageRenderer, RenderStream};
pub use loader::{LoaderOutput, LoaderRunInput, LoaderRunner};
pub mod session;
pub use session::{Session, SessionCodec};
pub mod trace_context;

use bytes::Bytes;
use flate2::write::GzEncoder;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, Limited};
use futures::Stream;
use std::pin::Pin;

use hyper::body::Incoming;
use hyper::{Request as HyperRequest, Response as HyperResponse};
use oxide_diagnostics::{Diagnostic, OxideError};
use oxide_render::RequestTimings;
use oxide_router::RulesEngine;
use std::collections::BTreeMap;
use std::io::Write as IoWrite;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

pub type Result<T> = std::result::Result<T, OxideError>;
pub use metrics::Metrics;

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Diagnostic::error(code, msg))
}

/// A platform-agnostic request handed to the pipeline (web-standard shape, G5).
#[derive(Debug, Clone)]
pub struct OxideRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
    pub request_id: String,
}

impl OxideRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A chunked response body stream. Each item is either a chunk of bytes
/// or an I/O error that aborts the response (hyper closes the
/// connection — a truncated 200 is never silently delivered).
pub type BodyStream = Pin<
    Box<
        dyn Stream<Item = std::result::Result<Bytes, std::io::Error>>
            + Send
            + Sync
            + 'static,
    >,
>;

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
        const CACHE_CTL: &str = "public, max-age=31536000, immutable";

        if let Some(inm) = req.header("if-none-match") {
            let matches = inm.split(',').map(|c| c.trim()).any(|cand| {
                let cand = cand.strip_prefix("W/").unwrap_or(cand);
                cand == "*" || cand.trim_matches('"') == etag_raw
            });
            if matches {
                return Some(
                    OxideResponse::new(304, Bytes::new())
                        .with_header("ETag", etag_h)
                        .with_header("Cache-Control", CACHE_CTL)
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
                if let Ok(body) = std::fs::read(variant) {
                    // Range on a compressed variant → full 200 (no partial decode).
                    let len = body.len().to_string();
                    let mut resp = OxideResponse::new(200, Bytes::from(body));
                    resp = with_asset_headers(resp, entry, &etag_h, CACHE_CTL);
                    resp.headers
                        .push(("Content-Encoding".into(), enc.to_string()));
                    resp.headers.push(("Content-Length".into(), len));
                    return Some(resp);
                }
                // variant unreadable → fall through to the identity body
            }
        }

        let Ok(full) = std::fs::read(&entry.path) else {
            return None;
        };

        if req.method.eq_ignore_ascii_case("HEAD") {
            let mut resp = OxideResponse::new(200, Bytes::new());
            resp = with_asset_headers(resp, entry, &etag_h, CACHE_CTL);
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
            None => OxideResponse::new(200, Bytes::from(full)),
        };
        resp = with_asset_headers(resp, entry, &etag_h, CACHE_CTL);
        Some(resp)
    }
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
            enc.write_all(body).map_err(|e| err("SRV-COMP", format!("gzip failed: {e}")))?;
            let out = enc.finish().map_err(|e| err("SRV-COMP", format!("gzip failed: {e}")))?;
            Ok(Bytes::from(out))
        }
        "zstd" => {
            let lvl = level.clamp(1, 19);
            let out = zstd::bulk::compress(body, lvl).map_err(|e| err("SRV-COMP", format!("zstd failed: {e}")))?;
            Ok(Bytes::from(out))
        }
        "br" => {
            // brotli's quality range is 0..=11; we clamp to the pragmatic band and
            // map level 0 → quality 4 (the fastest sane setting for a per-request
            // dynamic path — the spec's `zstd`-preferred ordering is honoured by
            // the caller, brotli is the second choice).
            let q = if level <= 0 { 4 } else { level.clamp(1, 11) as u32 };
            let mut out = Vec::with_capacity(body.len() / 2);
            {
                let mut w = brotli::CompressorWriter::new(&mut out, 4096, q, 22);
                w.write_all(body)
                    .map_err(|e| err("SRV-COMP", format!("brotli failed: {e}")))?;
            }
            Ok(Bytes::from(out))
        }
        other => Err(err("SRV-NOENC", format!("unsupported content encoding `{other}`"))),
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

fn cookie_value<'a>(cookie_header: &'a str, name: &str) -> Option<&'a str> {
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
            let mask = if len == 0 { 0 } else { u128::MAX << (128 - len) };
            (a & mask) == (n & mask)
        }
        _ => false,
    }
}

/// SEC-05: is `peer` inside any of the `trusted_proxies` CIDRs?
pub fn peer_is_trusted(peer: std::net::IpAddr, trusted: &[String]) -> bool {
    trusted.iter().any(|c| {
        parse_cidr(c).is_some_and(|(net, len)| ip_in_cidr(peer, net, len))
    })
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
pub fn sign_cookie(name: &str, value: &str, keys: &[&[u8]]) -> String {
    let key = keys.first().copied().unwrap_or(&[]);
    format!("{name}.{value}.{}", cookie_mac(key, name, value))
}

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
        let json = serde_json::to_string_pretty(&self.state_json())
            .unwrap_or_else(|_| "{}".into());
        format!(
            "<!doctype html><html><head><title>Oxide Inspector</title>\
             <style>body{{font:13px/1.5 ui-monospace,monospace;background:#0a0a0a;color:#e6e6e6;padding:24px}}\
             pre{{white-space:pre-wrap;word-break:break-word}}</style></head>\
             <body><h1>Oxide Inspector</h1><pre>{}</pre></body></html>",
            json.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
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

    let mut builder = hyper::Request::builder().method(req.method.as_str()).uri(url);
    for (k, v) in &req.headers {
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
pub struct Pipeline {
    pub rules: RulesEngine,
    pub assets: Option<StaticAssets>,
    pub limits: Limits,
    pub security: crate::SecuritySettings,
    pub rate_limiter: Arc<RateLimiter>,
    pub health_path: String,
    /// Prometheus metrics endpoint path (§20). `/_oxide/metrics` default;
    /// empty string disables.
    pub metrics_path: String,
    /// Optional metrics registry. When `Some`, `/metrics_path` renders it.
    pub metrics: Option<Arc<Metrics>>,
    /// Optional inspector provider (§13.4). When `Some`, `/__oxide` and
    /// `/__oxide/state` return the inspector snapshot. Never set in production.
    pub inspector: Option<Arc<dyn InspectorSnapshot>>,
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
/// render pool (`oxide-jshost`); tests can wire a table of closures.
#[async_trait::async_trait]
pub trait ServerFnDispatcher: Send + Sync {
    /// Look up the function registered under `id` and run it with `args`.
    /// `Ok(bytes)` is the response body — the *encoded* return value, in
    /// whatever wire format the dispatcher and client agreed on. `Err` is
    /// a coded failure the pipeline turns into a 4xx/5xx envelope.
    async fn dispatch(&self, id: &str, args_json: &[u8]) -> std::result::Result<Vec<u8>, DispatchError>;
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
/// The transform itself is `oxide_assets::ImageBackend`, but the server
/// crate must not depend on `oxide-assets` — that crate pulls ravif,
/// resvg, skrifa and lightningcss, and `oxide-server` is meant to build
/// lean. `oxide-server` names only the trait seams below; the CLI builds
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
    }
}

/// The backend seam. `oxide_assets::ImageBackend` is adapted to this in the
/// CLI, so the server has no compile-time tie to the asset crate.
pub trait ImageEndpointBackend: Send + Sync {
    /// Transform the source bytes per the request. The error string is
    /// surfaced as an `AST-*`-coded diagnostic in the HTTP response.
    fn process(
        &self,
        req: ImageEndpointRequest,
    ) -> std::result::Result<ImageEndpointResult, String>;
}

/// The subset of `oxide_assets::ImageRequest` the endpoint passes through.
#[derive(Debug, Clone)]
pub struct ImageEndpointRequest {
    pub source: Vec<u8>,
    pub format: Option<&'static str>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub quality: Option<u8>,
}

/// Bytes plus the chosen content type and a cache key.
#[derive(Debug, Clone)]
pub struct ImageEndpointResult {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    pub cache_key: String,
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
            rate_limiter: std::sync::Arc::new(Default::default()),
            health_path: String::new(),
            metrics_path: String::new(),
            metrics: None,
            inspector: None,
            page_renderer: None,
            image_endpoint: None,
            server_fns: None,
            actions: None,
            tls: false,
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
}

impl Default for SecuritySettings {
    fn default() -> Self {
        Self {
            csp: "strict".into(),
            csrf: "origin".into(),
            host_allow_list: Vec::new(),
            ssrf_allow_list: Vec::new(),
            cookie_keys: Vec::new(),
        }
    }
}

/// The request path of an OxideRequest URL (absolute URLs stripped to origin-form,
/// query/fragment dropped).
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
fn serve_image_endpoint(
    endpoint: Option<&ImageEndpoint>,
    raw_url: &str,
) -> NativePrefix {
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
            return NativePrefix::Raw(OxideResponse::new(400, format!("bad image request: {msg}"))
                .with_header("content-type", "text/plain; charset=utf-8"));
        }
    };

    if let Some(secret) = endpoint.signing_secret.as_deref() {
        let Some(sig) = parsed.signature.as_deref() else {
            return NativePrefix::Raw(OxideResponse::new(403, "image request not signed")
                .with_header("content-type", "text/plain; charset=utf-8"));
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
            return NativePrefix::Raw(OxideResponse::new(403, "image signature mismatch")
                .with_header("content-type", "text/plain; charset=utf-8"));
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
        return NativePrefix::Raw(OxideResponse::new(400, "image source must be a site-relative path or an https URL")
            .with_header("content-type", "text/plain; charset=utf-8"));
    }
    let rel = path_only.trim_start_matches('/');
    let abs = endpoint.asset_root.join(rel);
    // Reject path traversal: the resolved path must stay under the root.
    let canon_root = match endpoint.asset_root.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            return NativePrefix::Raw(OxideResponse::new(500, "image asset root missing")
                .with_header("content-type", "text/plain; charset=utf-8"));
        }
    };
    let canon_abs = match abs.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            return NativePrefix::Raw(OxideResponse::new(404, "image source not found")
                .with_header("content-type", "text/plain; charset=utf-8"));
        }
    };
    if !canon_abs.starts_with(&canon_root) {
        return NativePrefix::Raw(OxideResponse::new(403, "image source escapes the asset root")
            .with_header("content-type", "text/plain; charset=utf-8"));
    }

    let source = match std::fs::read(&canon_abs) {
        Ok(b) => b,
        Err(_) => {
            return NativePrefix::Raw(OxideResponse::new(404, "image source not readable")
                .with_header("content-type", "text/plain; charset=utf-8"));
        }
    };

    let format_static = parsed.format;
    let req = ImageEndpointRequest {
        source,
        format: format_static,
        width: parsed.width,
        height: parsed.height,
        quality: parsed.quality,
    };

    match endpoint.backend.process(req) {
        Ok(result) => NativePrefix::Raw(
            OxideResponse::new(200, Bytes::from(result.bytes))
                .with_header("content-type", result.content_type)
                // §16.1: transformed images are content-addressed by the
                // backend's cache key, so they are safe to cache immutably.
                .with_header("Cache-Control", "public, max-age=31536000, immutable")
                .with_header(
                    "ETag",
                    format!("\"{}\"", &result.cache_key[..16.min(result.cache_key.len())]),
                )
                .with_header("Vary", "Accept"),
        ),
        Err(msg) => NativePrefix::Raw(
            OxideResponse::new(400, format!("image transform failed: {msg}"))
                .with_header("content-type", "text/plain; charset=utf-8"),
        ),
    }
}

/// Split a request URL into `(path, params-before-s=)`. The signer signs
/// `path?params` with `s=` appended last, so removing the `s=` pair (in any
/// position) reconstructs the signed form.
/// Parsed `/_oxide/image` query. Mirrors `oxide_assets`'s type but lives
/// here so the server has no dependency on the asset crate.
struct ImageQuery {
    url: String,
    width: Option<u32>,
    height: Option<u32>,
    format: Option<&'static str>,
    quality: Option<u8>,
    signature: Option<String>,
}

fn parse_image_query(query: &str) -> std::result::Result<ImageQuery, String> {
    let mut url: Option<String> = None;
    let mut width = None;
    let mut height = None;
    let mut format: Option<&'static str> = None;
    let mut quality = None;
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
            "w" => width = Some(v.parse::<u32>().map_err(|_| format!("`w` not an integer: {v:?}"))?),
            "h" => height = Some(v.parse::<u32>().map_err(|_| format!("`h` not an integer: {v:?}"))?),
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
                let q = v.parse::<u32>().map_err(|_| format!("`q` not an integer: {v:?}"))?;
                if q == 0 || q > 100 {
                    return Err(format!("`q` out of range: {q}"));
                }
                quality = Some(q as u8);
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

/// Recompute the signature `oxide_assets::sign_image_url` would have
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
        signature: None,
    };
    plan_remote_image(endpoint, &parsed).map_err(|r| {
        (
            r.status,
            String::from_utf8_lossy(&r.body).into_owned(),
        )
    })
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

    let url = url::Url::parse(&parsed.url)
        .map_err(|e| err(400, format!("invalid source URL: {e}")))?;
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
        return Err(err(403, format!("host {host:?} is not an allowed image source")));
    }
    if !ssrf_allowed(&host, &endpoint.ssrf_allow_list) {
        return Err(err(403, "image source resolves to a blocked network range".into()));
    }

    // The request carries the resolved format; the fetch arm fills `source`.
    Ok((
        ImageEndpointRequest {
            source: Vec::new(),
            format: parsed.format,
            width: parsed.width,
            height: parsed.height,
            quality: parsed.quality,
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
    };
    match endpoint.backend.process(ireq) {
        Ok(result) => OxideResponse::new(200, Bytes::from(result.bytes))
            .with_header("content-type", result.content_type)
            .with_header("Cache-Control", "public, max-age=31536000, immutable")
            .with_header(
                "ETag",
                format!("\"{}\"", &result.cache_key[..16.min(result.cache_key.len())]),
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
        let path = if current.path().is_empty() { "/" } else { current.path() };
        let query = current
            .query()
            .map(|q| format!("?{q}"))
            .unwrap_or_default();
        let connect_url = format!("{scheme}://{ip_lit}:{port}{path}{query}");
        let host_header = if (scheme == "http" && port == 80)
            || (scheme == "https" && port == 443)
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
pub fn remote_pattern_allows(
    patterns: &[RemoteImagePattern],
    host: &str,
    scheme: &str,
) -> bool {
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
            let data: serde_json::Value = serde_json::from_slice(&body)
                .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&body).into_owned()));
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
) -> OxideResponse {
    let Some(dispatcher) = dispatcher else {
        return OxideResponse::json(
            500,
            serde_json::json!({"error": {"code": "ACT-NO-DISPATCHER", "message": "no action dispatcher configured"}}),
        );
    };
    match dispatcher
        .dispatch(module_path, action, form_json, params, url, method, headers)
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
    Dispatch(Vec<(String, String)>),
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

        match self.native_prefix(&mut req) {
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
            NativePrefix::RemoteImage { endpoint, request, format, rule_headers } => {
                let request_id = req.request_id.clone();
                let method = req.method.clone();
                let cookie = req.header("cookie").map(str::to_string);
                Box::pin(async move {
                    let resp = fetch_remote_image(endpoint, request, format).await;
                    self.finish_response_with_request(resp, &rule_headers, &request_id, Some(&method), cookie.as_deref())
                })
            }
            NativePrefix::ServerFn { id, args, rule_headers } => {
                // §9.4: the pipeline already checked method + content type
                // + CSRF in `native_prefix`; this arm runs the user's
                // function and builds the response envelope.
                let request_id = req.request_id.clone();
                let method = req.method.clone();
                let cookie = req.header("cookie").map(str::to_string);
                let dispatch = self.server_fns.clone();
                Box::pin(async move {
                    let resp = run_server_fn(dispatch.as_deref(), &id, &args).await;
                    self.finish_response_with_request(resp, &rule_headers, &request_id, Some(&method), cookie.as_deref())
                })
            }
            NativePrefix::Dispatch(rule_headers) => {
                // Capture the request id before `req` moves into `dispatch` —
                // `finish_response` needs it to derive the per-request CSP
                // nonce, and the framework dispatch derives the same value
                // from the same id to stamp the boot script tag (§17.1).
                let request_id = req.request_id.clone();
                let method = req.method.clone();
                let cookie = req.header("cookie").map(str::to_string);
                // Call dispatch synchronously; only the (Send) boxed future crosses the await.
                let fut = dispatch(req);
                Box::pin(async move {
                    let resp = fut.await;
                    self.finish_response_with_request(resp, &rule_headers, &request_id, Some(&method), cookie.as_deref())
                })
            }
        }
    }

    /// Synchronous native prefix (§12.1 steps 2-6): limits → host allow-list →
    /// health → rules → static → csrf. Mutates `req.url` on rewrites.
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
                        let route = it.get("route").and_then(|x| x.as_str()).unwrap_or("");
                        if !name.is_empty() {
                            if let Some(val) = value {
                                m.observe_vital(name, route, val);
                            }
                        }
                    }
                }
            }
            return NativePrefix::Raw(
                OxideResponse::new(204, Bytes::new()),
            );
        }

        // (3a.2) inspector endpoints (§13.4) — only when configured (dev only).
        if let Some(insp) = &self.inspector {
            if req.method.eq_ignore_ascii_case("GET") && path == "/__oxide" {
                return NativePrefix::Raw(
                    OxideResponse::new(200, insp.render_html())
                        .with_header("content-type", "text/html; charset=utf-8"),
                );
            }
            if req.method.eq_ignore_ascii_case("GET") && path == "/__oxide/state" {
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

        // (3c) image endpoint (§16.1). GET/HEAD only. The transform runs
        // in Rust, never entering JS; a missing endpoint or an invalid
        // request yields a coded 4xx that names the reason.
        if req.method.eq_ignore_ascii_case("GET")
            && path == "/_oxide/image"
        {
            return serve_image_endpoint(
                self.image_endpoint.as_deref(),
                &req.url,
            );
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
            let fn_client_key = req
                .header("x-forwarded-for")
                .and_then(|v| v.split(',').next())
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "local".to_string());
            match self.rules.evaluate_with_key(&path, &fn_client_key) {
                oxide_router::RuleOutcome::Headers(map) => {
                    rule_headers = map.into_iter().collect();
                }
                // SEC-11 (partial): a rate-limited server-fn 429s too.
                oxide_router::RuleOutcome::RateLimited { retry_after_secs } => {
                    return NativePrefix::Raw(
                        OxideResponse::new(429, "too many requests")
                            .with_header("retry-after", retry_after_secs.to_string()),
                    );
                }
                _ => {}
            }
            let args = req.body.clone();
            return NativePrefix::ServerFn { id, args, rule_headers };
        }

        // (4) native rules (§12.1 step 4).
        let mut rule_headers: Vec<(String, String)> = Vec::new();
        // SEC-01: pass the client key so `rate_limit.key = "ip"` buckets per
        // client. `X-Forwarded-For` is only trusted behind a proxy (SEC-05);
        // here we use it if present, else a constant loopback key (the socket
        // peer is not plumbed into `OxideRequest` yet).
        let client_key = req
            .header("x-forwarded-for")
            .and_then(|v| v.split(',').next())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "local".to_string());
        match self.rules.evaluate_with_key(&path, &client_key) {
            oxide_router::RuleOutcome::Redirect {
                destination,
                status,
            } => {
                return NativePrefix::Raw(
                    OxideResponse::new(status, Bytes::new()).with_header("Location", destination),
                );
            }
            oxide_router::RuleOutcome::Rewrite(dest) => {
                req.url = url_with_path(&req.url, &dest);
                path = dest;
            }
            oxide_router::RuleOutcome::Headers(map) => {
                rule_headers = map.into_iter().collect();
            }
            oxide_router::RuleOutcome::AuthRequired => {
                return NativePrefix::Raw(
                    OxideResponse::new(401, "unauthorized")
                        .with_header("WWW-Authenticate", "Basic realm=\"oxide\""),
                );
            }
            oxide_router::RuleOutcome::RateLimited { retry_after_secs } => {
                // SEC-01: a 429 with the seconds until a token refills.
                return NativePrefix::Raw(
                    OxideResponse::new(429, "too many requests")
                        .with_header("retry-after", retry_after_secs.to_string()),
                );
            }
            oxide_router::RuleOutcome::None => {}
        }

        // (5) static assets (§12.1 step 3): GET/HEAD with an extension or the
        // /_oxide/static prefix.
        let is_get_head = matches!(req.method.to_ascii_uppercase().as_str(), "GET" | "HEAD");
        if is_get_head && (path_has_extension(&path) || path.starts_with("/_oxide/static")) {
            if let Some(assets) = &self.assets {
                if let Some(resp) = assets.serve(req, &path) {
                    return NativePrefix::Static(resp, rule_headers);
                }
            }
        }

        // (6) dispatch (§12.1 step 7) with CSRF verification (§17.1) first.
        if let Err(d) = csrf_check(req, &self.security.csrf) {
            return NativePrefix::Raw(OxideResponse::new(403, Bytes::from(d.message)));
        }
        NativePrefix::Dispatch(rule_headers)
    }

    /// Append security headers (+ hash-mode CSP), rule headers and the Server-Timing
    /// placeholder (kept only when dispatch did not set one — §12.1 step 8).
    fn finish_response(
        &self,
        resp: OxideResponse,
        rule_headers: &[(String, String)],
        request_id: &str,
    ) -> OxideResponse {
        self.finish_response_with_request(resp, rule_headers, request_id, None, None)
    }

    /// `finish_response` plus the CSRF token issuer (SEC-04): on a GET/HEAD
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
        if self.security.csrf.eq_ignore_ascii_case("token")
            && matches!(method.map(|m| m.to_ascii_uppercase()).as_deref(), Some("GET") | Some("HEAD"))
            && cookie_value(cookie_header.unwrap_or(""), "oxide_csrf").is_none()
        {
            use ring::rand::SecureRandom as _;
            let rng = ring::rand::SystemRandom::new();
            let mut bytes = [0u8; 16];
            if rng.fill(&mut bytes).is_ok() {
                let hex: String = bytes.iter().flat_map(|b| [char::from_digit((b >> 4) as u32, 16).unwrap(), char::from_digit((b & 0xf) as u32, 16).unwrap()]).collect();
                r = r.with_header(
                    "Set-Cookie",
                    &format!("oxide_csrf={hex}; Path=/; SameSite=Lax"),
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
pub fn inject_before_close(
    src: BodyStream,
    close: &[u8],
    tag: &[u8],
) -> BodyStream {
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
    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(8);

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
pub fn to_hyper_response(
    resp: OxideResponse,
) -> HyperResponse<BoxBody<Bytes, std::io::Error>> {
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

    let mut out: HyperResponse<BoxBody<Bytes, std::io::Error>> =
        HyperResponse::new(boxed);
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
    let headers = req
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    // Bounded read (§12.1 step 2): any overflow/read failure maps to SRV-413.
    let body = Limited::new(req.into_body(), limits.max_body)
        .collect()
        .await
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
    Ok(OxideRequest {
        method,
        url,
        headers,
        body,
        request_id,
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
    let rng = ring::rand::SystemRandom::new();
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

    let domain = if addr.is_ipv4() { Domain::IPV4 } else { Domain::IPV6 };
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
        let listener = bind_reusable(addr)
            .map_err(|e| err("SRV-BIND", format!("cannot bind {addr}: {e}")))?;
        let tls_acceptor = tls.map(tokio_rustls::TlsAcceptor::from);
        let mut shutdown = shutdown;
        // SEC-06: enforce the header-read timeout so a slowloris (a client
        // that drips headers) is dropped instead of holding a connection.
        // `header_read_timeout` needs a `Timer` — configure both once.
        let mut builder =
            hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());
        let header_timeout =
            std::time::Duration::from_millis(pipeline.limits.slow_request_timeout_ms);
        builder
            .http1()
            .timer(hyper_util::rt::TokioTimer::new())
            .header_read_timeout(Some(header_timeout));
        let graceful = hyper_util::server::graceful::GracefulShutdown::new();
        // Bound on a TLS handshake: a peer that opens a socket and never
        // completes the handshake must not pin a task forever.
        const ACCEPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                accepted = listener.accept() => {
                    let (stream, _peer) = match accepted {
                        Ok(x) => x,
                        Err(e) => {
                            tracing::warn!(error = %e, "accept failed");
                            continue;
                        }
                    };
                    let pipeline_for_service = Arc::clone(&pipeline);
                    let dispatch_for_service = Arc::clone(&dispatch);
                    let service = hyper::service::service_fn(move |req: HyperRequest<Incoming>| {
                        let pipeline = Arc::clone(&pipeline_for_service);
                        let dispatch = Arc::clone(&dispatch_for_service);
                        async move {
                            let request_id = next_request_id();
                            match from_hyper_request(req, request_id, &pipeline.limits).await {
                                Ok(oreq) => {
                                    let resp = pipeline.serve_request(oreq, dispatch.as_ref()).await;
                                    Ok::<_, std::io::Error>(to_hyper_response(resp))
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
        let certs: Vec<CertificateDer<'static>> =
            rustls_pemfile::certs(&mut cert_reader)
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
            .ok_or_else(|| err("SRV-TLS", "key PEM contains no PRIVATE KEY block".to_string()))?;

        let config = tokio_rustls::rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| err("SRV-TLS", format!("rustls config failed: {e}")))?;
        Ok(Arc::new(config))
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
    use oxide_diagnostics::{Diagnostic, OxideError};
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
            hyper_resp.headers().get(hyper::header::CONTENT_LENGTH).is_none(),
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
            Err(std::io::Error::new(std::io::ErrorKind::Other, "worker crashed")),
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

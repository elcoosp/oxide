//! The resumable-streams substrate (spec v0.2, §V2-F07).
//!
//! A long streaming response (an LLM-style endpoint, a big export) survives
//! a dropped connection: the server tees the bytes into a bounded, TTL'd
//! buffer keyed by a stream id while the original response streams out, and
//! the client reattaches at `GET /__oxide/stream/<id>` with
//! `Range: bytes=<n>-` to receive only the bytes it missed — first the
//! backlog, then the live tail until the producer finishes. This module
//! owns the V8-free pieces:
//!
//! - [`StreamBuf`]: one streamed response's replay buffer — the chunks so
//!   far, the byte length, the done flag, the overflow flag and the
//!   progress channel readers wait on.
//! - [`StreamStore`]: the id → buffer map with the spec's bounds — a
//!   per-buffer cap (`max_each`: past it the buffer loses replay instead of
//!   eating memory), a per-buffer TTL (past it reattach answers `Gone`) and
//!   a total cap (`max_bytes_total`) whose accounting evicts the oldest
//!   completed buffer first, then the oldest buffer at all.
//! - [`read_from`]: the pure replay read — the bytes at or after `offset`,
//!   concatenated from the held chunks; `Some(empty)` at the exact end,
//!   `None` past it.
//! - [`stream_id`]: the `blake3(request_id, counter)` 16-hex id (the
//!   A04 build-id convention).
//! - [`parse_range_offset`]: the `Range: bytes=<n>-` header the reattach
//!   requires (absent header defaults to 0; every other spelling is
//!   refused).
//! - [`stream_resume_response`]: the reattach response — `206` +
//!   `content-range: bytes n-*/*` with a stream that emits the backlog and
//!   then follows the live tail until done; `200` + `x-oxide-stream-complete: 1`
//!   for a completed buffer reattached at its exact end; `410` when the
//!   offset skips bytes nobody holds.
//! - [`tee_stream`]: the body wrapper the wiring installs — every chunk the
//!   consumer sees is appended to the buffer, the natural end (or a
//!   producer error) marks it done.
//! - [`SessionBinding`] / [`SessionBindingSource`] / [`CookieBindingSource`]:
//!   the §13.4 session binding (`bind id to the original request's session
//!   cookie hash; mismatch gives 410`) — the reattach identity check the R05
//!   review recorded as the deployment-facing gap, now wired:
//!   [`StreamStore::begin_bound`] mints the binding from the ORIGINAL
//!   request's cookie, [`StreamStore::get_bound`] demands the same identity
//!   back and answers [`Reattach::Gone`] on a mismatch (the same `410` an
//!   unknown id gets), and the injected source keeps both sides hashing
//!   identically.
//!
//! One deliberate spec deviation (recorded in `docs/deviations.md`): the
//! spec's `StreamBuf` pseudocode waits on a `tokio::sync::Notify` with
//! `notify_waiters`, whose waiters only register on their first poll — a
//! chunk appended between a reader's backlog check and that first poll is
//! missed and the reattach stream hangs forever. [`StreamBuf`] carries a
//! `watch` channel over a [`Progress`] value instead (the F05 `Sub`
//! pattern): every append/finish sends a distinct value, so `changed()`
//! never misses and the held primary receiver makes sends infallible.
//!
//! The wiring — teeing endpoint responses in the dispatch closure, the
//! route handler, the per-IP reattach rate limit — is the A01-phase-3 CLI
//! adoption; see `docs/deviations.md`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::StreamExt;
use ring::rand::SecureRandom;
use tokio::sync::watch;

use crate::OxideResponse;

/// The response header carrying the stream id (§V2-F07 wiring step 1).
pub const STREAM_HEADER: &str = "x-oxide-stream";

/// The opt-in marker a `route.ts` sets through the `oxide/server`
/// `resumable` helper (§V2-F07: "sets `x-oxide-resumable: 1`"). The
/// wiring tees only streamed endpoint responses carrying it — a plain
/// `Response(stream)` without the marker streams out unbuffered, exactly
/// as before.
pub const RESUMABLE_HEADER: &str = "x-oxide-resumable";

/// The `x-oxide-resumable` value the helper sets.
pub const RESUMABLE_VALUE: &str = "1";

/// The response header marking a completed-buffer reattach (§V2-F07
/// wiring step 2: "A completed buffer with `n == len` returns `200`, empty
/// body, `x-oxide-stream-complete: 1`").
pub const COMPLETE_HEADER: &str = "x-oxide-stream-complete";

// ------------------------------------------------- the session binding (R05)

// The spec's security table (§13.4, `/__oxide/stream/:id`): "bind id to the
// original request's session cookie hash; mismatch gives 410". The stream id
// is a 256-bit digest disclosed only in the creating response's head, but a
// disclosed id still reattaches from any client that learns it — a
// multi-user deployment that treats stream bytes as per-session secret needs
// the reattach bound to the identity that created the stream. The R05
// review (`docs/security-review-v0.2.md`) recorded the missing binding as
// the deployment-facing gap; this section is the seam.

/// The opaque session identity a reattach must present (§13.4: the
/// "session cookie hash"). The digest never leaves the server — it is
/// stored beside the buffer and compared at reattach — so the cookie value
/// and its hash are equally non-disclosed; the hash exists so a leaked
/// store dump cannot be replayed as a session.
///
/// Comparison is constant-time (the crate's SEC-04 helper), `Debug`
/// redacts, and there is deliberately no `PartialEq`: every identity
/// check goes through [`SessionBinding::matches`], so no spelling of
/// equality can bypass the constant-time path.
#[derive(Clone)]
pub struct SessionBinding([u8; 32]);

impl SessionBinding {
    /// The explicit constructor: a 32-byte digest another source computed
    /// (the pins use it for fixed identities; a deployment that derives
    /// session identities elsewhere implements [`SessionBindingSource`]
    /// over it).
    #[must_use]
    pub fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    /// The digest bytes (the persisted form, if a store ever outlives the
    /// process — the in-memory store does not).
    #[must_use]
    pub fn as_digest(&self) -> &[u8; 32] {
        &self.0
    }

    /// Constant-time identity check (§16.6's SEC-04 rule — the same
    /// primitive the signed-cookie and image-signature compares use).
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        crate::constant_time_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for SessionBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the digest — it IS the reattach credential.
        f.debug_tuple("SessionBinding")
            .field(&"<redacted>")
            .finish()
    }
}

/// Where a request's session identity comes from (the object-safe seam the
/// wiring injects once at boot; both the tee site and the reattach arm call
/// the SAME instance, so the two sides of one stream hash identically).
///
/// `None` means "this request presented no session identity" — a stream
/// minted without a binding accepts any reattach (today's semantics); a
/// stream minted WITH one refuses every identity-less reattach.
pub trait SessionBindingSource: Send + Sync {
    /// The request's session binding, or `None` when the request carries
    /// no session identity at all.
    fn binding(&self, headers: &[(String, String)]) -> Option<SessionBinding>;
}

/// The default source: blake3 keyed with a per-boot random key over the
/// request's named cookie value (§13.4's "session cookie hash"). The key
/// lives only in this instance — two constructed sources never agree
/// (pinned), so a binding minted by one serve process never validates
/// against another's, and an offline brute-force against a leaked digest
/// must also guess the boot key.
pub struct CookieBindingSource {
    cookie_name: String,
    key: [u8; 32],
}

impl CookieBindingSource {
    /// A source over `cookie_name` with a fresh random key. The RNG is the
    /// session codec's (`ring::rand::SystemRandom`); a failure is boot-
    /// blocking — the caller decides, loudly, whether to serve.
    ///
    /// # Errors
    /// `SEC-SESSION` when the system RNG is unavailable — the same code
    /// the session codec's key material carries.
    pub fn new(cookie_name: impl Into<String>) -> crate::Result<Self> {
        let mut key = [0u8; 32];
        ring::rand::SystemRandom::new()
            .fill(&mut key)
            .map_err(|_| crate::err("SEC-SESSION", "system RNG unavailable"))?;
        Ok(Self {
            cookie_name: cookie_name.into(),
            key,
        })
    }
}

impl SessionBindingSource for CookieBindingSource {
    fn binding(&self, headers: &[(String, String)]) -> Option<SessionBinding> {
        let value = cookie_value(headers, &self.cookie_name)?;
        let mut hasher = blake3::Hasher::new_keyed(&self.key);
        hasher.update(value.as_bytes());
        let digest = hasher.finalize();
        Some(SessionBinding::from_digest(digest.into()))
    }
}

/// The named cookie's value out of the raw `cookie:` header(s) (RFC 6265
/// cookie-pair list; names are case-sensitive, the header name is not).
/// Multiple `cookie:` headers are all scanned — the first pair with the
/// name wins.
fn cookie_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("cookie"))
        .find_map(|(_, v)| {
            v.split(';').find_map(|pair| {
                let (n, val) = pair.split_once('=')?;
                (n.trim() == name).then_some(val.trim())
            })
        })
}

// ------------------------------------------------------------------ the buffer

/// The watched progress of one buffer: the byte length so far and whether
/// the producer finished. Every `append` and `finish` sends a fresh value,
/// so a reader's `changed()` never waits on an already-delivered state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Bytes buffered so far (the reattach offsets are relative to it).
    pub len: u64,
    /// The producer finished (normally or with an error — either way the
    /// replayable bytes are final).
    pub done: bool,
}

/// One streamed response's replay buffer (§V2-F07 `StreamBuf`).
pub struct StreamBuf {
    /// The stream id this buffer is stored under (the `x-oxide-stream`
    /// value the client reattaches with).
    id: String,
    chunks: std::sync::Mutex<Vec<Bytes>>,
    len: AtomicU64,
    done: AtomicBool,
    overflow: AtomicBool,
    /// The progress channel — latest-wins with a held primary receiver
    /// (the F05 `Sub` pattern), so `subscribe_progress` readers can wait
    /// losslessly for the next append or the finish.
    progress: watch::Sender<Progress>,
    _progress_rx: watch::Receiver<Progress>,
    /// When the buffer was created — the TTL anchor for `get`/`sweep`.
    born: Instant,
    /// The session identity that minted the stream (§13.4's binding) —
    /// `None` for the unbound (`begin`) path, whose reattach stays
    /// identity-free.
    binding: Option<SessionBinding>,
}

impl StreamBuf {
    /// The bytes buffered so far (the reattach offset space).
    #[must_use]
    pub fn len_now(&self) -> u64 {
        self.len.load(Ordering::Acquire)
    }

    /// Whether the producer finished (normally or with an error).
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    /// Whether the buffer outgrew the per-stream cap and lost replay.
    #[must_use]
    pub fn overflowed(&self) -> bool {
        self.overflow.load(Ordering::Acquire)
    }

    /// The stream id this buffer is stored under.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// A receiver over the buffer's [`Progress`] (the reattach body waits
    /// on it losslessly — see the module doc).
    #[must_use]
    pub fn subscribe_progress(&self) -> watch::Receiver<Progress> {
        self.progress.subscribe()
    }

    /// Whether the stream requires the minting session's identity back at
    /// reattach (§13.4's binding) — `false` for the plain-`begin` path.
    #[must_use]
    pub fn is_session_bound(&self) -> bool {
        self.binding.is_some()
    }
}

// ------------------------------------------------------------------- the store

/// The outcome of a reattach lookup (§V2-F07 `Reattach`): the buffer to
/// serve from, or `Gone` — unknown id, expired TTL or overflowed buffer —
/// which the endpoint answers with a `410`.
#[derive(Clone)]
pub enum Reattach {
    /// The stream expired, overflowed or never existed: `410`.
    Gone,
    /// The buffer is replayable.
    Ok(Arc<StreamBuf>),
}

/// The id → buffer map with the spec's three bounds (§V2-F07 wiring step 3
/// limits: `ttl`, `maxBytesEach`, `maxBytesTotal`).
pub struct StreamStore {
    map: dashmap::DashMap<String, Arc<StreamBuf>>,
    /// Insertion order, for the oldest-first total-accounting eviction.
    order: std::sync::Mutex<Vec<String>>,
    ttl: Duration,
    max_each: u64,
    max_total: u64,
    /// The bytes currently held across all buffers.
    total: AtomicU64,
}

impl StreamStore {
    /// A store with the spec's bounds: each buffer replays at most
    /// `max_each` bytes for `ttl` since its creation, and the store holds
    /// at most `max_total` bytes across buffers.
    #[must_use]
    pub fn new(ttl: Duration, max_each: u64, max_total: u64) -> Self {
        Self {
            map: dashmap::DashMap::new(),
            order: std::sync::Mutex::new(Vec::new()),
            ttl,
            max_each,
            max_total,
            total: AtomicU64::new(0),
        }
    }

    /// The bytes currently held across all buffers (the memory bound's
    /// readout).
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.total.load(Ordering::Acquire)
    }

    /// Register a new stream under `id` and return its buffer (§V2-F07
    /// `begin`). The wiring generates the id with [`stream_id`].
    pub fn begin(&self, id: String) -> Arc<StreamBuf> {
        self.begin_bound(id, None)
    }

    /// [`StreamStore::begin`] with the §13.4 session binding: the tee site
    /// passes the ORIGINAL request's session identity (the wiring's
    /// [`SessionBindingSource`] over the minting request's headers), and
    /// every reattach must present the same one (the store maps a mismatch
    /// to [`Reattach::Gone`] — the same `410` an unknown id gets, so a
    /// refusal never confirms the id exists). `None` is the unbound path
    /// (identity-free reattach, today's semantics).
    ///
    /// # Panics
    /// Only on a poisoned order mutex (an in-flight append panicking
    /// through the same lock) — the pre-existing store convention.
    pub fn begin_bound(&self, id: String, binding: Option<SessionBinding>) -> Arc<StreamBuf> {
        let (progress, progress_rx) = watch::channel(Progress {
            len: 0,
            done: false,
        });
        let buf = Arc::new(StreamBuf {
            id: id.clone(),
            chunks: std::sync::Mutex::new(Vec::new()),
            len: AtomicU64::new(0),
            done: AtomicBool::new(false),
            overflow: AtomicBool::new(false),
            progress,
            _progress_rx: progress_rx,
            born: Instant::now(),
            binding,
        });
        self.map.insert(id.clone(), Arc::clone(&buf));
        self.order
            .lock()
            .expect("stream order mutex poisoned")
            .push(id);
        buf
    }

    /// Append one chunk to a buffer (the tee's per-chunk call). A chunk
    /// that would push the buffer past `max_each` overflows it: the
    /// buffer's chunks are dropped and it leaves the store — replay is
    /// disabled rather than memory unbounded. Appends after `finish` (or
    /// after an overflow) are no-ops.
    pub fn append(&self, buf: &StreamBuf, chunk: Bytes) {
        if buf.is_done() || buf.overflowed() {
            return;
        }
        if buf.len_now() + chunk.len() as u64 > self.max_each {
            buf.overflow.store(true, Ordering::Release);
            buf.chunks
                .lock()
                .expect("stream chunks mutex poisoned")
                .clear();
            self.remove(buf.id());
            return;
        }
        let chunk_len = chunk.len() as u64;
        let new_len = buf.len.fetch_add(chunk_len, Ordering::AcqRel) + chunk_len;
        {
            let mut chunks = buf.chunks.lock().expect("stream chunks mutex poisoned");
            chunks.push(chunk);
        }
        buf.progress.send_replace(Progress {
            len: new_len,
            done: false,
        });
        self.total.fetch_add(chunk_len, Ordering::AcqRel);
        self.evict_over(buf.id());
    }

    /// Mark a buffer's bytes final (the tee's end-of-stream and
    /// error-path call — "done plus length suffices", §V2-F07 wiring).
    pub fn finish(&self, buf: &StreamBuf) {
        if buf.is_done() {
            return;
        }
        buf.done.store(true, Ordering::Release);
        buf.progress.send_replace(Progress {
            len: buf.len_now(),
            done: true,
        });
    }

    /// The reattach lookup (§V2-F07 `get`): `Ok` while the buffer is
    /// within its TTL and has not overflowed, `Gone` otherwise.
    #[must_use]
    pub fn get(&self, id: &str) -> Reattach {
        match self.map.get(id) {
            Some(b) if !b.overflowed() && b.born.elapsed() <= self.ttl => {
                Reattach::Ok(Arc::clone(b.value()))
            }
            _ => Reattach::Gone,
        }
    }

    /// [`StreamStore::get`] with the §13.4 session check: a buffer minted
    /// WITH a binding answers `Gone` unless the reattach presents the same
    /// identity (`presented` is the reattach request's own session
    /// binding, hashed by the SAME source instance the tee site used). A
    /// mismatch (foreign or missing cookie) is the same `Gone` an unknown
    /// id gets — the endpoint cannot tell them apart, so a refused
    /// reattach never confirms the id exists — and the refusal logs one
    /// loud warn (the cookie value and the digest never appear; the
    /// mismatch warn is the operator's only signal that identities are
    /// being probed).
    #[must_use]
    pub fn get_bound(&self, id: &str, presented: Option<&SessionBinding>) -> Reattach {
        match self.map.get(id) {
            Some(b) if !b.overflowed() && b.born.elapsed() <= self.ttl => {
                match (&b.value().binding, presented) {
                    (Some(bound), Some(p)) if !bound.matches(p) => {
                        tracing::warn!(
                            stream = %id,
                            "stream reattach refused: session binding mismatch"
                        );
                        Reattach::Gone
                    }
                    (Some(_), None) => {
                        tracing::warn!(
                            stream = %id,
                            "stream reattach refused: the bound stream's session cookie was absent"
                        );
                        Reattach::Gone
                    }
                    // An unbound buffer, or the presented identity matched.
                    _ => Reattach::Ok(Arc::clone(b.value())),
                }
            }
            _ => Reattach::Gone,
        }
    }

    /// Remove expired buffers (the wiring's housekeeping task; §V2-F07
    /// `sweep`). The total accounting stays exact: every removal pays its
    /// bytes back.
    pub fn sweep(&self) {
        let expired: Vec<String> = self
            .map
            .iter()
            .filter(|e| e.value().born.elapsed() > self.ttl)
            .map(|e| e.key().clone())
            .collect();
        for id in expired {
            self.remove(&id);
        }
    }

    /// Drop a buffer from the map and the ordering, paying its bytes back
    /// to the total accounting.
    fn remove(&self, id: &str) {
        if let Some((_, b)) = self.map.remove(id) {
            self.total.fetch_sub(b.len_now(), Ordering::AcqRel);
        }
        self.order
            .lock()
            .expect("stream order mutex poisoned")
            .retain(|e| e != id);
    }

    /// The total-accounting bound (§V2-F07 wiring step 3: "total
    /// accounting evicts oldest completed buffers first"). Runs after each
    /// append; the buffer being appended to is protected — an in-flight
    /// stream never loses its own replay mid-write (the last-resort
    /// overflow path is `max_each`'s job, not the store's).
    fn evict_over(&self, protect: &str) {
        while self.total_bytes() > self.max_total {
            let order = self.order.lock().expect("stream order mutex poisoned");
            let victim = order
                .iter()
                .find(|id| {
                    id.as_str() != protect && self.map.get(id.as_str()).is_some_and(|b| b.is_done())
                })
                .or_else(|| order.iter().find(|id| id.as_str() != protect));
            match victim {
                Some(id) => {
                    let id = id.clone();
                    drop(order);
                    self.remove(&id);
                }
                None => return, // only the protected buffer remains
            }
        }
    }
}

/// The chunk vector behind the buffer, locked for one read. Every lock is
/// held for a single copy-out; no method re-enters.
fn chunks_of(buf: &StreamBuf) -> std::sync::MutexGuard<'_, Vec<Bytes>> {
    buf.chunks.lock().expect("stream chunks mutex poisoned")
}

/// The housekeeping task the wiring boots once at serve start (§V2-F07
/// wiring step 3: "A background task checks every 30 s"): call
/// [`StreamStore::sweep`] every `cadence`, so a buffer whose TTL passed is
/// reclaimed even if no reattach ever arrives to trigger the check
/// lazily. The spec's cadence is 30 s; the wiring passes the value (the
/// pins shrink it to real-clock scale). The handle aborts nothing by
/// itself — dropping it detaches the task, exactly like the skew expiry
/// task's `_` binding at boot.
#[must_use]
pub fn spawn_sweep(store: Arc<StreamStore>, cadence: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(cadence);
        // `interval` fires immediately by default; skip the zero-th tick
        // so a boot-time sweep does not run before any buffer exists.
        tick.tick().await;
        loop {
            tick.tick().await;
            store.sweep();
        }
    })
}

/// The bytes at or after `offset`, concatenated from the held chunks
/// (§V2-F07 `read_from`). `Some(empty)` at the exact end (the complete
/// reattach), `None` past it (the client skipped bytes nobody holds).
#[must_use]
pub fn read_from(buf: &StreamBuf, offset: u64) -> Option<Bytes> {
    let chunks = chunks_of(buf);
    let mut pos = 0u64;
    let mut out = Vec::new();
    for c in chunks.iter() {
        let end = pos + c.len() as u64;
        if end > offset {
            // `end > offset` bounds the slice by the chunk length, so the
            // widening conversion cannot truncate.
            let from = usize::try_from(offset.saturating_sub(pos)).unwrap_or(0);
            out.extend_from_slice(&c[from..]);
        }
        pos = end;
    }
    if offset > pos {
        return None;
    }
    Some(Bytes::from(out))
}

/// The stream id (§V2-F07 wiring step 1): `blake3(request_id, counter)`
/// truncated to 16 lowercase hex — the A04 build-id convention. The
/// counter disambiguates the resumable responses one request produces.
#[must_use]
pub fn stream_id(request_id: &str, counter: u64) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(request_id.as_bytes());
    hasher.update(&counter.to_le_bytes());
    let hash = hasher.finalize();
    let hex = hash.to_hex();
    hex.as_str()[..16].to_string()
}

/// The `Range: bytes=<n>-` the reattach route requires (§V2-F07 wiring
/// step 2, "default 0"). An absent header is the default: the full replay
/// from byte 0. Every other spelling — suffix ranges (`bytes=-5`),
/// bounded ranges (`bytes=0-99`), multi ranges, wrong units — is refused:
/// the caller answers `410`.
#[must_use]
pub fn parse_range_offset(headers: &[(String, String)]) -> Option<u64> {
    let Some(raw) = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("range"))
        .map(|(_, v)| v.trim())
    else {
        return Some(0); // absent: the spec's default offset
    };
    if raw.is_empty() {
        return None;
    }
    let spec = raw.strip_prefix("bytes=").map(str::trim)?;
    let (start, end) = spec.split_once('-')?;
    if !end.trim().is_empty() {
        return None; // a bounded range needs a known total: not ours to serve
    }
    start.trim().parse::<u64>().ok()
}

/// The reattach response (§V2-F07 wiring step 2):
///
/// - offset past the buffer's length (or an overflowed buffer): `410`.
/// - a completed buffer reattached at its exact end: `200`, empty body,
///   `x-oxide-stream-complete: 1`.
/// - otherwise: `206` + `content-range: bytes n-*/*` with a stream that
///   first emits `read_from(buf, offset)` and then follows the live tail
///   (the watched progress) until done.
#[must_use]
pub fn stream_resume_response(buf: &Arc<StreamBuf>, offset: u64) -> OxideResponse {
    if buf.overflowed() || offset > buf.len_now() {
        return stream_gone_response();
    }
    if buf.is_done() && offset == buf.len_now() {
        return OxideResponse::new(200, Bytes::new()).with_header(COMPLETE_HEADER, "1");
    }
    let body = resume_body(Arc::clone(buf), offset);
    OxideResponse::stream(206, body).with_header("content-range", format!("bytes {offset}-*/*"))
}

/// The `410` the endpoint answers an unreattachable stream with (§V2-F07
/// wiring step 2: "If `Gone` or the offset is greater than `len`, respond
/// `410`").
#[must_use]
pub fn stream_gone_response() -> OxideResponse {
    OxideResponse::new(410, Bytes::new())
}

/// The reattach body: the backlog (`read_from` from the offset), then the
/// live tail — every chunk appended after the reattach — until done. The
/// wait is the watched progress channel, registered before the backlog
/// re-check (the module doc's lost-wakeup note).
fn resume_body(buf: Arc<StreamBuf>, start: u64) -> crate::BodyStream {
    let rx = buf.subscribe_progress();
    Box::pin(futures::stream::unfold(
        (buf, start, rx),
        |(buf, mut offset, mut rx)| async move {
            loop {
                if let Some(bytes) = read_from(&buf, offset)
                    && !bytes.is_empty()
                {
                    offset += bytes.len() as u64;
                    return Some((Ok(bytes), (buf, offset, rx)));
                }
                let progress = *rx.borrow_and_update();
                if progress.done {
                    // Done and fully consumed (the 410 pre-check already
                    // refused an offset past the length).
                    return None;
                }
                if rx.changed().await.is_err() {
                    // The sender lives inside the buffer, which the held
                    // primary receiver keeps alive; unreachable, but a
                    // clean end beats a panic.
                    return None;
                }
            }
        },
    ))
}

/// Wrap a producer's body so every byte the consumer sees is teed into the
/// store's buffer (§V2-F07 wiring step 1). The natural end and a producer
/// error both mark the buffer done — the replayable bytes are final either
/// way, and the error still reaches the consumer verbatim.
pub fn tee_stream(
    store: &Arc<StreamStore>,
    buf: &Arc<StreamBuf>,
    inner: crate::BodyStream,
) -> crate::BodyStream {
    let store = Arc::clone(store);
    let buf = Arc::clone(buf);
    Box::pin(futures::stream::unfold(
        (inner, store, buf),
        |(mut inner, store, buf)| async move {
            match StreamExt::next(&mut inner).await {
                Some(Ok(chunk)) => {
                    store.append(&buf, chunk.clone());
                    Some((Ok(chunk), (inner, store, buf)))
                }
                Some(Err(e)) => {
                    store.finish(&buf);
                    Some((Err(e), (inner, store, buf)))
                }
                None => {
                    store.finish(&buf);
                    None
                }
            }
        },
    ))
}

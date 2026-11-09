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
use tokio::sync::watch;

use crate::OxideResponse;

/// The response header carrying the stream id (§V2-F07 wiring step 1).
pub const STREAM_HEADER: &str = "x-oxide-stream";

/// The response header marking a completed-buffer reattach (§V2-F07
/// wiring step 2: "A completed buffer with `n == len` returns `200`, empty
/// body, `x-oxide-stream-complete: 1`").
pub const COMPLETE_HEADER: &str = "x-oxide-stream-complete";

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
        let (progress, _progress_rx) = watch::channel(Progress {
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
            _progress_rx,
            born: Instant::now(),
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
                    id.as_str() != protect
                        && self
                            .map
                            .get(id.as_str())
                            .map(|b| b.is_done())
                            .unwrap_or(false)
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

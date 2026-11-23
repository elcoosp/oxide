//! SPEC-12 / §11.2: an HTTP `CacheHandler` for multi-node deployments.
//!
//! One node runs the cache service (`crate::HttpCacheService`, F53 — the
//! protocol's server half, any [`CacheHandler`] hosted behind these shapes;
//! a plain HTTP store fronting it works too); every
//! node points `cache.handler = { type: 'http', url }` at it. The protocol is
//! small and JSON:
//!
//! - `GET  {base}/entry/{key}`        → 200 + JSON `Entry`, or 404
//! - `PUT  {base}/entry/{key}`        → 204, body JSON `Entry`
//! - `DELETE {base}/entry/{key}`      → 204
//! - `GET  {base}/tag/{tag}`          → 200 + JSON `[key, ...]`
//! - `POST {base}/invalidate/{tag}`   → 200 + JSON `{count}` (soft/hard in body)
//!
//! `publish_invalidation` POSTs the tag so peer nodes sharing the service
//! converge. A network error degrades to a miss/no-op — the local tier keeps
//! serving, and the build/render proceeds (a remote cache is an optimization).
//!
//! Since V2-F68 the client's fan-out carries its own bound:
//! `with_inflight_cap` gates `request` on a permit per concurrent
//! exchange (the verbs and the F63 probes alike), acquired BEFORE the
//! socket opens and INSIDE the deadline window — a saturated cap
//! degrades within the F60 clock, never past it, and the default stays
//! the pre-F68 unbounded shape (the F60 posture: the operator opts in,
//! the choice is greppable).
//!
//! Since V2-F63 the client also carries the PULL side of the plane's
//! observability surfaces: the [`PlanePeer`] trait's probes answer "how is
//! my remote cache doing?" — the F58 `GET /health` posture pull decoded
//! into [`crate::CacheHealth`] and the F61 `GET /metrics` §20 exposition
//! passed through byte-verbatim — riding the same [`HttpCacheHandler::request`]
//! core the entry verbs ride, so the F60 deadline and the `CACHE-HTTP`
//! error context come with the probes for free.
//!
//! Since V2-F65 the client's read half carries the bound the F64 shell's
//! write half always had: `with_read_limit` caps what one exchange may
//! buffer (a peer answering past the cap — hostile or misrouted, the F63
//! record's captive-portal shape — is refused LOUD under `CACHE-HTTP`
//! before its bytes finish landing; the default stays the pre-F65
//! unbounded shape, byte-identical for every existing deployment, the
//! F60 posture for the client half). Since V2-F66 the client also refuses
//! to SPEAK a key or tag the request line cannot carry
//! ([`HttpCacheHandler::plane_name`]): the URLs are built by
//! interpolation and the wire has no escaping, so a name with a CR, LF or
//! space corrupted the exchange — every verb checks the name before a
//! single wire byte leaves and fails loud instead.
//!
//! Since V2-F70 the client stops paying a fresh dial per exchange:
//! `with_keep_alive(max_idle)` speaks `Connection: keep-alive`, reads the
//! Content-Length-driven body (the plane always names its answer's
//! length, so the exact body end is knowable on the wire), and parks the
//! socket in a per-host idle pool bounded at `max_idle` — the next
//! exchange to the same host pops it instead of dialing. A REUSED socket
//! that died before its first response byte (the peer's idle close won
//! the race) gets ONE fresh-dial retry — the stale-pool remedy, so
//! keep-alive never turns a working deployment flaky. The default stays
//! the pre-F70 `Connection: close` shape — byte-identical, the F60
//! posture: the operator opts in, the choice is greppable.

use crate::{Entry, Result, auth, codec, err, health::CacheHealth};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// An HTTP-backed `CacheHandler`.
pub struct HttpCacheHandler {
    base: String,
    /// V2-F60: the whole-exchange deadline (`with_timeout`), `None` (the
    /// default) for the pre-F60 unbounded shape — an existing deployment
    /// keeps its byte behavior until it opts in.
    deadline: Option<Duration>,
    /// V2-F65: the per-exchange read bound (`with_read_limit`), `None`
    /// (the default) for the pre-F65 unbounded read — the F60 posture for
    /// the client half: the operator opts in, the choice is greppable.
    read_limit: Option<usize>,
    /// V2-F68: the inflight bound's operator state (`with_inflight_cap`),
    /// `None` (the default) for the pre-F68 unbounded fan-out — the F60
    /// posture again. Read back via [`HttpCacheHandler::inflight_cap`];
    /// set TOGETHER with the gate below, never alone.
    inflight_cap: Option<usize>,
    /// V2-F68: the inflight bound's resource — the permit gate every
    /// `request` acquires (the verbs and the F63 probes alike), built by
    /// `with_inflight_cap` and `None` when no cap rides. The permit is
    /// acquired INSIDE the deadline window (`request` schedules
    /// [`HttpCacheHandler::gated`]), so a saturated cap degrades within
    /// the F60 clock, never past it.
    inflight_gate: Option<Arc<Semaphore>>,
    /// V2-F70: the keep-alive idle-pool bound (`with_keep_alive`), `None`
    /// (the default) for the pre-F70 close-per-exchange shape — the F60
    /// posture again. Read back via [`HttpCacheHandler::keep_alive`].
    keep_alive: Option<usize>,
    /// V2-F70: the per-host idle-socket pool the reused exchanges pop
    /// from and the completed exchanges park into (bounded per host by
    /// the `keep_alive` bound). Built once and always present — the
    /// pre-F70 shape never touches it (an empty map costs nothing).
    pool: Arc<std::sync::Mutex<HashMap<String, Vec<tokio::net::TcpStream>>>>,
}

impl HttpCacheHandler {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
            deadline: None,
            read_limit: None,
            inflight_cap: None,
            inflight_gate: None,
            keep_alive: None,
            pool: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Bound one exchange's READ — headers and body share the cap
    /// (V2-F65): the client half of the F64 bounds story. The shell
    /// refuses an oversized `Content-Length` before the first body byte
    /// lands, but the CLIENT drained whatever the peer sent —
    /// `read_to_end` grew the buffer without bound, so a hostile or
    /// misrouted peer (the F63 record's captive-portal shape) answering
    /// an endless or gigantic body cost the caller node its memory one
    /// header line later. The F60 deadline bounds the exchange's TIME;
    /// nothing bounded its MEMORY. With a cap attached, the read loop
    /// refuses the byte that would exceed it: the exchange fails LOUD
    /// under the file's stable `CACHE-HTTP` code naming the cap, and the
    /// failure flows through this file's existing degradation arms —
    /// `get` to a miss, `set` to a loud `Err`, the probes to a loud
    /// `Err` — exactly like every other transport-class failure. The
    /// header terminator rides the same budget, so a
    /// terminatorless flood stops at the cap too. Chainable; `None` (the
    /// default) stays the pre-F65 unbounded read — byte-identical for
    /// every existing deployment until the operator opts in.
    #[must_use]
    pub fn with_read_limit(mut self, read_limit: usize) -> Self {
        self.read_limit = Some(read_limit);
        self
    }

    /// The attached read bound, `None` when the client is unbounded (the
    /// default) — the introspection seam the F60 `deadline()` accessor
    /// established: tests and hosts pin the state instead of guessing it.
    #[must_use]
    pub fn read_limit(&self) -> Option<usize> {
        self.read_limit
    }

    /// Bound the client's CONCURRENCY (V2-F68): at most `cap` exchanges
    /// are on the wire at once; an exchange past the cap waits for a
    /// permit BEFORE its socket opens, inside the deadline window.
    ///
    /// The client opens one fresh socket per exchange (`Connection:
    /// close`) and degrades transport failures through this file's arms —
    /// but nothing bounded HOW MANY exchanges ran at once: a render
    /// fanning out over a hundred keys opened a hundred concurrent
    /// sockets against the same plane, and a struggling peer met a
    /// thundering herd instead of a queue. The F60 deadline bounds an
    /// exchange's TIME, the F65 read bound bounds its MEMORY, the F66
    /// guard bounds its NAMES — the F68 inflight cap bounds the client's
    /// concurrency, the last unbounded resource of the fan-out. The
    /// permit rides the exchange's whole life and is acquired INSIDE the
    /// deadline window ([`HttpCacheHandler::request`] schedules
    /// [`HttpCacheHandler::gated`]), so a saturated cap degrades within
    /// the F60 clock — a loud `CACHE-HTTP` deadline `Err`, never a hang —
    /// and the F63 probes ride the same gate (they are `request`
    /// consumers like any verb). Chainable; `None` (the default) stays
    /// the pre-F68 unbounded shape — byte-identical for every existing
    /// deployment, the F60 posture: the operator opts in, the choice is
    /// greppable.
    #[must_use]
    pub fn with_inflight_cap(mut self, cap: usize) -> Self {
        self.inflight_cap = Some(cap);
        self.inflight_gate = Some(Arc::new(Semaphore::new(cap)));
        self
    }

    /// The attached inflight bound, `None` when the client is unbounded
    /// (the default) — the introspection seam the F60 `deadline()`
    /// accessor established: tests and hosts pin the state instead of
    /// guessing it.
    #[must_use]
    pub fn inflight_cap(&self) -> Option<usize> {
        self.inflight_cap
    }

    /// Reuse connections across exchanges (V2-F70): the client speaks
    /// `Connection: keep-alive`, reads the Content-Length-driven body,
    /// and parks each finished socket in a per-host idle pool bounded at
    /// `max_idle` — the next exchange to the same host pops it instead of
    /// paying the dial.
    ///
    /// The §11.2 client opened ONE fresh socket per exchange
    /// (`Connection: close`) and the plane answered the same posture —
    /// every exchange cost a TCP handshake. The bounds arc fenced the
    /// exchange's TIME (F60), MEMORY (F64/F65), NAMES (F66) and COUNT
    /// (F67/F68); the DIAL is the last per-exchange tax, and the plane's
    /// answers always carry `Content-Length`, so the body's exact end is
    /// knowable on the wire. A reused socket that died before its first
    /// response byte (the peer's idle close won the race) gets ONE
    /// fresh-dial retry — the stale-pool remedy, so keep-alive never
    /// turns a working deployment flaky. A peer that answers without a
    /// `Content-Length` (not the plane's shape) is answered
    /// close-delimited and parked nowhere. Chainable; `max_idle = 0`
    /// parks nothing (the wire speaks keep-alive, every socket still
    /// closes after its exchange — the honest kill switch). `None` (the
    /// default) stays the pre-F70 close shape — byte-identical for every
    /// existing deployment, the F60 posture.
    #[must_use]
    pub fn with_keep_alive(mut self, max_idle: usize) -> Self {
        self.keep_alive = Some(max_idle);
        self
    }

    /// The attached idle-pool bound, `None` when the client dials per
    /// exchange (the default) — the introspection seam the F60
    /// `deadline()` accessor established: tests and hosts pin the state
    /// instead of guessing it.
    #[must_use]
    pub fn keep_alive(&self) -> Option<usize> {
        self.keep_alive
    }

    /// The request-line trust boundary (V2-F66): the public predicate a
    /// host can pre-validate names with, and the check every verb runs
    /// BEFORE a single wire byte leaves.
    ///
    /// The client builds its URLs by interpolation — `format!(
    /// "{base}/entry/{key}")` — and the wire has no escaping: the
    /// service's `route` contract is "everything after the family prefix
    /// IS the name" (keys and tags written verbatim, real keys carry
    /// slashes). A name carrying an ASCII control byte, a space or DEL
    /// did not fail — it CORRUPTED the exchange: a space split the
    /// request line (`split_whitespace`, the plane stored the key's head
    /// and dropped the tail), a CR or LF injected a second exchange into
    /// the connection stream. Keys are app-facing (`route:` keys embed
    /// request paths; a slug is user input), so the boundary is real.
    /// The refusal is LOUD (the house convention) under the file's
    /// stable `CACHE-HTTP` code, naming the offending byte — a silent
    /// miss would hide the caller bug; a corrupt exchange would poison
    /// the plane. Everything the line speaks verbatim passes: route
    /// keys, query punctuation, percent escapes, UTF-8 multi-byte (a
    /// continuation byte is never an ASCII control).
    ///
    /// # Errors
    ///
    /// [`OxideError`](crate::OxideError) with code `CACHE-HTTP` when the
    /// name carries a byte the request line cannot speak — the message
    /// names the offending byte so the caller can rename.
    pub fn plane_name(name: &str) -> Result<()> {
        let offender = name
            .bytes()
            .find(|b| matches!(b, 0x00..=0x1f | 0x20 | 0x7f));
        match offender {
            None => Ok(()),
            Some(b) => Err(err(
                "CACHE-HTTP",
                format!(
                    "the name carries a byte the request line cannot speak \
                     (byte {b:#04x}); refusing before the exchange"
                ),
            )),
        }
    }

    /// Bound the whole exchange — connect through the response read — to
    /// `deadline` (V2-F60): the §11.2 "a remote cache is an optimization"
    /// contract's teeth. Without a deadline the client degrades only on a
    /// REFUSED connection; a peer that hangs (a firewall dropping SYNs, a
    /// wedged service, a half-open socket) held every request it touched
    /// forever. With one, an exchange that outlives the deadline fails
    /// under `CACHE-HTTP` and flows through this file's existing
    /// degradation arms — `get` to a miss (the local tier keeps serving),
    /// `set` to a loud `Err` — exactly like every other transport
    /// failure. Chainable; `None` (the default) stays unbounded.
    #[must_use]
    pub fn with_timeout(mut self, deadline: Duration) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// The attached deadline, `None` when the client is unbounded (the
    /// default) — the introspection seam tests and hosts pin.
    #[must_use]
    pub fn deadline(&self) -> Option<Duration> {
        self.deadline
    }

    /// `(status, body)` for one HTTP/1.1 exchange. A minimal client — the
    /// cache protocol is a handful of verbs on a URL.
    ///
    /// The transport failures carry the stable `CACHE-HTTP` code (the
    /// house error-context convention — every arm below degrades on
    /// `Err`, and a degraded arm that logs the error now names the code
    /// it degraded under), never a bare `String`. The V2-F60 deadline
    /// wraps the WHOLE exchange: connect, write and read share one
    /// budget, so a peer that accepts and never answers degrades on the
    /// same clock a refused connection already rode. Since V2-F68 the
    /// scheduled future is [`HttpCacheHandler::gated`] — the inflight
    /// bound's permit rides INSIDE the deadline window, so a saturated
    /// cap degrades on that same clock too (the queue is part of the
    /// exchange).
    async fn request(
        &self,
        method: &str,
        url: &str,
        body: Option<&[u8]>,
    ) -> Result<(u16, Vec<u8>)> {
        let exchange = self.gated(method, url, body);
        if let Some(deadline) = self.deadline {
            if let Ok(result) = tokio::time::timeout(deadline, exchange).await {
                result
            } else {
                tracing::warn!(
                    code = "CACHE-HTTP",
                    url = %url,
                    deadline = ?deadline,
                    "the remote cache did not answer within the deadline; \
                     degrading (§11.2: a remote cache is an optimization)"
                );
                Err(err(
                    "CACHE-HTTP",
                    format!("the exchange exceeded its {deadline:?} deadline"),
                ))
            }
        } else {
            exchange.await
        }
    }

    /// The inflight-bound wrapper `request` schedules (V2-F68): the
    /// permit is acquired BEFORE the socket opens and rides the whole
    /// exchange, so at most `cap` exchanges are on the wire at once —
    /// verbs and probes alike. Without a cap this is the pre-F68 shape
    /// verbatim (the exchange, one wrapper hop).
    async fn gated(&self, method: &str, url: &str, body: Option<&[u8]>) -> Result<(u16, Vec<u8>)> {
        if let Some(gate) = &self.inflight_gate {
            let _permit = gate
                .acquire()
                .await
                .map_err(|e| err("CACHE-HTTP", format!("the inflight gate: {e}")))?;
            self.exchange(method, url, body).await
        } else {
            self.exchange(method, url, body).await
        }
    }

    /// The exchange `request` schedules — the pre-F70 shape when no pool
    /// rides (byte-identical), the keep-alive reuse path when one does.
    async fn exchange(
        &self,
        method: &str,
        url: &str,
        body: Option<&[u8]>,
    ) -> Result<(u16, Vec<u8>)> {
        match self.keep_alive {
            Some(max_idle) => self.reusable_exchange(method, url, body, max_idle).await,
            None => self.close_exchange(method, url, body).await,
        }
    }

    /// The pre-F70 exchange, byte-identical: dial, write
    /// `Connection: close`, `read_to_end` until the peer's close, parse.
    async fn close_exchange(
        &self,
        method: &str,
        url: &str,
        body: Option<&[u8]>,
    ) -> Result<(u16, Vec<u8>)> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let rest = url
            .strip_prefix("http://")
            .ok_or_else(|| err("CACHE-HTTP", format!("http-only: {url}")))?;
        let (host_port, path) = rest.split_once('/').unwrap_or((rest, ""));
        let mut stream = Self::dial(host_port).await?;
        let b = body.unwrap_or(&[]);
        let req = format!(
            "{method} /{path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
            b.len()
        );
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|e| err("CACHE-HTTP", format!("write: {e}")))?;
        if !b.is_empty() {
            stream
                .write_all(b)
                .await
                .map_err(|e| err("CACHE-HTTP", format!("write body: {e}")))?;
        }
        // V2-F65: the read's bound. Without a cap this is the pre-F65
        // shape verbatim (`read_to_end`); with one, the buffer only ever
        // holds bytes that fit — the byte that would exceed the cap
        // refuses the exchange LOUD (the stable code, the cap named)
        // instead of landing. The header terminator rides the same
        // budget, so a terminatorless flood stops at the cap too — the
        // F64 shell's own posture, transposed onto the client half.
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        match self.read_limit {
            Some(limit) => loop {
                let n = stream
                    .read(&mut tmp)
                    .await
                    .map_err(|e| err("CACHE-HTTP", format!("read: {e}")))?;
                if n == 0 {
                    break;
                }
                if buf.len() + n > limit {
                    tracing::warn!(
                        code = "CACHE-HTTP",
                        url = %url,
                        limit = limit,
                        "the peer's answer exceeded the client's read limit; \
                         refusing the exchange"
                    );
                    return Err(err(
                        "CACHE-HTTP",
                        format!("the peer's answer exceeded the read limit ({limit} bytes)"),
                    ));
                }
                buf.extend_from_slice(&tmp[..n]);
            },
            None => {
                stream
                    .read_to_end(&mut buf)
                    .await
                    .map_err(|e| err("CACHE-HTTP", format!("read: {e}")))?;
            }
        }
        let split = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or_else(|| err("CACHE-HTTP", "no header terminator"))?;
        let head = String::from_utf8_lossy(&buf[..split]);
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| err("CACHE-HTTP", "no status"))?;
        Ok((status, buf[split + 4..].to_vec()))
    }

    /// The keep-alive exchange (V2-F70): pop the host's idle socket (else
    /// dial), write `Connection: keep-alive`, read the Content-Length-
    /// driven answer, park the socket back (bounded) when the answer is
    /// reusable. A REUSED socket that died before its first response byte
    /// gets ONE fresh-dial retry — the stale-pool remedy (the peer's idle
    /// close won the race; a fresh dial's failure is the transport arm's
    /// answer). The F65 read limit bounds the buffered answer here too,
    /// the terminator riding the same budget (the `close_exchange` shape).
    async fn reusable_exchange(
        &self,
        method: &str,
        url: &str,
        body: Option<&[u8]>,
        max_idle: usize,
    ) -> Result<(u16, Vec<u8>)> {
        let rest = url
            .strip_prefix("http://")
            .ok_or_else(|| err("CACHE-HTTP", format!("http-only: {url}")))?;
        let (host_port, path) = rest.split_once('/').unwrap_or((rest, ""));
        let b = body.unwrap_or(&[]);
        let req = format!(
            "{method} /{path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: keep-alive\r\nContent-Length: {}\r\n\r\n",
            b.len()
        );
        let (mut stream, was_pooled) = match self.pop_pooled(host_port) {
            Some(s) => (s, true),
            None => (Self::dial(host_port).await?, false),
        };
        match Self::keep_alive_round(&mut stream, &req, body, url, self.read_limit).await {
            Ok((status, bytes, reusable)) => {
                if reusable && max_idle > 0 {
                    self.park_pooled(host_port, stream, max_idle);
                }
                Ok((status, bytes))
            }
            // The stale-pool remedy: only a REUSED socket's early death
            // retries — once, on a fresh dial.
            Err(e) if was_pooled => {
                tracing::debug!(
                    code = "CACHE-HTTP",
                    url = %url,
                    error = %e.0.message,
                    "the pooled socket went stale; one fresh dial"
                );
                let mut fresh = Self::dial(host_port).await?;
                let (status, bytes, reusable) =
                    Self::keep_alive_round(&mut fresh, &req, body, url, self.read_limit).await?;
                if reusable && max_idle > 0 {
                    self.park_pooled(host_port, fresh, max_idle);
                }
                Ok((status, bytes))
            }
            Err(e) => Err(e),
        }
    }

    /// One keep-alive round on an open socket: write the request head
    /// (plus the body when one rides), read the answer Content-Length-
    /// driven. Returns the `(status, body, reusable)` triple — `reusable`
    /// false when the answer carries no `Content-Length` (close-delimited:
    /// drained to EOF, parked nowhere) or speaks `Connection: close`.
    /// The F65 read limit bounds every buffered byte, the terminator and
    /// the body riding the same budget.
    async fn keep_alive_round(
        stream: &mut tokio::net::TcpStream,
        req: &str,
        body: Option<&[u8]>,
        url: &str,
        read_limit: Option<usize>,
    ) -> Result<(u16, Vec<u8>, bool)> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|e| err("CACHE-HTTP", format!("write: {e}")))?;
        if let Some(b) = body
            && !b.is_empty()
        {
            stream
                .write_all(b)
                .await
                .map_err(|e| err("CACHE-HTTP", format!("write body: {e}")))?;
        }
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        // The head: incremental reads until the terminator, the F65 cap
        // riding every chunk (a terminatorless flood stops at the cap).
        let head_end = loop {
            let n = stream
                .read(&mut tmp)
                .await
                .map_err(|e| err("CACHE-HTTP", format!("read: {e}")))?;
            if n == 0 {
                // EOF before a parseable head — the stale-socket shape.
                return Err(err("CACHE-HTTP", "no header terminator"));
            }
            Self::check_limit(&buf, n, url, read_limit)?;
            buf.extend_from_slice(&tmp[..n]);
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i;
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]);
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| err("CACHE-HTTP", "no status"))?;
        let mut content_length: Option<usize> = None;
        let mut peer_close = false;
        for line in head.lines().skip(1) {
            let lower = line.to_ascii_lowercase();
            if let Some(v) = lower.strip_prefix("content-length:")
                && let Ok(parsed) = v.trim().parse()
            {
                content_length = Some(parsed);
            }
            if let Some(v) = lower.strip_prefix("connection:")
                && v.trim().eq_ignore_ascii_case("close")
            {
                peer_close = true;
            }
        }
        match content_length {
            Some(cl) => {
                // The body: exactly the named length — the socket's next
                // exchange starts right after it.
                while buf.len() < head_end + 4 + cl {
                    let n = stream
                        .read(&mut tmp)
                        .await
                        .map_err(|e| err("CACHE-HTTP", format!("read: {e}")))?;
                    if n == 0 {
                        return Err(err(
                            "CACHE-HTTP",
                            "the answer ended before its Content-Length",
                        ));
                    }
                    Self::check_limit(&buf, n, url, read_limit)?;
                    buf.extend_from_slice(&tmp[..n]);
                }
                let body = buf[head_end + 4..head_end + 4 + cl].to_vec();
                Ok((status, body, !peer_close))
            }
            // No Content-Length: the answer is close-delimited — drain to
            // EOF (the pre-F70 read's own shape), park nothing.
            None => loop {
                let n = stream
                    .read(&mut tmp)
                    .await
                    .map_err(|e| err("CACHE-HTTP", format!("read: {e}")))?;
                if n == 0 {
                    return Ok((status, buf[head_end + 4..].to_vec(), false));
                }
                Self::check_limit(&buf, n, url, read_limit)?;
                buf.extend_from_slice(&tmp[..n]);
            },
        }
    }

    /// The F65 read bound's check, shared by every keep-alive read loop:
    /// the chunk that would exceed the cap refuses the exchange LOUD (the
    /// stable code, the cap named) instead of landing — the terminator,
    /// the body and a close-delimited drain all riding the same budget.
    fn check_limit(buf: &[u8], n: usize, url: &str, limit: Option<usize>) -> Result<()> {
        match limit {
            Some(limit) if buf.len() + n > limit => {
                tracing::warn!(
                    code = "CACHE-HTTP",
                    url = %url,
                    limit = limit,
                    "the peer's answer exceeded the client's read limit; \
                     refusing the exchange"
                );
                Err(err(
                    "CACHE-HTTP",
                    format!("the peer's answer exceeded the read limit ({limit} bytes)"),
                ))
            }
            _ => Ok(()),
        }
    }

    /// The dial both exchange paths share.
    async fn dial(host_port: &str) -> Result<tokio::net::TcpStream> {
        tokio::net::TcpStream::connect(host_port)
            .await
            .map_err(|e| err("CACHE-HTTP", format!("connect: {e}")))
    }

    /// Pop the host's idle socket, `None` when nothing is parked (the
    /// pool lock's poison recovers with the house `into_inner` shape —
    /// a dead pool must not dead the client).
    fn pop_pooled(&self, host: &str) -> Option<tokio::net::TcpStream> {
        self.pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_mut(host)
            .and_then(Vec::pop)
    }

    /// Park a finished socket in the host's idle pool, bounded: the slot
    /// at `max_idle` drops the socket here — the close is the bound's
    /// answer.
    fn park_pooled(&self, host: &str, stream: tokio::net::TcpStream, max_idle: usize) {
        let mut pool = self
            .pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let slot = pool.entry(host.to_string()).or_default();
        if slot.len() < max_idle {
            slot.push(stream);
        }
    }
}

#[async_trait]
impl crate::CacheHandler for HttpCacheHandler {
    async fn get(&self, key: &str) -> Result<Option<Entry>> {
        // V2-F66: the request-line trust boundary — a name the line
        // cannot speak is a LOUD caller error, never a corrupt exchange
        // (the arms below degrade TRANSPORT failures; this is not one).
        Self::plane_name(key)?;
        let url = format!("{}/entry/{key}", self.base);
        match self.request("GET", &url, None).await {
            Ok((200, body)) => {
                // §11.3 / P0-8: the L2 envelope is MAC-sealed, same as
                // redis and s3. A payload that does not open is a miss,
                // not an error — an unsealed legacy entry (from an
                // older build) or a forged one both fail closed. This
                // is the one pair the audit found missing.
                let verified: &[u8] = match auth::open_entry(&body) {
                    Some(v) => v,
                    None => return Ok(None),
                };
                match codec::decode_entry(verified) {
                    Ok(e) => Ok(Some(e)),
                    Err(e) => Err(err("CACHE-HTTP", format!("entry decode: {e}"))),
                }
            }
            // V2-F68 session micro-debt: the last SILENT degradation
            // arms in the file learned the loud convention (the
            // F65-session repair's class — `tag_keys` and
            // `invalidate_tag` already warn). Behavior unchanged —
            // the §11.2 contract degrades to a miss, the local tier
            // keeps serving — the SILENCE is the repair: a non-200
            // names the status (the plane's 500 handler-failure arm
            // is the shape to expect), a transport failure names the
            // error.
            Ok((status, _)) => {
                tracing::warn!(
                    code = "CACHE-HTTP",
                    key = %key,
                    status,
                    "the remote entry lookup answered a non-200; treating as a miss"
                );
                Ok(None)
            }
            Err(e) => {
                tracing::warn!(
                    code = "CACHE-HTTP",
                    key = %key,
                    error = %e,
                    "the remote entry lookup failed; treating as a miss"
                );
                Ok(None)
            }
        }
    }

    async fn set(&self, key: &str, entry: Entry) -> Result<()> {
        // V2-F66: the guard runs before the seal — a corrupt exchange is
        // never spoken regardless of the payload.
        Self::plane_name(key)?;
        let url = format!("{}/entry/{key}", self.base);
        // §11.3 / P0-8: seal the envelope. Mirrors redis_handler.rs and
        // s3_handler.rs — a forged entry (an attacker with L2 write
        // access who just omits the MAC) is rejected on read.
        let body = auth::seal_entry(&codec::encode_entry(&entry)?);
        match self.request("PUT", &url, Some(&body)).await {
            Ok((s, _)) if (200..300).contains(&s) => Ok(()),
            Ok((s, _)) => Err(err("CACHE-HTTP", format!("set returned {s}"))),
            // The transport error already carries the `CACHE-HTTP` code.
            Err(e) => Err(e),
        }
    }

    async fn delete(&self, key: &str) -> Result<()> {
        // V2-F66: a bad name is a caller error, not a transport failure
        // — loud `Err`, never the degrade arms below.
        Self::plane_name(key)?;
        let url = format!("{}/entry/{key}", self.base);
        match self.request("DELETE", &url, None).await {
            Ok((s, _)) if (200..300).contains(&s) || s == 404 => Ok(()),
            // A failed remote delete is a staleness gap the operator has
            // to see (the house convention: loud, not silent) — a key the
            // remote still holds survives an invalidation that reported
            // success locally. Not a local `Err`: the local tiers already
            // served the delete, and the trait's delete contract stays
            // "the local outcome".
            Ok((s, _)) => {
                tracing::warn!(
                    code = "CACHE-HTTP",
                    key = %key,
                    status = s,
                    "the remote delete returned a non-success status; the key may survive remotely"
                );
                Ok(())
            }
            Err(e) => {
                tracing::warn!(
                    code = "CACHE-HTTP",
                    key = %key,
                    error = %e,
                    "the remote delete failed; the key may survive remotely"
                );
                Ok(())
            }
        }
    }

    async fn tag_keys(&self, tag: &str) -> Result<Vec<String>> {
        // V2-F66: the tag names the request line verbatim — the same
        // boundary, loud before the exchange.
        Self::plane_name(tag)?;
        let url = format!("{}/tag/{tag}", self.base);
        match self.request("GET", &url, None).await {
            Ok((200, body)) => match serde_json::from_slice(&body) {
                // A corrupt tag-key list self-heals on the next write,
                // but the invalidation gap must be VISIBLE (the house
                // convention: loud, not silent).
                Ok(keys) => Ok(keys),
                Err(e) => {
                    tracing::warn!(
                        code = "CACHE-HTTP",
                        tag = %tag,
                        error = %e,
                        "the tag-key response did not parse as a JSON list; treating as empty"
                    );
                    Ok(Vec::new())
                }
            },
            // V2-F65 session: the degrade arms name the gap now (the
            // house convention this file's sibling arms already carry) —
            // the §11.2 contract is unchanged (an empty list degrades,
            // the local tier keeps serving), the SILENCE is the repair.
            Ok((status, _)) => {
                tracing::warn!(
                    code = "CACHE-HTTP",
                    tag = %tag,
                    status,
                    "the remote tag listing answered a non-200; treating as empty"
                );
                Ok(Vec::new())
            }
            Err(e) => {
                tracing::warn!(
                    code = "CACHE-HTTP",
                    tag = %tag,
                    error = %e,
                    "the remote tag listing failed; treating as empty"
                );
                Ok(Vec::new())
            }
        }
    }

    async fn invalidate_tag(&self, tag: &str, soft: bool) -> Result<usize> {
        // V2-F66: the publish IS this POST — the boundary runs before the
        // body is built, and `publish_invalidation`'s best-effort arm
        // carries the loud warn.
        Self::plane_name(tag)?;
        let url = format!("{}/invalidate/{tag}", self.base);
        // The serialization of a bool map cannot fail, so the shape stays
        // infallible — but the impossible path is LOUD, not silent (the
        // house convention this file's sibling arms already carry).
        let body = serde_json::to_vec(&serde_json::json!({ "soft": soft })).unwrap_or_else(|e| {
            tracing::warn!(
                code = "CACHE-HTTP",
                tag = %tag,
                error = %e,
                "the invalidate body did not serialize; sending an empty body"
            );
            Vec::new()
        });
        match self.request("POST", &url, Some(&body)).await {
            Ok((200, b)) => {
                // A malformed count reports 0 — behavior unchanged —
                // but the malformed ANSWER is visible, not silent.
                if let Some(count) = serde_json::from_slice::<serde_json::Value>(&b)
                    .ok()
                    .and_then(|v| v.get("count").and_then(serde_json::Value::as_u64))
                {
                    Ok(usize::try_from(count).unwrap_or(usize::MAX))
                } else {
                    tracing::warn!(
                        code = "CACHE-HTTP",
                        tag = %tag,
                        "the invalidate response carried no `count`; reporting 0"
                    );
                    Ok(0)
                }
            }
            // V2-F65 session: the degrade arms name the gap (the house
            // convention) — the §11.2 contract is unchanged (0 reports,
            // the local tiers already served the invalidation), the
            // SILENCE is the repair.
            Ok((status, _)) => {
                tracing::warn!(
                    code = "CACHE-HTTP",
                    tag = %tag,
                    status,
                    "the remote invalidate answered a non-200; reporting 0"
                );
                Ok(0)
            }
            Err(e) => {
                tracing::warn!(
                    code = "CACHE-HTTP",
                    tag = %tag,
                    error = %e,
                    "the remote invalidate failed; reporting 0"
                );
                Ok(0)
            }
        }
    }

    async fn publish_invalidation(&self, tag: &str, soft: bool) -> Result<()> {
        // The invalidate POST *is* the publish — peers hitting the same
        // service see the tag gone on their next fetch. A failed publish
        // is a cross-node invalidation gap: loud (the house convention),
        // never silent — but not a local `Err`, for the same reason
        // `delete` warns instead of failing: the local tiers already
        // served the invalidation and the gap is the peers' alone.
        if let Err(e) = self.invalidate_tag(tag, soft).await {
            tracing::warn!(
                code = "CACHE-HTTP",
                tag = %tag,
                soft = soft,
                error = %e,
                "the invalidation publish failed; peers may still serve the tag"
            );
        }
        Ok(())
    }

    fn handler_kind(&self) -> &'static str {
        "http"
    }
}

// ------------------------------------------------------------------- the peer seam (F63)

/// The client half of the plane's observability surfaces (V2-F63): the
/// probes a node points at the plane it is wired to.
///
/// F58 gave the service `GET /health` and F61 gave it `GET /metrics`, but
/// both answered a DIRECT socket caller only — the node whose
/// `cache.handler = { type: 'http', url }` points at the plane had no way
/// to answer "how is my remote cache doing?" without hand-rolling a raw
/// exchange, exactly the duplication the F53 client/server split exists
/// to prevent. The trait is the pull side: one seam a host composes,
/// wraps (a failover pair, a circuit breaker) or fakes in tests, with the
/// client's natural impl riding the same `request` core the entry verbs
/// ride — the F60 deadline bounds a probe exactly as it bounds a `get`,
/// and every failure speaks the file's stable `CACHE-HTTP` code.
///
/// The pull contract is the F58 arm's, transposed onto the client: each
/// probe is a LIVE pull — a probe diffs two pulls, a dashboard holds
/// none; nothing freezes at construction. A non-200 or an undecodable
/// body is a LOUD error (the house convention): a probe that silently
/// reported a zeroed posture for a provider-less or misrouted peer would
/// be worse than one that fails — an operator watches the error, never
/// the fabricated zero.
#[async_trait]
pub trait PlanePeer: Send + Sync {
    /// The peer's live posture pull: `GET {base}/health` (the F58 arm)
    /// decoded into [`CacheHealth`] — the F52 pull's value shape over the
    /// wire. Errors under `CACHE-HTTP`: a non-200 (the arm rides a
    /// provider-attached plane; a provider-less peer answers the F40
    /// 404), an undecodable body (a 200 that is not a posture — a
    /// misrouted front, a captive portal), a transport failure, an
    /// exchange past the attached deadline.
    async fn peer_health(&self) -> Result<CacheHealth>;

    /// The peer's live exposition pull: `GET {base}/metrics` (the F61
    /// arm) as the §20 text, byte-verbatim — a scraper diffs two pulls of
    /// the returned string exactly as it diffs two scrapes of the wire.
    /// The body is not re-rendered or validated beyond the status: the
    /// exposition's contract is text (the renderer is ONE, on the peer's
    /// side), and the health arm's serde decode has no text counterpart.
    /// Errors under `CACHE-HTTP`: a non-200, a transport failure, an
    /// exchange past the attached deadline.
    async fn peer_metrics(&self) -> Result<String>;
}

#[async_trait]
impl PlanePeer for HttpCacheHandler {
    async fn peer_health(&self) -> Result<CacheHealth> {
        let url = format!("{}/health", self.base);
        match self.request("GET", &url, None).await {
            Ok((200, body)) => serde_json::from_slice::<CacheHealth>(&body).map_err(|e| {
                // A 200 that is not a posture is an anomaly worth the
                // warn (a misrouted front answered); the error is the
                // probe's answer, the warn its paper trail.
                tracing::warn!(
                    code = "CACHE-HTTP",
                    url = %url,
                    error = %e,
                    "the peer's /health body did not parse as a posture"
                );
                err(
                    "CACHE-HTTP",
                    format!("the peer's /health body did not parse as a posture: {e}"),
                )
            }),
            Ok((status, _)) => {
                // The expected shape is a provider-less peer's F40 404;
                // the probe names the status either way — loud, never a
                // fabricated zeroed posture.
                tracing::warn!(
                    code = "CACHE-HTTP",
                    url = %url,
                    status,
                    "the peer answered a non-200 for /health"
                );
                Err(err(
                    "CACHE-HTTP",
                    format!(
                        "the peer answered {status} for /health — the arm rides a \
                         provider-attached plane (the F58 shape); a provider-less \
                         plane answers the F40 404"
                    ),
                ))
            }
            // The transport arm already carries the `CACHE-HTTP` code and
            // the deadline's warn (the F60 shape) — nothing to add.
            Err(e) => Err(e),
        }
    }

    async fn peer_metrics(&self) -> Result<String> {
        let url = format!("{}/metrics", self.base);
        match self.request("GET", &url, None).await {
            Ok((200, body)) => Ok(String::from_utf8_lossy(&body).into_owned()),
            Ok((status, _)) => {
                tracing::warn!(
                    code = "CACHE-HTTP",
                    url = %url,
                    status,
                    "the peer answered a non-200 for /metrics"
                );
                Err(err(
                    "CACHE-HTTP",
                    format!(
                        "the peer answered {status} for /metrics — the arm rides a \
                         provider-attached plane (the F61 shape); a provider-less \
                         plane answers the F40 404"
                    ),
                ))
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// SPEC-12: `HttpCacheHandler` round-trips an Entry and publishes a tag
    /// invalidation against a tiny in-process HTTP server.
    #[tokio::test]
    async fn http_cache_handler_roundtrips() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let store = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
            String,
            Vec<u8>,
        >::new()));
        let store2 = store.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut s) = conn else { continue };
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                let he = loop {
                    let n = s.read(&mut tmp).unwrap_or(0);
                    if n == 0 {
                        break None;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(i);
                    }
                };
                let Some(he) = he else { continue };
                let head = String::from_utf8_lossy(&buf[..he]).to_string();
                let mut lines = head.lines();
                let reqline = lines.next().unwrap_or("").to_string();
                let mut clen = 0usize;
                for l in lines {
                    if let Some(v) = l.to_ascii_lowercase().strip_prefix("content-length:") {
                        clen = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = buf[he + 4..].to_vec();
                while body.len() < clen {
                    let n = s.read(&mut tmp).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&tmp[..n]);
                }
                let method = reqline.split_whitespace().next().unwrap_or("").to_string();
                let path = reqline.split_whitespace().nth(1).unwrap_or("").to_string();
                let mut store = store2.lock().unwrap();
                if path.starts_with("/entry/") {
                    let key = path.trim_start_matches("/entry/").to_string();
                    if method == "PUT" {
                        store.insert(key, body);
                        let _ = s.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    } else if let Some(b) = store.get(&key) {
                        let h = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            b.len()
                        );
                        let _ = s.write_all(h.as_bytes());
                        let _ = s.write_all(b);
                    } else {
                        let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                } else if path.starts_with("/invalidate/") {
                    let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\nConnection: close\r\n\r\n{\"count\":1}\n");
                } else {
                    let _ = s.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            }
        });

        let h = HttpCacheHandler::new(format!("http://{addr}"));
        use crate::CacheHandler as _;
        assert!(h.get("k").await.unwrap().is_none());
        let e = Entry {
            body: b"body".to_vec(),
            status: 200,
            headers: vec![],
            tags: vec!["t".into()],
            policy: crate::Policy::default(),
            stored_at: 0,
            etag: "e".into(),
        };
        h.set("k", e.clone()).await.unwrap();
        let got = h.get("k").await.unwrap().expect("hit");
        assert_eq!(got.body, b"body");
        assert_eq!(h.invalidate_tag("t", true).await.unwrap(), 1);
        h.publish_invalidation("t", true).await.unwrap();
    }

    /// §11.3 / P0-8: a tampered (or unsealed/legacy) payload is a miss,
    /// not a hit and not an error. The same contract redis/s3 pin; the
    /// audit found `HttpCacheHandler` missing the seal entirely — a
    /// forged entry (omit the MAC) was served to every reader.
    #[tokio::test]
    async fn http_handler_tampered_entry_is_a_miss() {
        use crate::CacheHandler as _;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // Server that returns whatever is in `bodies[key]` on GET, and
        // stores the raw request body on PUT — no seal checking on its
        // side (the client is the trust boundary, as for redis/s3).
        // The header terminator is the read boundary; Content-Length
        // drives the body drain, so the client never sees a reset.
        let bodies = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::<
            String,
            Vec<u8>,
        >::new()));
        let bodies2 = bodies.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut s) = conn else { continue };
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                // Read until the header terminator.
                let head_end = loop {
                    let n = s.read(&mut tmp).unwrap_or(0);
                    if n == 0 {
                        break None;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(i);
                    }
                };
                let Some(head_end) = head_end else { continue };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let reqline = head.lines().next().unwrap_or("").to_string();
                let mut clen = 0usize;
                for l in head.lines() {
                    if let Some(v) = l.to_ascii_lowercase().strip_prefix("content-length:") {
                        clen = v.trim().parse().unwrap_or(0);
                    }
                }
                // Drain the body: unread data + close = TCP RST, which
                // the client reports as "Connection reset by peer".
                let mut body = buf[head_end + 4..].to_vec();
                while body.len() < clen {
                    let n = s.read(&mut tmp).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&tmp[..n]);
                }
                let method = reqline.split_whitespace().next().unwrap_or("");
                let path = reqline.split_whitespace().nth(1).unwrap_or("");
                if path.starts_with("/entry/") {
                    let key = path.trim_start_matches("/entry/").to_string();
                    if method == "PUT" {
                        bodies2.lock().unwrap().insert(key, body);
                        let _ = s.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    } else if method == "GET" {
                        let bodies = bodies2.lock().unwrap();
                        if let Some(b) = bodies.get(&key) {
                            let h = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                b.len()
                            );
                            let _ = s.write_all(h.as_bytes());
                            let _ = s.write_all(b);
                        } else {
                            let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        }
                    } else {
                        let _ = s.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                } else {
                    let _ = s.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            }
        });

        let h = HttpCacheHandler::new(format!("http://{addr}"));

        // 1. A valid sealed roundtrip works.
        let entry = Entry {
            body: b"real body".to_vec(),
            status: 200,
            headers: vec![],
            tags: vec![],
            policy: crate::Policy::default(),
            stored_at: 0,
            etag: "e1".into(),
        };
        h.set("k1", entry).await.unwrap();
        assert!(
            h.get("k1").await.unwrap().is_some(),
            "valid sealed entry round-trips",
        );

        // 2. Inject a forged (unsealed plain-JSON) entry at a fresh key —
        //    exactly what an attacker with L2 write access would do to
        //    poison the cache. `auth::open_entry` must reject it.
        let forged = serde_json::to_vec(&Entry {
            body: b"forged".to_vec(),
            status: 200,
            headers: vec![],
            tags: vec![],
            policy: crate::Policy::default(),
            stored_at: 0,
            etag: "e2".into(),
        })
        .unwrap();
        bodies.lock().unwrap().insert("k2".into(), forged);
        assert!(
            h.get("k2").await.unwrap().is_none(),
            "unsealed/forged entry is a miss, not a hit",
        );

        // 3. A tampered valid entry (flip a byte in the sealed body) is
        //    also a miss — the MAC covers the payload.
        let mut sealed = bodies.lock().unwrap().get("k1").unwrap().clone();
        if let Some(byte) = sealed.last_mut() {
            *byte ^= 0x01;
        }
        bodies.lock().unwrap().insert("k1".into(), sealed);
        assert!(
            h.get("k1").await.unwrap().is_none(),
            "tampered sealed entry is a miss",
        );
    }
}

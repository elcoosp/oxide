//! The cache plane's HTTP service (SPEC-12 / §11.2): the `HttpCacheHandler`
//! client's missing server half.
//!
//! The protocol's doc block says "one node runs the cache service" — until F53
//! nothing did: the client spoke `GET/PUT/DELETE /entry/{key}`,
//! `GET /tag/{tag}` and `POST /invalidate/{tag}` at a store only tests
//! faked. [`HttpCacheService`] IS that store: it composes any
//! [`CacheHandler`] (a `TieredCache` is one — memory + disk + remote tiers,
//! tags, invalidation) behind the exact exchange shapes the client speaks,
//! so a multi-node deployment points every node's `cache.handler =
//! { type: 'http', url }` at one node running the service and the plane
//! becomes shared with no Redis/S3 dependency.
//!
//! The split is the one the rest of the crate rides (the §11.2 seam rule):
//!
//! - [`HttpCacheService::respond`] is the pure routing core — a
//!   [`PlaneRequest`] in, a [`PlaneResponse`] out, no sockets. Tests and
//!   benches drive it directly; degradations are statuses, never panics.
//! - [`HttpCacheService::serve`] is the socket shell — a minimal HTTP/1.1
//!   exchange (the same shape the client's `request` half rides: one
//!   request, `Connection: close`, Content-Length body) per connection,
//!   parsed into a [`PlaneRequest`] and answered with the core's
//!   [`PlaneResponse`].
//!
//! The trust boundary holds on BOTH sides of the wire (§11.3 / P0-8): the
//! client seals every entry it PUTs, so the service opens and decodes a PUT
//! body before it lands — a forged (unsealed) or tampered payload is a 400
//! and NEVER enters the backing store, the same miss-rule the client pins on
//! read, enforced where the payload writes. Served payloads re-seal through
//! the same `OXIDE_CACHE_MAC_KEY`, so every node sharing the env key opens
//! them (the multi-node rule `auth` documents).
//!
//! Degradations are loud, never silent (the house convention this file's
//! client sibling already carries): handler failures answer 5xx (the
//! client's `set` then reports `CACHE-HTTP`, its `get` degrades to a miss —
//! §11.2's "a remote cache is an optimization" contract), forged and
//! tampered PUTs warn under the `CACHE-HTTP` code, and a malformed exchange
//! answers 400 and closes without poisoning the accept loop. The plane
//! itself emits no observer rows — the tiers remain the observation point
//! (the exporter counts stay one-per-op; a service arm would double-count).

use crate::{CacheHandler, Result, auth, codec, err};
use std::sync::Arc;

// ------------------------------------------------------------------- exchange types

/// One cache-plane exchange, transport-free: the method, the path, the body.
///
/// The socket shell parses one HTTP/1.1 request into this shape; the pure
/// core (`HttpCacheService::respond`) routes it. Hosts and tests construct
/// it directly to drive the service without sockets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneRequest {
    /// The HTTP method (`GET`, `PUT`, `DELETE`, `POST`); compared
    /// case-insensitively by the core.
    pub method: String,
    /// The path (`/entry/{key}`, `/tag/{tag}`, `/invalidate/{tag}`) with
    /// the leading slash; an unknown shape answers 404.
    pub path: String,
    /// The request body (`PUT`'s sealed entry, `POST`'s invalidate JSON).
    pub body: Vec<u8>,
}

impl PlaneRequest {
    /// A `GET` exchange (`/entry/{key}` hit-or-miss, `/tag/{tag}` list).
    #[must_use]
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            method: "GET".into(),
            path: path.into(),
            body: Vec::new(),
        }
    }

    /// A `PUT` exchange carrying the sealed entry payload.
    #[must_use]
    pub fn put(path: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            method: "PUT".into(),
            path: path.into(),
            body,
        }
    }

    /// A `POST` exchange carrying the invalidate JSON.
    #[must_use]
    pub fn post(path: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            method: "POST".into(),
            path: path.into(),
            body,
        }
    }

    /// A `DELETE` exchange (`/entry/{key}`).
    #[must_use]
    pub fn delete(path: impl Into<String>) -> Self {
        Self {
            method: "DELETE".into(),
            path: path.into(),
            body: Vec::new(),
        }
    }
}

/// One cache-plane answer: the status and the body bytes (a sealed entry,
/// a JSON key list, a JSON count — or empty for 204/404).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneResponse {
    /// The HTTP status (`200` hit/list/count, `204` stored/deleted,
    /// `400` malformed-or-forged, `404` miss, `405` wrong method,
    /// `500` handler failure).
    pub status: u16,
    /// The response body (already the exact bytes the wire carries).
    pub body: Vec<u8>,
}

/// The protocol's path families (the routing core's first cut).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Segment {
    /// `/entry/{key}` — the entry store (GET/PUT/DELETE).
    Entry,
    /// `/tag/{tag}` — the tag-key listing (GET).
    Tag,
    /// `/invalidate/{tag}` — the invalidation (POST).
    Invalidate,
    /// Anything else — 404.
    Unknown,
}

/// Route one path into its family + name. The name is the FULL remainder
/// of the path — the client writes keys and tags verbatim into the URL
/// (`format!("{base}/entry/{key}")`, no percent-encoding), and real keys
/// in this workspace carry slashes (`route:/blog/1`), so the service
/// speaks the same shape: everything after the family prefix IS the name.
/// A bare prefix (`/entry/`) has no name and answers 404 (the `None`).
fn route(path: &str) -> (Segment, Option<&str>) {
    let (segment, rest) = match path.strip_prefix("/entry/") {
        Some(r) => (Segment::Entry, r),
        None => match path.strip_prefix("/tag/") {
            Some(r) => (Segment::Tag, r),
            None => match path.strip_prefix("/invalidate/") {
                Some(r) => (Segment::Invalidate, r),
                None => return (Segment::Unknown, None),
            },
        },
    };
    (!rest.is_empty())
        .then_some(rest)
        .map_or((segment, None), |n| (segment, Some(n)))
}

/// The reason phrases the socket shell emits (the protocol's statuses only).
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    }
}

// ------------------------------------------------------------------- the service

/// The §11.2 cache service: any [`CacheHandler`] behind the protocol the
/// `HttpCacheHandler` client speaks.
///
/// Build with [`HttpCacheService::new`], then either drive the pure core
/// ([`HttpCacheService::respond`]) or host it on a listener
/// ([`HttpCacheService::serve`]). The service is transport-minimal by
/// design — the protocol is a handful of verbs, the same posture the
/// client's `request` half rides; a real deployment fronts it with
/// whatever TLS/proxy layer the operator already runs.
pub struct HttpCacheService {
    handler: Arc<dyn CacheHandler>,
}

/// A wire-capped exchange: the header block refuses past this bound so a
/// hostile client cannot grow the buffer unbounded (a refusal is a 400 +
/// close). The cap guards the PARSER, not the codec — the codec re-checks
/// the decompressed size itself.
const HEADER_CAP: usize = 64 * 1024;

impl HttpCacheService {
    /// Compose the service over a backing handler (a `TieredCache`, a
    /// feature-gated driver, or a host's own [`CacheHandler`]).
    pub fn new(handler: Arc<dyn CacheHandler>) -> Self {
        Self { handler }
    }

    /// The backing handler's self-named kind (the F32 introspection
    /// seam) — what the posture of a plane rooted here would report.
    #[must_use]
    pub fn backing_kind(&self) -> &'static str {
        self.handler.handler_kind()
    }

    /// The pure routing core: route one [`PlaneRequest`] through the
    /// backing handler and answer a [`PlaneResponse`].
    ///
    /// Infallible by contract — every degradation is a status plus a
    /// `tracing` arm under the `CACHE-HTTP` code (loud, not silent):
    ///
    /// - `GET /entry/{key}` → `200` + the re-sealed entry, `404` on a
    ///   miss, `500` when the handler errors (a miss caches; an error is
    ///   loud — the client's `get` degrades it to a miss, §11.2).
    /// - `PUT /entry/{key}` → `204` after the body opens and decodes and
    ///   lands through the handler; `400` when the payload does not open
    ///   (a forged/unsealed body) or does not decode (a corrupt one) —
    ///   the payload never enters the store; `500` when the handler's
    ///   `set` fails.
    /// - `DELETE /entry/{key}` → `204` (the trait's local-outcome
    ///   contract; the handler owns its own loud arms).
    /// - `GET /tag/{tag}` → `200` + the JSON key list.
    /// - `POST /invalidate/{tag}` → `200` + `{"count":N}`; the body's
    ///   `{"soft":bool}` selects soft/hard (an empty body is hard, the
    ///   protocol's default), a malformed body is `400`.
    /// - An unknown path is `404`; a known path with the wrong method is
    ///   `405`.
    pub async fn respond(&self, req: &PlaneRequest) -> PlaneResponse {
        let method = req.method.to_ascii_uppercase();
        let (segment, name) = route(&req.path);

        let Some(name) = name else {
            return PlaneResponse {
                status: 404,
                body: Vec::new(),
            };
        };

        // The method table per family: the protocol is a handful of verbs,
        // and a wrong verb on a known family is a 405 (loud), not a 404.
        let wrong_method = |allowed: &'static str| {
            tracing::warn!(
                code = "CACHE-HTTP",
                method = %req.method,
                path = %req.path,
                allowed = allowed,
                "unsupported method for the path family"
            );
            PlaneResponse {
                status: 405,
                body: Vec::new(),
            }
        };

        match segment {
            Segment::Entry => match method.as_str() {
                "GET" => self.serve_get(name).await,
                "PUT" => self.serve_put(name, &req.body).await,
                "DELETE" => self.serve_delete(name).await,
                _ => wrong_method("GET/PUT/DELETE"),
            },
            Segment::Tag => {
                if method == "GET" {
                    self.serve_tag(name).await
                } else {
                    wrong_method("GET")
                }
            }
            Segment::Invalidate => {
                if method == "POST" {
                    self.serve_invalidate(name, &req.body).await
                } else {
                    wrong_method("POST")
                }
            }
            Segment::Unknown => PlaneResponse {
                status: 404,
                body: Vec::new(),
            },
        }
    }

    /// The loud 500: a handler failure is NOT a miss (a miss caches; an
    /// error is loud). The client degrades it — the §11.2 "a remote cache
    /// is an optimization" contract.
    fn handler_error(op: &'static str, key: &str, msg: &str) -> PlaneResponse {
        tracing::warn!(
            code = "CACHE-HTTP",
            op = op,
            key = %key,
            error = %msg,
            "the backing handler failed; answering 500"
        );
        PlaneResponse {
            status: 500,
            body: Vec::new(),
        }
    }

    /// `GET /entry/{k}`: the handler's entry, re-sealed for the wire.
    async fn serve_get(&self, key: &str) -> PlaneResponse {
        match self.handler.get(key).await {
            Ok(Some(entry)) => {
                let sealed = match codec::encode_entry(&entry) {
                    Ok(encoded) => auth::seal_entry(&encoded),
                    Err(e) => {
                        // Impossible for a decoded entry (the codec's own
                        // cap bounds the body it just produced) — but the
                        // impossible path is LOUD, not silent.
                        tracing::warn!(
                            code = "CACHE-HTTP",
                            key = %key,
                            error = %e.0.message,
                            "the stored entry did not re-encode; answering 500"
                        );
                        return PlaneResponse {
                            status: 500,
                            body: Vec::new(),
                        };
                    }
                };
                PlaneResponse {
                    status: 200,
                    body: sealed,
                }
            }
            Ok(None) => PlaneResponse {
                status: 404,
                body: Vec::new(),
            },
            Err(e) => Self::handler_error("get", key, &e.0.message),
        }
    }

    /// `PUT /entry/{k}`: open + decode + land through the handler, or 400
    /// without storing (the trust boundary where the payload writes).
    async fn serve_put(&self, key: &str, body: &[u8]) -> PlaneResponse {
        let Some(opened) = auth::open_entry(body) else {
            tracing::warn!(
                code = "CACHE-HTTP",
                key = %key,
                "the PUT payload did not open (forged or unsealed); refusing without storing"
            );
            return PlaneResponse {
                status: 400,
                body: Vec::new(),
            };
        };
        let entry = match codec::decode_entry(opened) {
            Ok(entry) => entry,
            Err(e) => {
                tracing::warn!(
                    code = "CACHE-HTTP",
                    key = %key,
                    error = %e.0.message,
                    "the PUT payload opened but did not decode; refusing without storing"
                );
                return PlaneResponse {
                    status: 400,
                    body: Vec::new(),
                };
            }
        };
        match self.handler.set(key, entry).await {
            Ok(()) => PlaneResponse {
                status: 204,
                body: Vec::new(),
            },
            Err(e) => Self::handler_error("set", key, &e.0.message),
        }
    }

    /// `DELETE /entry/{k}`: the trait's local-outcome contract.
    async fn serve_delete(&self, key: &str) -> PlaneResponse {
        match self.handler.delete(key).await {
            Ok(()) => PlaneResponse {
                status: 204,
                body: Vec::new(),
            },
            Err(e) => Self::handler_error("delete", key, &e.0.message),
        }
    }

    /// `GET /tag/{t}`: the JSON key list.
    async fn serve_tag(&self, tag: &str) -> PlaneResponse {
        match self.handler.tag_keys(tag).await {
            Ok(keys) => PlaneResponse {
                status: 200,
                body: serde_json::to_vec(&keys).unwrap_or_default(),
            },
            Err(e) => Self::handler_error("tag_keys", tag, &e.0.message),
        }
    }

    /// `POST /invalidate/{t}`: the JSON count; the body's `{"soft":bool}`
    /// selects soft/hard (an empty body is hard — the protocol's default),
    /// a malformed body is a 400 and never reaches the handler.
    async fn serve_invalidate(&self, tag: &str, body: &[u8]) -> PlaneResponse {
        let soft = if body.is_empty() {
            false
        } else {
            match serde_json::from_slice::<serde_json::Value>(body) {
                Ok(v) => v
                    .get("soft")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                Err(e) => {
                    tracing::warn!(
                        code = "CACHE-HTTP",
                        tag = %tag,
                        error = %e,
                        "the invalidate body did not parse as JSON; answering 400"
                    );
                    return PlaneResponse {
                        status: 400,
                        body: Vec::new(),
                    };
                }
            }
        };
        match self.handler.invalidate_tag(tag, soft).await {
            Ok(count) => PlaneResponse {
                status: 200,
                body: serde_json::to_vec(&serde_json::json!({ "count": count }))
                    .unwrap_or_default(),
            },
            Err(e) => Self::handler_error("invalidate", tag, &e.0.message),
        }
    }

    /// Host the service on a bound listener: one minimal HTTP/1.1 exchange
    /// per connection (`Connection: close`, the client's own posture),
    /// parsed into a [`PlaneRequest`] and answered with the core's
    /// [`PlaneResponse`]. Connections run concurrently; a malformed
    /// exchange answers 400 and closes without touching the accept loop.
    ///
    /// # Errors
    ///
    /// [`OxideError`] with code `CACHE-HTTP` when the listener itself
    /// fails (`accept`) — the only fatal arm; per-connection failures are
    /// statuses, never loop failures.
    pub async fn serve(self, listener: tokio::net::TcpListener) -> Result<()> {
        let this = Arc::new(self);
        loop {
            let (stream, _peer) = listener
                .accept()
                .await
                .map_err(|e| err("CACHE-HTTP", format!("accept: {e}")))?;
            let this = Arc::clone(&this);
            tokio::spawn(async move {
                if let Err(e) = this.handle_conn(stream).await {
                    // `handle_conn`'s Err is the transport arm (a read or
                    // write failed mid-exchange) — the exchange dies with
                    // the connection; the loop and the plane do not.
                    tracing::debug!(code = "CACHE-HTTP", error = %e.0.message, "exchange aborted");
                }
            });
        }
    }

    /// One connection's exchange: parse the request line + headers, drain
    /// the body, route, answer, close. Never panics — a parse refusal is
    /// a 400 (or a bare close when the peer is already gone).
    async fn handle_conn(&self, mut stream: tokio::net::TcpStream) -> Result<()> {
        use tokio::io::AsyncReadExt;

        // Read until the header terminator, bounded (the hostile-client
        // cap): an exchange that never terminates answers 400 + close.
        let mut buf: Vec<u8> = Vec::new();
        let mut tmp = [0u8; 4096];
        let head_end = loop {
            let n = stream
                .read(&mut tmp)
                .await
                .map_err(|e| err("CACHE-HTTP", format!("read: {e}")))?;
            if n == 0 {
                // The peer closed before a parseable exchange arrived —
                // nothing to answer; the close IS the answer.
                return Ok(());
            }
            buf.extend_from_slice(&tmp[..n]);
            if buf.len() > HEADER_CAP {
                Self::answer(
                    &mut stream,
                    &PlaneResponse {
                        status: 400,
                        body: Vec::new(),
                    },
                )
                .await?;
                return Ok(());
            }
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i;
            }
        };

        let head = String::from_utf8_lossy(&buf[..head_end]);
        let mut lines = head.lines();
        let reqline = lines.next().unwrap_or("");
        let mut parts = reqline.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();

        // Content-Length drives the body drain (the client's own shape:
        // unread data + close = a TCP RST the peer reports as an error).
        let mut clen = 0usize;
        for line in lines {
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                clen = v.trim().parse().unwrap_or(0);
            }
        }
        let mut body = buf[head_end + 4..].to_vec();
        while body.len() < clen {
            let n = stream
                .read(&mut tmp)
                .await
                .map_err(|e| err("CACHE-HTTP", format!("read body: {e}")))?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&tmp[..n]);
        }
        body.truncate(clen);

        let req = PlaneRequest { method, path, body };
        let resp = self.respond(&req).await;
        Self::answer(&mut stream, &resp).await
    }

    /// Serialize one [`PlaneResponse`] onto the wire (status line,
    /// Content-Length, `Connection: close`, the body).
    async fn answer(stream: &mut tokio::net::TcpStream, resp: &PlaneResponse) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        let head = format!(
            "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            resp.status,
            reason(resp.status),
            resp.body.len(),
        );
        stream
            .write_all(head.as_bytes())
            .await
            .map_err(|e| err("CACHE-HTTP", format!("write: {e}")))?;
        stream
            .write_all(&resp.body)
            .await
            .map_err(|e| err("CACHE-HTTP", format!("write body: {e}")))?;
        // The drop closes the socket — the client's `read_to_end` completes.
        Ok(())
    }
}

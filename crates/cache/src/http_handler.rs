//! SPEC-12 / §11.2: an HTTP `CacheHandler` for multi-node deployments.
//!
//! One node runs the cache service (or a plain HTTP store fronts it); every
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

use crate::{Entry, Result, auth, codec, err};
use async_trait::async_trait;

/// An HTTP-backed `CacheHandler`.
pub struct HttpCacheHandler {
    base: String,
}

impl HttpCacheHandler {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
        }
    }

    /// `(status, body)` for one HTTP/1.1 exchange. A minimal client — the
    /// cache protocol is a handful of verbs on a URL.
    async fn request(
        &self,
        method: &str,
        url: &str,
        body: Option<&[u8]>,
    ) -> std::result::Result<(u16, Vec<u8>), String> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let rest = url
            .strip_prefix("http://")
            .ok_or_else(|| format!("http-only: {url}"))?;
        let (host_port, path) = rest.split_once('/').unwrap_or((rest, ""));
        let mut stream = tokio::net::TcpStream::connect(host_port)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        let b = body.unwrap_or(&[]);
        let req = format!(
            "{method} /{path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
            b.len()
        );
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|e| format!("write: {e}"))?;
        if !b.is_empty() {
            stream
                .write_all(b)
                .await
                .map_err(|e| format!("write body: {e}"))?;
        }
        let mut buf = Vec::new();
        stream
            .read_to_end(&mut buf)
            .await
            .map_err(|e| format!("read: {e}"))?;
        let split = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or_else(|| "no header terminator".to_string())?;
        let head = String::from_utf8_lossy(&buf[..split]);
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| "no status".to_string())?;
        Ok((status, buf[split + 4..].to_vec()))
    }
}

#[async_trait]
impl crate::CacheHandler for HttpCacheHandler {
    async fn get(&self, key: &str) -> Result<Option<Entry>> {
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
            Ok(_) | Err(_) => Ok(None),
        }
    }

    async fn set(&self, key: &str, entry: Entry) -> Result<()> {
        let url = format!("{}/entry/{key}", self.base);
        // §11.3 / P0-8: seal the envelope. Mirrors redis_handler.rs and
        // s3_handler.rs — a forged entry (an attacker with L2 write
        // access who just omits the MAC) is rejected on read.
        let body = auth::seal_entry(&codec::encode_entry(&entry)?);
        match self.request("PUT", &url, Some(&body)).await {
            Ok((s, _)) if (200..300).contains(&s) => Ok(()),
            Ok((s, _)) => Err(err("CACHE-HTTP", format!("set returned {s}"))),
            Err(e) => Err(err("CACHE-HTTP", e)),
        }
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let url = format!("{}/entry/{key}", self.base);
        let _ = self.request("DELETE", &url, None).await;
        Ok(())
    }

    async fn tag_keys(&self, tag: &str) -> Result<Vec<String>> {
        let url = format!("{}/tag/{tag}", self.base);
        match self.request("GET", &url, None).await {
            Ok((200, body)) => Ok(serde_json::from_slice(&body).unwrap_or_default()),
            _ => Ok(Vec::new()),
        }
    }

    async fn invalidate_tag(&self, tag: &str, soft: bool) -> Result<usize> {
        let url = format!("{}/invalidate/{tag}", self.base);
        let body = serde_json::to_vec(&serde_json::json!({ "soft": soft })).unwrap_or_default();
        match self.request("POST", &url, Some(&body)).await {
            Ok((200, b)) => Ok(serde_json::from_slice::<serde_json::Value>(&b)
                .ok()
                .and_then(|v| v.get("count").and_then(|c| c.as_u64()))
                .unwrap_or(0) as usize),
            _ => Ok(0),
        }
    }

    async fn publish_invalidation(&self, tag: &str, soft: bool) -> Result<()> {
        // The invalidate POST *is* the publish — peers hitting the same
        // service see the tag gone on their next fetch.
        let _ = self.invalidate_tag(tag, soft).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// SPEC-12: HttpCacheHandler round-trips an Entry and publishes a tag
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
            policy: Default::default(),
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
    /// audit found HttpCacheHandler missing the seal entirely — a
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
            policy: Default::default(),
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
            policy: Default::default(),
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

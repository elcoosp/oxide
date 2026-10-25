//! oxide-fw-graph — demand-driven incremental engine + content-addressed store (spec §5, D6).
//!
//! Tier A: in-memory demand graph with fine-grained dependency tracking and early cutoff.
//! Tier B: persistent CAS (`blake3`, `redb` index, `zstd` blobs, rkyv-ready artifacts).
//! Tracked inputs: file contents (by blake3), directory listings, resolver results,
//! env vars, config keys, plugin options, tool versions (§5.2).
//!
//! Layout: [`demand`] holds the Tier A engine, [`cas`] the Tier B store,
//! [`trackers`] the concrete input trackers handed to computations.

pub mod artifacts;
#[cfg(feature = "s3")]
pub mod remote_s3;

mod cas;
mod demand;
mod trackers;

pub use cas::{CasKey, CasWriteMeta, ContentStore, ContentStoreExt};
pub use demand::DemandGraph;
pub use trackers::{ConfigTrackerImpl, EnvTrackerImpl, FsTrackerImpl};

use dashmap::DashMap;
use oxide_fw_diagnostics::{Diagnostic, OxideError};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub type Result<T> = std::result::Result<T, OxideError>;

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Box::new(Diagnostic::error(code, msg)))
}

/// A node's stable key, e.g. `transform:<file>:<version>` or `chunk:<hash>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeKey(pub Arc<str>);

impl NodeKey {
    pub fn new(s: impl AsRef<str>) -> Self {
        Self(Arc::from(s.as_ref()))
    }
}

/// What a node read while computing — recorded as graph edges (§5.2).
///
/// `Ord` is implemented manually (`blake3::Hash` has no `Ord`) by comparing the
/// discriminant, then the identity fields, then hash bytes — this gives the
/// deterministic `BTreeSet<Input>` ordering the workspace conventions require.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Input {
    /// File content hash (blake3).
    File { path: PathBuf, hash: blake3::Hash },
    /// Directory listing (names + mtimes hash).
    DirListing {
        path: PathBuf,
        listing_hash: blake3::Hash,
    },
    /// Environment variable observed.
    Env {
        key: String,
        value_hash: blake3::Hash,
    },
    /// Config key observed (dotted path).
    ConfigKey {
        key: String,
        value_hash: blake3::Hash,
    },
    /// Tool version (compiler version strings invalidate transforms).
    ToolVersion { name: String, version: String },
    /// Another node's output hash (node dependency).
    Node(NodeKey, blake3::Hash),
    /// SPEC-5B: a bare module specifier was resolved to a concrete path.
    /// Hash covers `resolved-path + package-version + matched conditions`.
    Resolver {
        specifier: String,
        from: String,
        resolved_hash: blake3::Hash,
    },
    /// SPEC-5B: a plugin's options (from config) were observed by a
    /// transform node. Changing the options invalidates the plugin's outputs.
    PluginOptions {
        plugin: String,
        options_hash: blake3::Hash,
    },
}

impl PartialOrd for Input {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Input {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        fn tag(i: &Input) -> u8 {
            match i {
                Input::File { .. } => 0,
                Input::DirListing { .. } => 1,
                Input::Env { .. } => 2,
                Input::ConfigKey { .. } => 3,
                Input::ToolVersion { .. } => 4,
                Input::Node(..) => 5,
                Input::Resolver { .. } => 6,
                Input::PluginOptions { .. } => 7,
            }
        }
        fn hash_bytes(h: &blake3::Hash) -> [u8; 32] {
            *h.as_bytes()
        }
        match (self, other) {
            (Input::File { path: a, hash: ha }, Input::File { path: b, hash: hb }) => {
                a.cmp(b).then(hash_bytes(ha).cmp(&hash_bytes(hb)))
            }
            (
                Input::DirListing {
                    path: a,
                    listing_hash: ha,
                },
                Input::DirListing {
                    path: b,
                    listing_hash: hb,
                },
            ) => a.cmp(b).then(hash_bytes(ha).cmp(&hash_bytes(hb))),
            (
                Input::Env {
                    key: a,
                    value_hash: ha,
                },
                Input::Env {
                    key: b,
                    value_hash: hb,
                },
            ) => a.cmp(b).then(hash_bytes(ha).cmp(&hash_bytes(hb))),
            (
                Input::ConfigKey {
                    key: a,
                    value_hash: ha,
                },
                Input::ConfigKey {
                    key: b,
                    value_hash: hb,
                },
            ) => a.cmp(b).then(hash_bytes(ha).cmp(&hash_bytes(hb))),
            (
                Input::ToolVersion {
                    name: a,
                    version: va,
                },
                Input::ToolVersion {
                    name: b,
                    version: vb,
                },
            ) => a.cmp(b).then(va.cmp(vb)),
            (Input::Node(a, ha), Input::Node(b, hb)) => {
                a.cmp(b).then(hash_bytes(ha).cmp(&hash_bytes(hb)))
            }
            (
                Input::Resolver {
                    specifier: sa,
                    from: fa,
                    resolved_hash: ha,
                },
                Input::Resolver {
                    specifier: sb,
                    from: fb,
                    resolved_hash: hb,
                },
            ) => sa
                .cmp(sb)
                .then(fa.cmp(fb))
                .then(hash_bytes(ha).cmp(&hash_bytes(hb))),
            (
                Input::PluginOptions {
                    plugin: pa,
                    options_hash: ha,
                },
                Input::PluginOptions {
                    plugin: pb,
                    options_hash: hb,
                },
            ) => pa.cmp(pb).then(hash_bytes(ha).cmp(&hash_bytes(hb))),
            _ => tag(self).cmp(&tag(other)),
        }
    }
}

/// Computed node output: content hash + optional stored artifact key in the CAS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeOutput {
    pub hash: blake3::Hash,
    pub artifact: Option<CasKey>,
    /// Dependencies read during the last computation.
    pub deps: BTreeSet<Input>,
}

/// A pure computation over the graph.
pub trait Compute: Send + Sync {
    fn compute(&self, ctx: &mut ComputeCtx) -> Result<NodeOutput>;
}

/// Context handed to computations; every read is recorded as a dependency.
///
/// The `fs`/`env`/`config` fields are trait objects whose concrete values are
/// recording wrappers created per computation (see [`demand`]); reads through
/// them append to the node's dependency set.
pub struct ComputeCtx<'a> {
    pub fs: &'a mut dyn FileTracker,
    pub env: &'a mut dyn EnvTracker,
    pub config: &'a mut dyn ConfigTracker,
    pub node_outputs: &'a DashMap<NodeKey, NodeOutput>,
    /// Cancellation state shared with the driving [`DemandGraph`] (§5.6).
    pub cancel: &'a CancelToken,
    /// The graph this computation runs on — used by [`ComputeCtx::dep`] to
    /// demand-compute upstream nodes.
    pub graph: &'a DemandGraph,
    /// Dependencies recorded so far for the running computation.
    pub(crate) deps: &'a RefCell<BTreeSet<Input>>,
}

impl<'a> ComputeCtx<'a> {
    /// Read another node's output, recording an `Input::Node` edge (§5.2).
    ///
    /// Memo-fresh nodes are returned directly; missing ones are demand-computed
    /// through the graph's compute registry ([`DemandGraph::register`]). Cycles
    /// are rejected with `GRAPH-CYCLE`.
    pub fn dep(&self, key: &NodeKey) -> Result<Arc<NodeOutput>> {
        self.graph.dep_from_ctx(self, key)
    }

    /// Cooperative cancellation check (§5.6): call between computation steps;
    /// returns `GRAPH-CANCEL` once the token has been cancelled.
    pub fn check_cancel(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(err(
                "GRAPH-CANCEL",
                "computation cancelled: a newer change invalidated in-flight work",
            ))
        } else {
            Ok(())
        }
    }

    /// Record a tool-version input (§5.2): call when the computation depends on
    /// an external tool (compiler, bundler, …) identified by `name` at
    /// `version`. A later [`DemandGraph::invalidate`] naming the same tool
    /// invalidates this node.
    pub fn tool_version(&mut self, name: &str, version: &str) {
        self.deps.borrow_mut().insert(Input::ToolVersion {
            name: name.to_string(),
            version: version.to_string(),
        });
    }

    /// Record a resolver edge (§5.2 / SPEC-5B): the computation resolved
    /// `specifier` from `from` to something whose identity hashes to
    /// `resolved_hash` (path + package version + matched conditions). A later
    /// [`DemandGraph::invalidate`] with a matching `Input::Resolver` (e.g. a
    /// lockfile bump that changes the resolved package) invalidates this node.
    pub fn record_resolver(&mut self, specifier: &str, from: &str, resolved_hash: blake3::Hash) {
        self.deps.borrow_mut().insert(Input::Resolver {
            specifier: specifier.to_string(),
            from: from.to_string(),
            resolved_hash,
        });
    }

    /// Record a plugin-options edge (§5.2 / SPEC-5B): the computation depends
    /// on the JSON options a plugin was configured with. A change to the
    /// plugin's options invalidates this node.
    pub fn record_plugin_options(&mut self, plugin: &str, options_hash: blake3::Hash) {
        self.deps.borrow_mut().insert(Input::PluginOptions {
            plugin: plugin.to_string(),
            options_hash,
        });
    }
}

pub trait FileTracker {
    /// Read a file's bytes and record `Input::File`.
    fn read_file(&mut self, path: &Path) -> Result<Vec<u8>>;
    /// List a directory and record `Input::DirListing`.
    fn list_dir(&mut self, path: &Path) -> Result<Vec<PathBuf>>;
}

pub trait EnvTracker {
    /// Read an env var and record `Input::Env`.
    fn get(&mut self, key: &str) -> Option<String>;
}

pub trait ConfigTracker {
    /// Read a config key and record `Input::ConfigKey`.
    fn get(&mut self, key: &str) -> Option<serde_json::Value>;
}

/// Cancellation: a newer file change cancels in-flight stale work (§5.6).
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<std::sync::atomic::AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
    /// Clear the cancelled state so fresh gets are accepted again (a dev server
    /// does this when it starts a new build window).
    pub fn reset(&self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// RemoteCache trait (§5.4): HTTP/S3 for CI; read-through by default,
/// write on trusted branches.
#[async_trait::async_trait]
pub trait RemoteCache: Send + Sync {
    async fn fetch(&self, key: &CasKey) -> Result<Option<Vec<u8>>>;
    async fn store(&self, key: &CasKey, data: &[u8]) -> Result<()>;
}

/// SPEC-04 / §5.4: an HTTP remote cache. `GET {base}/{key-hex}` for a
/// read-through; `PUT` to write. A minimal HTTP/1.1 client over tokio (no
/// hyper dependency) — the cache protocol is two verbs on a URL, so a full
/// client is overkill.
///
/// A non-200 GET is a miss (not an error): the remote may not have the key
/// yet, and a read-through cache must degrade to a local compute. A network
/// failure is a miss too — the build proceeds.
pub struct HttpRemoteCache {
    base: String,
}

impl HttpRemoteCache {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_string(),
        }
    }

    /// Minimal one-request HTTP/1.1 exchange. Returns `(status, body)`.
    async fn request(
        &self,
        method: &str,
        key: &CasKey,
        body: Option<&[u8]>,
    ) -> Result<(u16, Vec<u8>)> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let url = format!("{}/{}.bin", self.base, key.hex());
        let rest = url.strip_prefix("http://").ok_or_else(|| {
            err(
                "CAS-REMOTE",
                format!("remote cache URL must be http:// ({url})"),
            )
        })?;
        let (host_port, path) = rest.split_once('/').unwrap_or((rest, ""));
        let path = format!("/{path}");
        let mut stream = tokio::net::TcpStream::connect(host_port)
            .await
            .map_err(|e| err("CAS-REMOTE", format!("connect {host_port}: {e}")))?;
        let body_bytes = body.unwrap_or(&[]);
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
            body_bytes.len()
        );
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|e| err("CAS-REMOTE", format!("write request: {e}")))?;
        if !body_bytes.is_empty() {
            stream
                .write_all(body_bytes)
                .await
                .map_err(|e| err("CAS-REMOTE", format!("write body: {e}")))?;
        }
        let mut buf = Vec::new();
        stream
            .read_to_end(&mut buf)
            .await
            .map_err(|e| err("CAS-REMOTE", format!("read response: {e}")))?;
        let split = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or_else(|| err("CAS-REMOTE", "response missing header terminator"))?;
        let head = String::from_utf8_lossy(&buf[..split]);
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| err("CAS-REMOTE", "response missing status"))?;
        Ok((status, buf[split + 4..].to_vec()))
    }
}

#[async_trait::async_trait]
impl RemoteCache for HttpRemoteCache {
    async fn fetch(&self, key: &CasKey) -> Result<Option<Vec<u8>>> {
        match self.request("GET", key, None).await {
            Ok((200, body)) => Ok(Some(body)),
            // Any other status / a network error is a miss.
            _ => Ok(None),
        }
    }
    async fn store(&self, key: &CasKey, data: &[u8]) -> Result<()> {
        // A failed write is logged, not fatal: the local build already
        // produced the artifact; the remote is an optimization.
        match self.request("PUT", key, Some(data)).await {
            Ok((s, _)) if (200..300).contains(&s) => Ok(()),
            Ok((s, _)) => Err(err("CAS-REMOTE", format!("remote store returned {s}"))),
            Err(e) => Err(e),
        }
    }
}

/// In-memory [`RemoteCache`] used for tests and as a read-through fallback.
#[derive(Default)]
pub struct MemoryRemoteCache {
    entries: std::sync::Mutex<std::collections::BTreeMap<CasKey, Vec<u8>>>,
}

#[async_trait::async_trait]
impl RemoteCache for MemoryRemoteCache {
    async fn fetch(&self, key: &CasKey) -> Result<Option<Vec<u8>>> {
        Ok(self.entries.lock().unwrap().get(key).cloned())
    }
    async fn store(&self, key: &CasKey, data: &[u8]) -> Result<()> {
        self.entries
            .lock()
            .unwrap()
            .insert(key.clone(), data.to_vec());
        Ok(())
    }
}

/// Minimal executor for immediately-ready futures in tests (no tokio dep; the
/// in-memory remote cache never yields `Pending`).
#[cfg(test)]
mod spec04_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// SPEC-04: HttpRemoteCache round-trips against a tiny in-process HTTP
    /// server (PUT stores, GET returns 200 + body).
    #[tokio::test]
    async fn http_remote_cache_roundtrips() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // A one-thread mock: PUT stores the body, GET returns it (404 if absent).
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
                // Read headers.
                let header_end = loop {
                    let n = s.read(&mut tmp).unwrap_or(0);
                    if n == 0 {
                        break None;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(i);
                    }
                };
                let Some(he) = header_end else { continue };
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
                let path = reqline.split_whitespace().nth(1).unwrap_or("").to_string();
                let method = reqline.split_whitespace().next().unwrap_or("").to_string();
                let mut store = store2.lock().unwrap();
                if method == "PUT" {
                    store.insert(path, body);
                    let _ = s.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                } else if let Some(b) = store.get(&path) {
                    let h = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        b.len()
                    );
                    let _ = s.write_all(h.as_bytes());
                    let _ = s.write_all(b);
                } else {
                    let _ = s.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                }
            }
        });

        let rc = HttpRemoteCache::new(format!("http://{addr}"));
        let key = CasKey::of(b"hello");
        // Miss before store.
        assert!(rc.fetch(&key).await.unwrap().is_none());
        // Store, then hit.
        rc.store(&key, b"payload").await.unwrap();
        assert_eq!(rc.fetch(&key).await.unwrap(), Some(b"payload".to_vec()));
    }
}

#[cfg(test)]
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    use std::pin::pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};
    struct NoopWake;
    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }
    let waker = Waker::from(Arc::new(NoopWake));
    let mut cx = Context::from_waker(&waker);
    let mut fut = pin!(fut);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            // The MemoryRemoteCache completes synchronously; a real remote
            // driver would park on a woken reactor here.
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_remote_cache_roundtrip() {
        let cache = MemoryRemoteCache::default();
        let key = CasKey::of(b"artifact");
        assert!(block_on(cache.fetch(&key)).unwrap().is_none());
        block_on(cache.store(&key, b"artifact")).unwrap();
        assert_eq!(
            block_on(cache.fetch(&key)).unwrap().as_deref(),
            Some(b"artifact".as_slice())
        );
    }

    #[test]
    fn cancel_token_lifecycle() {
        let t = CancelToken::new();
        assert!(!t.is_cancelled());
        t.cancel();
        assert!(t.is_cancelled());
        t.reset();
        assert!(!t.is_cancelled());
    }

    #[test]
    fn node_key_is_stable_across_clones() {
        let k = NodeKey::new("chunk:abc");
        let k2 = k.clone();
        assert_eq!(k, k2);
        assert_eq!(k.0.as_ref(), "chunk:abc");
    }
}

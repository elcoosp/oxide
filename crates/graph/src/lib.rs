//! oxide-graph — demand-driven incremental engine + content-addressed store (spec §5, D6).
//!
//! Tier A: in-memory demand graph with fine-grained dependency tracking and early cutoff.
//! Tier B: persistent CAS (`blake3`, `redb` index, `zstd` blobs, rkyv-ready artifacts).
//! Tracked inputs: file contents (by blake3), directory listings, resolver results,
//! env vars, config keys, plugin options, tool versions (§5.2).
//!
//! Layout: [`demand`] holds the Tier A engine, [`cas`] the Tier B store,
//! [`trackers`] the concrete input trackers handed to computations.

mod cas;
mod demand;
mod trackers;

pub use cas::{CasKey, CasWriteMeta, ContentStore, ContentStoreExt};
pub use demand::DemandGraph;
pub use trackers::{ConfigTrackerImpl, EnvTrackerImpl, FsTrackerImpl};

use dashmap::DashMap;
use oxide_diagnostics::{Diagnostic, OxideError};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub type Result<T> = std::result::Result<T, OxideError>;

pub fn err(code: &'static str, msg: impl Into<String>) -> OxideError {
    OxideError(Diagnostic::error(code, msg))
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

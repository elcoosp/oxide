//! The hot-reload seam (the V2-F09 dev-watch follow-up): the serve
//! side picks up a rebuilt `dist/search/index` without a restart.
//!
//! The dev loop re-emits the F09 index on a content change (the
//! emitter's CAS marker skips an unchanged corpus); the running server
//! — which mmap-opened the index once, read-only, at serve start — must
//! then swap the fresh corpus in without dropping a request. The
//! codebase already owns the pattern for exactly this shape: the CLI's
//! `SwappableRouteTree` handle (`Arc<RwLock<Arc<T>>>` snapshot swap,
//! tiny critical sections). [`SwappableSearchIndex`] is that pattern
//! plus the disk-driven reload: the fingerprint is the index
//! directory's `meta.json` bytes, so
//!
//! - an unchanged corpus (the CAS skip left the directory untouched) is
//!   byte-identical — the reload is `Unchanged` and swaps nothing (the
//!   live L1 entries stay valid, see [`SearchIndex::generation`]);
//! - a rebuilt corpus carries fresh segment UUIDs — the bytes differ,
//!   the rebuild is opened read-only and swapped in atomically
//!   (`Swapped { docs }`);
//! - a missing, mid-wipe or corrupt directory refuses with `SRC-101`
//!   and the previous snapshot keeps serving — a failed reload never
//!   degrades the running answers (the zero-downtime posture the serve
//!   wire-up documents).
//!
//! There is deliberately no raw `store`: the only way in is a
//! directory-backed reload, so the serving snapshot always IS the
//! on-disk artifact (a caller cannot swap in an index that does not
//! match the dist).

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use oxide_fw_diagnostics::OxideError;

use crate::build::TantivyIndex;
use crate::err;
use crate::query::{SearchEngineOptions, SearchIndex, SearchQuery, SearchResponse};

/// One [`SwappableSearchIndex::try_reload`] outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// The directory's `meta.json` is byte-identical to the serving
    /// snapshot's (the emitter's CAS skip): nothing swapped, the cache
    /// generation is untouched.
    Unchanged,
    /// A rebuilt index was opened read-only and swapped in; `docs` is
    /// the fresh corpus' document count.
    Swapped {
        /// The swapped-in corpus' document count.
        docs: u32,
    },
}

/// The serving snapshot: the engine plus the fingerprint that decided
/// it. Swapped in and out as one unit under the handle's lock.
struct HotSnapshot {
    index: Arc<dyn SearchIndex>,
    /// The index directory's `meta.json` bytes at open/swap time (the
    /// reload's change signal — fresh segment UUIDs make a rebuild's
    /// bytes differ, a CAS-skipped rebuild's bytes do not).
    fingerprint: String,
    /// The swap ordinal that produced this snapshot (0 = the open).
    /// [`SearchIndex::generation`] reads it; the endpoint's L1 keys
    /// partition on it.
    generation: u64,
}

/// The serve side's hot-swappable engine handle: one read-only snapshot
/// behind an `Arc<RwLock<…>>`, replaced only by [`Self::try_reload`].
///
/// The type itself is the object-safe seam ([`SearchIndex`]) — the
/// endpoint holds it exactly like the plain [`TantivyIndex`]; `search`
/// and `doc_count` clone the current snapshot out of the lock and query
/// it lock-free (a reload never blocks a request longer than the Arc
/// clone).
pub struct SwappableSearchIndex {
    dir: PathBuf,
    opts: SearchEngineOptions,
    inner: Arc<RwLock<HotSnapshot>>,
}

impl SwappableSearchIndex {
    /// Open a built index read-only (the serve wire-up's posture) and
    /// seed the first snapshot.
    ///
    /// # Errors
    ///
    /// `SRC-101` when the directory does not hold an openable index —
    /// the wire-up turns that into the loud warning + not-configured
    /// 404, never a serve-start failure.
    pub fn open(dir: &Path, opts: &SearchEngineOptions) -> Result<Self, OxideError> {
        let (index, fingerprint) = open_snapshot(dir, opts)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            opts: opts.clone(),
            inner: Arc::new(RwLock::new(HotSnapshot {
                index: Arc::new(index),
                fingerprint,
                generation: 0,
            })),
        })
    }

    /// The current snapshot (the CLI's `SwappableRouteTree`-handle
    /// idiom): clone the `Arc` out of the lock, drop the lock, query
    /// lock-free.
    #[must_use]
    pub fn load(&self) -> Arc<dyn SearchIndex> {
        self.lock().index.clone()
    }

    /// Re-read the index directory and swap in a rebuild when the
    /// on-disk `meta.json` differs from the serving snapshot's.
    ///
    /// The change signal is byte-exact: the emitter's CAS marker means
    /// an unchanged corpus leaves the directory untouched (byte-identical
    /// `meta.json` → `Unchanged`), while a rebuild commits fresh segment
    /// UUIDs (different bytes → open + swap). A directory that cannot be
    /// fingerprinted or opened (missing, mid-wipe, corrupt) refuses with
    /// `SRC-101` and the previous snapshot keeps serving — call this
    /// best-effort, exactly like the wire-up's best-effort open.
    ///
    /// # Errors
    ///
    /// `SRC-101` (the engine's open code) when the directory is not an
    /// openable index. The serving answers are never affected.
    pub fn try_reload(&self) -> Result<ReloadOutcome, OxideError> {
        // The fast path: the fingerprint read doubles as the
        // mid-rebuild probe — an unreadable meta.json falls through to
        // the authoritative open, which fails with the same SRC-101.
        if let Ok(current) = std::fs::read_to_string(self.dir.join("meta.json"))
            && current == self.lock().fingerprint
        {
            return Ok(ReloadOutcome::Unchanged);
        }
        let (index, fingerprint) = open_snapshot(&self.dir, &self.opts)?;
        let docs = index.doc_count();
        let mut snapshot = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let generation = snapshot.generation + 1;
        *snapshot = HotSnapshot {
            index: Arc::new(index),
            fingerprint,
            generation,
        };
        tracing::info!(dir = %self.dir.display(), docs, generation, "search index reloaded");
        Ok(ReloadOutcome::Swapped { docs })
    }

    /// The serving snapshot under the read lock (the two `SearchIndex`
    /// reads and the reload's compare all route through here; the
    /// poison policy is the crate's — a poisoned lock unwraps into the
    /// guard, the same posture `endpoint.rs` documents).
    fn lock(&self) -> std::sync::RwLockReadGuard<'_, HotSnapshot> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Open the directory read-only with the options attached and take the
/// fingerprint in the same breath (the caller swaps both under one
/// lock, so the served corpus and its fingerprint can never disagree).
fn open_snapshot(
    dir: &Path,
    opts: &SearchEngineOptions,
) -> Result<(TantivyIndex, String), OxideError> {
    let index = TantivyIndex::open_read_only(dir)?.with_options(opts);
    let fingerprint = std::fs::read_to_string(dir.join("meta.json")).map_err(|e| {
        err(
            "SRC-101",
            format!("search index fingerprint {}: {e}", dir.display()),
        )
    })?;
    Ok((index, fingerprint))
}

impl SearchIndex for SwappableSearchIndex {
    fn search(&self, query: &SearchQuery) -> Result<SearchResponse, OxideError> {
        self.load().search(query)
    }

    fn doc_count(&self) -> u32 {
        self.load().doc_count()
    }

    fn generation(&self) -> u64 {
        self.lock().generation
    }
}

impl std::fmt::Debug for SwappableSearchIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let snapshot = self.lock();
        f.debug_struct("SwappableSearchIndex")
            .field("dir", &self.dir)
            .field("doc_count", &snapshot.index.doc_count())
            .field("generation", &snapshot.generation)
            .finish_non_exhaustive()
    }
}

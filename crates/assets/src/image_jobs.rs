//! The parallel, resumable image job model (spec v0.2, §V2-C09): the
//! per-request transform path behind the images endpoint stops being
//! "every request pays its own encode".
//!
//! # The four pieces the spec names
//!
//! - **The bounded CPU lane** ([`ImageJobs::cpu_permits`]): a
//!   [`tokio::sync::Semaphore`] sized `available_parallelism() - 1` by
//!   default (`ImageJobs::with_lanes` lets the caller size it from
//!   `images.maxConcurrency`), so image CPU work cannot starve render.
//! - **The AVIF lane**: its own *smaller* semaphore (default: half the CPU
//!   lane, never zero) — AVIF encodes are the slowest lane of the pipeline
//!   and the speed knob (`images.avifSpeed`, default 6) trades rate for
//!   quality independently of admission.
//! - **Single-flight per output key**: N concurrent requests for the same
//!   variant share ONE encode — a [`DashMap`] of
//!   `Shared<BoxFuture<...>>` keyed by the module's `cache_key()`; losers
//!   await the leader's outcome instead of running their own job. A failed
//!   (or panicked) job drops its entry, so the next request retries.
//! - **CAS before and after**: the injectable [`ImageCas`] seam is probed
//!   before any work and receives the result after a successful encode
//!   (the existing image cache key already hashes every input; the CLI
//!   adoption wires the real tiered store in).
//!
//! Encoders run in [`tokio::task::spawn_blocking`] — the semaphores gate
//! admission, the blocking pool never runs two encoders the lanes did not
//! admit, and a panicking encoder surfaces as `AST-JOBS` without poisoning
//! the lane or the map.
//!
//! The wiring — the images endpoint constructing `ImageJobs` from config
//! and feeding the tiered CAS — rides the A01-phase-3 CLI adoption (see
//! `docs/deviations.md`); `crates/assets/examples/image_jobs.rs` walks the
//! whole contract against fakes.

use crate::{Diagnostic, OxideError, Result, err};
use dashmap::DashMap;
use futures::future::{FutureExt, Shared};
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// The content-addressed image store the jobs consult before working and
/// fill after succeeding (§V2-C09 step c). Object-safe and lock-free
/// friendly: `get`/`put` are sync — the tiered L0/L1 stores behind the
/// images endpoint answer from memory or disk without an await.
pub trait ImageCas: Send + Sync {
    /// The stored bytes for `key`, when a previous encode landed.
    fn get(&self, key: &str) -> Option<Arc<Vec<u8>>>;
    /// Store the encode result under `key`.
    fn put(&self, key: &str, bytes: Arc<Vec<u8>>);
}

/// An already-shared store satisfies the seam by delegation, so callers
/// hand `Arc<dyn ImageCas>` (or `Arc<MemoryCas>`) to the constructors
/// interchangeably.
impl<T: ImageCas + ?Sized> ImageCas for Arc<T> {
    fn get(&self, key: &str) -> Option<Arc<Vec<u8>>> {
        (**self).get(key)
    }
    fn put(&self, key: &str, bytes: Arc<Vec<u8>>) {
        (**self).put(key, bytes);
    }
}

/// The disk-backed CAS the serve path uses: one file per output key under
/// a root directory (`.oxide/image-cas`). The key is the blake3 hex
/// digest the caller computes, so the filename needs no escaping — the
/// traversal guard is the alphabet itself. `put` writes through a temp
/// file + rename so a concurrent reader never sees a half-written image.
pub struct DirImageCas {
    root: std::path::PathBuf,
}

impl DirImageCas {
    /// A store over `root` (created lazily on the first `put`).
    #[must_use]
    pub fn new(root: std::path::PathBuf) -> Self {
        Self { root }
    }

    fn path_for(&self, key: &str) -> std::path::PathBuf {
        self.root.join(key)
    }
}

impl ImageCas for DirImageCas {
    fn get(&self, key: &str) -> Option<Arc<Vec<u8>>> {
        std::fs::read(self.path_for(key))
            .ok()
            .filter(|b| !b.is_empty())
            .map(Arc::new)
    }

    fn put(&self, key: &str, bytes: Arc<Vec<u8>>) {
        // The CAS is best-effort by contract (a missing variant is a
        // cache miss that re-encodes), but the gap must be VISIBLE — the
        // house convention is loud, not silent — else a disk problem
        // burns the encode lanes on every request invisibly.
        if let Err(e) = std::fs::create_dir_all(&self.root) {
            tracing::warn!(
                code = "AST-JOBS",
                root = %self.root.display(),
                error = %e,
                "the image CAS root could not be created; variants will not persist"
            );
            return;
        }
        let path = self.path_for(key);
        let tmp = self.root.join(format!(".{key}.tmp"));
        if let Err(e) = std::fs::write(&tmp, bytes.as_ref()) {
            tracing::warn!(
                code = "AST-JOBS",
                key = %key,
                error = %e,
                "the image variant's scratch write failed; serving will re-encode"
            );
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            tracing::warn!(
                code = "AST-JOBS",
                key = %key,
                error = %e,
                "the image variant's final rename failed; serving will re-encode"
            );
            // A failed rename leaves the scratch file beside its target —
            // debris the next put overwrites as the scratch name (never
            // read), so this removal is hygiene, not correctness.
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// The shared outcome one encode's waiters all receive.
type JobOutcome = Result<Arc<Vec<u8>>>;

/// One leader's encode: the boxed future whose [`Shared`] handle every
/// waiter for the same output key joins.
type LeaderFuture = std::pin::Pin<Box<dyn Future<Output = JobOutcome> + Send>>;

/// The single-flight map: output key → the leader's shared outcome.
type InflightMap = DashMap<String, Shared<LeaderFuture>>;

/// The parallel, resumable image job model (§V2-C09). Cloneable — the
/// endpoint hands clones to per-request tasks; the lanes and the
/// single-flight map are shared state behind `Arc`s.
#[derive(Clone)]
pub struct ImageJobs {
    cpu: Arc<Semaphore>,
    avif: Arc<Semaphore>,
    /// The configured CPU-lane size (`available_permits` fluctuates with
    /// traffic; the doctor reports the configured bound).
    cpu_size: usize,
    inflight: Arc<InflightMap>,
    cas: Arc<dyn ImageCas>,
}

impl ImageJobs {
    /// A job model over an owned store with the spec's default lanes: the
    /// CPU lane sized `available_parallelism() - 1` (never below one) and
    /// the AVIF lane at half of it (never below one — the smaller lane the
    /// spec names).
    ///
    /// # Panics
    ///
    /// Never: `available_parallelism` falls back to 1 on failure and the
    /// lane math saturates.
    #[must_use]
    pub fn new(cas: impl ImageCas + 'static) -> Self {
        Self::from_shared(Arc::new(cas))
    }

    /// [`ImageJobs::new`] over a store the caller already boxes (the
    /// endpoint's tiered CAS is built as `Arc<dyn ImageCas>` upstream).
    #[must_use]
    pub fn from_shared(cas: Arc<dyn ImageCas>) -> Self {
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        let cpu = cores.saturating_sub(1).max(1);
        Self::with_lanes_shared(cas, cpu, (cpu / 2).max(1))
    }

    /// A job model with explicit lane sizes (the config-sized shape: the
    /// CLI adoption feeds `images.maxConcurrency` here and sizes the AVIF
    /// lane separately).
    #[must_use]
    pub fn with_lanes(
        cas: impl ImageCas + 'static,
        cpu_permits: usize,
        avif_permits: usize,
    ) -> Self {
        Self::with_lanes_shared(Arc::new(cas), cpu_permits, avif_permits)
    }

    /// [`ImageJobs::with_lanes`] over an already-boxed store.
    #[must_use]
    pub fn with_lanes_shared(
        cas: Arc<dyn ImageCas>,
        cpu_permits: usize,
        avif_permits: usize,
    ) -> Self {
        let cpu_size = cpu_permits.max(1);
        Self {
            cpu: Arc::new(Semaphore::new(cpu_size)),
            avif: Arc::new(Semaphore::new(avif_permits.max(1))),
            cpu_size,
            inflight: Arc::new(DashMap::new()),
            cas,
        }
    }

    /// The CPU lane's configured permit count (for `oxide doctor`-style
    /// reporting).
    #[must_use]
    pub fn cpu_permits(&self) -> usize {
        self.cpu_size
    }

    /// Run one image job keyed by `key` (the module's `cache_key()`
    /// spelling): CAS hit answers without work; an in-flight job for the
    /// same key is shared; otherwise the job acquires its lane and runs
    /// in `spawn_blocking`.
    ///
    /// `avif` selects the lane: AVIF encodes are slower and get their own
    /// smaller semaphore so one heavy variant cannot evict every other
    /// image transform from the CPU lane.
    ///
    /// # Errors
    ///
    /// The job's own diagnostic; `AST-JOBS` when the job panicked inside
    /// `spawn_blocking`, or when the image lane is closed (the lanes are
    /// closed only when the whole `ImageJobs` is dropped, which cancels
    /// this future before it polls — the refusal is the honest shape for
    /// the impossible case, not a panic). A failed or panicked job is
    /// NOT cached — its single-flight entry is dropped so the next
    /// request retries.
    ///
    /// # Panics
    ///
    /// Never: a closed lane is the coded `AST-JOBS` refusal, not a panic.
    pub async fn run<F>(&self, key: &str, avif: bool, job: F) -> JobOutcome
    where
        F: FnOnce() -> Result<Vec<u8>> + Send + 'static,
    {
        // (c) CAS lookup before work.
        if let Some(bytes) = self.cas.get(key) {
            tracing::debug!(key, "image job CAS hit");
            return Ok(bytes);
        }

        let key = key.to_string();
        // Single-flight: build the leader future lazily, then insert its
        // shared handle. The future does not run until first polled, so
        // constructing it under the shard lock is safe; the guard is
        // dropped BEFORE awaiting (a held shard lock across an await
        // deadlocks every other key in the same shard).
        let lane = Arc::clone(if avif { &self.avif } else { &self.cpu });
        let cas = Arc::clone(&self.cas);
        let job_key = key.clone();
        let leader = async move {
            // The lanes are never closed (the semaphores live as long as
            // the `ImageJobs`), so this arm is the honest coded refusal
            // for the impossible case — a diagnostic, not a panic in the
            // leader future the pipeline awaits.
            let _permit = match lane.acquire().await {
                Ok(permit) => permit,
                Err(closed) => {
                    return Err(err(
                        "AST-JOBS",
                        format!("the image lane is closed; the encoder cannot run: {closed}"),
                    ));
                }
            };
            let outcome = tokio::task::spawn_blocking(job).await;
            match outcome {
                Ok(Ok(bytes)) => {
                    let bytes = Arc::new(bytes);
                    cas.put(&job_key, Arc::clone(&bytes));
                    tracing::debug!(key = %job_key, "image job encoded");
                    Ok(bytes)
                }
                Ok(Err(e)) => Err(e),
                Err(join) => Err(err(
                    "AST-JOBS",
                    format!("the image encoder panicked: {join}"),
                )),
            }
        };

        let entry = self
            .inflight
            .entry(key.clone())
            .or_insert_with(|| leader.boxed().shared());
        // Deref the guard: clone the shared handle, then release the
        // shard lock BEFORE awaiting (a held lock across an await would
        // deadlock every other key in the same shard).
        let shared = (*entry).clone();
        drop(entry);

        let outcome = shared.await;
        // Failed encodes (and panics) must not stay cached in flight: the
        // next request retries. Successes keep the entry — later waiters
        // share the resolved output until the CAS answers first anyway.
        if outcome.is_err() {
            self.inflight.remove(&key);
        }
        outcome
    }

    /// How many jobs are currently in flight (single-flight entries),
    /// for the doctor/metrics surface.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.inflight.len()
    }
}

/// A convenience constructor the pins use to shape a failing job: the
/// same diagnostic vocabulary every other backend error carries.
#[must_use]
pub fn job_error(code: &'static str, msg: impl Into<String>) -> OxideError {
    let mut d = Diagnostic::error(code, msg);
    d.fix = Some("inspect the encoder diagnostic; the next request retries the encode".into());
    OxideError(Box::new(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MapCas(std::sync::Mutex<std::collections::BTreeMap<String, Arc<Vec<u8>>>>);
    impl Default for MapCas {
        fn default() -> Self {
            Self(std::sync::Mutex::new(std::collections::BTreeMap::new()))
        }
    }
    impl ImageCas for MapCas {
        fn get(&self, key: &str) -> Option<Arc<Vec<u8>>> {
            self.0.lock().unwrap().get(key).cloned()
        }
        fn put(&self, key: &str, bytes: Arc<Vec<u8>>) {
            self.0.lock().unwrap().insert(key.to_string(), bytes);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn defaults_keep_one_lane_per_core_boundary() {
        // With one core (or an unknown core count) the CPU lane never
        // drops to zero permits and the AVIF lane is never zero either.
        let jobs = ImageJobs::new(MapCas::default());
        assert!(jobs.cpu_permits() >= 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn job_error_carries_the_fix_hint() {
        let e = job_error("AST-IMG", "boom");
        assert_eq!(e.0.code, "AST-IMG");
        assert!(e.0.fix.is_some());
    }
}

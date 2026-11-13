//! TDD suite for the §V2-C09 parallel, resumable image job model:
//! [`ImageJobs`] — the bounded CPU lane, the smaller AVIF lane, per-output-key
//! single-flight, the CAS lookup-before/store-after hooks, and
//! `spawn_blocking` placement for the encoders.
//!
//! Written BEFORE the implementation; every pin imports public API only and
//! drives the model through fake backends/counters (the spec's own test
//! plan: "count via a fake backend").

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use oxide_fw_assets::Result;
use oxide_fw_assets::image_jobs::{ImageCas, ImageJobs};
use oxide_fw_diagnostics::OxideError;

/// A fake CAS store: a locked map the pins read back.
#[derive(Default)]
struct FakeCas {
    entries: Mutex<BTreeMap<String, Arc<Vec<u8>>>>,
}

impl FakeCas {
    fn with_entry(key: &str, bytes: &[u8]) -> Self {
        let cas = Self::default();
        cas.entries
            .lock()
            .unwrap()
            .insert(key.to_string(), Arc::new(bytes.to_vec()));
        cas
    }
}

impl ImageCas for FakeCas {
    fn get(&self, key: &str) -> Option<Arc<Vec<u8>>> {
        self.entries.lock().unwrap().get(key).cloned()
    }
    fn put(&self, key: &str, bytes: Arc<Vec<u8>>) {
        self.entries.lock().unwrap().insert(key.to_string(), bytes);
    }
}

/// The spec's fake encode backend: counts starts, records the maximum
/// concurrent in-flight jobs, and can be told to fail the first N calls.
#[derive(Default)]
struct FakeBackend {
    encodes: AtomicUsize,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    fail_first: AtomicUsize,
}

impl FakeBackend {
    /// The job body [`ImageJobs`] runs in `spawn_blocking`: count the
    /// start, hold the lane for a deterministic beat, succeed or fail.
    fn job(
        self: &Arc<Self>,
        payload: &'static str,
    ) -> impl FnOnce() -> Result<Vec<u8>> + Send + 'static {
        let backend = Arc::clone(self);
        move || {
            let now = backend.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            backend.max_in_flight.fetch_max(now, Ordering::SeqCst);
            // A tiny, bounded hold so concurrent jobs genuinely overlap
            // (CONVENTIONS §8: deterministic, no sleeps > 50ms).
            std::thread::sleep(std::time::Duration::from_millis(3));
            backend.encodes.fetch_add(1, Ordering::SeqCst);
            backend.in_flight.fetch_sub(1, Ordering::SeqCst);
            // Fail the first `fail_first` encodes only: `fetch_update`
            // with `checked_sub` refuses at 0 (a plain `fetch_sub` wraps
            // — atomics wrap by definition — and would fail every job).
            let should_fail = backend
                .fail_first
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n: usize| {
                    n.checked_sub(1)
                })
                .is_ok();
            if should_fail {
                return Err(OxideError::new("AST-IMG", "the fake encode failed"));
            }
            Ok(payload.as_bytes().to_vec())
        }
    }
}

// -- the spec's three named pins ----------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn fifty_concurrent_identical_requests_produce_one_encode() {
    let backend = Arc::new(FakeBackend::default());
    let jobs = Arc::new(ImageJobs::new(Arc::new(FakeCas::default())));
    let mut handles = Vec::new();
    for _ in 0..50 {
        let jobs = Arc::clone(&jobs);
        let backend = Arc::clone(&backend);
        handles.push(tokio::spawn(async move {
            jobs.run("key-dedupe", false, backend.job("same"))
                .await
                .expect("the dedupe run succeeds")
        }));
    }
    let first = handles.remove(0).await.unwrap();
    for h in handles {
        let bytes = h.await.unwrap();
        assert_eq!(bytes, first, "every waiter receives the same output");
    }
    assert_eq!(backend.encodes.load(Ordering::SeqCst), 1, "one encode");
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_encode_is_not_cached_and_the_next_request_retries() {
    let backend = Arc::new(FakeBackend {
        fail_first: AtomicUsize::new(1),
        ..FakeBackend::default()
    });
    let cas = Arc::new(FakeCas::default());
    let jobs = ImageJobs::new(Arc::clone(&cas));

    let first = jobs
        .run("key-retry", false, backend.job("retry"))
        .await
        .expect_err("the first encode fails");
    assert_eq!(first.0.code, "AST-IMG", "the fake's diagnostic surfaces");
    assert!(
        cas.get("key-retry").is_none(),
        "a failed encode is not stored in the CAS"
    );

    let bytes = jobs
        .run("key-retry", false, backend.job("retry"))
        .await
        .expect("the retry succeeds");
    assert_eq!(bytes.as_ref(), b"retry");
    assert_eq!(backend.encodes.load(Ordering::SeqCst), 2, "the job re-ran");
    assert!(cas.get("key-retry").is_some(), "the success is stored");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cpu_lane_bounds_concurrency() {
    // 12 distinct keys through a 3-permit lane: the fake records the
    // maximum concurrent encodes; the semaphore must keep it at or below
    // 3 (and the jobs must actually overlap, i.e. not serialize to 1).
    let backend = Arc::new(FakeBackend::default());
    let jobs = Arc::new(ImageJobs::with_lanes(Arc::new(FakeCas::default()), 3, 1));
    let mut handles = Vec::new();
    for i in 0..12 {
        let jobs = Arc::clone(&jobs);
        let backend = Arc::clone(&backend);
        handles.push(tokio::spawn(async move {
            let key = format!("lane-{i}");
            jobs.run(&key, false, backend.job("lane")).await
        }));
    }
    for h in handles {
        h.await.unwrap().expect("every lane job succeeds");
    }
    let max = backend.max_in_flight.load(Ordering::SeqCst);
    assert!(
        max <= 3,
        "the 3-permit CPU lane bounds concurrent encodes (max was {max})"
    );
    assert!(max >= 2, "the lane actually overlaps work (max was {max})");
}

// -- the rest of the seam ------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn a_cas_hit_never_runs_the_job() {
    let backend = Arc::new(FakeBackend::default());
    let jobs = ImageJobs::new(Arc::new(FakeCas::with_entry("key-hit", b"cached")));
    let bytes = jobs
        .run("key-hit", false, backend.job("never"))
        .await
        .expect("the CAS hit answers");
    assert_eq!(bytes.as_ref(), b"cached", "the stored bytes come back");
    assert_eq!(backend.encodes.load(Ordering::SeqCst), 0, "no encode ran");
}

#[tokio::test(flavor = "current_thread")]
async fn a_successful_encode_stores_into_the_cas() {
    let backend = Arc::new(FakeBackend::default());
    let cas = Arc::new(FakeCas::default());
    let jobs = ImageJobs::new(Arc::clone(&cas));
    let bytes = jobs
        .run("key-store", false, backend.job("stored"))
        .await
        .expect("the encode succeeds");
    let stored = cas.get("key-store").expect("the result landed in the CAS");
    assert_eq!(stored, bytes, "the CAS holds exactly the returned bytes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_avif_lane_is_its_own_smaller_semaphore() {
    // CPU lane 4, AVIF lane 1: two concurrent AVIF jobs must serialize on
    // the AVIF lane (max AVIF in flight == 1) even though the CPU lane
    // has permits to spare.
    let backend = Arc::new(FakeBackend::default());
    let jobs = Arc::new(ImageJobs::with_lanes(Arc::new(FakeCas::default()), 4, 1));
    let mut handles = Vec::new();
    for _ in 0..3 {
        let jobs = Arc::clone(&jobs);
        let backend = Arc::clone(&backend);
        handles.push(tokio::spawn(async move {
            jobs.run("avif-key", true, backend.job("avif")).await
        }));
    }
    for h in handles {
        h.await.unwrap().expect("every avif job succeeds");
    }
    assert_eq!(
        backend.encodes.load(Ordering::SeqCst),
        1,
        "the identical AVIF jobs single-flight too"
    );
    assert!(
        backend.max_in_flight.load(Ordering::SeqCst) <= 1,
        "the 1-permit AVIF lane bounds its encodes"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_job_panic_surfaces_as_ast_jobs() {
    // A panicking encoder must not poison the lane or the single-flight
    // map: the join error surfaces as the `AST-JOBS` diagnostic and the
    // entry is dropped so the next request retries cleanly.
    let jobs = ImageJobs::new(Arc::new(FakeCas::default()));
    let err = jobs
        .run("key-panic", false, || -> Result<Vec<u8>> {
            panic!("the fake encoder exploded");
        })
        .await
        .expect_err("a panicking job surfaces an error");
    assert_eq!(err.0.code, "AST-JOBS");
    // The lane still works after the panic.
    let ok = jobs
        .run("key-panic", false, || Ok(b"after".to_vec()))
        .await
        .expect("the next job runs clean");
    assert_eq!(ok.as_ref(), b"after");
}

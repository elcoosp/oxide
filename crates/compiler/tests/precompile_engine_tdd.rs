//! TDD suite for the §V2-B07 parallel-precompile engine
//! (`crates/compiler/src/precompile_engine.rs`) — phase 2 of the WP: the
//! bounded-concurrency compile of the independent jobs phase 1 discovered.
//!
//! The engine lives in `oxide-fw-compiler` (V2.0 item 3: pure logic does
//! not pay the V8 link cost) and is generic over the per-job compile pass,
//! so the CLI's `compile_one` (cache lookup + tooling-pool call) drops in
//! unchanged and the pins here run against fakes in seconds.
//!
//! Written BEFORE the implementation; every pin imports public API only.
//! The pins drive the spec's three named behaviors: determinism under
//! out-of-order completion (`precompile_is_deterministic`), the overlap
//! window (`precompile_overlaps_calls`, at least 4 in flight on 20 files
//! with the spec's `workers * 2` sizing) and failure isolation
//! (`precompile_failure_isolated` — one failing file does not drop the
//! others).

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use oxide_fw_compiler::precompile_engine::{PrecompileJob, compile_jobs, width_for};

/// A deterministic pseudo-latency per path: the fake completes in a
/// different order than the input order, so the pins exercise the
/// unordered collection the bounded window implies.
fn pseudo_latency(path: &std::path::Path) -> u64 {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("x");
    name.bytes().map(u64::from).sum::<u64>() % 25 + 1
}

fn jobs(n: usize) -> Vec<PrecompileJob> {
    (0..n)
        .map(|i| PrecompileJob {
            path: PathBuf::from(format!("src/routes/comp-{i:03}.vue")),
            kind: "vue",
            source: format!("<template><p>{i}</p></template>"),
        })
        .collect()
}

/// The in-flight tracker the overlap pins read: a counter the fake bumps
/// while its future is parked on the fake latency.
#[derive(Default)]
struct Flight {
    current: AtomicUsize,
    max: AtomicUsize,
}

impl Flight {
    fn enter(&self) {
        let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.max.fetch_max(now, Ordering::SeqCst);
    }
    fn leave(&self) {
        self.current.fetch_sub(1, Ordering::SeqCst);
    }
    fn peak(&self) -> usize {
        self.max.load(Ordering::SeqCst)
    }
}

/// Run one batch through the engine with the fake pass: every job sleeps
/// its pseudo-latency (out-of-order completion), then returns its JS —
/// except the job at `fail_idx`, which errs (the failure-isolation pin).
async fn run_batch(
    batch: Vec<PrecompileJob>,
    width: usize,
    flight: &Flight,
    fail_idx: Option<usize>,
) -> Vec<(PathBuf, Result<String, String>)> {
    compile_jobs(batch, width, |job| {
        // `file_stem` ("comp-005"), not `file_name` — the suffix would
        // break the parse and no job would ever err.
        let idx = job
            .path
            .file_stem()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("comp-"))
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(usize::MAX);
        async move {
            flight.enter();
            tokio::time::sleep(Duration::from_millis(pseudo_latency(&job.path))).await;
            let out = if Some(idx) == fail_idx {
                Err(format!("compile failed for {}", job.path.display()))
            } else {
                Ok(format!("js for {}", job.path.display()))
            };
            flight.leave();
            out
        }
    })
    .await
}

#[tokio::test]
async fn precompile_is_deterministic() {
    // §V2-B07: "run twice, equal maps, with a fake pool that completes
    // jobs in random order" — the engine's output rows follow the INPUT
    // order (the caller folds them into its map), so out-of-order
    // completion never changes the observable result.
    let flight = Flight::default();
    let first = run_batch(jobs(20), width_for(2), &flight, None).await;
    let second = run_batch(jobs(20), width_for(2), &flight, None).await;
    assert_eq!(first.len(), 20);
    assert_eq!(first, second, "two runs produce identical results");
    // Input order: row i is comp-i.
    for (i, (path, js)) in first.iter().enumerate() {
        assert_eq!(path, &PathBuf::from(format!("src/routes/comp-{i:03}.vue")));
        assert_eq!(
            js.as_deref(),
            Ok(format!("js for src/routes/comp-{i:03}.vue").as_str())
        );
    }
}

#[tokio::test]
async fn precompile_overlaps_calls() {
    // §V2-B07: "fake pool that records concurrent in-flight maximum;
    // assert at least 4 on 20 files" — with the spec's sizing
    // (`workers * 2` for a 2-worker pool) the window holds 4.
    let flight = Flight::default();
    let width = width_for(2);
    assert_eq!(width, 4, "a 2-worker pool sizes the window to 4");
    run_batch(jobs(20), width, &flight, None).await;
    let peak = flight.peak();
    assert!(peak >= 4, "at least 4 calls overlap (peak was {peak})");
    assert!(peak <= width, "the window is never exceeded (peak {peak})");
}

#[tokio::test]
async fn the_window_bounds_concurrency() {
    // The narrower window holds too: width 2 on the same 20 jobs peaks at
    // exactly 2 — the bound is the bound, not a suggestion.
    let flight = Flight::default();
    run_batch(jobs(20), 2, &flight, None).await;
    assert_eq!(flight.peak(), 2, "width 2 holds at most 2 in flight");
}

#[tokio::test]
async fn precompile_failure_isolated() {
    // §V2-B07: "one failing file does not drop the others" — the failed
    // job's row carries its error, every other row still compiles, and
    // the row order survives.
    let flight = Flight::default();
    let out = run_batch(jobs(10), width_for(2), &flight, Some(5)).await;
    assert_eq!(out.len(), 10, "every job produces a row");
    for (i, (path, r)) in out.iter().enumerate() {
        assert_eq!(path, &PathBuf::from(format!("src/routes/comp-{i:03}.vue")));
        if i == 5 {
            assert!(r.is_err(), "the failing file errs");
        } else {
            assert!(r.is_ok(), "file {i} survives the sibling's failure");
        }
    }
}

#[test]
fn width_for_sizes_the_window_from_the_pool() {
    // The spec's sizing rule as one definition site: `workers * 2`, never
    // zero (a 0-worker report still compiles sequentially, one at a time
    // — the `max(1)` floor).
    assert_eq!(width_for(0), 2);
    assert_eq!(width_for(1), 2);
    assert_eq!(width_for(2), 4);
    assert_eq!(width_for(8), 16);
}

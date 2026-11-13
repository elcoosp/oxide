//! The §V2-B07 parallel-precompile engine — phase 2 of the WP: the
//! bounded-concurrency compile of the independent jobs phase 1 discovered.
//!
//! ## Why this lives here
//!
//! V2.0 item 3 puts pure logic where it does not pay the V8 link cost.
//! The engine is generic over the per-job compile pass, so the CLI's
//! `compile_one` (cache lookup + tooling-pool call) drops in unchanged
//! while the pins in `tests/precompile_engine_tdd.rs` run against fakes
//! in seconds — the spec's three named behaviors pinned without a single
//! worker spawn.
//!
//! ## The shapes
//!
//! - [`PrecompileJob`]: one discovered job — the file path (the
//!   precompiled-map key), the tooling kind and the source text.
//! - [`compile_jobs`]: the bounded window. `width` futures run at most
//!   concurrently (`buffer_unordered`); results come back in INPUT order
//!   regardless of completion order, so the caller's map fold stays
//!   deterministic (the `precompile_is_deterministic` pin).
//! - [`width_for`]: the spec's window sizing — `pool.stats().workers * 2`
//!   (each tooling call occupies a worker while the orchestration prepares
//!   the next), floored at 1 worker so a 0-worker report still compiles
//!   sequentially.

use std::future::Future;
use std::path::PathBuf;

use futures::stream::{self, StreamExt};

/// One discovered compile job (§V2-B07 phase 1 output): the file path —
/// the precompiled-map key the bundler looks the compiled JS up by — the
/// tooling kind (`vue`/`svelte`/`solid`/`mdx`) and the file's source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrecompileJob {
    /// The precompiled-map key: the resolved, normalized file path.
    pub path: PathBuf,
    /// The tooling kind the compile pass dispatches on.
    pub kind: &'static str,
    /// The file's source text (read during discovery, before any compile).
    pub source: String,
}

/// The spec's window sizing rule (§V2-B07) as one definition site: two
/// orchestration slots per pool worker — each tooling call occupies a
/// worker while the engine prepares the next job — floored so a 0-worker
/// report still compiles one at a time.
///
/// The native-compiler path (C01's CLI adoption) sizes its own window
/// from `available_parallelism()` and passes it straight to
/// [`compile_jobs`]; this function is the tooling-pool shape.
#[must_use]
pub fn width_for(workers: usize) -> usize {
    workers.max(1) * 2
}

/// Compile independent jobs with a bounded-concurrency window (§V2-B07
/// phase 2). At most `width` compile futures run concurrently; every job
/// produces exactly one result row, in INPUT order — a caller folding the
/// rows into its map gets the same map no matter what order the futures
/// resolve in (the determinism contract the CLI's `BTreeMap` fold relies
/// on). A failing job does not cancel or drop its siblings: the failure
/// rides its own row (the failure-isolation contract — the pass is
/// best-effort per file, the §6.3 convention).
///
/// `compile` is the injected per-job pass. Panics inside a pass cancel
/// the whole batch (a panicking compiler is a bug, not a user-file
/// problem — the panic surfaces instead of being swallowed per-row).
///
/// The window is clamped to `width.max(1)`: a zero-width request still
/// compiles, sequentially.
pub async fn compile_jobs<F, Fut, E>(
    jobs: Vec<PrecompileJob>,
    width: usize,
    compile: F,
) -> Vec<(PathBuf, Result<String, E>)>
where
    F: Fn(PrecompileJob) -> Fut,
    Fut: Future<Output = Result<String, E>>,
{
    let compile_ref = &compile;
    let results: Vec<(usize, PathBuf, Result<String, E>)> =
        stream::iter(jobs.into_iter().enumerate())
            .map(|(i, job)| {
                let path = job.path.clone();
                // The pass is shared by reference: `compile_jobs` is generic over
                // `Fn` (an immutable pass — the cache + pool sit behind `&self`),
                // so every future borrows the same closure.
                async move {
                    let out = compile_ref(job).await;
                    (i, path, out)
                }
            })
            .buffer_unordered(width.max(1))
            .collect()
            .await;

    // Fold back to input order: the deterministic row order the caller's
    // map fold (and the determinism pin) rely on. The indices are unique
    // by construction (one per input job), so the sort is total and the
    // fold is panic-free.
    let mut results: Vec<(usize, PathBuf, Result<String, E>)> = results;
    results.sort_by_key(|(i, _, _)| *i);
    results
        .into_iter()
        .map(|(_, path, out)| (path, out))
        .collect()
}

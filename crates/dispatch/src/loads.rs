//! The §9.2 parallel load graph (spec v0.2, V2-A06 step 2).
//!
//! The dispatch closure originally ran the layout chain sequentially (the
//! pre-A06 comment near `cli/src/lib.rs`, the `§9.2: layout loads first`
//! banner, named the gap: layout and page loads ran one after another even
//! when neither calls `parent()`). Both dispatch legs now run THIS module
//! instead — the SSR render (`stage_loaders`, adopted 2026-11-16) and the
//! `GET /__data` endpoint (`route_data_response`, the A06 follow-up): a
//! node list (outer layouts first, page last) where independent loads run
//! concurrently and a load whose scanner verdict is
//! [`LoadNode::needs_parent`] starts only after its predecessor resolves
//! and receives its data as `LoaderRunInput::parent_data`.
//!
//! Semantics (pinned by `crates/dispatch/tests/loads_tdd.rs`):
//!
//! - **Concurrency**: nodes that do not need a parent all start on the
//!   first poll. Two 50 ms loads finish within one delay of virtual time,
//!   not two.
//! - **Ordering**: the result is in *input* order (outer layouts first,
//!   page last), never completion order, so which short-circuit wins is
//!   deterministic.
//! - **Over-run**: independent loads may all run even if an outer one
//!   short-circuits (§9.2 already requires loaders to be idempotent and
//!   free of user-visible side effects). Pinning this so a future
//!   cancel-on-short-circuit is a deliberate change.
//! - **Failure poisoning**: a runner crash (`Err`) poisons the nodes that
//!   needed its data (they never run); independent siblings are unaffected.
//!   A loader *HTTP* short-circuit (non-200 `Ok`) does not poison its
//!   dependents — they run with the short-circuit output as their parent
//!   data, and the short-circuit output wins the result anyway.
//!
//! The node list comes from `RouteContextTable`'s
//! `layout_loads[i].needs_parent` (V2-B01), which threads verbatim from the
//! scanner's token-exact `parent()` verdict (V2-A06 step 1). The CLI wiring
//! adopts the graph through [`run_load_graph_outcomes`]: the page node runs
//! through the runner's streaming entry (B2 settle streaming) and the
//! per-node elapsed column feeds the waterfall's `overlap-saved` mark.

use futures::future::{BoxFuture, FutureExt, Shared};
use oxide_fw_diagnostics::Diagnostic;
use oxide_fw_server::{LoaderOutput, LoaderRunInput, LoaderRunner};
use std::sync::Arc;

/// One load to run: the runner input plus the scanner's `parent()` verdict.
///
/// Node *i* with `needs_parent == true` depends on node *i - 1* (the
/// immediately preceding entry in the input order — its outer layout). The
/// chain head (`i == 0`) ignores the flag: there is nothing to await.
#[derive(Debug)]
pub struct LoadNode {
    /// The loader call the graph makes on this node's turn.
    pub input: LoaderRunInput,
    /// Whether the load may call `await parent()` (scanner verdict).
    pub needs_parent: bool,
}

/// The graph's terminal answer, in input-order semantics.
#[derive(Debug)]
pub enum LoadResult {
    /// Every load ran clean: one OWNED output per node, in input order
    /// (outer layouts first, page last). The graph runs each slot behind a
    /// shared `Arc` for the `parent_data` threading; once every slot has
    /// completed the sharing is over, so the winner is unwrapped
    /// (`Arc::try_unwrap`) and the caller moves the fields out without a
    /// deep clone. Side effects (cookies, headers) are applied by the
    /// caller walking this Vec in order — the same outer-to-inner
    /// accumulation the sequential loop does today.
    Data(Vec<LoaderOutput>),
    /// The first node *in input order* whose output short-circuits
    /// (non-200): the response the pipeline builds from it, regardless of
    /// how many other loads also short-circuited or completed earlier.
    ShortCircuit(Arc<LoaderOutput>),
    /// The first runner failure *in input order*: the failing node's index
    /// (the caller distinguishes its layout/page diagnostics from it) and
    /// the runner's diagnostic (code + message — the code feeds the log,
    /// the message the response body). Dependent nodes of a failed slot
    /// never run.
    Failed {
        /// The input-order index of the node whose runner call failed.
        index: usize,
        /// The diagnostic code (`RT-…`), verbatim from the runner error.
        code: String,
        /// The runner's diagnostic message.
        message: String,
    },
}

/// The per-node outcomes a graph run collects alongside the terminal
/// [`LoadResult`] — what the `stage_loaders` wiring consumes over the bare
/// result.
#[derive(Debug)]
pub struct GraphOutcomes {
    /// B2 settle receivers, by node index: `Some` for every node listed in
    /// [`run_load_graph_outcomes`]'s `streaming` set that completed its
    /// runner call, `None` otherwise (over-run: a short-circuit terminal
    /// result still leaves every completed slot's receiver here).
    pub settles: Vec<Option<tokio::sync::mpsc::Receiver<serde_json::Value>>>,
    /// Per-node wall time in milliseconds — the node's own runner call
    /// only, *excluding* the `await parent()` wait (a poisoned dependent
    /// that never ran reads `0.0`). In input order, regardless of
    /// completion order; measured on the tokio clock, so paused-clock
    /// tests see virtual time.
    pub elapsed_ms: Vec<f64>,
}

/// One shared slot per node: the single future `join_all` drives, cloned into
/// dependents that await the same result.
type Slot = Shared<BoxFuture<'static, Result<Arc<LoaderOutput>, Arc<Diagnostic>>>>;

/// The shared per-slot outcome slots the node futures write and the caller
/// reads back after `join_all` (every slot has completed by then).
#[derive(Default)]
struct SlotOutcomes {
    settles: Vec<Option<tokio::sync::mpsc::Receiver<serde_json::Value>>>,
    elapsed_ms: Vec<f64>,
}

/// Run the load graph: independent nodes concurrently, `parent()`-needing
/// nodes behind their predecessor.
///
/// The runner is shared behind `Arc<dyn LoaderRunner>` (the pipeline holds
/// it exactly so); each node owns its `LoaderRunInput` and threads the
/// predecessor's resolved `data` through as `parent_data` when
/// `needs_parent` is set.
pub async fn run_load_graph(runner: Arc<dyn LoaderRunner>, nodes: Vec<LoadNode>) -> LoadResult {
    run_load_graph_outcomes(runner, nodes, &[]).await.0
}

/// [`run_load_graph`] with the per-node outcomes: the nodes listed in
/// `streaming` (by index — the wiring passes the page's) run through the
/// runner's `run_streaming` entry and contribute their settle receivers to
/// [`GraphOutcomes::settles`]; every node contributes its runner-call
/// duration to [`GraphOutcomes::elapsed_ms`].
pub async fn run_load_graph_outcomes(
    runner: Arc<dyn LoaderRunner>,
    nodes: Vec<LoadNode>,
    streaming: &[usize],
) -> (LoadResult, GraphOutcomes) {
    let count = nodes.len();
    let outcomes = Arc::new(std::sync::Mutex::new(SlotOutcomes {
        settles: (0..count).map(|_| None).collect(),
        elapsed_ms: vec![0.0; count],
    }));
    let mut slots: Vec<Slot> = Vec::with_capacity(count);
    for (i, node) in nodes.into_iter().enumerate() {
        let prev: Option<Slot> = if node.needs_parent && i > 0 {
            Some(slots[i - 1].clone())
        } else {
            None
        };
        let runner = Arc::clone(&runner);
        let outcomes = Arc::clone(&outcomes);
        let streams = streaming.contains(&i);
        let mut input = node.input;
        let fut: BoxFuture<'static, Result<Arc<LoaderOutput>, Arc<Diagnostic>>> = async move {
            if let Some(p) = prev {
                let parent = p.await?;
                input.parent_data = parent.data.clone();
            }
            // The elapsed column times the node's OWN runner call — the
            // `await parent()` wait above is excluded, and a poisoned
            // dependent that never reaches the call leaves its `0.0`.
            let started = tokio::time::Instant::now();
            let produced = if streams {
                runner.run_streaming(input).await.map(|(out, rx)| {
                    if let Ok(mut slots) = outcomes.lock() {
                        slots.settles[i] = Some(rx);
                    }
                    out
                })
            } else {
                runner.run(input).await
            };
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            if let Ok(mut slots) = outcomes.lock() {
                slots.elapsed_ms[i] = elapsed_ms;
            }
            produced.map(Arc::new).map_err(|e| Arc::new(*e.0))
        }
        .boxed();
        slots.push(fut.shared());
    }
    let results = futures::future::join_all(slots).await;
    let graph_outcomes = {
        let slots = outcomes
            .lock()
            .map_or_else(|_| SlotOutcomes::default(), |mut s| std::mem::take(&mut *s));
        GraphOutcomes {
            settles: slots.settles,
            elapsed_ms: slots.elapsed_ms,
        }
    };
    // Input-order scan, exactly as before: the FIRST slot that is a runner
    // failure or a short-circuit decides — never completion order.
    let mut outs = Vec::with_capacity(results.len());
    for (i, r) in results.into_iter().enumerate() {
        match r {
            Err(m) => {
                let diagnostic = Arc::unwrap_or_clone(m);
                return (
                    LoadResult::Failed {
                        index: i,
                        code: diagnostic.code,
                        message: diagnostic.message,
                    },
                    graph_outcomes,
                );
            }
            Ok(o) if o.short_circuits() => {
                return (LoadResult::ShortCircuit(o), graph_outcomes);
            }
            Ok(o) => outs.push(match Arc::try_unwrap(o) {
                // The slots completed, so the shared cell released its
                // handle: the common path unwraps without a clone.
                Ok(owned) => owned,
                Err(arc) => (*arc).clone(),
            }),
        }
    }
    (LoadResult::Data(outs), graph_outcomes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chain-head no-op: `needs_parent` on node 0 cannot await anything
    /// (there is no predecessor slot) — the graph must not panic or stall.
    #[tokio::test]
    async fn head_needs_parent_is_a_no_op() {
        struct NopRunner;
        #[async_trait::async_trait]
        impl LoaderRunner for NopRunner {
            fn name(&self) -> &'static str {
                "nop"
            }
            async fn run(&self, input: LoaderRunInput) -> oxide_fw_server::Result<LoaderOutput> {
                Ok(LoaderOutput::ok(serde_json::json!({ "ran": input.route })))
            }
        }
        let nodes = vec![LoadNode {
            input: LoaderRunInput::new("/m.ts", "routes/layout", "/"),
            needs_parent: true,
        }];
        let result = run_load_graph(Arc::new(NopRunner), nodes).await;
        let outs = match result {
            LoadResult::Data(v) => v,
            other => panic!("expected Data, got {other:?}"),
        };
        assert_eq!(outs.len(), 1);
        assert_eq!(outs[0].data["ran"], "routes/layout");
    }
}

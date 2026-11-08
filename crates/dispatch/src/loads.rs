//! The §9.2 parallel load graph (spec v0.2, V2-A06 step 2).
//!
//! The dispatch closure runs the layout chain sequentially and its comment
//! (near `cli/src/lib.rs`, the `§9.2: layout loads first` banner) names the
//! gap: layout and page loads run one after another even when neither calls
//! `parent()`. This module is the substrate `stage_loaders` wires instead:
//! a node list (outer layouts first, page last) where independent loads run
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
//! is deferred to the A01 phase-3 move (see `docs/deviations.md`).

use futures::future::{BoxFuture, FutureExt, Shared};
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
    /// Every load ran clean: one output per node, in input order (outer
    /// layouts first, page last). Side effects (cookies, headers) are
    /// applied by the caller walking this Vec in order — the same
    /// outer-to-inner accumulation the sequential loop does today.
    Data(Vec<Arc<LoaderOutput>>),
    /// The first node *in input order* whose output short-circuits
    /// (non-200): the response the pipeline builds from it, regardless of
    /// how many other loads also short-circuited or completed earlier.
    ShortCircuit(Arc<LoaderOutput>),
    /// The first runner failure *in input order*, with the diagnostic
    /// message. Dependent nodes of a failed slot never run.
    Failed(String),
}

/// One shared slot per node: the single future `join_all` drives, cloned into
/// dependents that await the same result.
type Slot = Shared<BoxFuture<'static, Result<Arc<LoaderOutput>, Arc<String>>>>;

/// Run the load graph: independent nodes concurrently, `parent()`-needing
/// nodes behind their predecessor.
///
/// The runner is shared behind `Arc<dyn LoaderRunner>` (the pipeline holds
/// it exactly so); each node owns its `LoaderRunInput` and threads the
/// predecessor's resolved `data` through as `parent_data` when
/// `needs_parent` is set.
pub async fn run_load_graph(runner: Arc<dyn LoaderRunner>, nodes: Vec<LoadNode>) -> LoadResult {
    let mut slots: Vec<Slot> = Vec::with_capacity(nodes.len());
    for (i, node) in nodes.into_iter().enumerate() {
        let prev: Option<Slot> = if node.needs_parent && i > 0 {
            Some(slots[i - 1].clone())
        } else {
            None
        };
        let runner = Arc::clone(&runner);
        let mut input = node.input;
        let fut: BoxFuture<'static, Result<Arc<LoaderOutput>, Arc<String>>> = async move {
            if let Some(p) = prev {
                let parent = p.await?;
                input.parent_data = parent.data.clone();
            }
            runner
                .run(input)
                .await
                .map(Arc::new)
                .map_err(|e| Arc::new(e.0.message.clone()))
        }
        .boxed();
        slots.push(fut.shared());
    }
    let results = futures::future::join_all(slots).await;
    let mut outs = Vec::with_capacity(results.len());
    for r in results {
        match r {
            Err(m) => return LoadResult::Failed((*m).clone()),
            Ok(o) if o.short_circuits() => return LoadResult::ShortCircuit(o),
            Ok(o) => outs.push(o),
        }
    }
    LoadResult::Data(outs)
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

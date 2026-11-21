//! The V2-A06 §9.2 load-graph walkthrough: the substrate `stage_loaders`
//! and (since the follow-up adoption) the `GET /__data` endpoint run for
//! every request. The example builds the canonical three-node chain — an
//! independent root layout, an independent nested layout and a page that
//! calls `parent()` — runs it through [`run_load_graph_outcomes`] with a
//! fake 40 ms runner, and prints the wall clock against the sequential
//! sum: the two independent layouts overlap, the page serializes behind
//! its predecessor and receives its data, and the per-node `elapsed_ms`
//! column (the waterfall's `overlap-saved` input) reports each node's own
//! run in input order.
//!
//! Run with:
//!
//! ```text
//! cargo run -p oxide-fw-dispatch --example load_graph
//! ```

use async_trait::async_trait;
use oxide_fw_dispatch::loads::{LoadNode, LoadResult, run_load_graph_outcomes};
use oxide_fw_server::{LoaderOutput, LoaderRunInput, LoaderRunner};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A fake runner that sleeps a fixed delay and echoes the route plus the
/// parent data it saw — the shape the TDD pins use (`loads_tdd.rs`), so
/// the printed picture is the pinned semantics.
struct EchoRunner {
    delay: Duration,
}

#[async_trait]
impl LoaderRunner for EchoRunner {
    fn name(&self) -> &'static str {
        "echo"
    }

    async fn run(&self, input: LoaderRunInput) -> oxide_fw_server::Result<LoaderOutput> {
        tokio::time::sleep(self.delay).await;
        Ok(LoaderOutput::ok(serde_json::json!({
            "route": input.route,
            "saw_parent": input.parent_data,
        })))
    }
}

/// One node per entry: `(route id, needs_parent verdict)` — the verdict
/// rides each layout's `LayoutLoadSpec` (the RouteContextTable's
/// token-exact `parent()` scan) and the page's own `RouteEntry`.
fn node(route: &str, needs_parent: bool) -> LoadNode {
    LoadNode {
        input: LoaderRunInput::new(format!("{route}.server.ts"), route, "/blog/x"),
        needs_parent,
    }
}

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        let runner: Arc<dyn LoaderRunner> = Arc::new(EchoRunner {
            delay: Duration::from_millis(40),
        });
        // The /blog/x chain: root layout (independent), blog layout
        // (independent), page (calls `await parent()`).
        let nodes = vec![
            node("routes/layout", false),
            node("routes/blog/layout", false),
            node("routes/blog/page", true),
        ];
        let sequential_sum: f64 = (nodes.len() as f64) * 40.0;

        let wall = Instant::now();
        let (result, outcomes) = run_load_graph_outcomes(Arc::clone(&runner), nodes, &[]).await;
        let wall_ms = wall.elapsed().as_secs_f64() * 1000.0;

        println!("§9.2 load graph — three nodes (two independent + one parent-calling):");
        print_result(&result);
        println!(
            "per-node elapsed_ms (input order, parent wait excluded): {:?}",
            outcomes.elapsed_ms
        );
        println!(
            "wall {wall_ms:.1} ms vs the sequential sum {sequential_sum:.1} ms — the two \
                  independent layouts overlapped, the page serialized behind its predecessor"
        );
        println!(
            "overlap-saved mark (sum − wall): {:.1} ms",
            (outcomes.elapsed_ms.iter().sum::<f64>() - wall_ms).max(0.0)
        );
    });
}

/// Print the terminal answer the wiring consumes: the input-order data
/// vector, the winning short-circuit, or the failed node's index.
fn print_result(result: &LoadResult) {
    match result {
        LoadResult::Data(outs) => {
            for (i, out) in outs.iter().enumerate() {
                println!(
                    "  [{i}] {} -> {}",
                    out.data["route"], out.data["saw_parent"]
                );
            }
        }
        LoadResult::ShortCircuit(out) => {
            println!("  short-circuit in input order: status {}", out.status);
        }
        LoadResult::Failed {
            index,
            code,
            message,
        } => {
            println!("  failed node [{index}] {code}: {message}");
        }
    }
}

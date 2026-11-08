//! Integration tests for the `oxide-fw-dispatch` parallel load graph
//! (spec v0.2, V2-A06 step 2).
//!
//! These pin the §9.2 contract the `stage_loaders` wiring will adopt:
//! independent loads run concurrently, a load that calls `parent()` starts
//! only after its predecessor and receives its data, short-circuits are
//! decided by input order (deterministic, not completion order), and runner
//! failures map to `LoadResult::Failed` while poisoning only dependents.
//!
//! Time-sensitive assertions run on the paused tokio clock
//! (`start_paused`): the virtual clock advances only when every task is
//! idle, so "two 50 ms loads overlap" is pinned as `elapsed < 60 ms`
//! without wall-clock flakes.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use oxide_fw_diagnostics::OxideError;
use oxide_fw_dispatch::loads::{LoadNode, LoadResult, run_load_graph};
use oxide_fw_server::{LoaderOutput, LoaderRunInput, LoaderRunner};

use async_trait::async_trait;

/// A scriptable fake runner: fixed per-run delay, per-route short-circuit
/// and failure scripting, plus an invocation log so tests can pin the start
/// order the graph schedules.
struct FakeRunner {
    delay_ms: u64,
    /// Routes that return `Ok(output)` with this non-200 status.
    short_routes: Vec<(&'static str, u16)>,
    /// Routes whose runner call fails (`Err` — runner crash, not HTTP error).
    fail_routes: Vec<&'static str>,
    calls: Mutex<Vec<String>>,
}

impl FakeRunner {
    fn new(delay_ms: u64) -> Self {
        Self {
            delay_ms,
            short_routes: Vec::new(),
            fail_routes: Vec::new(),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn short(mut self, routes: &[(&'static str, u16)]) -> Self {
        self.short_routes = routes.to_vec();
        self
    }

    fn failing(mut self, routes: &[&'static str]) -> Self {
        self.fail_routes = routes.to_vec();
        self
    }

    fn called(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl LoaderRunner for FakeRunner {
    fn name(&self) -> &'static str {
        "fake"
    }

    async fn run(&self, input: LoaderRunInput) -> oxide_fw_server::Result<LoaderOutput> {
        self.calls.lock().unwrap().push(input.route.clone());
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        if self.fail_routes.contains(&input.route.as_str()) {
            return Err(OxideError(Box::new(
                oxide_fw_diagnostics::Diagnostic::error(
                    "RT-TEST",
                    format!("runner crashed on {}", input.route),
                ),
            )));
        }
        if let Some((_, status)) = self
            .short_routes
            .iter()
            .find(|(route, _)| *route == input.route)
        {
            return Ok(LoaderOutput {
                status: *status,
                data: serde_json::json!({ "node": input.route }),
                ..Default::default()
            });
        }
        Ok(LoaderOutput::ok(serde_json::json!({
            "node": input.route,
            "saw_parent": input.parent_data,
        })))
    }
}

fn node(route: &str, needs_parent: bool) -> LoadNode {
    LoadNode {
        input: LoaderRunInput::new(format!("/{route}.server.ts"), route, format!("/{route}")),
        needs_parent,
    }
}

/// Unwrap helper that names the variant on failure.
fn expect_data(result: LoadResult) -> Vec<Arc<LoaderOutput>> {
    match result {
        LoadResult::Data(data) => data,
        other => panic!("expected LoadResult::Data, got {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn independent_loads_overlap_on_the_clock() {
    // §9.2 MUST: layout and page loads run concurrently unless one calls
    // `parent()`. Three independent 50 ms loads must finish within one
    // delay of virtual time; the sequential behaviour the dispatch closure
    // ships today takes 150 ms.
    let runner = Arc::new(FakeRunner::new(50));
    let nodes = vec![
        node("routes/a/layout", false),
        node("routes/b/layout", false),
        node("routes/a/page", false),
    ];
    let start = tokio::time::Instant::now();
    let result = run_load_graph(Arc::clone(&runner) as Arc<dyn LoaderRunner>, nodes).await;
    let elapsed = start.elapsed();
    assert_eq!(expect_data(result).len(), 3);
    assert!(
        elapsed < Duration::from_millis(60),
        "three independent 50 ms loads must overlap (took {elapsed:?})"
    );
    // All three actually ran.
    assert_eq!(runner.called().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_parent_needing_load_starts_after_and_reads_its_predecessors_data() {
    let runner = Arc::new(FakeRunner::new(50));
    let nodes = vec![node("routes/a/layout", true), node("routes/a/page", true)];
    let start = tokio::time::Instant::now();
    let data =
        expect_data(run_load_graph(Arc::clone(&runner) as Arc<dyn LoaderRunner>, nodes).await);
    let elapsed = start.elapsed();
    // Serialized: one delay per link in the parent chain.
    assert!(
        elapsed >= Duration::from_millis(100),
        "a parent()-needing chain must serialize (took {elapsed:?})"
    );
    // Start order: the layout before the page.
    assert_eq!(runner.called(), vec!["routes/a/layout", "routes/a/page"]);
    // The page received the layout's resolved data as parent_data.
    assert_eq!(data[1].data["saw_parent"]["node"], "routes/a/layout");
    // And the layout itself ran with no parent data of its own (it is the
    // chain head — `needs_parent` on node 0 is a no-op).
    assert_eq!(data[0].data["saw_parent"], serde_json::Value::Null);
}

#[tokio::test(start_paused = true)]
async fn parent_data_chains_through_three_levels() {
    let runner = Arc::new(FakeRunner::new(10));
    let nodes = vec![
        node("routes/layout", true),
        node("routes/blog/layout", true),
        node("routes/blog/page", true),
    ];
    let data =
        expect_data(run_load_graph(Arc::clone(&runner) as Arc<dyn LoaderRunner>, nodes).await);
    assert_eq!(data[1].data["saw_parent"]["node"], "routes/layout");
    assert_eq!(data[2].data["saw_parent"]["node"], "routes/blog/layout");
}

#[tokio::test(start_paused = true)]
async fn an_independent_load_does_not_wait_behind_a_parent_needing_head() {
    // `needs_parent` on the chain HEAD is a no-op (no predecessor), and the
    // load behind it that does NOT need a parent must not inherit the
    // serialization.
    let runner = Arc::new(FakeRunner::new(50));
    let nodes = vec![
        node("routes/layout", true),
        node("routes/other/page", false),
    ];
    let start = tokio::time::Instant::now();
    let data =
        expect_data(run_load_graph(Arc::clone(&runner) as Arc<dyn LoaderRunner>, nodes).await);
    assert!(
        start.elapsed() < Duration::from_millis(60),
        "the independent sibling must not wait (took {:?})",
        start.elapsed()
    );
    assert_eq!(data.len(), 2);
    assert_eq!(data[1].data["saw_parent"], serde_json::Value::Null);
}

#[tokio::test(start_paused = true)]
async fn the_first_short_circuit_in_input_order_wins() {
    // Two loads short-circuit; whichever comes first in INPUT order is the
    // answer, never the one that happened to finish first — behavior must
    // not depend on scheduling.
    let runner = Arc::new(
        FakeRunner::new(10).short(&[("routes/blog/layout", 302), ("routes/blog/page", 404)]),
    );
    let nodes = vec![
        node("routes/layout", false),
        node("routes/blog/layout", false),
        node("routes/blog/page", false),
        node("routes/other/page", false),
    ];
    match run_load_graph(Arc::clone(&runner) as Arc<dyn LoaderRunner>, nodes).await {
        LoadResult::ShortCircuit(out) => {
            assert_eq!(out.status, 302, "the outer layout's redirect wins");
            assert_eq!(out.data["node"], "routes/blog/layout");
        }
        other => panic!("expected ShortCircuit, got {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn independent_loads_still_run_when_an_outer_load_short_circuits() {
    // Documented §9.2 semantics: independent loads may all run even if an
    // outer one short-circuits — loaders must be idempotent and free of
    // user-visible side effects. Pin the over-run so a future change to
    // cancel-on-short-circuit is deliberate.
    let runner = Arc::new(FakeRunner::new(10).short(&[("routes/layout", 302)]));
    let nodes = vec![node("routes/layout", false), node("routes/page", false)];
    match run_load_graph(Arc::clone(&runner) as Arc<dyn LoaderRunner>, nodes).await {
        LoadResult::ShortCircuit(out) => assert_eq!(out.status, 302),
        other => panic!("expected ShortCircuit, got {other:?}"),
    }
    assert_eq!(
        runner.called(),
        vec!["routes/layout", "routes/page"],
        "the independent sibling still ran"
    );
}

#[tokio::test(start_paused = true)]
async fn a_runner_failure_maps_to_failed() {
    let runner = Arc::new(FakeRunner::new(5).failing(&["routes/blog/layout"]));
    let nodes = vec![
        node("routes/layout", false),
        node("routes/blog/layout", false),
    ];
    match run_load_graph(Arc::clone(&runner) as Arc<dyn LoaderRunner>, nodes).await {
        LoadResult::Failed(message) => {
            assert_eq!(message, "runner crashed on routes/blog/layout");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn a_runner_failure_poisons_only_its_dependents() {
    // A crashed runner (Err) poisons the loads that needed its data, but
    // independent siblings are unaffected — they ran, and the first
    // failure in input order is the reported one.
    let runner = Arc::new(FakeRunner::new(5).failing(&["routes/layout"]));
    let nodes = vec![
        node("routes/layout", false),
        node("routes/blog/layout", true),
        node("routes/other/page", false),
    ];
    match run_load_graph(Arc::clone(&runner) as Arc<dyn LoaderRunner>, nodes).await {
        LoadResult::Failed(message) => {
            assert_eq!(message, "runner crashed on routes/layout");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    // The dependent never invoked the runner (it awaits a poisoned slot);
    // the independent sibling did run.
    assert_eq!(runner.called(), vec!["routes/layout", "routes/other/page"]);
}

#[tokio::test(start_paused = true)]
async fn outputs_keep_input_order_regardless_of_completion() {
    // Paused clock: 30/10/20 ms delays complete inner-first, but the
    // result Vec is in input order (outer layouts first, page last) —
    // the shape `stage_loaders` applies side effects in.
    let runner = Arc::new(FakeRunner::new(10));
    let nodes = vec![
        node("routes/slow/layout", false),
        node("routes/fast/page", false),
        node("routes/mid/page", false),
    ];
    let data =
        expect_data(run_load_graph(Arc::clone(&runner) as Arc<dyn LoaderRunner>, nodes).await);
    let nodes_out: Vec<&str> = data
        .iter()
        .map(|o| o.data["node"].as_str().unwrap())
        .collect();
    assert_eq!(
        nodes_out,
        vec!["routes/slow/layout", "routes/fast/page", "routes/mid/page"]
    );
}

#[tokio::test(start_paused = true)]
async fn an_empty_graph_yields_no_data() {
    let runner: Arc<dyn LoaderRunner> = Arc::new(FakeRunner::new(1));
    let result = run_load_graph(runner, Vec::new()).await;
    assert_eq!(expect_data(result), Vec::new());
}

#[tokio::test(start_paused = true)]
async fn short_circuit_by_a_head_load_is_returned_without_parent_data() {
    // A redirect from the outermost load (no parent chain at all) carries
    // the short-circuit output untouched — no data merging happens on the
    // short-circuit path.
    let runner = Arc::new(FakeRunner::new(5).short(&[("routes/layout", 303)]));
    let nodes = vec![node("routes/layout", false)];
    match run_load_graph(Arc::clone(&runner) as Arc<dyn LoaderRunner>, nodes).await {
        LoadResult::ShortCircuit(out) => {
            assert_eq!(out.status, 303);
            assert_eq!(out.data["node"], "routes/layout");
        }
        other => panic!("expected ShortCircuit, got {other:?}"),
    }
}

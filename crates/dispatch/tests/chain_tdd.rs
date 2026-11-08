//! Integration tests for the `oxide-fw-dispatch` middleware chain runner
//! (spec v0.2, V2-A05 step 1 — the V8-free `run_chain` substrate).
//!
//! These pin the §8.5 contract the `stage_middleware` wiring will adopt,
//! mirroring the semantics the CLI's inline loop (crates/cli/src/lib.rs)
//! already ships and the CLI test `middleware_chain_runs_outer_to_inner_
//! and_threads_locals` pins end-to-end:
//!
//! - the chain runs outer to inner, one dispatcher call per layer;
//! - each layer sees the locals merged so far (None while nothing has
//!   accumulated — the trait doc's contract);
//! - `locals` merge later-wins; response headers accumulate with
//!   last-writer-wins per case-insensitive name (the CLI's B-06 fix);
//! - `short` and `redirect` stop the chain and return the raw result;
//! - `rewrite` stops the chain and names the rewriting layer so the
//!   caller's re-dispatch can resume *after* it (the CLI's B-22 fix);
//! - a resume marker skips the already-run layers and does not run the
//!   rewriting layer twice;
//! - a dispatcher error propagates verbatim.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use oxide_fw_dispatch::chain::{ChainOutcome, ChainRequest, run_chain};
use oxide_fw_server::{
    MiddlewareDispatchArgs, MiddlewareDispatcher, MiddlewareRunError, MiddlewareRunResult,
};

/// A scripted fake middleware dispatcher: every layer maps to one scripted
/// `MiddlewareRunResult` (in chain order), and each call records the module
/// path plus the `locals` argument it received, so tests can pin ordering
/// and locals threading.
struct ScriptedDispatcher {
    /// One script entry per *layer* (by index in the chain order
    /// `run_chain` walks). A layer past the end of the script answers
    /// `Default::default()` (no short, no redirect, no locals).
    script: Vec<MiddlewareRunResult>,
    /// When true, every dispatch call fails with a fixed error.
    fail_all: bool,
    calls: Mutex<Vec<(String, Option<serde_json::Value>)>>,
}

impl ScriptedDispatcher {
    fn new(script: Vec<MiddlewareRunResult>) -> Self {
        Self {
            script,
            fail_all: false,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn failing() -> Self {
        Self {
            script: Vec::new(),
            fail_all: true,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn called(&self) -> Vec<(String, Option<serde_json::Value>)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl MiddlewareDispatcher for ScriptedDispatcher {
    async fn dispatch(
        &self,
        args: MiddlewareDispatchArgs<'_>,
    ) -> std::result::Result<MiddlewareRunResult, MiddlewareRunError> {
        self.calls
            .lock()
            .unwrap()
            .push((args.module_path.to_owned(), args.locals.cloned()));
        if self.fail_all {
            return Err(MiddlewareRunError {
                status: 500,
                code: "MW-TEST".into(),
                message: "scripted failure".into(),
            });
        }
        let idx = self
            .calls
            .lock()
            .unwrap()
            .len()
            .saturating_sub(1)
            .min(self.script.len().saturating_sub(1));
        Ok(self.script.get(idx).cloned().unwrap_or_default())
    }
}

/// The request the chain runs: constant across every layer in these tests.
fn request() -> ChainRequest<'static> {
    static PARAMS: std::sync::OnceLock<serde_json::Map<String, serde_json::Value>> =
        std::sync::OnceLock::new();
    ChainRequest {
        method: "GET",
        url: "/blog/post",
        params: PARAMS.get_or_init(serde_json::Map::new),
        headers: &[],
        body: &[],
    }
}

fn layer(name: &str) -> PathBuf {
    PathBuf::from(name)
}

fn outcome_locals(outcome: ChainOutcome) -> serde_json::Value {
    match outcome {
        ChainOutcome::Continue { locals, .. } | ChainOutcome::Rewrite { locals, .. } => locals,
        ChainOutcome::Respond(_) => panic!("expected a continue/rewrite outcome, got Respond"),
    }
}

fn outcome_headers(outcome: &ChainOutcome) -> Vec<(String, String)> {
    match outcome {
        ChainOutcome::Continue { headers, .. } | ChainOutcome::Rewrite { headers, .. } => {
            headers.clone()
        }
        ChainOutcome::Respond(r) => r.headers.clone(),
    }
}

#[tokio::test]
async fn runs_outer_to_inner() {
    // §8.5: `routes/middleware` wraps `routes/blog/middleware`, which wraps
    // the page. The dispatcher must see them in exactly that order.
    let dispatcher = ScriptedDispatcher::new(vec![]);
    let chain = [
        layer("routes/middleware.ts"),
        layer("routes/blog/middleware.ts"),
    ];
    let dispatcher = Arc::new(dispatcher);
    let outcome = run_chain(
        dispatcher.as_ref(),
        &chain,
        request(),
        serde_json::json!({}),
        None,
    )
    .await
    .expect("chain ran clean");
    assert!(matches!(outcome, ChainOutcome::Continue { .. }));
    let called = dispatcher.called();
    let names: Vec<String> = called.iter().map(|(p, _)| p.clone()).collect();
    assert_eq!(
        names,
        vec![
            "routes/middleware.ts".to_string(),
            "routes/blog/middleware.ts".to_string()
        ],
        "outer middleware runs first"
    );
}

#[tokio::test]
async fn inner_sees_outer_locals_and_empty_stays_none() {
    // The first layer has nothing accumulated: the trait contract is
    // `None`, not `Some({})`. The second layer sees the first layer's
    // contribution as the `locals` argument.
    let script = vec![MiddlewareRunResult {
        locals: Some(serde_json::json!({ "section": "blog" })),
        ..Default::default()
    }];
    let dispatcher = Arc::new(ScriptedDispatcher::new(script));
    let chain = [
        layer("routes/middleware.ts"),
        layer("routes/blog/middleware.ts"),
    ];
    run_chain(
        dispatcher.as_ref(),
        &chain,
        request(),
        serde_json::json!({}),
        None,
    )
    .await
    .expect("chain ran clean");
    let called = dispatcher.called();
    assert_eq!(called.len(), 2);
    assert!(
        called[0].1.is_none(),
        "outer layer sees None while nothing accumulated, got {:?}",
        called[0].1
    );
    assert_eq!(
        called[1].1.as_ref(),
        Some(&serde_json::json!({ "section": "blog" })),
        "inner layer sees the outer layer's locals"
    );
}

#[tokio::test]
async fn locals_merge_later_wins() {
    // An inner middleware refines what an outer one set: same key, later
    // (inner) value wins the final merged map.
    let script = vec![
        MiddlewareRunResult {
            locals: Some(serde_json::json!({ "section": "root", "extra": 1 })),
            ..Default::default()
        },
        MiddlewareRunResult {
            locals: Some(serde_json::json!({ "section": "blog" })),
            ..Default::default()
        },
    ];
    let dispatcher = Arc::new(ScriptedDispatcher::new(script));
    let chain = [
        layer("routes/middleware.ts"),
        layer("routes/blog/middleware.ts"),
    ];
    let outcome = run_chain(
        dispatcher.as_ref(),
        &chain,
        request(),
        serde_json::json!({}),
        None,
    )
    .await
    .expect("chain ran clean");
    assert_eq!(
        outcome_locals(outcome),
        serde_json::json!({ "section": "blog", "extra": 1 }),
        "inner `section` overwrites outer, outer's other keys survive"
    );
}

#[tokio::test]
async fn initial_locals_seed_the_chain() {
    // The pipeline seeds `locals.session` before the chain runs (§16.6);
    // the first layer must already see it.
    let dispatcher = Arc::new(ScriptedDispatcher::new(vec![]));
    let chain = [layer("routes/middleware.ts")];
    run_chain(
        dispatcher.as_ref(),
        &chain,
        request(),
        serde_json::json!({ "session": { "user": "elmo" } }),
        None,
    )
    .await
    .expect("chain ran clean");
    let called = dispatcher.called();
    assert_eq!(
        called[0].1.as_ref(),
        Some(&serde_json::json!({ "session": { "user": "elmo" } })),
        "seeded locals reach the first layer"
    );
}

#[tokio::test]
async fn short_circuit_stops_chain() {
    // The outer layer answers (401): the inner layer must not run and the
    // raw result rides out for the caller to build the response.
    let script = vec![MiddlewareRunResult {
        short: true,
        status: 401,
        body: b"nope".to_vec(),
        ..Default::default()
    }];
    let dispatcher = Arc::new(ScriptedDispatcher::new(script));
    let chain = [
        layer("routes/middleware.ts"),
        layer("routes/blog/middleware.ts"),
    ];
    let outcome = run_chain(
        dispatcher.as_ref(),
        &chain,
        request(),
        serde_json::json!({}),
        None,
    )
    .await
    .expect("short-circuit is not an error");
    match outcome {
        ChainOutcome::Respond(r) => {
            assert!(r.short);
            assert_eq!(r.status, 401);
            assert_eq!(r.body, b"nope");
        }
        other => panic!("expected Respond, got {other:?}"),
    }
    let called = dispatcher.called();
    assert_eq!(called.len(), 1, "inner layer never ran");
}

#[tokio::test]
async fn redirect_stops_chain() {
    // §8.5: `{ short: true, status, redirect }` — the sibling redirect
    // field, not a location header. The chain must stop on it.
    let script = vec![MiddlewareRunResult {
        short: true,
        status: 302,
        redirect: Some("/login".into()),
        ..Default::default()
    }];
    let dispatcher = Arc::new(ScriptedDispatcher::new(script));
    let chain = [
        layer("routes/middleware.ts"),
        layer("routes/blog/middleware.ts"),
    ];
    let outcome = run_chain(
        dispatcher.as_ref(),
        &chain,
        request(),
        serde_json::json!({}),
        None,
    )
    .await
    .expect("redirect is not an error");
    match outcome {
        ChainOutcome::Respond(r) => {
            assert_eq!(r.redirect.as_deref(), Some("/login"));
        }
        other => panic!("expected Respond, got {other:?}"),
    }
    assert_eq!(dispatcher.called().len(), 1, "inner layer never ran");
}

#[tokio::test]
async fn rewrite_returns_target_with_locals_and_resume_marker() {
    // §8.5 / B-22: a rewrite stops the walk; the caller re-dispatches the
    // request and needs (a) the merged locals so far, (b) the accumulated
    // headers, (c) the rewriting layer's path as the resume marker so the
    // next pass does not run it twice.
    let script = vec![
        MiddlewareRunResult {
            locals: Some(serde_json::json!({ "from": "outer" })),
            ..Default::default()
        },
        MiddlewareRunResult {
            rewrite: Some("/new-url".into()),
            ..Default::default()
        },
    ];
    let dispatcher = Arc::new(ScriptedDispatcher::new(script));
    let chain = [
        layer("routes/middleware.ts"),
        layer("routes/blog/middleware.ts"),
    ];
    let outcome = run_chain(
        dispatcher.as_ref(),
        &chain,
        request(),
        serde_json::json!({}),
        None,
    )
    .await
    .expect("rewrite is not an error");
    match outcome {
        ChainOutcome::Rewrite {
            to,
            locals,
            resume_after,
            ..
        } => {
            assert_eq!(to, "/new-url");
            assert_eq!(locals, serde_json::json!({ "from": "outer" }));
            assert_eq!(
                resume_after.as_deref(),
                Some("routes/blog/middleware.ts"),
                "the rewriting layer is the resume marker"
            );
        }
        other => panic!("expected Rewrite, got {other:?}"),
    }
    assert_eq!(
        dispatcher.called().len(),
        2,
        "no layer ran after the rewrite"
    );
}

#[tokio::test]
async fn resume_after_skips_run_layers_and_the_rewriting_layer() {
    // B-22: after the re-dispatch, layers before the marker already ran;
    // the marker layer itself must not run twice. Only the layers after it
    // execute.
    let script = vec![MiddlewareRunResult {
        locals: Some(serde_json::json!({ "from": "inner" })),
        ..Default::default()
    }];
    let dispatcher = Arc::new(ScriptedDispatcher::new(script));
    let chain = [
        layer("routes/middleware.ts"),
        layer("routes/blog/middleware.ts"),
        layer("routes/blog/post/middleware.ts"),
    ];
    let outcome = run_chain(
        dispatcher.as_ref(),
        &chain,
        request(),
        serde_json::json!({ "from": "outer" }),
        Some("routes/blog/middleware.ts"),
    )
    .await
    .expect("resumed chain ran clean");
    assert!(matches!(outcome, ChainOutcome::Continue { .. }));
    let names: Vec<String> = dispatcher.called().iter().map(|(p, _)| p.clone()).collect();
    assert_eq!(
        names,
        vec!["routes/blog/post/middleware.ts".to_string()],
        "only the layer after the resume marker runs"
    );
}

#[tokio::test]
async fn resume_marker_absent_from_chain_runs_everything() {
    // The CLI only honours the marker when the target layer is present in
    // this route's chain (e.g. the rewrite landed on a shallower route);
    // otherwise the whole chain runs normally.
    let dispatcher = Arc::new(ScriptedDispatcher::new(vec![]));
    let chain = [
        layer("routes/middleware.ts"),
        layer("routes/other/middleware.ts"),
    ];
    let outcome = run_chain(
        dispatcher.as_ref(),
        &chain,
        request(),
        serde_json::json!({}),
        Some("routes/blog/middleware.ts"),
    )
    .await
    .expect("chain ran clean");
    assert!(matches!(outcome, ChainOutcome::Continue { .. }));
    assert_eq!(dispatcher.called().len(), 2, "both layers ran");
}

#[tokio::test]
async fn headers_accumulate_last_writer_wins() {
    // B-06: an inner middleware refines what an outer one set — per
    // case-insensitive header name — instead of emitting duplicate lines.
    let script = vec![
        MiddlewareRunResult {
            headers: vec![
                ("x-oxide-cache".into(), "miss".into()),
                ("Set-Cookie".into(), "a=1".into()),
            ],
            ..Default::default()
        },
        MiddlewareRunResult {
            headers: vec![
                ("X-Oxide-Cache".into(), "hit".into()),
                ("x-custom".into(), "v".into()),
            ],
            ..Default::default()
        },
    ];
    let dispatcher = Arc::new(ScriptedDispatcher::new(script));
    let chain = [
        layer("routes/middleware.ts"),
        layer("routes/blog/middleware.ts"),
    ];
    let outcome = run_chain(
        dispatcher.as_ref(),
        &chain,
        request(),
        serde_json::json!({}),
        None,
    )
    .await
    .expect("chain ran clean");
    assert_eq!(
        outcome_headers(&outcome),
        vec![
            ("x-oxide-cache".to_string(), "hit".to_string()),
            ("Set-Cookie".to_string(), "a=1".to_string()),
            ("x-custom".to_string(), "v".to_string()),
        ],
        "same-name headers collapse (later value wins), distinct names keep order"
    );
}

#[tokio::test]
async fn error_propagates_verbatim() {
    // A dispatcher failure surfaces as Err with the worker's status/code/
    // message so the caller can emit the MIDDLEWARE-coded envelope.
    let dispatcher = Arc::new(ScriptedDispatcher::failing());
    let chain = [
        layer("routes/middleware.ts"),
        layer("routes/blog/middleware.ts"),
    ];
    let err = run_chain(
        dispatcher.as_ref(),
        &chain,
        request(),
        serde_json::json!({}),
        None,
    )
    .await
    .expect_err("scripted failure");
    assert_eq!(err.status, 500);
    assert_eq!(err.code, "MW-TEST");
    assert_eq!(err.message, "scripted failure");
    assert_eq!(dispatcher.called().len(), 1, "the walk stops at the error");
}

#[tokio::test]
async fn empty_chain_continues_with_initial_locals() {
    // A route with no middleware: the outcome still carries the seeded
    // locals (and no headers), so the caller's downstream stages see one
    // uniform shape.
    let dispatcher = Arc::new(ScriptedDispatcher::new(vec![]));
    let outcome = run_chain(
        dispatcher.as_ref(),
        &[],
        request(),
        serde_json::json!({ "session": { "user": "elmo" } }),
        None,
    )
    .await
    .expect("empty chain ran clean");
    match outcome {
        ChainOutcome::Continue { locals, headers } => {
            assert_eq!(locals, serde_json::json!({ "session": { "user": "elmo" } }));
            assert_eq!(headers, Vec::<(String, String)>::new());
        }
        other => panic!("expected Continue, got {other:?}"),
    }
    assert_eq!(
        dispatcher.called(),
        Vec::<(String, Option<serde_json::Value>)>::new()
    );
}

#[tokio::test]
async fn non_object_initial_locals_normalizes_to_empty() {
    // The spec sketch normalizes a non-object seed (`null`, an array) to
    // an empty locals map instead of panicking on `as_object_mut`.
    let dispatcher = Arc::new(ScriptedDispatcher::new(vec![MiddlewareRunResult {
        locals: Some(serde_json::json!({ "ok": true })),
        ..Default::default()
    }]));
    let chain = [layer("routes/middleware.ts")];
    let outcome = run_chain(
        dispatcher.as_ref(),
        &chain,
        request(),
        serde_json::Value::Null,
        None,
    )
    .await
    .expect("chain ran clean");
    assert_eq!(outcome_locals(outcome), serde_json::json!({ "ok": true }));
}

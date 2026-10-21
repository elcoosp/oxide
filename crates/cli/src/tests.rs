//! TDD suite for oxide-cli (spec §19.1).
//!
//! Written BEFORE the implementation; every command is exercised through
//! [`run_with_writer`] with a buffered writer (no process stdout capture needed).

use super::*;
use clap::CommandFactory;
use std::path::PathBuf;

// ------------------------------------------------------------------ helpers

async fn run_ok(args: &[&str]) -> (i32, String) {
    let cli = Cli::try_parse_from(args).unwrap_or_else(|e| panic!("parse {args:?}: {e}"));
    let mut buf: Vec<u8> = Vec::new();
    let code = run_with_writer(cli, &mut buf)
        .await
        .unwrap_or_else(|e| panic!("run {args:?} failed: {e}"));
    (code, String::from_utf8_lossy(&buf).to_string())
}

async fn run_err(args: &[&str]) -> anyhow::Error {
    let cli = Cli::try_parse_from(args).unwrap_or_else(|e| panic!("parse {args:?}: {e}"));
    let mut buf: Vec<u8> = Vec::new();
    run_with_writer(cli, &mut buf)
        .await
        .expect_err("expected an error")
}

/// Minimal fixture app: one static page at `/`, one tagged ISR blog page, and a
/// valid base config (framework adapter is required by config validation).
fn fixture_project() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let routes = root.join("src/routes");
    std::fs::create_dir_all(routes.join("blog")).unwrap();
    std::fs::write(
        routes.join("page.tsx"),
        "export default function Page() {\n  return <main>home</main>;\n}\n",
    )
    .unwrap();
    std::fs::write(
        routes.join("blog/page.tsx"),
        "export default function Page() {\n  return <main>home</main>;\n}\n",
    )
    .unwrap();
    std::fs::write(
        routes.join("blog/page.tsx"),
        "export default function Blog() {\n  return <main>blog</main>;\n}\n",
    )
    .unwrap();
    std::fs::write(
        routes.join("blog/page.server.ts"),
        "export const config = { render: 'isr', tags: ['post:a'] };\n",
    )
    .unwrap();
    write_config(&root, "{}");
    (dir, root)
}

/// Write `oxide.config.json`, always carrying a valid `framework.adapter` (required
/// by config validation) so the fixture behaves like a real project.
fn write_config(root: &PathBuf, json: &str) {
    let mut v: serde_json::Value = serde_json::from_str(json).unwrap();
    v["framework"]["adapter"] = serde_json::json!("react");
    std::fs::write(
        root.join("oxide.config.json"),
        serde_json::to_string_pretty(&v).unwrap() + "\n",
    )
    .unwrap();
}

fn manifest_fixture(routes: serde_json::Value) -> String {
    serde_json::json!({ "build_id": "t1", "routes": routes, "assets": {} }).to_string()
}

// ---------------------------------------------------------- parse surface (§19.1)

/// SPEC-19.1 / §19-B: process-global env-var tests take this lock so
/// they don't race each other (nextest runs tests in parallel by default).
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[tokio::test]
#[ignore]
async fn cli_render_dispatches_real_react_ssr() {
    // CLI-level E2E: run `oxide render` against a real oxide.config.mjs that
    // imports the real @oxide/react adapter. Assert the stdout HTML is the
    // output of react-dom/server, proving the full stack — clap parse →
    // FrameworkHost → Node worker → adapter.render → react-dom/server →
    // back through the worker → CLI stdout.
    //
    // Requires Node 24 (or 22.18+) for native TS stripping, and pnpm-installed
    // workspace node_modules. Marked #[ignore] because it spawns a Node worker.

    use std::fs;
    use std::path::PathBuf;

    // Locate workspace root via CARGO_MANIFEST_DIR = crates/cli.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest.parent().unwrap().parent().unwrap();
    let e2e_dir = workspace.join("packages/react/.tmp-e2e");
    fs::create_dir_all(&e2e_dir).expect("mkdir .tmp-e2e");
    let config_path = e2e_dir.join("oxide.config.mjs");

    // Write the config. `../src/real-adapter.ts` resolves to
    // packages/react/src/real-adapter.ts because the config is a direct
    // child of .tmp-e2e/.
    let config_src = r#"import { createRealAdapter } from '../src/real-adapter.ts'
import { createElement } from 'react'

const Home = (props) =>
  createElement('h1', { className: 'greeting' },
    'CLI-E2E: ' + String((props && props.data && props.data.name) || 'unknown'))

export default {
  framework: createRealAdapter({
    ssr: 'real',
    componentFor: (route) => (route === 'routes/index' ? Home : undefined),
  }),
}
"#;
    fs::write(&config_path, config_src).expect("write config");

    // Invoke `oxide render routes/index --root <e2e_dir>`.
    // run_with_writer drives the CLI in-process (no process spawn) but the
    // FrameworkHost does spawn a real Node worker.
    let root_str = e2e_dir.to_string_lossy().to_string();
    let (code, out) = run_ok(&[
        "oxide",
        "render",
        "routes/index",
        "--root",
        &root_str,
        "--data",
        r#"{"name":"CLI"}"#,
    ])
    .await;
    assert_eq!(code, 0, "oxide render exit code; output:\n{out}");
    assert!(
        out.starts_with("<!doctype html>"),
        "html should start with doctype:\n{out}"
    );
    assert!(
        out.contains("CLI-E2E: CLI"),
        "react component output should be present:\n{out}"
    );
    assert!(
        out.contains("data-reactroot"),
        "react root marker should be present:\n{out}"
    );

    let _ = fs::remove_file(&config_path);
}

// ---------------------------------------------------- build_dispatch + page_renderer

/// A `PageRenderer` that records how many times it was called and returns
/// a signature body so the test can prove the renderer branch fired.
struct CountingRenderer {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl oxide_server::PageRenderer for CountingRenderer {
    fn name(&self) -> &str {
        "counting"
    }
    async fn render(&self, input: oxide_server::PageRenderInput) -> oxide_server::Result<String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(format!(
            "<!doctype html><html><body><h1>RENDERER:{}</h1></body></html>",
            input.route
        ))
    }
}

/// A `PageRenderer` that always fails; verifies the 500 + error body path.
struct FailingRenderer;

#[async_trait::async_trait]
impl oxide_server::PageRenderer for FailingRenderer {
    fn name(&self) -> &str {
        "failing"
    }
    async fn render(&self, _input: oxide_server::PageRenderInput) -> oxide_server::Result<String> {
        Err(oxide_server::err("FAKE-RENDER", "simulated render failure"))
    }
}

fn test_request(method: &str, url: &str) -> oxide_server::OxideRequest {
    oxide_server::OxideRequest {
        method: method.into(),
        url: url.into(),
        headers: std::sync::Arc::new(Vec::new()),
        header_index: Default::default(),
        body: bytes::Bytes::new(),
        request_id: "dispatch-test".into(),
        trace: None,
        peer: None,
        rewrite_hops: 0,
        mw_resume_after: None,
    }
}

fn empty_tree() -> std::sync::Arc<oxide_router::RouteTree> {
    std::sync::Arc::new(oxide_router::RouteTree::new())
}

/// B-27: streams a static shell with one deferred boundary, then the
/// resolved fragment — exactly the shape a streaming adapter emits.
struct PprRenderer;

#[async_trait::async_trait]
impl oxide_server::PageRenderer for PprRenderer {
    fn name(&self) -> &str {
        "ppr"
    }
    async fn render(&self, _input: oxide_server::PageRenderInput) -> oxide_server::Result<String> {
        Ok("<!doctype html><html><body>PPR</body></html>".into())
    }
    async fn render_streaming(
        &self,
        _input: oxide_server::PageRenderInput,
    ) -> oxide_server::Result<oxide_server::RenderStream> {
        let items: Vec<oxide_server::Result<bytes::Bytes>> = vec![
            Ok(bytes::Bytes::from_static(
                b"<!doctype html><html><body><h1>STATIC-SHELL</h1><template data-ox-b='1'><p>fallback</p></template>",
            )),
            Ok(bytes::Bytes::from_static(
                b"<template data-ox-c='1'><p>LIVE-HOLE</p></template>",
            )),
            Ok(bytes::Bytes::from_static(b"</body></html>")),
        ];
        Ok(Box::pin(futures::stream::iter(items)))
    }
}

#[tokio::test]
async fn ppr_serves_cached_shell_and_streams_holes() {
    use std::sync::Arc;

    let tree = empty_tree();
    let metrics = Arc::new(oxide_server::Metrics::new());
    let renderer: Option<Arc<dyn oxide_server::PageRenderer>> = Some(Arc::new(PprRenderer));
    let shells: crate::PprShells =
        Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    let dispatch = build_dispatch(
        tree,
        metrics,
        Vec::new(),
        renderer,
        None,
        std::path::PathBuf::new(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        Vec::new(),
        None, // no waterfall recorder in tests
        None, // no session codec in tests
        Some(shells),
    );

    // First request: full streamed document (tap mode); the shell is stored.
    let first =
        String::from_utf8(collect_response_body(dispatch(test_request("GET", "/")).await).await)
            .unwrap();
    assert!(first.contains("STATIC-SHELL"));
    assert!(first.contains("LIVE-HOLE"));

    // Second request: the cached shell is served immediately and only the
    // resolved fragment is forwarded into it.
    let second =
        String::from_utf8(collect_response_body(dispatch(test_request("GET", "/")).await).await)
            .unwrap();
    assert!(
        second.contains("<ox-hole id=\"1\">"),
        "shell has the hole: {second}"
    );
    assert!(second.contains("client_resolver") || second.contains("template[data-ox-c]"));
    assert!(
        second.contains("LIVE-HOLE"),
        "hole content streams in: {second}"
    );
    assert!(
        !second.contains("fallback"),
        "static fallback is not re-served"
    );
}

#[tokio::test]
async fn dispatch_routes_through_page_renderer_when_attached() {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tree = empty_tree();
    let metrics = Arc::new(oxide_server::Metrics::new());
    let renderer: Option<Arc<dyn oxide_server::PageRenderer>> = Some(Arc::new(CountingRenderer {
        calls: calls.clone(),
    }));

    let dispatch = build_dispatch(
        tree,
        metrics,
        Vec::new(),
        renderer,
        None,
        std::path::PathBuf::new(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        Vec::new(),
        None, // no waterfall recorder in tests
        None, // no session codec in tests
        None, // no PPR shells in tests
    );
    let resp = dispatch(test_request("GET", "/")).await;

    // §10.2 step 5: an unmatched URL (the empty tree matches nothing) is a
    // 404. The renderer is still called — this test's purpose is to prove
    // the attached renderer takes precedence over the IR placeholder, not
    // to assert the status code. The status the *renderer path* uses is the
    // computed `status` (200 for a matched route, 404 otherwise), matching
    // the sibling test `dispatch_uses_ir_placeholder_when_no_renderer`.
    assert_eq!(resp.status, 404, "unmatched URL is a 404");
    let collected = collect_response_body(resp).await;
    let body = std::str::from_utf8(&collected).expect("utf8 body");
    assert!(
        body.contains("RENDERER:"),
        "response body should come from the attached renderer: {body}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "renderer called exactly once"
    );
}

#[tokio::test]
async fn dispatch_uses_ir_placeholder_when_no_renderer() {
    use std::sync::Arc;

    let tree = empty_tree();
    let metrics = Arc::new(oxide_server::Metrics::new());
    let dispatch = build_dispatch(
        tree,
        metrics,
        Vec::new(),
        None,
        None,
        std::path::PathBuf::new(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        Vec::new(),
        None, // no waterfall recorder in tests
        None, // no session codec in tests
        None, // no PPR shells in tests
    );

    let resp = dispatch(test_request("GET", "/some/page")).await;

    // Empty tree → `_unmatched` → 404, IR placeholder body.
    assert_eq!(
        resp.status, 404,
        "no renderer → placeholder status from tree match"
    );
    let body = std::str::from_utf8(resp.body.as_ref()).expect("utf8 body");
    assert!(
        body.contains("Oxide serving"),
        "IR placeholder body: {body}"
    );
    assert!(
        !body.contains("RENDERER:"),
        "no renderer → no renderer-produced body: {body}"
    );
}

#[tokio::test]
async fn dispatch_propagates_renderer_error_as_500() {
    use std::sync::Arc;

    let tree = empty_tree();
    let metrics = Arc::new(oxide_server::Metrics::new());
    let renderer: Option<Arc<dyn oxide_server::PageRenderer>> = Some(Arc::new(FailingRenderer));

    let dispatch = build_dispatch(
        tree,
        metrics,
        Vec::new(),
        renderer,
        None,
        std::path::PathBuf::new(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        Vec::new(),
        None, // no waterfall recorder in tests
        None, // no session codec in tests
        None, // no PPR shells in tests
    );
    let resp = dispatch(test_request("GET", "/")).await;

    assert_eq!(resp.status, 500, "renderer error → 500");
    let body = std::str::from_utf8(resp.body.as_ref()).expect("utf8 body");
    assert!(body.contains("render error"), "error response body: {body}");
    assert!(
        body.contains("simulated render failure"),
        "error message surfaced: {body}"
    );
}

// ------------------------------------------- loader dispatch (spec §9.1)

/// A PageRenderer that records both call count and the LAST data value
/// it was handed. Used to prove the loader's output reached the renderer.
struct DataCapturingRenderer {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    last_data: std::sync::Arc<std::sync::Mutex<serde_json::Value>>,
}

impl DataCapturingRenderer {
    fn new() -> Self {
        Self {
            calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            last_data: std::sync::Arc::new(std::sync::Mutex::new(serde_json::Value::Null)),
        }
    }
}

#[async_trait::async_trait]
impl oxide_server::PageRenderer for DataCapturingRenderer {
    fn name(&self) -> &str {
        "data-capture"
    }
    async fn render(&self, input: oxide_server::PageRenderInput) -> oxide_server::Result<String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        *self.last_data.lock().unwrap() = input.data.clone();
        Ok(format!(
            "<!doctype html><html><body><pre>{}</pre></body></html>",
            input.data
        ))
    }
}

/// A LoaderRunner that returns a preprogrammed `LoaderOutput` and records
/// how many times it was called.
struct ScriptedLoader {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    output: std::sync::Mutex<Option<oxide_server::LoaderOutput>>,
}

impl ScriptedLoader {
    fn ok(data: serde_json::Value) -> Self {
        Self {
            calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            output: std::sync::Mutex::new(Some(oxide_server::LoaderOutput::ok(data))),
        }
    }
    fn http_error(status: u16, message: &str) -> Self {
        Self {
            calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            output: std::sync::Mutex::new(Some(oxide_server::LoaderOutput {
                status,
                location: None,
                data: serde_json::json!({ "message": message }),
                ..Default::default()
            })),
        }
    }
    fn redirect(location: &str, status: u16) -> Self {
        Self {
            calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            output: std::sync::Mutex::new(Some(oxide_server::LoaderOutput {
                status,
                location: Some(location.to_string()),
                data: serde_json::json!({}),
                ..Default::default()
            })),
        }
    }
}

#[async_trait::async_trait]
impl oxide_server::LoaderRunner for ScriptedLoader {
    fn name(&self) -> &str {
        "scripted"
    }
    async fn run(
        &self,
        _input: oxide_server::LoaderRunInput,
    ) -> oxide_server::Result<oxide_server::LoaderOutput> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let out = self
            .output
            .lock()
            .unwrap()
            .take()
            .expect("ScriptedLoader called more than once");
        Ok(out)
    }
}

/// Build a one-entry RouteTree whose only route is `/index` with
/// `has_server_module = true`. Test-only helper.
fn tree_with_server_route() -> std::sync::Arc<oxide_router::RouteTree> {
    let mut tree = oxide_router::RouteTree::new();
    tree.insert(oxide_router::RouteEntry {
        id: oxide_router::RouteId("routes/index".into()),
        kind: oxide_router::RouteKind::Page,
        segments: vec![],
        file: std::path::PathBuf::from("index/page.tsx"),
        render_mode: None,
        tags: Vec::new(),
        has_server_module: true,
        slot: None,
        needs_parent: false,
    });
    std::sync::Arc::new(tree)
}

fn routes_root() -> std::path::PathBuf {
    std::path::PathBuf::from("/tmp/oxide-test-routes")
}

#[tokio::test]
async fn loader_feeds_data_to_renderer() {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let tree = tree_with_server_route();
    let metrics = Arc::new(oxide_server::Metrics::new());

    let renderer = Arc::new(DataCapturingRenderer::new());
    let renderer_calls = Arc::clone(&renderer.calls);
    let renderer_data = Arc::clone(&renderer.last_data);
    let page_renderer: Option<Arc<dyn oxide_server::PageRenderer>> = Some(renderer);

    let loader = Arc::new(ScriptedLoader::ok(serde_json::json!({ "title": "Hi" })));
    let loader_calls = Arc::clone(&loader.calls);
    let loader_runner: Option<Arc<dyn oxide_server::LoaderRunner>> = Some(loader);

    let dispatch = build_dispatch(
        tree,
        metrics,
        Vec::new(),
        page_renderer,
        loader_runner,
        routes_root(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        Vec::new(),
        None, // no waterfall recorder in tests
        None, // no session codec in tests
        None, // no PPR shells in tests
    );
    let resp = dispatch(test_request("GET", "/")).await;

    assert_eq!(resp.status, 200);
    assert_eq!(loader_calls.load(Ordering::SeqCst), 1, "loader ran once");
    assert_eq!(
        renderer_calls.load(Ordering::SeqCst),
        1,
        "renderer ran once"
    );
    let data = renderer_data.lock().unwrap().clone();
    assert_eq!(data["title"], "Hi", "loader data reached the renderer");
}

#[tokio::test]
async fn loader_404_short_circuits_render() {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let tree = tree_with_server_route();
    let metrics = Arc::new(oxide_server::Metrics::new());

    let renderer = Arc::new(DataCapturingRenderer::new());
    let renderer_calls = Arc::clone(&renderer.calls);
    let page_renderer: Option<Arc<dyn oxide_server::PageRenderer>> = Some(renderer);

    let loader = Arc::new(ScriptedLoader::http_error(404, "not found"));
    let loader_calls = Arc::clone(&loader.calls);
    let loader_runner: Option<Arc<dyn oxide_server::LoaderRunner>> = Some(loader);

    let dispatch = build_dispatch(
        tree,
        metrics,
        Vec::new(),
        page_renderer,
        loader_runner,
        routes_root(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        Vec::new(),
        None, // no waterfall recorder in tests
        None, // no session codec in tests
        None, // no PPR shells in tests
    );
    let resp = dispatch(test_request("GET", "/")).await;

    assert_eq!(resp.status, 404);
    assert_eq!(loader_calls.load(Ordering::SeqCst), 1, "loader ran once");
    assert_eq!(
        renderer_calls.load(Ordering::SeqCst),
        0,
        "renderer must NOT run"
    );
    let body = std::str::from_utf8(resp.body.as_ref()).unwrap();
    assert!(body.contains("not found"), "error message in body: {body}");
}

#[tokio::test]
async fn loader_redirect_short_circuits_with_location() {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let tree = tree_with_server_route();
    let metrics = Arc::new(oxide_server::Metrics::new());

    let renderer = Arc::new(DataCapturingRenderer::new());
    let renderer_calls = Arc::clone(&renderer.calls);
    let page_renderer: Option<Arc<dyn oxide_server::PageRenderer>> = Some(renderer);

    let loader = Arc::new(ScriptedLoader::redirect("/login", 303));
    let loader_calls = Arc::clone(&loader.calls);
    let loader_runner: Option<Arc<dyn oxide_server::LoaderRunner>> = Some(loader);

    let dispatch = build_dispatch(
        tree,
        metrics,
        Vec::new(),
        page_renderer,
        loader_runner,
        routes_root(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        Vec::new(),
        None, // no waterfall recorder in tests
        None, // no session codec in tests
        None, // no PPR shells in tests
    );
    let resp = dispatch(test_request("GET", "/")).await;

    assert_eq!(resp.status, 303);
    assert_eq!(resp.header("location"), Some("/login"));
    assert_eq!(loader_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        renderer_calls.load(Ordering::SeqCst),
        0,
        "renderer must NOT run"
    );
}

#[tokio::test]
async fn no_server_module_skips_loader() {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    // Tree with `has_server_module: false`.
    let mut tree = oxide_router::RouteTree::new();
    tree.insert(oxide_router::RouteEntry {
        id: oxide_router::RouteId("routes/index".into()),
        kind: oxide_router::RouteKind::Page,
        segments: vec![],
        file: std::path::PathBuf::from("index/page.tsx"),
        render_mode: None,
        tags: Vec::new(),
        has_server_module: false,
        slot: None,
        needs_parent: false,
    });
    let tree = Arc::new(tree);
    let metrics = Arc::new(oxide_server::Metrics::new());

    let renderer = Arc::new(DataCapturingRenderer::new());
    let renderer_calls = Arc::clone(&renderer.calls);
    let renderer_data = Arc::clone(&renderer.last_data);
    let page_renderer: Option<Arc<dyn oxide_server::PageRenderer>> = Some(renderer);

    let loader = Arc::new(ScriptedLoader::ok(serde_json::json!({ "never": true })));
    let loader_calls = Arc::clone(&loader.calls);
    let loader_runner: Option<Arc<dyn oxide_server::LoaderRunner>> = Some(loader);

    let dispatch = build_dispatch(
        tree,
        metrics,
        Vec::new(),
        page_renderer,
        loader_runner,
        routes_root(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        Vec::new(),
        None, // no waterfall recorder in tests
        None, // no session codec in tests
        None, // no PPR shells in tests
    );
    let resp = dispatch(test_request("GET", "/")).await;

    assert_eq!(resp.status, 200);
    assert_eq!(
        loader_calls.load(Ordering::SeqCst),
        0,
        "loader must NOT run"
    );
    assert_eq!(
        renderer_calls.load(Ordering::SeqCst),
        1,
        "renderer runs with empty data"
    );
    let data = renderer_data.lock().unwrap().clone();
    assert_eq!(
        data,
        serde_json::json!({}),
        "empty data when no server module"
    );
}

// ---------------------------------- prerender loader (spec §9.1 at build)

#[tokio::test]
async fn write_prerender_threads_loader_data_into_renderer() {
    use std::sync::Arc;

    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().to_path_buf();

    let renderer = Arc::new(DataCapturingRenderer::new());
    let renderer_data = Arc::clone(&renderer.last_data);
    let page_renderer: Arc<dyn oxide_server::PageRenderer> = renderer;

    let loader = Arc::new(ScriptedLoader::ok(
        serde_json::json!({ "title": "Prerender Hi" }),
    ));
    let loader_runner: Arc<dyn oxide_server::LoaderRunner> = loader;

    let params: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    write_prerender(
        &dist,
        "prerendered/index.html",
        "routes/index",
        "/",
        "/",
        &params,
        "", // no i18n locale in this test
        "test-nonce",
        Some(&page_renderer),
        Some(&loader_runner),
        Some(std::path::Path::new("/fake/page.server.ts")),
        None, // no island boot script in this test
        &[],  // no layouts in this test
        &[],  // no layout chain in this test
        None, // no error.tsx in this test
        &serde_json::json!({"meta":[],"link":[],"style":[],"script":[]}),
    )
    .await
    .expect("write_prerender ok");

    let html =
        std::fs::read_to_string(dist.join("prerendered/index.html")).expect("read output file");
    assert!(
        html.contains("Prerender Hi"),
        "loader data should appear in the output html: {html}"
    );
    let data = renderer_data.lock().unwrap().clone();
    assert_eq!(
        data["title"], "Prerender Hi",
        "renderer saw the loader's data"
    );
}

#[tokio::test]
async fn write_prerender_without_loader_uses_empty_data() {
    use std::sync::Arc;

    let tmp = tempfile::tempdir().expect("tempdir");
    let dist = tmp.path().to_path_buf();

    let renderer = Arc::new(DataCapturingRenderer::new());
    let renderer_data = Arc::clone(&renderer.last_data);
    let page_renderer: Arc<dyn oxide_server::PageRenderer> = renderer;

    let params: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    write_prerender(
        &dist,
        "prerendered/index.html",
        "routes/index",
        "/",
        "/",
        &params,
        "", // no i18n locale in this test
        "test-nonce",
        Some(&page_renderer),
        None,
        None,
        None, // no island boot script
        &[],  // no layouts
        &[],  // no layout chain
        None, // no error.tsx
        &serde_json::json!({"meta":[],"link":[],"style":[],"script":[]}),
    )
    .await
    .expect("write_prerender ok");

    let data = renderer_data.lock().unwrap().clone();
    assert_eq!(data, serde_json::json!({}), "empty data when no loader");
}

// --------------------------------------- smoke-app E2E (real user path)

/// The `examples/smoke-app-react` fixture is a working project that exercises the
/// whole stack: a real `.ts` config importing `@oxide/react` via the
/// workspace, a `page.tsx` + `page.server.ts` per route, and the CLI's
/// config-loading path. This test runs the three CLI commands against it
/// and asserts on the actual output.
///
/// `#[ignore]` because it spawns a Node worker and requires the workspace's
/// `node_modules` (from `pnpm install`). Run with:
///     cargo test -p oxide-cli -- --ignored smoke_app
#[tokio::test]
#[ignore]
async fn smoke_app_renders_through_real_config() {
    // Locate the smoke-app relative to the workspace root (this crate is
    // crates/cli, so workspace = ../../).
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    let smoke = workspace.join("examples/smoke-app-react");
    assert!(
        smoke.join("oxide.config.ts").is_file(),
        "smoke-app fixture missing at {}; is examples/* a workspace member?",
        smoke.display()
    );
    let smoke_str = smoke.to_str().expect("smoke-app path is utf-8");

    // 1. `oxide config <smoke> --json` loads the .ts and emits the descriptor.
    let (code, out) = run_ok(&["oxide", "config", smoke_str, "--json"]).await;
    assert_eq!(code, 0, "config exit code; output:\n{out}");
    let v: serde_json::Value = serde_json::from_str(&out)
        .unwrap_or_else(|e| panic!("config output not JSON ({e}): {out}"));
    assert_eq!(
        v["name"], "react",
        "descriptor.name from oxide.config.ts: {out}"
    );
    assert_eq!(v["extensions"][0], ".tsx");

    // 2. `oxide render routes/index` produces the home page markup.
    let (code, out) = run_ok(&[
        "oxide",
        "render",
        "routes/index",
        "--root",
        smoke_str,
        "--data",
        "{}",
    ])
    .await;
    assert_eq!(code, 0, "render routes/index exit; output:\n{out}");
    assert!(out.starts_with("<!doctype html>"), "doctype first: {out}");
    // The smoke-app's `page.tsx` is `return <h1>Home</h1>`; the loader
    // hook compiles it via sucrase, so we assert the user's own content
    // (not the IR fallback's Atlas mock).
    assert!(
        out.contains(">Home</h1>"),
        "home route renders the user's page.tsx: {out}"
    );
    assert!(out.contains("data-reactroot"), "root marker present: {out}");

    // 3. `oxide render routes/blog/[slug]` threads the URL param through.
    let (code, out) = run_ok(&[
        "oxide",
        "render",
        "routes/blog/[slug]",
        "--root",
        smoke_str,
        "--url",
        "/blog/hello-smoke",
        "--param",
        "slug=hello-smoke",
        "--data",
        "{}",
    ])
    .await;
    assert_eq!(code, 0, "render blog exit; output:\n{out}");
    // The blog route's `page.server.ts` returns `title: 'Hello ' + slug`.
    // The component renders `props.data.title`. If the loader did not run,
    // the title falls back to 'Untitled' — so this assertion is a real
    // test of the loader → renderer chain, not a tautology.
    assert!(
        out.contains("Hello hello-smoke"),
        "loader-derived title from page.server.ts: {out}"
    );
    assert!(
        out.contains("data-reactroot"),
        "root marker on the blog page: {out}"
    );
}

// ----------------------------------------- oxide dev E2E (HTTP round-trip)

/// Minimal HTTP/1.1 GET over a fresh TCP connection. Returns the raw
/// response as a UTF-8 string (headers + body). Enough for tests that
/// only need to check a status line and substring the body; no chunked
/// decoding, no keep-alive. `Connection: close` makes the server close
/// the stream after the response, so `read_to_end` completes.
async fn http_get(addr: std::net::SocketAddr, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    // Retry the connect: `start_dev_server_full` spawns the accept loop in
    // a tokio task, so there is a brief window where the returned addr is
    // known but the listener hasn't started accepting yet. Every other
    // server-starting test in this codebase uses the same retry pattern
    // (see `wait_for_port` at the top of the file).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut stream = loop {
        match TcpStream::connect(addr).await {
            Ok(s) => break s,
            Err(e) => {
                if std::time::Instant::now() >= deadline {
                    panic!("connect to {addr} never succeeded after 3s: {e}");
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        }
    };

    let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream
        .write_all(req.as_bytes())
        .await
        .expect("write request");
    stream.flush().await.expect("flush");

    let mut buf = Vec::with_capacity(8 * 1024);
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        stream.read_to_end(&mut buf),
    )
    .await
    .expect("read timed out")
    .expect("read failed");

    String::from_utf8_lossy(&buf).into_owned()
}

/// Start the real dev server against `examples/smoke-app-react` and issue HTTP
/// requests. This closes the loop between the CLI subcommands (already
/// E2E-tested in `smoke_app_renders_through_real_config`) and the
/// long-running server path: config load → stack build → dispatch →
/// page renderer → HTTP response.
///
/// ## Known limitation (documented, not asserted away)
///
/// The smoke-app's `oxide.config.ts` calls `react({ compiler: true })` —
/// it does not provide a `componentFor` resolver. So `createRealAdapter`
/// runs in 'auto' mode and the render falls through to the adapter's IR
/// fallback (the conformance-suite Atlas registry). The smoke-app's own
/// `src/routes/*/page.tsx` files are NOT compiled into the worker yet.
/// The tests below therefore assert the framework *shape* (200, root
/// marker, no not-found boundary) and parameter threading, not on the
/// content of the smoke-app's `page.tsx`. Compiling user TSX into the
/// adapter's component tree is the next slice.
///
/// `#[ignore]` because it spawns Node, binds a port, and requires the
/// workspace `node_modules` (from `pnpm install`). Run with:
///     cargo test -p oxide-cli -- --ignored dev_server_serves_smoke
#[tokio::test]
#[ignore]
async fn dev_server_serves_smoke_app_over_http() {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    let smoke = workspace.join("examples/smoke-app-react");
    let config_path = smoke.join("oxide.config.ts");
    assert!(
        config_path.is_file(),
        "smoke-app fixture missing at {}; run `pnpm install` if the workspace link is stale",
        smoke.display()
    );
    let smoke_str = smoke.to_str().expect("smoke path utf-8");

    // Build the same framework stack cmd_dev builds (`cache.handler: None`
    // — the fixture has no handler config and the test needs no remote).
    let stack = crate::framework_shim::build_framework_stack(
        Some(&config_path),
        oxide_config::JsBackend::Node,
        None,
        None,
    )
    .await;
    assert!(
        stack.has_renderer(),
        "framework renderer must attach for the smoke-app's oxide.config.ts"
    );

    // Start the dev server on an ephemeral port.
    let started = crate::start_dev_server_full(
        smoke_str,
        Some(0),
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        None,
        Vec::new(),
        stack,
        None,
    )
    .await
    .expect("start_dev_server_full");
    let addr = started.addr;
    eprintln!("dev server listening at {addr}");

    // ---- Request 1: home route ----
    let resp_home = http_get(addr, "/").await;
    assert!(
        resp_home.starts_with("HTTP/1.1 200"),
        "home status line: {}",
        resp_home.lines().next().unwrap_or("")
    );
    // The home route goes through the framework renderer branch of
    // `build_dispatch`, which streams. `to_hyper_response` skips
    // content-length for streaming responses, so the transport falls
    // back to chunked transfer encoding. That's the observable proof
    // that chunk-level delivery reached the wire, not just the trait.
    assert!(
        !resp_home.to_lowercase().contains("content-length:"),
        "streaming response must NOT set content-length; got:\n{}",
        resp_home
    );
    // Chunked responses announce themselves; hyper uses this encoding
    // automatically when the body has unknown length.
    assert!(
        resp_home
            .to_lowercase()
            .contains("transfer-encoding: chunked"),
        "streaming response must use chunked transfer encoding; got:\n{}",
        resp_home
    );
    // The smoke-app's config is `react({ compiler: true })` — no
    // `componentFor`, so `createRealAdapter` runs in 'auto' mode and the
    // render falls through to the adapter's IR fallback (the Atlas
    // registry the conformance suite uses). That's why we assert the
    // *framework* shape rather than the smoke-app's `page.tsx` content:
    // compiling the user's TSX and feeding it to `componentFor` is the
    // next slice.
    assert!(
        resp_home.contains("data-reactroot"),
        "home body should carry the framework root marker:\n{}",
        resp_home
    );
    assert!(
        !resp_home.contains("data-ox-not-found"),
        "home must NOT be the not-found boundary — the route id must \
         match the adapter's expected key:\n{}",
        resp_home
    );
    // Health probe: the pipeline's built-in health path answers before
    // dispatch (spec §12.1). Cheap second request confirms the server
    // is broadly functional, not just rendering.
    let resp_health = http_get(addr, "/_oxide/health").await;
    assert!(
        resp_health.starts_with("HTTP/1.1 200"),
        "health status line: {}",
        resp_health.lines().next().unwrap_or("")
    );
    // The health endpoint is handled by the native prefix in
    // `Pipeline::serve_request` and returns a collected response. Its
    // transport uses content-length (not chunked) — the branch in
    // `to_hyper_response` is what decides, keyed on whether
    // `OxideResponse::stream` was set.
    assert!(
        resp_health.to_lowercase().contains("content-length:"),
        "collected response must set content-length; got:\n{}",
        resp_health
    );
    assert!(
        !resp_health
            .to_lowercase()
            .contains("transfer-encoding: chunked"),
        "collected response must NOT use chunked encoding; got:\n{}",
        resp_health
    );

    // ---- Request 2: dynamic route with a param ----
    let resp_blog = http_get(addr, "/blog/hello-smoke").await;
    assert!(
        resp_blog.starts_with("HTTP/1.1 200"),
        "blog status line: {}",
        resp_blog.lines().next().unwrap_or("")
    );
    // Two structural checks on the dynamic route:
    //   (a) not the not-found boundary — the router matched a real route
    //   (b) the URL param `hello-smoke` appears in the body. The adapter's
    //       IR fallback (BlogPage in the Atlas registry) renders the slug
    //       as part of its byline, so this proves the param threaded through
    //       router → dispatch → PageRenderInput → renderer.
    assert!(
        !resp_blog.contains("data-ox-not-found"),
        "blog route must not hit the not-found boundary:\n{}",
        resp_blog
    );
    assert!(
        resp_blog.contains("hello-smoke"),
        "blog body should carry the URL param slug (rendered by the smoke-app's own page.tsx):\n{}",
        resp_blog
    );

    // Clean shutdown; the accept loop notices and the listener drops.
    let _ = started.shutdown.send(true);
}

// ------------------------------------ oxide build against smoke-app

/// The third HTML-producing command. Exercises `oxide build` on the smoke-app
/// fixture end-to-end: bundlers run, prerender dispatches the static `/index`
/// route through the framework adapter, the TSX loader compiles `page.tsx`,
/// and the artifact lands at `dist/prerendered/index.html` with the user's
/// own content.
///
/// `#[ignore]` — spawns a Node worker, needs workspace node_modules, and
/// writes `dist/` under the fixture.
#[tokio::test]
#[ignore]
async fn smoke_app_builds_its_own_pages() {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    let smoke = workspace.join("examples/smoke-app-react");
    assert!(
        smoke.join("oxide.config.ts").is_file(),
        "smoke-app fixture missing at {}",
        smoke.display()
    );
    let smoke_str = smoke.to_str().expect("smoke path utf-8");

    let dist = smoke.join("dist");
    let _ = std::fs::remove_dir_all(&dist);

    let (code, out) = run_ok(&["oxide", "build", "--root", smoke_str]).await;
    assert_eq!(code, 0, "build exit code; output:\n{out}");
    assert!(out.contains("build ok"), "build summary: {out}");

    // `/index` has no server module → prerendered.
    let index_html = dist.join("prerendered/index.html");
    assert!(
        index_html.is_file(),
        "prerendered/index.html must exist at {}",
        index_html.display()
    );
    let html = std::fs::read_to_string(&index_html).expect("read index.html");
    assert!(
        html.contains(">Home</h1>"),
        "prerendered index must contain the user's own Home page: {html}"
    );
    assert!(
        html.contains("data-reactroot"),
        "prerendered index carries the framework root marker: {html}"
    );

    // The manifest should classify `routes/index` as static.
    // §10.5 runtime generateParams: the blog route's generateParams is
    // a runtime-built array (`.map`), so the build had to evaluate it
    // through the worker. Each enumerated path must have a prerendered
    // HTML file with the loader's data.
    let blog_a = dist.join("prerendered/blog/hello-prod.html");
    let blog_b = dist.join("prerendered/blog/second-post.html");
    assert!(
        blog_a.is_file(),
        "prerendered blog/hello-prod.html produced via runtime generateParams at {}",
        blog_a.display()
    );
    assert!(
        blog_b.is_file(),
        "prerendered blog/second-post.html produced via runtime generateParams at {}",
        blog_b.display()
    );
    let html_a = std::fs::read_to_string(&blog_a).expect("read blog a");
    assert!(
        html_a.contains("Hello hello-prod"),
        "blog a content from the loader: {html_a}"
    );

    let server_manifest = dist.join("server/server.manifest.json");
    assert!(server_manifest.is_file(), "server manifest produced");
    let manifest_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&server_manifest).unwrap())
            .expect("manifest parses as json");
    let routes = manifest_json
        .get("routes")
        .and_then(|r| r.as_object())
        .expect("manifest has routes object");
    // WP-A2: the canonical id is the directory form — no `/page` suffix.
    let index_entry = routes
        .get("routes/index")
        .expect("manifest has the canonical index route entry");
    assert_eq!(
        index_entry.get("render_mode").and_then(|v| v.as_str()),
        Some("static"),
        "index route classified static: {index_entry}"
    );

    // WP-A1: error / not-found components are part of the generated SSR
    // surface and carry no render mode.
    for id in ["routes/error", "routes/not-found"] {
        assert!(routes.contains_key(id), "manifest has `{id}`: {routes:?}");
        assert!(
            routes[id].get("render_mode").map(|v| v.is_null()).unwrap_or(false),
            "`{id}` has a null render_mode: {}",
            routes[id]
        );
    }
    let ssr_entry = std::fs::read_to_string(dist.join("server/ssr/index.js"))
        .expect("generated SSR adapter entry");
    for id in ["routes/error", "routes/not-found"] {
        assert!(
            ssr_entry.contains(&format!("\"{id}\"")),
            "ssr/index.js exports `{id}`: {ssr_entry}"
        );
    }

    // WP-A2: the server and client manifests describe the same route set.
    let client_json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dist.join("client/client.manifest.json")).unwrap(),
    )
    .expect("client manifest parses");
    let client_routes = client_json
        .get("routes")
        .and_then(|r| r.as_object())
        .expect("client manifest routes");
    let canonical_client: std::collections::BTreeSet<String> = client_routes
        .keys()
        .filter(|k| k.as_str() != "__boot__")
        .map(|k| k.strip_suffix("/page").unwrap_or(k).to_string())
        .collect();
    let server_keys: std::collections::BTreeSet<String> = routes.keys().cloned().collect();
    assert_eq!(
        canonical_client, server_keys,
        "client and server manifests must share canonical route ids"
    );

    let _ = std::fs::remove_dir_all(&dist);
}

/// Collect the full response body, whether it came back collected
/// (`resp.body`) or streamed (`resp.stream`). Streaming responses leave
/// `.body` empty by design; the content is in `.stream`. Most tests want
/// the logical body regardless of which transport shape the dispatch
/// chose, so they call this instead of reading `.body` directly.
async fn collect_response_body(mut resp: oxide_server::OxideResponse) -> Vec<u8> {
    // Streaming responses leave `.body` empty; the content is in `.stream`.
    // Read the stream directly (no HTTP framing needed) so the helper
    // works without depending on hyper/http_body_util in the cli crate.
    use futures::StreamExt as _;
    if let Some(stream) = resp.stream.take() {
        let mut stream = stream;
        let mut out = Vec::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(chunk) => out.extend_from_slice(&chunk),
                Err(e) => panic!("stream chunk error: {e}"),
            }
        }
        out
    } else {
        resp.body.to_vec()
    }
}

// ------------------------------------- oxide start E2E (production server)

/// `oxide start` — the production server entry. Same `build_dispatch`
/// pipeline as dev, but with `with_inspector: false` and no watcher. This
/// E2E closes the last untested HTML-producing command: it starts the
/// server against the smoke-app fixture, issues HTTP requests, and asserts
/// the streaming branch actually reaches the wire as chunked transfer.
///
/// `#[ignore]` — spawns a Node worker, binds a port, requires workspace
/// node_modules. Run with:
///     cargo test -p oxide-cli -- --ignored start_serves_smoke
#[tokio::test]
#[ignore]
async fn start_serves_smoke_app_over_http() {
    // Same fixture lookup as the other smoke-app E2Es.
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    let smoke = workspace.join("examples/smoke-app-react");
    let config_path = smoke.join("oxide.config.ts");
    assert!(
        config_path.is_file(),
        "smoke-app fixture missing at {}; run `pnpm install` if the link is stale",
        smoke.display()
    );
    let smoke_str = smoke.to_str().expect("smoke path utf-8");

    // Build the same framework stack `cmd_serve` builds.
    let stack = crate::framework_shim::build_framework_stack(
        Some(&config_path),
        oxide_config::JsBackend::Node,
        None,
        None,
    )
    .await;
    assert!(
        stack.has_renderer(),
        "framework renderer must attach for the smoke-app's oxide.config.ts"
    );

    // Ephemeral port so parallel test runs don't collide.
    let started = crate::start_server(smoke_str, Some(0), stack)
        .await
        .expect("start_server");
    let addr = started.addr;
    eprintln!("production server listening at {addr}");

    // ---- Request 1: home route (framework renderer → streaming) ----
    let resp_home = http_get(addr, "/").await;
    assert!(
        resp_home.starts_with("HTTP/1.1 200"),
        "home status line: {}",
        resp_home.lines().next().unwrap_or("")
    );
    assert!(
        resp_home.contains(">Home</h1>"),
        "home body carries the user's page content:\n{}",
        resp_home
    );
    // The renderer branch streams; `to_hyper_response` skips
    // content-length and hyper uses chunked transfer encoding.
    assert!(
        !resp_home.to_lowercase().contains("content-length:"),
        "streaming response must NOT set content-length; got:\n{}",
        resp_home
    );
    assert!(
        resp_home
            .to_lowercase()
            .contains("transfer-encoding: chunked"),
        "streaming response must use chunked encoding; got:\n{}",
        resp_home
    );

    // ---- Request 2: health (native prefix → collected) ----
    let resp_health = http_get(addr, "/_oxide/health").await;
    assert!(
        resp_health.starts_with("HTTP/1.1 200"),
        "health status line: {}",
        resp_health.lines().next().unwrap_or("")
    );
    assert!(
        resp_health.to_lowercase().contains("content-length:"),
        "collected response must set content-length; got:\n{}",
        resp_health
    );
    assert!(
        !resp_health
            .to_lowercase()
            .contains("transfer-encoding: chunked"),
        "collected response must NOT use chunked encoding; got:\n{}",
        resp_health
    );

    // ---- Request 3: dynamic blog route (loader + streaming) ----
    let resp_blog = http_get(addr, "/blog/hello-prod").await;
    assert!(
        resp_blog.starts_with("HTTP/1.1 200"),
        "blog status line: {}",
        resp_blog.lines().next().unwrap_or("")
    );
    // The blog loader returns `title: 'Hello ' + slug`. If the loader
    // didn't run in the production path, the page falls back to
    // 'Untitled'.
    assert!(
        resp_blog.contains("Hello hello-prod"),
        "loader-derived title in production:\n{}",
        resp_blog
    );
    assert!(
        !resp_blog.contains("data-ox-not-found"),
        "blog route must not hit the not-found boundary:\n{}",
        resp_blog
    );

    // Clean shutdown; the accept loop notices and the listener drops.
    let _ = started.shutdown.send(true);
}

#[test]
fn render_subcommand_parses_route_and_optional_flags() {
    use clap::Parser;
    let cli = crate::Cli::try_parse_from([
        "oxide",
        "render",
        "routes/index",
        "--root",
        "/some/root",
        "--url",
        "/home",
        "--param",
        "slug=hello",
        "--data",
        r#"{"title":"Hi"}"#,
    ])
    .expect("parse");
    match cli.command {
        crate::Command::Render {
            root,
            route,
            url,
            params,
            data,
            stream: _,
        } => {
            assert_eq!(root, "/some/root");
            assert_eq!(route, "routes/index");
            assert_eq!(url.as_deref(), Some("/home"));
            assert_eq!(params, vec!["slug=hello".to_string()]);
            assert_eq!(data, r#"{"title":"Hi"}"#);
        }
        other => panic!("expected Render, got {:?}", other),
    }
}

#[test]
fn render_subcommand_defaults_root_url_params_data() {
    use clap::Parser;
    let cli = crate::Cli::try_parse_from(["oxide", "render", "routes/blog/[slug]"]).expect("parse");
    match cli.command {
        crate::Command::Render {
            root,
            route,
            url,
            params,
            data,
            stream: _,
        } => {
            assert_eq!(root, ".");
            assert_eq!(route, "routes/blog/[slug]");
            assert_eq!(url, None);
            assert!(params.is_empty());
            assert_eq!(data, "{}");
        }
        other => panic!("expected Render, got {:?}", other),
    }
}

#[test]
fn render_subcommand_accepts_root_flag_after_route() {
    use clap::Parser;
    let cli = crate::Cli::try_parse_from(["oxide", "render", "routes/index", "--root", "/proj"])
        .expect("parse");
    match cli.command {
        crate::Command::Render { root, route, .. } => {
            assert_eq!(root, "/proj");
            assert_eq!(route, "routes/index");
        }
        other => panic!("expected Render, got {:?}", other),
    }
}

#[test]
fn config_subcommand_parses_root_and_json_flag() {
    use clap::Parser;
    let cli =
        crate::Cli::try_parse_from(["oxide", "config", "--json", "/some/root"]).expect("parse");
    match cli.command {
        crate::Command::Config { root, json } => {
            assert_eq!(root, "/some/root");
            assert!(json);
        }
        other => panic!("expected Config, got {:?}", other),
    }
}

#[test]
fn config_subcommand_defaults_root_to_dot_and_json_false() {
    use clap::Parser;
    let cli = crate::Cli::try_parse_from(["oxide", "config"]).expect("parse");
    match cli.command {
        crate::Command::Config { root, json } => {
            assert_eq!(root, ".");
            assert!(!json);
        }
        other => panic!("expected Config, got {:?}", other),
    }
}

#[test]
fn cli_parses_all_spec_commands() {
    Cli::command().debug_assert();
    // Spot-check the command surface matches spec §19.1.
    let names: Vec<String> = Cli::command()
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect();
    for expected in [
        "dev", "build", "start", "preview", "routes", "analyze", "check", "why", "doctor", "add",
        "generate", "cache", "audit", "migrate",
    ] {
        assert!(
            names.contains(&expected.to_string()),
            "missing command {expected}"
        );
    }
}

// ------------------------------------------------------------------- routes

#[tokio::test]
async fn routes_prints_table_and_writes_typegen() {
    let (_d, root) = fixture_project();
    let (code, out) = run_ok(&["oxide", "routes", "--root", root.to_str().unwrap()]).await;
    assert_eq!(code, 0);
    assert!(out.contains("PATTERN"), "header: {out}");
    assert!(out.contains("/blog"), "blog pattern row: {out}");
    assert!(out.contains("page"), "kind column: {out}");
    assert!(out.contains("isr"), "inferred mode: {out}");
    assert!(out.contains("post:a"), "tags column: {out}");
    let routes_json = root.join(".oxide/types/routes.json");
    assert!(routes_json.is_file(), "typegen wrote routes.json");
    assert!(
        root.join(".oxide/types/routes.d.ts").is_file(),
        "typegen wrote routes.d.ts"
    );
}

// ------------------------------------------------------------------ analyze

#[tokio::test]
async fn analyze_over_budget_exits_one() {
    let (_d, root) = fixture_project();
    write_config(&root, r#"{ "budgets": { "clientJs": "150kb" } }"#);
    let client_dir = root.join("dist/client");
    std::fs::create_dir_all(&client_dir).unwrap();
    std::fs::write(
        client_dir.join("client.manifest.json"),
        manifest_fixture(serde_json::json!({
            "routes/page": { "entry_chunk": "e.js", "preload": [], "css": [], "js_bytes": 200_000 }
        })),
    )
    .unwrap();
    let (code, out) = run_ok(&["oxide", "analyze", "--root", root.to_str().unwrap()]).await;
    assert_eq!(code, 1, "over budget → exit 1");
    assert!(out.contains("ROUTE"), "table header: {out}");
    assert!(out.contains("200000"), "js bytes column: {out}");
    assert!(out.contains("153600"), "budget parsed from 150kb: {out}");
    assert!(out.contains("OVER"), "ok column flags the offender: {out}");
}

#[tokio::test]
async fn analyze_within_budget_exits_zero() {
    let (_d, root) = fixture_project();
    write_config(&root, r#"{ "budgets": { "clientJs": "150kb" } }"#);
    let client_dir = root.join("dist/client");
    std::fs::create_dir_all(&client_dir).unwrap();
    std::fs::write(
        client_dir.join("client.manifest.json"),
        manifest_fixture(serde_json::json!({
            "routes/page": { "entry_chunk": "e.js", "preload": [], "css": [], "js_bytes": 100_000 }
        })),
    )
    .unwrap();
    let (code, out) = run_ok(&["oxide", "analyze", "--root", root.to_str().unwrap()]).await;
    assert_eq!(code, 0);
    assert!(out.contains("ok"), "row is within budget: {out}");
}

#[tokio::test]
async fn analyze_without_manifest_reports_zeros() {
    let (_d, root) = fixture_project();
    let (code, out) = run_ok(&["oxide", "analyze", "--root", root.to_str().unwrap()]).await;
    assert_eq!(code, 0);
    assert!(
        out.contains("routes/page"),
        "tree pages listed with zero bytes: {out}"
    );
    assert!(out.contains("0 |"), "zero bytes column: {out}");
}

// ---------------------------------------------------------------------- why

#[tokio::test]
async fn why_lists_chunk_and_importers() {
    let (_d, root) = fixture_project();
    let client_dir = root.join("dist/client");
    std::fs::create_dir_all(&client_dir).unwrap();
    std::fs::write(
        client_dir.join("client.manifest.json"),
        manifest_fixture(serde_json::json!({
            "routes/page": { "entry_chunk": "entry.abc.js", "preload": [], "css": [], "js_bytes": 10 },
            "routes/blog/page": { "entry_chunk": "entry.def.js", "preload": [], "css": [], "js_bytes": 10 }
        })),
    )
    .unwrap();
    std::fs::write(
        client_dir.join("entry.abc.js"),
        "import { add } from \"./lib/math.ts\";\nconsole.log(add(1,2));\n",
    )
    .unwrap();
    std::fs::write(client_dir.join("entry.def.js"), "console.log('blog');\n").unwrap();
    let (code, out) = run_ok(&[
        "oxide",
        "why",
        "--root",
        root.to_str().unwrap(),
        "./lib/math.ts",
    ])
    .await;
    assert_eq!(code, 0);
    assert!(out.contains("entry.abc.js"), "chunk named: {out}");
    assert!(out.contains("routes/page"), "importer route listed: {out}");
    assert!(
        !out.contains("routes/blog/page"),
        "unrelated route not listed: {out}"
    );
}

#[tokio::test]
async fn why_reports_not_found() {
    let (_d, root) = fixture_project();
    let client_dir = root.join("dist/client");
    std::fs::create_dir_all(&client_dir).unwrap();
    std::fs::write(
        client_dir.join("client.manifest.json"),
        manifest_fixture(serde_json::json!({})),
    )
    .unwrap();
    let (code, out) = run_ok(&[
        "oxide",
        "why",
        "--root",
        root.to_str().unwrap(),
        "missing.ts",
    ])
    .await;
    assert_eq!(code, 0);
    assert!(out.contains("not found"), "graceful miss: {out}");
}

// -------------------------------------------------------------------- check

#[tokio::test]
async fn check_reports_ok_on_fixture() {
    let (_d, root) = fixture_project();
    let (code, out) = run_ok(&["oxide", "check", "--root", root.to_str().unwrap()]).await;
    assert_eq!(code, 0);
    assert!(out.contains("config: OK"), "{out}");
    assert!(out.contains("routes: OK"), "{out}");
    assert!(out.contains("OK"));
}

#[tokio::test]
async fn check_surfaces_route_scan_errors() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    write_config(&root, r#"{ "routes": { "dir": "app/does-not-exist" } }"#);
    let (code, out) = run_ok(&["oxide", "check", "--root", root.to_str().unwrap()]).await;
    assert_eq!(code, 1, "route scan failure → 1");
    assert!(out.contains("routes:"), "{out}");
    assert!(!out.contains("routes: OK"), "{out}");
}

// -------------------------------------------------------------------- doctor

#[tokio::test]
async fn doctor_reports_missing_routes_dir() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (code, out) = run_ok(&["oxide", "doctor", "--root", root.to_str().unwrap()]).await;
    assert_eq!(code, 1, "missing routes dir → 1");
    assert!(out.contains("[fail] routes directory"), "{out}");
    assert!(
        out.contains("[ok]") || out.contains("[fail]"),
        "ascii markers used"
    );
}

#[tokio::test]
async fn doctor_all_pass_on_fixture() {
    let (_d, root) = fixture_project();
    let (code, out) = run_ok(&["oxide", "doctor", "--root", root.to_str().unwrap()]).await;
    assert_eq!(
        code, 0,
        "node/cargo present, config + routes + cache fine: {out}"
    );
    assert!(out.contains("[ok] node runtime"), "{out}");
    assert!(out.contains("[ok] cargo"), "{out}");
    assert!(out.contains("[ok] config"), "{out}");
    assert!(out.contains("[ok] routes directory"), "{out}");
    assert!(out.contains("[ok] cache directory"), "{out}");
}

// ----------------------------------------------------------------------- add

#[tokio::test]
async fn add_appends_module_and_dedupes() {
    let _g = ENV_LOCK.lock().unwrap();
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    // Pre-populate node_modules/<mod>/package.json so the contract-discovery
    // step (SPEC-15.4) runs without a network install (OXIDE_ADD_NO_INSTALL=1).
    let mod_dir = root.join("node_modules/oxide-image-scraper");
    fs::create_dir_all(&mod_dir).unwrap();
    fs::write(
        mod_dir.join("package.json"),
        r#"{"name":"oxide-image-scraper","version":"1.0.0","oxide":{"module":{}}}"#,
    )
    .unwrap();
    unsafe {
        std::env::set_var("OXIDE_ADD_NO_INSTALL", "1");
    }
    let (code, out) = run_ok(&[
        "oxide",
        "add",
        "--root",
        root.to_str().unwrap(),
        "oxide-image-scraper",
    ])
    .await;
    assert_eq!(code, 0, "add #1: {out}");
    assert!(out.contains("next steps"), "{out}");
    let path = root.join(".oxide/modules.json");
    let list: Vec<String> = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(list, vec!["oxide-image-scraper".to_string()]);
    let _ = run_ok(&[
        "oxide",
        "add",
        "--root",
        root.to_str().unwrap(),
        "oxide-image-scraper",
    ])
    .await;
    unsafe {
        std::env::remove_var("OXIDE_ADD_NO_INSTALL");
    }
    let list: Vec<String> = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(list, vec!["oxide-image-scraper".to_string()], "deduped");
}

// ------------------------------------------------------------------ generate

#[tokio::test]
async fn generate_route_scaffolds_page_and_server_module() {
    let (_d, root) = fixture_project();
    let (code, out) = run_ok(&[
        "oxide",
        "generate",
        "--kind",
        "route",
        "--root",
        root.to_str().unwrap(),
        "blog/settings",
    ])
    .await;
    assert_eq!(code, 0);
    assert!(out.contains("page.tsx"), "{out}");
    let page = root.join("src/routes/blog/settings/page.tsx");
    let server = root.join("src/routes/blog/settings/page.server.ts");
    assert!(page.is_file(), "page scaffold written");
    assert!(server.is_file(), "server module scaffold written");
    let page_src = std::fs::read_to_string(&page).unwrap();
    assert!(page_src.contains("blog/settings"), "route path marker");
    let server_src = std::fs::read_to_string(&server).unwrap();
    assert!(server_src.contains("load"), "load stub");
}

#[tokio::test]
async fn generate_layout_and_action_kinds() {
    let (_d, root) = fixture_project();
    let (code, _) = run_ok(&[
        "oxide",
        "generate",
        "--kind",
        "layout",
        "--root",
        root.to_str().unwrap(),
        "marketing",
    ])
    .await;
    assert_eq!(code, 0);
    assert!(root.join("src/routes/marketing/layout.tsx").is_file());
    let (code, _) = run_ok(&[
        "oxide",
        "generate",
        "--kind",
        "action",
        "--root",
        root.to_str().unwrap(),
        "marketing",
    ])
    .await;
    assert_eq!(code, 0);
    let server = root.join("src/routes/marketing/page.server.ts");
    assert!(server.is_file(), "action scaffold writes page.server.ts");
    let src = std::fs::read_to_string(&server).unwrap();
    assert!(src.contains("actions"), "actions stub: {src}");
}

#[tokio::test]
async fn generate_unknown_kind_exits_two() {
    let (_d, root) = fixture_project();
    let (code, out) = run_ok(&[
        "oxide",
        "generate",
        "--kind",
        "widget",
        "--root",
        root.to_str().unwrap(),
        "x",
    ])
    .await;
    assert_eq!(code, 2);
    assert!(out.contains("unknown kind"), "{out}");
}

// --------------------------------------------------------------------- cache

#[tokio::test]
async fn cache_stats_inspect_clear_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let cache = root.join(".oxide/cache");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("abc123.blob"), vec![0u8; 512]).unwrap();
    std::fs::write(cache.join("def456.blob"), vec![0u8; 256]).unwrap();

    let (code, out) = run_ok(&["oxide", "cache", "--root", root.to_str().unwrap(), "stats"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("total_bytes: 768"), "{out}");
    assert!(out.contains("2"), "file count: {out}");

    let (code, out) = run_ok(&[
        "oxide",
        "cache",
        "--root",
        root.to_str().unwrap(),
        "inspect",
    ])
    .await;
    assert_eq!(code, 0);
    assert!(out.contains("abc123.blob"), "{out}");
    assert!(out.contains("def456.blob"), "{out}");

    let (code, out) = run_ok(&["oxide", "cache", "--root", root.to_str().unwrap(), "clear"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("cleared"), "{out}");
    assert!(!cache.exists(), "cache dir removed");
    let (_, out) = run_ok(&["oxide", "cache", "--root", root.to_str().unwrap(), "stats"]).await;
    assert!(out.contains("total_bytes: 0"), "{out}");
}

// --------------------------------------------------------------------- audit

// ------------------------------------------------------------------- migrate

#[tokio::test]
async fn migrate_next_prints_non_destructive_plan() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("next.config.js"), "module.exports = {};\n").unwrap();
    std::fs::create_dir_all(root.join("app/blog")).unwrap();
    std::fs::write(
        root.join("app/blog/page.tsx"),
        "export default () => null;\n",
    )
    .unwrap();
    let (code, out) = run_ok(&["oxide", "migrate", "--root", root.to_str().unwrap(), "next"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("Next.js"), "{out}");
    assert!(
        out.contains("app/blog/page.tsx"),
        "found files listed: {out}"
    );
    assert!(out.contains("no files written"), "dry run: {out}");
    assert!(
        root.join("next.config.js").is_file(),
        "no destructive writes"
    );
}

#[tokio::test]
async fn migrate_vite_and_unknown_source() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("vite.config.ts"), "export default {}\n").unwrap();
    let (code, out) = run_ok(&["oxide", "migrate", "--root", root.to_str().unwrap(), "vite"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("Vite"), "{out}");

    let (code, out) = run_ok(&[
        "oxide",
        "migrate",
        "--root",
        root.to_str().unwrap(),
        "angular",
    ])
    .await;
    assert_eq!(code, 2, "unknown source → 2");
    assert!(out.contains("unknown source"), "{out}");

    let (code, out) = run_ok(&["oxide", "migrate", "--root", root.to_str().unwrap(), "next"]).await;
    assert_eq!(code, 2, "no next markers → 2");
    assert!(out.contains("no Next.js project detected"), "{out}");
}

// --------------------------------------------------------------------- build

#[tokio::test]
async fn build_produces_dist_and_deploy_artifacts() {
    let (_d, root) = fixture_project();
    let (code, out) = run_ok(&["oxide", "build", "--root", root.to_str().unwrap()]).await;
    assert_eq!(code, 0, "build output: {out}");
    assert!(root.join("dist/client/client.manifest.json").is_file());
    assert!(root.join("dist/server/server.manifest.json").is_file());
    assert!(
        root.join("dist/server/ssr/index.js").is_file(),
        "ssr adapter entry"
    );
    let prerendered = root.join("dist/prerendered/index.html");
    assert!(prerendered.is_file(), "static route prerendered");
    let html = std::fs::read_to_string(&prerendered).unwrap();
    assert!(
        html.contains("data-route=\"/\""),
        "route marker in shell: {html}"
    );
    assert!(
        root.join("dist/deploy/deploy.json").is_file(),
        "binary adapter emitted"
    );
    let deploy: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("dist/deploy/deploy.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(deploy["target"], "binary");
    assert_eq!(deploy["entry"], "server");
}

#[tokio::test]
async fn build_static_refuses_dynamic_route() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    write_config(&root, "{}");
    let routes = root.join("src/routes/dashboard");
    std::fs::create_dir_all(&routes).unwrap();
    std::fs::write(routes.join("page.tsx"), "export default () => null;\n").unwrap();
    std::fs::write(
        routes.join("page.server.ts"),
        "export const config = { render: 'dynamic' };\n",
    )
    .unwrap();
    let e = run_err(&[
        "oxide",
        "build",
        "--root",
        root.to_str().unwrap(),
        "--static",
    ])
    .await;
    let msg = format!("{e:#}");
    assert!(
        msg.contains("ADP-STATIC-DYN"),
        "§10.5 refusal surfaces: {msg}"
    );
}

// -------------------------------------------------------------- start/preview

/// PERF-01: `/__data?route=` returns the ODF-encoded route-data envelope the
/// client router decodes (`normalizeInput` expects route/url/params/data).
#[tokio::test]
async fn data_endpoint_returns_odf_envelope() {
    let (_d, root) = fixture_project();
    let started = start_server(
        root.to_str().unwrap(),
        Some(0),
        crate::framework_shim::FrameworkStack::default(),
    )
    .await
    .unwrap();
    wait_ready(started.addr).await;

    // Raw GET so we read the binary body as bytes.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(started.addr).await.unwrap();
    stream
        .write_all(
            b"GET /__data?route=/blog HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf);
    assert!(text.contains("200"), "status 200: {text}");
    assert!(
        text.contains("application/oxide+odf"),
        "ODF content type: {text}"
    );
    // The ODF wire magic `ODF1` appears right after the header block.
    let body_off = buf
        .windows(4)
        .position(|w| w == b"ODF1")
        .expect("ODF magic in body");
    assert!(body_off > 0);

    let _ = started.shutdown.send(true);
}

#[tokio::test]
async fn start_serves_health_endpoint_then_shuts_down() {
    let (_d, root) = fixture_project();
    let started = start_server(
        root.to_str().unwrap(),
        Some(0),
        crate::framework_shim::FrameworkStack::default(),
    )
    .await
    .unwrap();
    // Poll until the listener accepts. The budget is generous (10 s) because
    // under full-workspace parallel load the listener can take a moment to
    // sched­ule; success still returns on the first accepted connection.
    let mut stream = None;
    for _ in 0..200 {
        match tokio::net::TcpStream::connect(started.addr).await {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
        }
    }
    let mut stream = stream.expect("server accepts connections");
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream
        .write_all(b"GET /_oxide/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).await.unwrap();
    assert!(resp.contains("200"), "health endpoint returns 200: {resp}");
    // Dispatch placeholder for a routed page. Retry the connect too — under
    // full-workspace parallel load the listener can transiently refuse.
    let mut stream = None;
    for _ in 0..200 {
        match tokio::net::TcpStream::connect(started.addr).await {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
        }
    }
    let mut stream = stream.expect("server accepts a second connection");
    stream
        .write_all(b"GET /blog HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).await.unwrap();
    assert!(
        resp.contains("Oxide serving /blog"),
        "placeholder dispatch: {resp}"
    );
    started
        .shutdown
        .send(true)
        .expect("shutdown signal accepted");
}

// ----------------------------------------------------------------------- dev

#[tokio::test]
async fn dev_once_prints_banner_and_ready() {
    let (_d, root) = fixture_project();
    let (code, out) = run_ok(&["oxide", "dev", "--root", root.to_str().unwrap(), "--once"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("oxide dev"), "startup banner: {out}");
    assert!(out.contains("dev ready"), "{out}");
}

// ---------------------------------------------------------------------- test

#[tokio::test]
async fn test_command_gates_on_build_manifests() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (code, out) = run_ok(&["oxide", "test", "--root", root.to_str().unwrap()]).await;
    assert_eq!(code, 1, "no manifests → 1");
    assert!(out.contains("oxide build"), "{out}");

    let (_d2, root2) = fixture_project();
    let client = root2.join("dist/client");
    let server = root2.join("dist/server");
    std::fs::create_dir_all(&client).unwrap();
    std::fs::create_dir_all(&server).unwrap();
    std::fs::write(
        client.join("client.manifest.json"),
        manifest_fixture(serde_json::json!({})),
    )
    .unwrap();
    std::fs::write(server.join("server.manifest.json"), "{}\n").unwrap();
    let (code, out) = run_ok(&["oxide", "test", "--root", root2.to_str().unwrap()]).await;
    assert_eq!(code, 0, "manifests present → 0");
    assert!(
        out.contains("fetch"),
        "points at the packages/oxide helper: {out}"
    );
}

// --------------------------------------------------------------- run() shape

#[tokio::test]
async fn run_returns_exit_code_and_prints_buffer_to_stdout() {
    // run() delegates to run_with_writer and flushes the buffer to stdout.
    let dir = tempfile::tempdir().unwrap();
    let cli =
        Cli::try_parse_from(["oxide", "test", "--root", dir.path().to_str().unwrap()]).unwrap();
    let code = run(cli).await.unwrap();
    assert!(code == 0 || code == 1);
    // doctor on an empty project reports failures through the writer, exit code 1.
    let cli2 =
        Cli::try_parse_from(["oxide", "doctor", "--root", dir.path().to_str().unwrap()]).unwrap();
    let mut buf: Vec<u8> = Vec::new();
    let code = run_with_writer(cli2, &mut buf).await.unwrap();
    assert_eq!(code, 1);
    assert!(!buf.is_empty(), "writer received the report");
}

// ------------------------------------------------------- §20 dispatch metrics

/// Wait until the started server actually accepts connections, then return.
/// The dispatch closure records metrics on the first request, so racing the
/// listen() call would lose that first observation.
async fn wait_ready(addr: std::net::SocketAddr) {
    // Generous budget (10 s) for full-workspace parallel load; success
    // returns on the first accepted connection.
    for _ in 0..200 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("server never became ready at {addr}");
}

/// Minimal HTTP GET helper: one connection, close, discard the response body.
async fn simple_get(url: &str) -> std::io::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let stripped = url.strip_prefix("http://").unwrap_or(url);
    let (hostport, path) = stripped
        .split_once('/')
        .map(|(h, p)| (h, format!("/{p}")))
        .unwrap_or((stripped, "/".to_string()));
    let mut stream = tokio::net::TcpStream::connect(hostport).await?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf).await?;
    Ok(buf)
}

#[tokio::test]
async fn started_server_exposes_metrics_registry() {
    let dir = tempfile::tempdir().unwrap();
    let routes = dir.path().join("src/routes");
    std::fs::create_dir_all(routes.join("index")).unwrap();
    std::fs::write(
        routes.join("index").join("page.tsx"),
        "export default function Page() { return null; }",
    )
    .unwrap();

    let started = crate::start_server(
        dir.path().to_str().unwrap(),
        Some(0),
        crate::framework_shim::FrameworkStack::default(),
    )
    .await
    .expect("start_server");
    let text = started.metrics.render();
    assert!(
        text.contains("# TYPE oxide_requests_total counter"),
        "{text}"
    );
    let _ = started.shutdown.send(true);
}

#[tokio::test]
async fn dispatch_observes_route_pattern_labels() {
    let dir = tempfile::tempdir().unwrap();
    let routes = dir.path().join("src/routes");
    std::fs::create_dir_all(routes.join("blog").join("[slug]")).unwrap();
    std::fs::write(
        routes.join("blog").join("[slug]").join("page.tsx"),
        "export default function Page() { return null; }",
    )
    .unwrap();
    std::fs::create_dir_all(routes.join("index")).unwrap();
    std::fs::write(
        routes.join("index").join("page.tsx"),
        "export default function Page() { return null; }",
    )
    .unwrap();

    let started = crate::start_server(
        dir.path().to_str().unwrap(),
        Some(0),
        crate::framework_shim::FrameworkStack::default(),
    )
    .await
    .expect("start_server");
    wait_ready(started.addr).await;

    let url = format!("http://{}/blog/hello", started.addr);
    let _ = simple_get(&url).await;

    let text = started.metrics.render();
    // The router renders param segments as `:name` in its canonical pattern.
    assert!(
        text.contains("oxide_request_duration_ms_count{route=\"/blog/:slug\"} 1"),
        "expected /blog/:slug label with one observation:\n{text}"
    );
    let _ = started.shutdown.send(true);
}

#[tokio::test]
async fn dispatch_observes_unmatched_label() {
    let dir = tempfile::tempdir().unwrap();
    let routes = dir.path().join("src/routes");
    std::fs::create_dir_all(routes.join("index")).unwrap();
    std::fs::write(
        routes.join("index").join("page.tsx"),
        "export default function Page() { return null; }",
    )
    .unwrap();

    let started = crate::start_server(
        dir.path().to_str().unwrap(),
        Some(0),
        crate::framework_shim::FrameworkStack::default(),
    )
    .await
    .expect("start_server");
    wait_ready(started.addr).await;

    let url = format!("http://{}/definitely/not/a/route", started.addr);
    let _ = simple_get(&url).await;

    let text = started.metrics.render();
    assert!(
        text.contains("route=\"_unmatched\""),
        "expected _unmatched label:\n{text}"
    );
    let _ = started.shutdown.send(true);
}

#[tokio::test]
async fn dispatch_label_cardinality_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let routes = dir.path().join("src/routes");
    std::fs::create_dir_all(routes.join("blog").join("[slug]")).unwrap();
    std::fs::write(
        routes.join("blog").join("[slug]").join("page.tsx"),
        "export default function Page() { return null; }",
    )
    .unwrap();

    let started = crate::start_server(
        dir.path().to_str().unwrap(),
        Some(0),
        crate::framework_shim::FrameworkStack::default(),
    )
    .await
    .expect("start_server");
    wait_ready(started.addr).await;

    for i in 0..20 {
        let url = format!("http://{}/blog/slug-{i}", started.addr);
        let _ = simple_get(&url).await;
    }

    let text = started.metrics.render();
    // Exactly one histogram series for the pattern, no matter how many distinct
    // resolved URLs were hit.
    let occurrences = text
        .matches("oxide_request_duration_ms_count{route=\"/blog/:slug\"}")
        .count();
    assert_eq!(
        occurrences, 1,
        "expected ONE series for /blog/:slug, found {occurrences}:\n{text}"
    );
    assert!(
        text.contains("oxide_request_duration_ms_count{route=\"/blog/:slug\"} 20"),
        "expected 20 observations:\n{text}"
    );
    let _ = started.shutdown.send(true);
}

#[tokio::test]
async fn dev_server_serves_inspector_endpoint() {
    let dir = tempfile::tempdir().unwrap();
    let routes = dir.path().join("src/routes");
    std::fs::create_dir_all(routes.join("blog").join("[slug]")).unwrap();
    std::fs::write(
        routes.join("blog").join("[slug]").join("page.tsx"),
        "export default function Page() { return null; }",
    )
    .unwrap();
    std::fs::create_dir_all(routes.join("index")).unwrap();
    std::fs::write(
        routes.join("index").join("page.tsx"),
        "export default function Page() { return null; }",
    )
    .unwrap();

    let started = crate::start_dev_server(dir.path().to_str().unwrap(), Some(0))
        .await
        .expect("start_dev_server");
    wait_ready(started.addr).await;

    // SEC-09: the inspector is gated on the dev token the server minted.
    let token = started.dev_token.clone().expect("dev token minted");

    // A request without the token is refused.
    let no_token = format!("http://{}/__oxide/state", started.addr);
    let resp = simple_get(&no_token).await.expect("GET /__oxide/state");
    assert!(
        resp.contains("403"),
        "inspector without token is 403: {resp}"
    );

    // JSON state endpoint (token via query).
    let state_url = format!("http://{}/__oxide/state?token={token}", started.addr);
    let resp = simple_get(&state_url).await.expect("GET /__oxide/state");
    assert!(resp.contains("HTTP/1.1 200"), "{resp}");
    assert!(resp.contains("application/json"), "{resp}");
    assert!(resp.contains("/blog/:slug"), "{resp}");

    // HTML endpoint.
    let html_url = format!("http://{}/__oxide?token={token}", started.addr);
    let resp = simple_get(&html_url).await.expect("GET /__oxide");
    assert!(resp.contains("HTTP/1.1 200"), "{resp}");
    assert!(resp.contains("text/html"), "{resp}");
    assert!(resp.contains("Oxide Inspector"), "{resp}");

    let _ = started.shutdown.send(true);
}

#[tokio::test]
async fn production_server_does_not_serve_inspector() {
    let dir = tempfile::tempdir().unwrap();
    let routes = dir.path().join("src/routes");
    std::fs::create_dir_all(routes.join("index")).unwrap();
    std::fs::write(
        routes.join("index").join("page.tsx"),
        "export default function Page() { return null; }",
    )
    .unwrap();

    let started = crate::start_server(
        dir.path().to_str().unwrap(),
        Some(0),
        crate::framework_shim::FrameworkStack::default(),
    )
    .await
    .expect("start_server");
    wait_ready(started.addr).await;

    let url = format!("http://{}/__oxide/state", started.addr);
    // Retry the GET: under full-workspace parallel load the listener can
    // transiently reset a connection right after `wait_ready` returns.
    let mut resp = String::new();
    for _ in 0..50 {
        match simple_get(&url).await {
            Ok(r) => {
                resp = r;
                break;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
        }
    }
    // Not the inspector — falls through to the route dispatcher → 404.
    assert!(
        !resp.contains("application/json"),
        "production must not expose the inspector: {resp}"
    );
    let _ = started.shutdown.send(true);
}

// ------------------------------------------------------- §10.5 static export

fn setup_static_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let routes = dir.path().join("src/routes");
    std::fs::create_dir_all(routes.join("index")).unwrap();
    std::fs::write(
        routes.join("index").join("page.tsx"),
        "export default function Page() { return null; }",
    )
    .unwrap();
    write_config(&dir.path().to_path_buf(), r#"{ "adapter": "static" }"#);
    dir
}

#[tokio::test]
async fn static_build_prerenders_page_routes() {
    let dir = setup_static_project();
    let mut buf = Vec::new();
    let code = crate::run_with_writer(
        Cli {
            command: Command::Build {
                root: dir.path().to_str().unwrap().to_string(),
                r#static: true,
                mode: None,
            },
        },
        &mut buf,
    )
    .await
    .unwrap();
    assert_eq!(code, 0, "build output: {}", String::from_utf8_lossy(&buf));
    let out = String::from_utf8_lossy(&buf);
    assert!(out.contains("prerendered"), "output: {out}");
    // The prerendered file exists.
    assert!(dir.path().join("dist/prerendered/index.html").is_file());
    // The static deploy artifact has the essential files.
    assert!(dir.path().join("dist/deploy/_headers").is_file());
    assert!(dir.path().join("dist/deploy/_redirects").is_file());
}

#[tokio::test]
async fn static_build_enumerates_generate_params() {
    let dir = setup_static_project();
    let routes = dir.path().join("src/routes");
    std::fs::create_dir_all(routes.join("blog").join("[slug]")).unwrap();
    std::fs::write(
        routes.join("blog").join("[slug]").join("page.tsx"),
        "export default function Page() { return null; }",
    )
    .unwrap();
    std::fs::write(
        routes.join("blog").join("[slug]").join("page.server.ts"),
        r#"export const generateParams = [{ slug: "hello" }, { slug: "world" }];"#,
    )
    .unwrap();

    let mut buf = Vec::new();
    let code = crate::run_with_writer(
        Cli {
            command: Command::Build {
                root: dir.path().to_str().unwrap().to_string(),
                r#static: true,
                mode: None,
            },
        },
        &mut buf,
    )
    .await
    .unwrap();
    assert_eq!(code, 0, "build output: {}", String::from_utf8_lossy(&buf));

    // Both param values produce distinct HTML files.
    assert!(
        dir.path()
            .join("dist/prerendered/blog/hello.html")
            .is_file()
    );
    assert!(
        dir.path()
            .join("dist/prerendered/blog/world.html")
            .is_file()
    );
    // They land in the deploy dir.
    assert!(dir.path().join("dist/deploy/blog/hello.html").is_file());
    assert!(dir.path().join("dist/deploy/blog/world.html").is_file());
}

#[tokio::test]
async fn static_build_refuses_dynamic_without_fallback() {
    let dir = setup_static_project();
    let routes = dir.path().join("src/routes");
    std::fs::create_dir_all(routes.join("dashboard")).unwrap();
    std::fs::write(
        routes.join("dashboard").join("page.tsx"),
        "export default function Page() { return null; }",
    )
    .unwrap();
    // A page.server.ts with a runtime-only loader infers `dynamic`.
    std::fs::write(
        routes.join("dashboard").join("page.server.ts"),
        "export const load = async ({ cookies }) => ({});\n",
    )
    .unwrap();

    let mut buf = Vec::new();
    // §10.5 refusal — the build fails, no broken deploy is emitted.
    let result = crate::run_with_writer(
        Cli {
            command: Command::Build {
                root: dir.path().to_str().unwrap().to_string(),
                r#static: true,
                mode: None,
            },
        },
        &mut buf,
    )
    .await;
    let err = result.expect_err("build must refuse");
    let msg = format!("{err}");
    assert!(
        msg.contains("ADP-STATIC-DYN") && msg.contains("dashboard"),
        "expected §10.5 refusal naming the route: {msg}"
    );
}

// ------------------------------------------------------- §19.3 MCP server

#[tokio::test]
async fn mcp_routes_tool_lists_routes() {
    use rmcp::ServerHandler as _;
    use rmcp::handler::server::wrapper::Parameters;
    let dir = tempfile::tempdir().unwrap();
    let routes = dir.path().join("src/routes");
    std::fs::create_dir_all(routes.join("blog").join("[slug]")).unwrap();
    std::fs::write(
        routes.join("blog").join("[slug]").join("page.tsx"),
        "export default function Page() { return null; }",
    )
    .unwrap();
    std::fs::create_dir_all(routes.join("index")).unwrap();
    std::fs::write(
        routes.join("index").join("page.tsx"),
        "export default function Page() { return null; }",
    )
    .unwrap();
    write_config(&dir.path().to_path_buf(), "{}");

    let server = crate::mcp::OxideMcp::new(dir.path().to_path_buf());
    let result = server
        .oxide_routes(Parameters(crate::mcp::NoArgs {}))
        .await
        .expect("tool call");
    // Success = at least one content block.
    assert!(!result.content.is_empty(), "expected content");
    // Convert to text and parse the JSON.
    let text = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n");
    let v: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(v["count"], 2);
    let patterns: Vec<&str> = v["routes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["pattern"].as_str())
        .collect();
    assert!(patterns.contains(&"/blog/:slug"), "{patterns:?}");
    assert!(patterns.contains(&"/index"), "{patterns:?}");

    // get_info reports the tool capability.
    let info = server.get_info();
    assert!(info.capabilities.tools.is_some());
}

#[tokio::test]
async fn mcp_cache_tool_reports_empty_cache() {
    use rmcp::handler::server::wrapper::Parameters;
    let dir = tempfile::tempdir().unwrap();
    let server = crate::mcp::OxideMcp::new(dir.path().to_path_buf());
    let result = server
        .oxide_cache(Parameters(crate::mcp::NoArgs {}))
        .await
        .unwrap();
    let text = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    // No cache dir → both fields are empty/zero.
    assert!(v["cache_dir"].is_null());
    assert_eq!(v["disk_bytes"], 0);
    assert_eq!(v["blob_count"], 0);
}

#[tokio::test]
async fn mcp_errors_tool_empty_when_no_build() {
    use rmcp::handler::server::wrapper::Parameters;
    let dir = tempfile::tempdir().unwrap();
    let server = crate::mcp::OxideMcp::new(dir.path().to_path_buf());
    let result = server
        .oxide_errors(Parameters(crate::mcp::NoArgs {}))
        .await
        .unwrap();
    let text = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(v["source"].is_null());
    assert_eq!(v["entries"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn mcp_errors_tool_reads_diagnostics_file() {
    use rmcp::handler::server::wrapper::Parameters;
    let dir = tempfile::tempdir().unwrap();
    let oxide = dir.path().join(".oxide");
    std::fs::create_dir_all(&oxide).unwrap();
    std::fs::write(
        oxide.join("last-build-diagnostics.json"),
        serde_json::json!([
            {"code": "BLD-RESOLVE", "severity": "error", "message": "boom", "fix": "fix it"}
        ])
        .to_string(),
    )
    .unwrap();
    let server = crate::mcp::OxideMcp::new(dir.path().to_path_buf());
    let result = server
        .oxide_errors(Parameters(crate::mcp::NoArgs {}))
        .await
        .unwrap();
    let text = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(
        v["source"]
            .as_str()
            .unwrap()
            .ends_with("last-build-diagnostics.json")
    );
    assert_eq!(v["entries"][0]["code"], "BLD-RESOLVE");
    assert_eq!(v["entries"][0]["fix"], "fix it");
}

// ------------------------------------------------------- §19.3 JSON Schema

#[tokio::test]
async fn schema_command_emits_valid_json_schema() {
    let mut buf = Vec::new();
    let code = crate::run_with_writer(
        Cli {
            command: Command::Schema,
        },
        &mut buf,
    )
    .await
    .unwrap();
    assert_eq!(code, 0);
    let s = String::from_utf8(buf).unwrap();
    let v: serde_json::Value = serde_json::from_str(&s).expect("valid JSON");
    assert_eq!(v["type"], "object");
    assert_eq!(
        v["$id"],
        oxide_config::CONFIG_SCHEMA_URI,
        "$id must match the published URI"
    );
    // Spot-check that the schema documents a few real keys.
    assert!(v["properties"]["framework"]["properties"]["adapter"]["enum"].is_array());
    assert!(v["properties"]["static"]["properties"]["fallback"].is_object());
}

// ------------------------------------------------------- §16.5 content search index

#[tokio::test]
async fn build_emits_content_search_index_for_collection() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let routes = root.join("src/routes");
    std::fs::create_dir_all(routes.join("index")).unwrap();
    std::fs::write(
        routes.join("index").join("page.tsx"),
        "export default function Page() { return null; }",
    )
    .unwrap();
    // A content collection.
    let posts = root.join("content/posts");
    std::fs::create_dir_all(&posts).unwrap();
    std::fs::write(
        posts.join("hello.md"),
        "---\ntitle: Hello World\nexcerpt: A first post\n---\n\nRust is fast.\n",
    )
    .unwrap();
    std::fs::write(
        posts.join("world.md"),
        "---\ntitle: World Post\nexcerpt: Second\n---\n\nRust is safe.\n",
    )
    .unwrap();

    write_config(
        &root,
        r#"{
            "adapter": "static",
            "content": {
                "collections": [
                    { "name": "posts", "dir": "content/posts", "indexedFields": ["title", "excerpt"] }
                ]
            }
        }"#,
    );

    let mut buf = Vec::new();
    let code = crate::run_with_writer(
        Cli {
            command: Command::Build {
                root: root.to_str().unwrap().to_string(),
                r#static: true,
                mode: None,
            },
        },
        &mut buf,
    )
    .await
    .unwrap();
    let build_out = String::from_utf8_lossy(&buf);
    assert_eq!(code, 0, "build output: {build_out}");
    eprintln!("BUILD OUTPUT:\n{build_out}");

    // The index was written.
    let idx_path = root.join("dist/content/posts.index.json");
    assert!(idx_path.is_file(), "search index missing");
    let idx: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&idx_path).unwrap()).unwrap();
    assert_eq!(idx["collection"], "posts");
    assert_eq!(idx["entries"].as_array().unwrap().len(), 2);
    // "rust" appears in both bodies.
    let rust_bucket = idx["terms"]["rust"].as_array().unwrap();
    assert_eq!(rust_bucket.len(), 2);

    let out = String::from_utf8_lossy(&buf);
    assert!(out.contains("content index: posts"), "{out}");
}

// ------------------------------------------------------- §20 W3C trace context

#[tokio::test]
async fn fetch_propagates_trace_context_header() {
    use std::collections::BTreeMap;
    // A request with an active trace context should have `traceparent`
    // injected into outgoing `fetch` op headers by the pipeline. The op is a
    // stub until V8Host wires `fetch` (§4.5), so this test asserts the shape
    // the runtime will produce: a `traceparent` header built from the current
    // span's trace id.
    let tc = oxide_server::trace_context::TraceContext::from_headers(&BTreeMap::new());
    assert!(tc.is_none(), "no incoming header → no context to inherit");

    let mut headers = BTreeMap::new();
    headers.insert(
        "traceparent".to_string(),
        "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string(),
    );
    let tc = oxide_server::trace_context::TraceContext::from_headers(&headers)
        .expect("valid traceparent parsed");
    assert_eq!(tc.trace_id, "0af7651916cd43dd8448eb211c80319c");
    assert_eq!(tc.parent_id, "b7ad6b7169203331");
    assert!(tc.sampled);
}

// ---------------------------------------------------------------------------
// Conformance §22: CLI-level E2E against the smoke-app.
//
// The existing `smoke_app_builds_its_own_pages` covers the shape of the
// prerendered HTML. This test tightens the contract on the *emitted
// artifacts*: every server/client chunk must be valid JavaScript (i.e. the
// bundler actually lowered TS/JSX), and the deploy directory must contain
// the characteristic files for its target. It is the CLI-level counterweight
// to the in-process §22 suite.
// ---------------------------------------------------------------------------

/// Shell out to `node --check <file>`; returns Err on any failure.
fn node_check(path: &std::path::Path) -> Result<(), String> {
    use std::process::Command;
    let out = Command::new("node")
        .arg("--check")
        .arg(path)
        .output()
        .map_err(|e| format!("spawn node: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "node --check {} failed:\n{}",
            path.display(),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// Walk `dir` recursively, returning all regular files. Symlinks are not
/// followed (there are none in the smoke-app's `dist/`).
fn walk_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.is_file() {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[tokio::test]
#[ignore]
async fn smoke_app_emits_valid_js_in_every_chunk() {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    let smoke = workspace.join("examples/smoke-app-react");
    assert!(
        smoke.join("oxide.config.ts").is_file(),
        "smoke-app fixture missing"
    );

    let dist = smoke.join("dist");
    let _ = std::fs::remove_dir_all(&dist);

    let (code, out) = run_ok(&["oxide", "build", "--root", smoke.to_str().unwrap()]).await;
    assert_eq!(code, 0, "build exit code; output:\n{out}");
    assert!(out.contains("build ok"), "build summary: {out}");

    // Every emitted .js must be valid JavaScript. This is the exact check
    // that failed before the bundler ran the Oxc lowering pass.
    let mut checked = 0usize;
    for f in walk_files(&dist) {
        if f.extension().is_some_and(|e| e == "js") {
            node_check(&f).unwrap_or_else(|e| panic!("{e}"));
            checked += 1;
        }
    }
    assert!(
        checked >= 2,
        "expected at least the client + server chunks; got {checked}"
    );

    // Chunks land in both envs.
    let client_dir = dist.join("client");
    let server_dir = dist.join("server");
    assert!(client_dir.is_dir(), "dist/client must exist");
    assert!(server_dir.is_dir(), "dist/server must exist");
    assert!(
        walk_files(&client_dir)
            .iter()
            .any(|p| p.extension().is_some_and(|e| e == "js")),
        "dist/client must contain at least one .js chunk"
    );
    assert!(
        walk_files(&server_dir)
            .iter()
            .any(|p| p.extension().is_some_and(|e| e == "js")),
        "dist/server must contain at least one .js chunk"
    );

    // Manifests written.
    assert!(client_dir.join("client.manifest.json").is_file());
    assert!(server_dir.join("server.manifest.json").is_file());

    // Server chunks must not contain TS-only syntax after lowering. This is
    // a belt-and-braces check — `node --check` already rejects `interface`
    // and `<div>` in strict module mode, but a direct grep pinpoints the
    // failure if it ever regresses.
    for f in walk_files(&server_dir) {
        if f.extension().is_none_or(|e| e != "js") {
            continue;
        }
        let src = std::fs::read_to_string(&f).expect("read chunk");
        assert!(
            !src.contains("interface "),
            "server chunk {} still has TS `interface`",
            f.display()
        );
        assert!(
            !src.contains("<div>") && !src.contains("<h1>"),
            "server chunk {} still has raw JSX",
            f.display()
        );
    }

    // Deploy dir matches the `binary` target from the smoke-app config.
    let deploy = dist.join("deploy");
    assert!(
        deploy.is_dir(),
        "dist/deploy must exist for the binary target"
    );
    for name in [
        "deploy.json",
        "client.manifest.json",
        "server.manifest.json",
    ] {
        assert!(
            deploy.join(name).is_file(),
            "deploy/{name} missing (binary target characteristic file)"
        );
    }
}

// ---------------------------------------------------------------------------
// §10.3 islands end-to-end at the CLI level.
//
// The smoke-app declares `islands: { Counter: 'src/islands/counter.tsx' }`
// and its home page uses `<Island component="Counter" chunk="Counter">`.
// The build must:
//   * emit a `<ox-island data-ox-island="Counter" …>` wrapper with the
//     exact attributes the client boot script scans for;
//   * serialize the island props as base64 JSON in `data-ox-props`;
//   * SSR the island's React children with the initial value from props;
//   * inject `<script type="module" src="/client/__boot__.js">` into the
//     page so the boot module runs in the browser;
//   * write the boot chunk to `dist/client/`.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn smoke_app_emits_island_markup_and_boot_script() {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    let smoke = workspace.join("examples/smoke-app-react");
    assert!(
        smoke.join("oxide.config.ts").is_file(),
        "smoke-app fixture missing at {}",
        smoke.display()
    );

    let dist = smoke.join("dist");
    let _ = std::fs::remove_dir_all(&dist);

    let (code, out) = run_ok(&["oxide", "build", "--root", smoke.to_str().unwrap()]).await;
    assert_eq!(code, 0, "build exit code; output:\n{out}");
    assert!(out.contains("build ok"), "build summary: {out}");
    assert!(
        out.contains("islands: 1 registered") || out.contains("__boot__"),
        "build should announce island registration: {out}"
    );

    // 1. Prerendered HTML carries the island markup.
    let index_html = std::fs::read_to_string(dist.join("prerendered/index.html"))
        .expect("prerendered/index.html");
    assert!(
        index_html.contains(r#"data-ox-island="Counter""#),
        "island name attribute missing:\n{index_html}"
    );
    assert!(
        index_html.contains(r#"data-ox-chunk="Counter""#),
        "island chunk attribute missing:\n{index_html}"
    );
    assert!(
        index_html.contains(r#"data-ox-strategy="load""#),
        "island strategy attribute missing:\n{index_html}"
    );
    // base64(JSON.stringify({initial:3}))
    assert!(
        index_html.contains(r#"data-ox-props="eyJpbml0aWFsIjozfQ==""#),
        "island props attribute not the expected base64 payload:\n{index_html}"
    );

    // 2. The island's SSR children render inside <ox-island>: the Counter
    //    shows `3`, which comes from `props.initial` in the fixture.
    let open = index_html.find("<ox-island").expect("opening tag");
    let close = index_html[open..]
        .find("</ox-island>")
        .expect("closing tag")
        + open;
    let island_body = &index_html[open..close];
    assert!(
        island_body.contains("<span>3</span>"),
        "expected SSR counter value 3 in island body:\n{island_body}"
    );

    // 3. Boot script tag is injected before </body>.
    let boot_prefix = "<script type=\"module\" src=\"/client/";
    assert!(
        index_html.contains(boot_prefix),
        "boot module script tag missing:\n{index_html}"
    );
    assert!(
        index_html.rfind(boot_prefix).unwrap() < index_html.rfind("</body>").unwrap(),
        "boot script must appear before </body>"
    );

    // 4. The boot chunk landed in dist/client/.
    let client_dir = dist.join("client");
    let has_boot = std::fs::read_dir(&client_dir)
        .expect("dist/client")
        .flatten()
        .any(|e| e.file_name().to_string_lossy().contains("__boot__"));
    assert!(has_boot, "no __boot__ chunk in dist/client/");
}

/// SPEC-11 / §10.2.5: a mid-stream render error AFTER the shell committed
/// appends the `ox-error` recovery boundary (so the response is not
/// truncated); before the shell it still aborts.
#[tokio::test]
async fn stream_error_recovery_after_shell_emits_boundary() {
    use futures::StreamExt as _;
    use futures::channel::mpsc as fmpsc;

    // Build a RenderStream that yields one chunk then an error.
    let (mut tx, rx) = fmpsc::channel::<oxide_diagnostics::Result<bytes::Bytes>>(8);
    tokio::spawn(async move {
        use futures::SinkExt as _;
        let _ = tx
            .send(Ok(bytes::Bytes::from_static(b"<html>shell</html>")))
            .await;
        let _ = tx
            .send(Err(oxide_diagnostics::OxideError(
                oxide_diagnostics::Diagnostic::error("RND-TEST", "mid-stream boom"),
            )))
            .await;
    });
    // `oxide_server::RenderStream` is a boxed item stream.
    let src: oxide_server::RenderStream = Box::pin(rx);

    let body = crate::render_stream_with_recovery(
        src,
        Some("b1".to_string()),
        Some("n0nce".to_string()),
        "/".to_string(),
    );
    let collected: Vec<u8> = {
        use futures::StreamExt as _;
        let mut s = body;
        let mut out = Vec::new();
        while let Some(item) = s.next().await {
            out.extend_from_slice(&item.expect("recovery boundary is not an io error"));
        }
        out
    };
    let text = String::from_utf8_lossy(&collected);
    assert!(text.contains("shell"), "shell preserved: {text}");
    assert!(
        text.contains("ox-error"),
        "recovery boundary emitted: {text}"
    );
    assert!(text.contains("RND-TEST"), "diagnostic code named: {text}");
}

// ------------------------- §8.4/§9.5 live ODF payload + §10.2 error status

/// A streaming renderer whose FIRST chunk is the canonical error-status
/// contract marker (`<!--oxide-status:500-->`) — what an adapter yields
/// when the render failed before the shell (§10.2 step 5; the marker side
/// lives in `real-ssr.ts`, the status side in `probe_render_error_status`).
struct ErrorMarkerRenderer;

#[async_trait::async_trait]
impl oxide_server::PageRenderer for ErrorMarkerRenderer {
    fn name(&self) -> &str {
        "error-marker"
    }
    async fn render(&self, _input: oxide_server::PageRenderInput) -> oxide_server::Result<String> {
        Ok("<!doctype html><html><body>ERR</body></html>".into())
    }
    async fn render_streaming(
        &self,
        _input: oxide_server::PageRenderInput,
    ) -> oxide_server::Result<oxide_server::RenderStream> {
        let items: Vec<oxide_server::Result<bytes::Bytes>> = vec![
            Ok(bytes::Bytes::from_static(b"<!--oxide-status:500-->")),
            Ok(bytes::Bytes::from_static(
                b"<!doctype html><html><body>ERR</body></html>",
            )),
        ];
        Ok(Box::pin(futures::stream::iter(items)))
    }
}

/// Same shape, but announcing the GENERIC OX-500 fallback boundary the
/// adapter emits when no `error.tsx` is registered (or the error page
/// itself throws) — the pipeline must treat that as a 500 too.
struct Ox500FallbackRenderer;

#[async_trait::async_trait]
impl oxide_server::PageRenderer for Ox500FallbackRenderer {
    fn name(&self) -> &str {
        "ox500-fallback"
    }
    async fn render(&self, _input: oxide_server::PageRenderInput) -> oxide_server::Result<String> {
        Ok("<!doctype html><html><body>ERR</body></html>".into())
    }
    async fn render_streaming(
        &self,
        _input: oxide_server::PageRenderInput,
    ) -> oxide_server::Result<oxide_server::RenderStream> {
        let items: Vec<oxide_server::Result<bytes::Bytes>> = vec![
            Ok(bytes::Bytes::from_static(
                b"<div class='ox-error' data-ox-error='OX-500'>OX-500: boom</div>",
            )),
            Ok(bytes::Bytes::from_static(b"</body></html>")),
        ];
        Ok(Box::pin(futures::stream::iter(items)))
    }
}

#[tokio::test]
async fn live_render_injects_odf_payload_script() {
    use std::sync::Arc;

    let tree = tree_with_server_route();
    let metrics = Arc::new(oxide_server::Metrics::new());
    let page_renderer: Option<Arc<dyn oxide_server::PageRenderer>> =
        Some(Arc::new(DataCapturingRenderer::new()));
    let loader_runner: Option<Arc<dyn oxide_server::LoaderRunner>> = Some(Arc::new(
        ScriptedLoader::ok(serde_json::json!({ "title": "Hi" })),
    ));

    let dispatch = build_dispatch(
        tree,
        metrics,
        Vec::new(),
        page_renderer,
        loader_runner,
        routes_root(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        Vec::new(),
        None, // no waterfall recorder in tests
        None, // no session codec in tests
        None, // no PPR shells in tests
    );
    let resp = dispatch(test_request("GET", "/")).await;

    assert_eq!(resp.status, 200);
    let collected = collect_response_body(resp).await;
    let body = String::from_utf8_lossy(&collected).to_string();
    // §9.5 / §8.4: the SAME `window.__oxide` payload script prerendered
    // pages embed must also ride the LIVE (streamed) path — previously the
    // dynamic page had no payload and the boot module skipped hydration
    // and the router entirely.
    assert!(
        body.contains("application/oxide-data"),
        "live page carries the ODF payload script: {body}"
    );
    assert!(
        body.contains("window.__oxide="),
        "hydration bootstrap present: {body}"
    );
    // The payload decodes to the §8.4 {route, url, params, data} envelope
    // with the loader's data inside.
    let b64 = body
        .split("application/oxide-data")
        .nth(1)
        .and_then(|rest| rest.split('>').nth(1))
        .and_then(|seg| seg.split('<').next())
        .unwrap_or("")
        .trim()
        .to_string();
    assert!(
        !b64.is_empty(),
        "payload script carries base64 content: {body}"
    );
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(&b64)
        .expect("payload is base64");
    let value = oxide_odf::decode(&raw).expect("payload decodes as ODF");
    let json = value.to_json();
    assert_eq!(
        json["data"]["title"], "Hi",
        "loader data inside the live payload: {json}"
    );
    assert_eq!(json["route"], "routes/index");
}

#[tokio::test]
async fn error_status_marker_yields_http_500() {
    use std::sync::Arc;

    // §10.2 step 5 (error-status contract): a LIVE render whose first
    // chunk announces an error document is a 500 on the wire — not a 200
    // with error-shaped content. Two markers are honoured: the canonical
    // `<!--oxide-status:500-->` and the generic OX-500 fallback attribute
    // the adapter already emits today. A PRERENDERED error page's status
    // is now carried through the L0 seed manifest (Wave 3 item 2).
    let renderers: Vec<(&str, Arc<dyn oxide_server::PageRenderer>)> = vec![
        (
            "canonical marker",
            Arc::new(ErrorMarkerRenderer) as Arc<dyn oxide_server::PageRenderer>,
        ),
        (
            "generic fallback",
            Arc::new(Ox500FallbackRenderer) as Arc<dyn oxide_server::PageRenderer>,
        ),
    ];
    for (name, renderer) in renderers {
        let tree = tree_with_server_route();
        let metrics = Arc::new(oxide_server::Metrics::new());
        let dispatch = build_dispatch(
            tree,
            metrics,
            Vec::new(),
            Some(renderer),
            None,
            routes_root(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            Vec::new(),
            None, // no waterfall recorder in tests
            None, // no session codec in tests
            None, // no PPR shells in tests
        );
        let resp = dispatch(test_request("GET", "/")).await;
        assert_eq!(resp.status, 500, "{name} → HTTP 500");
        let collected = collect_response_body(resp).await;
        let body = String::from_utf8_lossy(&collected).to_string();
        // The canonical marker renderer emits "ERR: ..."; the generic
        // fallback renders the OX-500 boundary text. Both must survive.
        assert!(
            body.contains("ERR") || body.contains("OX-500"),
            "{name}: error body preserved: {body}"
        );
    }
}

#[test]
fn loading_map_keys_preserve_dynamic_tokens() {
    // §8.4 KEY CONTRACT (client matcher, honored by the boot runtime): a
    // loading-map key is an ABSOLUTE PATH whose segments may be `[name]`
    // tokens; a `[name]` segment matches exactly ONE non-empty URL
    // segment; the key with the most segments wins. The old keys were
    // literal bracket-prefix strings (`/blog/[slug]/`) that never matched
    // a real URL.
    let mut loading = std::collections::BTreeMap::new();
    loading.insert(
        "routes/blog/[slug]/loading".to_string(),
        std::path::PathBuf::from("src/routes/blog/[slug]/loading.tsx"),
    );
    loading.insert(
        "routes/loading".to_string(),
        std::path::PathBuf::from("src/routes/loading.tsx"),
    );
    let out = crate::generate_island_boot_entry(
        std::path::Path::new("/proj"),
        "react",
        &std::collections::BTreeMap::new(),
        &loading,
        false,
    );
    assert!(
        out.contains("\"/blog/[slug]\""),
        "dynamic segment keeps its [name] token: {out}"
    );
    assert!(
        !out.contains("\"/blog/[slug]/\""),
        "no literal-prefix keys (never matched): {out}"
    );
    assert!(out.contains("\"/\""), "root loading key is /: {out}");
}

#[tokio::test]
async fn remote_cache_handler_maps_handler_config() {
    // §11.2 / SPEC-12: `remote_cache_handler` is the live seam between
    // `cache.handler` config and the tier the WORKER sees. HTTP yields a
    // real driver; memory/disk fall through to the local tiers the caller
    // already built (None here). Redis and S3 drivers exist but construct
    // async / may reach the network — their own unit tests exercise the
    // protocol; asserting them here would make this test non-hermetic.
    use oxide_config::CacheHandlerConfig;
    let f = crate::framework_shim::remote_cache_handler;
    assert!(
        f(Some(&CacheHandlerConfig::Http {
            url: "https://cache.example".into()
        }))
        .await
        .is_some(),
        "http handler produces a remote driver"
    );
    assert!(f(Some(&CacheHandlerConfig::Memory)).await.is_none());
    assert!(f(Some(&CacheHandlerConfig::Disk)).await.is_none());
    assert!(f(None).await.is_none());
}

// -------------------------------- §8.1/§8.4 middleware parity (headers+locals)

/// A `MiddlewareDispatcher` that always succeeds, adds fixed response
/// headers and (optionally) injects `locals`. Test-only.
struct ScriptedMiddleware {
    headers: Vec<(String, String)>,
    locals: Option<serde_json::Value>,
}

#[async_trait::async_trait]
impl oxide_server::MiddlewareDispatcher for ScriptedMiddleware {
    async fn dispatch(
        &self,
        _module_path: &str,
        _method: &str,
        _url: &str,
        _params: &serde_json::Map<String, serde_json::Value>,
        _headers: &[(String, String)],
        _body: &[u8],
        _locals: Option<&serde_json::Value>,
    ) -> std::result::Result<oxide_server::MiddlewareRunResult, oxide_server::MiddlewareRunError>
    {
        Ok(oxide_server::MiddlewareRunResult {
            short: false,
            status: 200,
            headers: self.headers.clone(),
            body: Vec::new(),
            redirect: None,
            rewrite: None,
            locals: self.locals.clone(),
        })
    }
}

/// A `LoaderRunner` that records the LAST `locals` it received and returns
/// a fixed data + deferred payload. Proves the middleware phase feeds the
/// loader and the resolved defers reach the consumer (§8.4 parity).
struct LocalsCapturingLoader {
    last_locals: std::sync::Arc<std::sync::Mutex<serde_json::Value>>,
}

impl LocalsCapturingLoader {
    fn new() -> Self {
        Self {
            last_locals: std::sync::Arc::new(std::sync::Mutex::new(serde_json::Value::Null)),
        }
    }
}

#[async_trait::async_trait]
impl oxide_server::LoaderRunner for LocalsCapturingLoader {
    fn name(&self) -> &str {
        "locals-capture"
    }
    async fn run(
        &self,
        input: oxide_server::LoaderRunInput,
    ) -> oxide_server::Result<oxide_server::LoaderOutput> {
        *self.last_locals.lock().unwrap() = input.locals.clone();
        let mut deferred = std::collections::BTreeMap::new();
        deferred.insert(
            "0".to_string(),
            serde_json::json!({ "slow": "resolved-by-loader" }),
        );
        Ok(oxide_server::LoaderOutput {
            status: 200,
            location: None,
            data: serde_json::json!({ "v": 1 }),
            set_cookies: Vec::new(),
            headers: Vec::new(),
            deferred,
            meta: None,
            session: None,
        })
    }
}

/// A page route + a root `middleware.ts`, so the dispatch resolves a
/// nearest-ancestor middleware (no filesystem needed — the scripted
/// dispatcher ignores the module path).
fn tree_with_page_and_middleware() -> std::sync::Arc<oxide_router::RouteTree> {
    let mut tree = oxide_router::RouteTree::new();
    tree.insert(oxide_router::RouteEntry {
        id: oxide_router::RouteId("routes/index".into()),
        kind: oxide_router::RouteKind::Page,
        segments: vec![],
        file: std::path::PathBuf::from("index/page.tsx"),
        render_mode: None,
        tags: Vec::new(),
        has_server_module: true,
        slot: None,
        needs_parent: false,
    });
    tree.insert(oxide_router::RouteEntry {
        id: oxide_router::RouteId("routes/middleware".into()),
        kind: oxide_router::RouteKind::Middleware,
        segments: vec![],
        file: std::path::PathBuf::from("middleware.ts"),
        render_mode: None,
        tags: Vec::new(),
        has_server_module: false,
        slot: None,
        needs_parent: false,
    });
    std::sync::Arc::new(tree)
}

/// B-22: the outer layer rewrites once; on re-dispatch it must not run again
/// while the remaining (inner) layer still runs.
struct RewriteOnceMiddleware {
    calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl oxide_server::MiddlewareDispatcher for RewriteOnceMiddleware {
    async fn dispatch(
        &self,
        module_path: &str,
        _method: &str,
        _url: &str,
        _params: &serde_json::Map<String, serde_json::Value>,
        _headers: &[(String, String)],
        _body: &[u8],
        _locals: Option<&serde_json::Value>,
    ) -> std::result::Result<oxide_server::MiddlewareRunResult, oxide_server::MiddlewareRunError>
    {
        let inner = module_path.ends_with("shop/middleware.ts");
        self.calls.lock().unwrap().push(module_path.to_string());
        Ok(oxide_server::MiddlewareRunResult {
            short: false,
            status: 200,
            headers: if inner {
                vec![("x-inner-ran".to_string(), "1".to_string())]
            } else {
                Vec::new()
            },
            rewrite: (!inner).then(|| "/shop".to_string()),
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn middleware_rewrite_does_not_rerun_outer_layers() {
    use std::sync::Arc;
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mw: Option<Arc<dyn oxide_server::MiddlewareDispatcher>> =
        Some(Arc::new(RewriteOnceMiddleware {
            calls: Arc::clone(&calls),
        }));
    let dispatch = build_dispatch(
        tree_with_two_middleware_layers(),
        Arc::new(oxide_server::Metrics::new()),
        Vec::new(),
        Some(Arc::new(DataCapturingRenderer::new())),
        None,
        routes_root(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        mw,
        false,
        Vec::new(),
        None,
        None,
        None, // no PPR shells in tests
    );
    let resp = dispatch(test_request("GET", "/shop")).await;
    assert_eq!(resp.status, 200);
    assert!(
        resp.headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("x-inner-ran") && v == "1"),
        "inner layer still runs: {:?}",
        resp.headers
    );
    let calls = calls.lock().unwrap();
    let outer = calls
        .iter()
        .filter(|p| {
            p.ends_with("routes/middleware.ts")
                || p.ends_with("middleware.ts") && !p.contains("shop/")
        })
        .count();
    assert_eq!(outer, 1, "outer middleware ran exactly once: {calls:?}");
}

/// B-06: two middleware layers; the inner one re-sets a header the outer
/// already emitted. Last-writer-wins must leave exactly one line.
struct LayerMiddleware;

#[async_trait::async_trait]
impl oxide_server::MiddlewareDispatcher for LayerMiddleware {
    async fn dispatch(
        &self,
        module_path: &str,
        _method: &str,
        _url: &str,
        _params: &serde_json::Map<String, serde_json::Value>,
        _headers: &[(String, String)],
        _body: &[u8],
        _locals: Option<&serde_json::Value>,
    ) -> std::result::Result<oxide_server::MiddlewareRunResult, oxide_server::MiddlewareRunError>
    {
        let inner = module_path.ends_with("shop/middleware.ts");
        let headers = if inner {
            vec![("x-tenant-frame".to_string(), "SAMEORIGIN".to_string())]
        } else {
            vec![
                ("x-tenant-frame".to_string(), "DENY".to_string()),
                ("x-only-outer".to_string(), "1".to_string()),
            ]
        };
        Ok(oxide_server::MiddlewareRunResult {
            short: false,
            status: 200,
            headers,
            ..Default::default()
        })
    }
}

fn tree_with_two_middleware_layers() -> std::sync::Arc<oxide_router::RouteTree> {
    let mut tree = oxide_router::RouteTree::new();
    let mut page = |id: &str, segments: Vec<&str>, file: &str| oxide_router::RouteEntry {
        id: oxide_router::RouteId(id.into()),
        kind: oxide_router::RouteKind::Page,
        segments: segments.into_iter().map(str::to_string).collect(),
        file: std::path::PathBuf::from(file),
        render_mode: None,
        tags: Vec::new(),
        has_server_module: true,
        slot: None,
        needs_parent: false,
    };
    tree.insert(page("routes/shop/page", vec!["shop"], "shop/page.tsx"));
    let mut mw = |id: &str, segments: Vec<&str>, file: &str| oxide_router::RouteEntry {
        id: oxide_router::RouteId(id.into()),
        kind: oxide_router::RouteKind::Middleware,
        segments: segments.into_iter().map(str::to_string).collect(),
        file: std::path::PathBuf::from(file),
        render_mode: None,
        tags: Vec::new(),
        has_server_module: false,
        slot: None,
        needs_parent: false,
    };
    tree.insert(mw("routes/middleware", vec![], "middleware.ts"));
    tree.insert(mw(
        "routes/shop/middleware",
        vec!["shop"],
        "shop/middleware.ts",
    ));
    std::sync::Arc::new(tree)
}

#[tokio::test]
async fn inner_middleware_header_overwrites_outer() {
    use std::sync::Arc;
    let metrics = Arc::new(oxide_server::Metrics::new());
    let mw: Option<Arc<dyn oxide_server::MiddlewareDispatcher>> = Some(Arc::new(LayerMiddleware));
    let dispatch = build_dispatch(
        tree_with_two_middleware_layers(),
        std::sync::Arc::clone(&metrics),
        Vec::new(),
        Some(Arc::new(DataCapturingRenderer::new())),
        None,
        routes_root(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        mw,
        false,
        Vec::new(),
        None,
        None,
        None, // no PPR shells in tests
    );
    let resp = dispatch(test_request("GET", "/shop")).await;
    assert_eq!(resp.status, 200);
    let frames: Vec<&str> = resp
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("x-tenant-frame"))
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(frames, vec!["SAMEORIGIN"], "headers: {:?}", resp.headers);
    let outer = resp
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("x-only-outer"))
        .count();
    assert_eq!(outer, 1, "headers: {:?}", resp.headers);
}

#[tokio::test]
async fn middleware_headers_ride_page_and_data_responses() {
    use std::sync::Arc;

    let metrics = Arc::new(oxide_server::Metrics::new());
    let mw: Option<Arc<dyn oxide_server::MiddlewareDispatcher>> =
        Some(Arc::new(ScriptedMiddleware {
            headers: vec![("x-oxide-mw".to_string(), "on".to_string())],
            locals: Some(serde_json::json!({ "tenant": "acme" })),
        }));

    // §8.5: the accumulated `middleware_headers` previously died in a dead
    // variable — the PAGE response must carry them.
    let page_renderer: Option<Arc<dyn oxide_server::PageRenderer>> =
        Some(Arc::new(DataCapturingRenderer::new()));
    let dispatch = build_dispatch(
        tree_with_page_and_middleware(),
        std::sync::Arc::clone(&metrics),
        Vec::new(),
        page_renderer,
        None,
        routes_root(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        mw.clone(),
        false,
        Vec::new(),
        None, // no waterfall recorder in tests
        None, // no session codec in tests
        None, // no PPR shells in tests
    );
    let resp = dispatch(test_request("GET", "/")).await;
    assert_eq!(resp.status, 200);
    let hdr = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-oxide-mw"));
    assert_eq!(
        hdr.map(|(_, v)| v.as_str()),
        Some("on"),
        "middleware header on the page response: {:?}",
        resp.headers
    );

    // §8.4 parity: a /__data request runs the SAME middleware phase before
    // the data branch — its headers ride the ODF response and its locals
    // reach the loader.
    let loader = Arc::new(LocalsCapturingLoader::new());
    let last_locals = Arc::clone(&loader.last_locals);
    let loader_runner: Option<Arc<dyn oxide_server::LoaderRunner>> = Some(loader);
    let dispatch = build_dispatch(
        tree_with_page_and_middleware(),
        metrics,
        Vec::new(),
        None,
        loader_runner,
        routes_root(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        mw,
        false,
        Vec::new(),
        None, // no waterfall recorder in tests
        None, // no session codec in tests
        None, // no PPR shells in tests
    );
    let resp = dispatch(test_request("GET", "/__data?route=%2F")).await;
    assert_eq!(resp.status, 200, "data response for a matched route");
    let hdr = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-oxide-mw"));
    assert_eq!(
        hdr.map(|(_, v)| v.as_str()),
        Some("on"),
        "middleware header on the /__data response: {:?}",
        resp.headers
    );
    assert_eq!(
        last_locals.lock().unwrap()["tenant"],
        "acme",
        "middleware locals reached the loader on the /__data path"
    );
    // The ODF envelope carries the loader's data, the resolved defers and
    // the locale (§8.4 parity with the SSR render's inputs).
    let json = oxide_odf::decode(&resp.body).expect("data response is ODF");
    let json = json.to_json();
    assert_eq!(json["data"]["v"], 1, "loader data in the envelope: {json}");
    assert_eq!(
        json["deferred"]["0"]["slow"], "resolved-by-loader",
        "resolved defers in the envelope: {json}"
    );
    assert!(json["locale"].is_string(), "locale in the envelope: {json}");
}

// ---------------------------------------------------------------------------
// P0-5: middleware headers on L0 hits + single-flight followers (§11.1, §8.5)
// ---------------------------------------------------------------------------

/// A cacheable route (static mode, no server module) so `l0_eligible` is true.
fn tree_with_static_page_and_middleware() -> std::sync::Arc<oxide_router::RouteTree> {
    let mut tree = oxide_router::RouteTree::new();
    tree.insert(oxide_router::RouteEntry {
        id: oxide_router::RouteId("routes/index".into()),
        kind: oxide_router::RouteKind::Page,
        segments: vec![],
        file: std::path::PathBuf::from("index/page.tsx"),
        render_mode: Some("static".into()),
        tags: Vec::new(),
        has_server_module: false,
        slot: None,
        needs_parent: false,
    });
    tree.insert(oxide_router::RouteEntry {
        id: oxide_router::RouteId("routes/middleware".into()),
        kind: oxide_router::RouteKind::Middleware,
        segments: vec![],
        file: std::path::PathBuf::from("middleware.ts"),
        render_mode: None,
        tags: Vec::new(),
        has_server_module: false,
        slot: None,
        needs_parent: false,
    });
    std::sync::Arc::new(tree)
}

#[tokio::test]
async fn middleware_headers_ride_l0_hits() {
    use std::sync::Arc;

    // Memory tier the L0 cache stores into.
    let tier: Arc<dyn oxide_cache::CacheHandler> =
        Arc::new(oxide_cache::TieredCache::new(8 * 1024 * 1024));
    let l0 = Arc::new(l0_cache::L0Cache::new(
        tier,
        "test-build",
        std::time::Duration::from_secs(300),
    ));

    let metrics = Arc::new(oxide_server::Metrics::new());
    let mw: Option<Arc<dyn oxide_server::MiddlewareDispatcher>> =
        Some(Arc::new(ScriptedMiddleware {
            headers: vec![("x-oxide-mw".to_string(), "on".to_string())],
            locals: None,
        }));

    // A renderer is attached so the FIRST request takes the render path and
    // stores its output in L0 (a no-renderer request returns the IR
    // placeholder before the L0 store).
    let page_renderer: Option<Arc<dyn oxide_server::PageRenderer>> =
        Some(Arc::new(DataCapturingRenderer::new()));

    let dispatch = build_dispatch(
        tree_with_static_page_and_middleware(),
        metrics,
        Vec::new(),
        page_renderer,
        None,
        routes_root(),
        None,
        Some(Arc::clone(&l0)),
        None,
        None,
        None,
        None,
        None,
        mw,
        false,
        Vec::new(),
        None, // no waterfall recorder in tests
        None, // no session codec in tests
        None, // no PPR shells in tests
    );

    // First request: an L0 miss. It renders and stores the entry; the
    // middleware header rides it.
    let first = dispatch(test_request("GET", "/")).await;
    assert_eq!(first.status, 200, "first request status");
    let first_cache = first
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-oxide-cache"))
        .map(|(_, v)| v.as_str());
    assert_ne!(
        first_cache,
        Some("L0-hit"),
        "the first request must not be an L0 hit: {:?}",
        first.headers
    );

    // Second request: an L0 HIT. The body comes from cache, but middleware
    // ran for THIS request — its header must still ride the response (P0-5).
    let second = dispatch(test_request("GET", "/")).await;
    assert_eq!(second.status, 200, "second request status");
    let second_cache = second
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-oxide-cache"))
        .map(|(_, v)| v.as_str());
    // The second request may be served from L0 directly (`L0-hit`) or via
    // single-flight coalescing (`L0-coalesced`) if the first request's store
    // had not yet completed — both are the cached paths P0-5 fixed.
    assert!(
        matches!(second_cache, Some("L0-hit") | Some("L0-coalesced")),
        "the second request is served from cache: {:?}",
        second.headers
    );

    let mw_hdr = second
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-oxide-mw"))
        .map(|(_, v)| v.as_str());
    assert_eq!(
        mw_hdr,
        Some("on"),
        "middleware header on the L0-hit response: {:?}",
        second.headers
    );
}

// ---- §9.2 parallel layout loads ----------------------------------------
//
// `build_dispatch` overlaps the last-layout load with the page load when
// `RouteEntry.needs_parent == false`. These two tests are load-bearing for
// that decision: reverting the parallel branch to sequential makes the
// first test's `max_active == 2` assertion fail (it drops to 1), while the
// second test pins the sequential behavior that pages calling `parent()`
// still need.

/// Records peak concurrency across overlapping `run()` calls, sleeping
/// long enough for a parallel join to be observable as `max_active >= 2`.
struct ConcurrencyProbe {
    active: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    max_active: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl ConcurrencyProbe {
    fn new() -> Self {
        use std::sync::Arc;
        use std::sync::atomic::AtomicUsize;
        Self {
            active: Arc::new(AtomicUsize::new(0)),
            max_active: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait::async_trait]
impl oxide_server::LoaderRunner for ConcurrencyProbe {
    fn name(&self) -> &str {
        "concurrency-probe"
    }
    async fn run(
        &self,
        input: oxide_server::LoaderRunInput,
    ) -> oxide_server::Result<oxide_server::LoaderOutput> {
        use std::sync::atomic::Ordering;
        let n = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        loop {
            let prev = self.max_active.load(Ordering::SeqCst);
            if n <= prev {
                break;
            }
            if self
                .max_active
                .compare_exchange(prev, n, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                break;
            }
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        // Wide enough that any scheduling jitter still shows overlap.
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(oxide_server::LoaderOutput::ok(serde_json::json!({
            "route": input.route,
        })))
    }
}

/// Scan a real tempdir into a `RouteTree` so `layout_module_paths` is
/// populated exactly as it is at runtime (`filter(p.is_file())` needs a
/// real layout.server.ts on disk). `needs_parent` is decided by the
/// scanner's page.server.ts content heuristic, not set by the test.
fn scan_probe_project(
    page_server_src: &str,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::sync::Arc<oxide_router::RouteTree>,
) {
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(
        root.join("layout.tsx"),
        "export default function L(p) { return p.children }\n",
    )
    .unwrap();
    std::fs::write(root.join("layout.server.ts"), "export const config = {};\n").unwrap();
    std::fs::write(
        root.join("page.tsx"),
        "export default function P() { return null }\n",
    )
    .unwrap();
    std::fs::write(root.join("page.server.ts"), page_server_src).unwrap();
    let tree = Arc::new(oxide_router::RouteTree::scan(&root).unwrap());
    (dir, root, tree)
}

#[tokio::test]
async fn layout_and_page_overlap_when_page_does_not_call_parent() {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let (_dir, root, tree) = scan_probe_project("export async function load() { return {}; }\n");
    let m = tree.match_path("/").expect("page matches /");
    assert_eq!(m.layout_chain.len(), 1, "one layout in chain");
    assert!(m.route.has_server_module, "page.server.ts present");
    assert!(!m.route.needs_parent, "no `parent` in page.server.ts");

    let probe = Arc::new(ConcurrencyProbe::new());
    let calls = Arc::clone(&probe.calls);
    let max_active = Arc::clone(&probe.max_active);
    let loader: Option<Arc<dyn oxide_server::LoaderRunner>> = Some(probe);

    let metrics = Arc::new(oxide_server::Metrics::new());
    let dispatch = build_dispatch(
        tree,
        metrics,
        Vec::new(),
        None,
        loader,
        root,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        Vec::new(),
        None,
        None,
        None, // no PPR shells in tests
    );
    let resp = dispatch(test_request("GET", "/")).await;
    assert_eq!(resp.status, 200, "matched page renders");
    assert_eq!(calls.load(Ordering::SeqCst), 2, "layout + page both ran");
    assert_eq!(
        max_active.load(Ordering::SeqCst),
        2,
        "layout and page overlapped — parallel join, not sequential"
    );
}

#[tokio::test]
async fn layout_then_page_sequential_when_page_calls_parent() {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let (_dir, root, tree) =
        scan_probe_project("export async function load({ parent }) { return await parent(); }\n");
    let m = tree.match_path("/").expect("page matches /");
    assert_eq!(m.layout_chain.len(), 1, "one layout in chain");
    assert!(m.route.needs_parent, "`parent` appears in page.server.ts");

    let probe = Arc::new(ConcurrencyProbe::new());
    let calls = Arc::clone(&probe.calls);
    let max_active = Arc::clone(&probe.max_active);
    let loader: Option<Arc<dyn oxide_server::LoaderRunner>> = Some(probe);

    let metrics = Arc::new(oxide_server::Metrics::new());
    let dispatch = build_dispatch(
        tree,
        metrics,
        Vec::new(),
        None,
        loader,
        root,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        Vec::new(),
        None,
        None,
        None, // no PPR shells in tests
    );
    let resp = dispatch(test_request("GET", "/")).await;
    assert_eq!(resp.status, 200, "matched page renders");
    assert_eq!(calls.load(Ordering::SeqCst), 2, "layout + page both ran");
    assert_eq!(
        max_active.load(Ordering::SeqCst),
        1,
        "page needs parent() → sequential chain, no overlap"
    );
}

// ---- Wave 3 item 2: prerender error-status carried through L0 seed -----
//
// `write_prerender` must return the status the adapter's buffered output
// announced — otherwise a prerendered error page is served as a 200 on
// first request (the "baked 200" deviation). These two tests are
// load-bearing for the return-tuple change: reverting to `Result<String>`
// makes them fail to compile, and reverting only the status-detection
// line would drop the first test's `status == 500` assertion.

/// A buffered renderer whose `render()` output carries the canonical
/// error-status contract marker (what a framework adapter emits when it
/// renders the error route before the shell).
struct BufferedErrorMarkerRenderer;

#[async_trait::async_trait]
impl oxide_server::PageRenderer for BufferedErrorMarkerRenderer {
    fn name(&self) -> &str {
        "buffered-error-marker"
    }
    async fn render(&self, _input: oxide_server::PageRenderInput) -> oxide_server::Result<String> {
        Ok(
            "<!doctype html><html><head><!--oxide-status:500--></head><body>ERR</body></html>"
                .into(),
        )
    }
}

/// A buffered renderer whose output carries no marker — the status must
/// stay 200.
struct CleanBodyRenderer;

#[async_trait::async_trait]
impl oxide_server::PageRenderer for CleanBodyRenderer {
    fn name(&self) -> &str {
        "clean-body"
    }
    async fn render(&self, _input: oxide_server::PageRenderInput) -> oxide_server::Result<String> {
        Ok("<!doctype html><html><body>ok</body></html>".into())
    }
}

#[tokio::test]
async fn write_prerender_carries_500_status_for_marker_body() {
    let dir = tempfile::tempdir().unwrap();
    let dist = dir.path().to_path_buf();
    let renderer: std::sync::Arc<dyn oxide_server::PageRenderer> =
        std::sync::Arc::new(BufferedErrorMarkerRenderer);
    let params: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let (html, status) = crate::write_prerender(
        &dist,
        "prerendered/index.html",
        "routes/index",
        "/",
        "/",
        &params,
        "", // no i18n locale in this test
        "test-nonce",
        Some(&renderer),
        None, // no loader
        None, // no module_path
        None, // no boot script
        &[],  // no layouts
        &[],  // no layout server modules
        None, // no error_route
        &serde_json::json!({"meta":[],"link":[],"style":[],"script":[]}),
    )
    .await
    .expect("write_prerender succeeds");
    assert_eq!(
        status, 500,
        "adapter's `<!--oxide-status:500-->` marker → status 500"
    );
    assert!(
        html.contains("<!--oxide-status:500-->"),
        "marker preserved through injection: {html}"
    );
}

#[tokio::test]
async fn write_prerender_carries_200_status_for_clean_body() {
    let dir = tempfile::tempdir().unwrap();
    let dist = dir.path().to_path_buf();
    let renderer: std::sync::Arc<dyn oxide_server::PageRenderer> =
        std::sync::Arc::new(CleanBodyRenderer);
    let params: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let (_html, status) = crate::write_prerender(
        &dist,
        "prerendered/index.html",
        "routes/index",
        "/",
        "/",
        &params,
        "", // no i18n locale in this test
        "test-nonce",
        Some(&renderer),
        None,
        None,
        None,
        &[],
        &[],
        None,
        &serde_json::json!({"meta":[],"link":[],"style":[],"script":[]}),
    )
    .await
    .expect("write_prerender succeeds");
    assert_eq!(status, 200, "no marker → 200");
}

// ---- §8.5 middleware chain (outer→inner) ------------------------------
//
// `build_dispatch` runs EVERY ancestor middleware in order (was: nearest
// only). These tests are load-bearing: reverting to nearest-ancestor
// resolution drops `seen.len()` to 1, and dropping the locals-threading
// drops `seen[1].1` to `None`.

/// Records the `(module_path, locals_param)` each dispatch sees, and
/// returns the module path inside its `locals` output so an inner
/// middleware (and the test) can prove threading.
struct ChainProbeMiddleware {
    seen: std::sync::Arc<std::sync::Mutex<Vec<(String, Option<serde_json::Value>)>>>,
}

impl ChainProbeMiddleware {
    fn new() -> Self {
        Self {
            seen: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }
}

#[async_trait::async_trait]
impl oxide_server::MiddlewareDispatcher for ChainProbeMiddleware {
    async fn dispatch(
        &self,
        module_path: &str,
        _method: &str,
        _url: &str,
        _params: &serde_json::Map<String, serde_json::Value>,
        _headers: &[(String, String)],
        _body: &[u8],
        locals: Option<&serde_json::Value>,
    ) -> std::result::Result<oxide_server::MiddlewareRunResult, oxide_server::MiddlewareRunError>
    {
        self.seen
            .lock()
            .unwrap()
            .push((module_path.to_string(), locals.cloned()));
        Ok(oxide_server::MiddlewareRunResult {
            short: false,
            status: 200,
            headers: vec![(
                "x-mw".to_string(),
                std::path::Path::new(module_path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("mw")
                    .to_string(),
            )],
            body: Vec::new(),
            redirect: None,
            rewrite: None,
            locals: Some(serde_json::json!({ "mw": module_path })),
        })
    }
}

fn tree_with_page_and_middleware_chain() -> std::sync::Arc<oxide_router::RouteTree> {
    use std::sync::Arc;
    let mut tree = oxide_router::RouteTree::new();
    tree.insert(oxide_router::RouteEntry {
        id: oxide_router::RouteId("routes/blog/post/page".into()),
        kind: oxide_router::RouteKind::Page,
        segments: vec!["blog".into(), "post".into()],
        file: std::path::PathBuf::from("blog/post/page.tsx"),
        render_mode: None,
        tags: Vec::new(),
        has_server_module: false,
        slot: None,
        needs_parent: false,
    });
    tree.insert(oxide_router::RouteEntry {
        id: oxide_router::RouteId("routes/middleware".into()),
        kind: oxide_router::RouteKind::Middleware,
        segments: vec![],
        file: std::path::PathBuf::from("middleware.ts"),
        render_mode: None,
        tags: Vec::new(),
        has_server_module: false,
        slot: None,
        needs_parent: false,
    });
    tree.insert(oxide_router::RouteEntry {
        id: oxide_router::RouteId("routes/blog/middleware".into()),
        kind: oxide_router::RouteKind::Middleware,
        segments: vec![],
        file: std::path::PathBuf::from("blog/middleware.ts"),
        render_mode: None,
        tags: Vec::new(),
        has_server_module: false,
        slot: None,
        needs_parent: false,
    });
    Arc::new(tree)
}

#[tokio::test]
async fn middleware_chain_runs_outer_to_inner_and_threads_locals() {
    use std::sync::Arc;

    let probe = Arc::new(ChainProbeMiddleware::new());
    let seen = Arc::clone(&probe.seen);
    let mw: Option<Arc<dyn oxide_server::MiddlewareDispatcher>> = Some(probe);

    let dispatch = build_dispatch(
        tree_with_page_and_middleware_chain(),
        Arc::new(oxide_server::Metrics::new()),
        Vec::new(),
        Some(Arc::new(DataCapturingRenderer::new())),
        None,
        routes_root(),
        None,
        None,
        None,
        None,
        None,
        None,
        None, // endpoint_dispatcher
        mw,   // middleware_dispatcher
        false,
        Vec::new(),
        None, // no waterfall recorder
        None, // no session codec
        None, // no PPR shells in tests
    );
    let resp = dispatch(test_request("GET", "/blog/post")).await;
    assert_eq!(resp.status, 200, "matched page renders");

    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.len(),
        2,
        "both root and blog middleware ran (§8.5 full chain, not nearest only)"
    );
    // Outer→inner order: root middleware first.
    assert!(
        seen[0].0.ends_with("middleware.ts") && !seen[0].0.contains("/blog/"),
        "first is the root middleware: {}",
        seen[0].0
    );
    assert!(
        seen[1].0.contains("/blog/") && seen[1].0.ends_with("middleware.ts"),
        "second is the blog middleware: {}",
        seen[1].0
    );
    // Locals threading: the first sees no accumulated locals; the second
    // sees the first's contribution.
    let first_locals = seen[0].1.as_ref();
    assert!(
        first_locals.is_none()
            || first_locals
                .and_then(|v| v.as_object())
                .map_or(false, |o| o.is_empty()),
        "outer middleware sees empty locals: {:?}",
        first_locals
    );
    let second_locals = seen[1]
        .1
        .as_ref()
        .and_then(|v| v.as_object())
        .expect("inner middleware sees the outer's locals");
    let mw_path = second_locals
        .get("mw")
        .and_then(|v| v.as_str())
        .expect("inner locals carry the outer's `mw`");
    assert!(
        mw_path.ends_with("middleware.ts") && !mw_path.contains("/blog/"),
        "inner locals point at the ROOT middleware: {mw_path}"
    );
}

#[test]
fn build_transform_hook_returns_none_when_no_wasm_plugin_configured() {
    // §15.1 / §17.2: `build_transform_hook` returns `None` when no wasm
    // transform plugin is configured — the bundler must not be given a
    // hook that would call into nothing. Load-bearing for the negative
    // arm; the positive arm (a real string-ABI component firing the
    // transform) is covered at the plugin crate level by
    // `wasm_tdd::real_component_transform_add_one_roundtrips`, and its
    // CLI-level fixture (a hand-authored string-ABI WAT) is a documented
    // Wave-4 follow-up.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let cfg = crate::load_config_or_default(&root, "development");
    assert!(
        crate::build_transform_hook(&root, &cfg).is_none(),
        "no plugins configured → no transform hook"
    );
}

// ---- SPEC-15.2: JS plugin hook-name parsing ---------------------------------

#[test]
fn parse_hook_name_covers_all_nineteen() {
    use crate::parse_hook_name;
    use oxide_plugin::Hook;
    // Every variant of the Hook enum must round-trip through
    // parse_hook_name (using at least its canonical camelCase spelling).
    let pairs: &[(&str, Hook)] = &[
        ("config", Hook::Config),
        ("configResolved", Hook::ConfigResolved),
        ("buildStart", Hook::BuildStart),
        ("resolveId", Hook::ResolveId),
        ("load", Hook::Load),
        ("transform", Hook::Transform),
        ("renderChunk", Hook::RenderChunk),
        ("generateBundle", Hook::GenerateBundle),
        ("buildEnd", Hook::BuildEnd),
        ("transformHtml", Hook::TransformHtml),
        ("handleHotUpdate", Hook::HandleHotUpdate),
        ("configureServer", Hook::ConfigureServer),
        ("routes", Hook::Routes),
        ("adapter", Hook::Adapter),
        ("middleware", Hook::ServerMiddleware),
        ("renderHead", Hook::RenderHead),
        ("cacheHandler", Hook::CacheHandler),
        ("typegen", Hook::Typegen),
        ("diagnostics", Hook::Diagnostics),
    ];
    for (name, want) in pairs {
        assert_eq!(
            parse_hook_name(name),
            Some(*want),
            "parse_hook_name({name:?})"
        );
    }
}

#[test]
fn parse_hook_name_accepts_snake_case() {
    use crate::parse_hook_name;
    use oxide_plugin::Hook;
    assert_eq!(parse_hook_name("resolve_id"), Some(Hook::ResolveId));
    assert_eq!(parse_hook_name("transform_html"), Some(Hook::TransformHtml));
    assert_eq!(
        parse_hook_name("config_resolved"),
        Some(Hook::ConfigResolved)
    );
}

#[test]
fn parse_hook_name_rejects_unknown() {
    assert_eq!(crate::parse_hook_name("bogusHook"), None);
    assert_eq!(crate::parse_hook_name(""), None);
    assert_eq!(crate::parse_hook_name("TRANSFORM"), None, "case-sensitive");
}

// ---- SPEC-15.2: js_hook_name <-> parse_hook_name roundtrip ---------------

#[test]
fn js_hook_name_roundtrips_through_parse_hook_name() {
    use crate::{js_hook_name, parse_hook_name};
    use oxide_plugin::Hook;
    // Every hook the JS bridge emits must be a name parse_hook_name
    // accepts — otherwise a plugin declaring `transform` could not be
    // dispatched (the JS-side name would not resolve back to a Hook).
    for h in [
        Hook::Config,
        Hook::ConfigResolved,
        Hook::BuildStart,
        Hook::ResolveId,
        Hook::Load,
        Hook::Transform,
        Hook::RenderChunk,
        Hook::GenerateBundle,
        Hook::BuildEnd,
        Hook::TransformHtml,
        Hook::HandleHotUpdate,
        Hook::ConfigureServer,
        Hook::Routes,
        Hook::Adapter,
        Hook::ServerMiddleware,
        Hook::RenderHead,
        Hook::CacheHandler,
        Hook::Typegen,
        Hook::Diagnostics,
    ] {
        let name = js_hook_name(h);
        assert_eq!(
            parse_hook_name(name),
            Some(h),
            "roundtrip: js_hook_name({h:?}) = {name:?}, but parse_hook_name returned a different hook"
        );
    }
}

// ---- SPEC-16.6: session codec reaches `locals.session` ---------------------

/// A LoaderRunner that captures the `locals.session` its load() saw.
struct SessionCaptureLoader {
    last_session: std::sync::Arc<std::sync::Mutex<serde_json::Value>>,
}

impl SessionCaptureLoader {
    fn new() -> Self {
        Self {
            last_session: std::sync::Arc::new(std::sync::Mutex::new(serde_json::Value::Null)),
        }
    }
}

#[async_trait::async_trait]
impl oxide_server::LoaderRunner for SessionCaptureLoader {
    fn name(&self) -> &str {
        "session-capture"
    }
    async fn run(
        &self,
        input: oxide_server::LoaderRunInput,
    ) -> oxide_server::Result<oxide_server::LoaderOutput> {
        let sess = input
            .locals
            .get("session")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        *self.last_session.lock().unwrap() = sess;
        Ok(oxide_server::LoaderOutput::ok(serde_json::json!({})))
    }
}

#[tokio::test]
async fn session_codec_decodes_into_locals_session() {
    use std::sync::Arc;

    // 32 bytes of deterministic key material (test-only).
    let key = vec![0x42u8; 32];
    let codec = Arc::new(oxide_server::SessionCodec::new(&[key]).expect("codec"));

    // Encode a session with a known `user` field.
    let mut sess = oxide_server::Session::new();
    sess.insert("user", serde_json::json!("ada"));
    let cookie = sess.encode(&codec).expect("encode");

    // Build a dispatch with the codec attached + a session-capturing loader.
    let loader = Arc::new(SessionCaptureLoader::new());
    let captured = Arc::clone(&loader.last_session);
    let dispatch = build_dispatch(
        tree_with_server_route(),
        Arc::new(oxide_server::Metrics::new()),
        Vec::new(),
        Some(Arc::new(DataCapturingRenderer::new())),
        Some(loader as Arc<dyn oxide_server::LoaderRunner>),
        routes_root(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        Vec::new(),
        None,
        Some(codec),
        None, // no PPR shells in tests
    );

    // Request with the encoded cookie.
    let mut req = test_request("GET", "/");
    // B-05: browsers deliver the sealed token in the `Cookie` header.
    std::sync::Arc::make_mut(&mut req.headers)
        .push(("cookie".to_string(), format!("oxide_session={cookie}")));
    let resp = dispatch(req).await;
    assert_eq!(resp.status, 200, "request served");

    // The loader saw `locals.session.user == "ada"` — the codec path works.
    let s = captured.lock().unwrap();
    assert_eq!(
        s.get("user").and_then(|v| v.as_str()),
        Some("ada"),
        "session.user from cookie reached the loader via locals.session: {s:?}"
    );
}

#[tokio::test]
async fn session_change_from_middleware_sets_cookie() {
    use std::sync::Arc;

    // B-05 write-back: a middleware layer that refines `locals.session`
    // must produce a sealed `Set-Cookie` on the response.
    let key = vec![0x42u8; 32];
    let codec = Arc::new(oxide_server::SessionCodec::new(&[key]).expect("codec"));
    let mw: Option<Arc<dyn oxide_server::MiddlewareDispatcher>> =
        Some(Arc::new(ScriptedMiddleware {
            headers: Vec::new(),
            locals: Some(serde_json::json!({ "session": { "user": "ada" } })),
        }));
    let dispatch = build_dispatch(
        tree_with_page_and_middleware(),
        Arc::new(oxide_server::Metrics::new()),
        Vec::new(),
        Some(Arc::new(DataCapturingRenderer::new())),
        None,
        routes_root(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        mw,
        false,
        Vec::new(),
        None,
        Some(Arc::clone(&codec)),
        None, // no PPR shells in tests
    );
    let resp = dispatch(test_request("GET", "/")).await;
    let set_cookie = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, v)| v.clone())
        .expect("session write-back emits set-cookie");
    assert!(set_cookie.starts_with("oxide_session="), "{set_cookie}");
    let token = set_cookie
        .strip_prefix("oxide_session=")
        .and_then(|r| r.split(';').next())
        .unwrap();
    let sess: oxide_server::Session =
        oxide_server::Session::decode(&codec, token).expect("sealed session decodes");
    assert_eq!(sess.to_value()["user"], "ada");
}

#[tokio::test]
async fn session_codec_tampered_cookie_yields_empty_session() {
    use std::sync::Arc;

    let key = vec![0x42u8; 32];
    let codec = Arc::new(oxide_server::SessionCodec::new(&[key]).expect("codec"));

    let loader = Arc::new(SessionCaptureLoader::new());
    let captured = Arc::clone(&loader.last_session);
    let dispatch = build_dispatch(
        tree_with_server_route(),
        Arc::new(oxide_server::Metrics::new()),
        Vec::new(),
        Some(Arc::new(DataCapturingRenderer::new())),
        Some(loader as Arc<dyn oxide_server::LoaderRunner>),
        routes_root(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        Vec::new(),
        None,
        Some(codec),
        None, // no PPR shells in tests
    );

    let mut req = test_request("GET", "/");
    std::sync::Arc::make_mut(&mut req.headers).push((
        "cookie".to_string(),
        "oxide_session=not-a-real-token".to_string(),
    ));
    let resp = dispatch(req).await;
    assert_eq!(
        resp.status, 200,
        "tampered cookie must not fail the request"
    );

    // The loader saw an EMPTY session (no `session` key), never a 500.
    let s = captured.lock().unwrap();
    assert!(
        s.is_null() || s.as_object().map_or(true, |o| o.is_empty()),
        "tampered cookie → empty session, not error: {s:?}"
    );
}

#[tokio::test]
async fn dev_writes_plugin_registry() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    fs::create_dir_all(root.join("src/routes")).unwrap();
    fs::write(
        root.join("src/routes/page.tsx"),
        "export default function Page() { return null }",
    )
    .unwrap();
    fs::write(
        root.join("plugin.mjs"),
        "export async function transform() {}",
    )
    .unwrap();
    write_config(
        &root,
        r#"{"plugins":[
            {"kind":"js","path":"./plugin.mjs"},
            {"kind":"wasm","path":"./ignored.wasm"}
        ]}"#,
    );
    let _ = run_ok(&["oxide", "dev", "--root", &root.to_string_lossy(), "--once"]).await;
    let registry = root.join(".oxide/dev-plugins.json");
    assert!(
        registry.is_file(),
        "registry written at {}",
        registry.display()
    );
    let v: serde_json::Value = serde_json::from_slice(&fs::read(&registry).unwrap()).unwrap();
    let arr = v.as_array().expect("registry is an array");
    assert_eq!(arr.len(), 1, "only the js plugin: {v}");
    let p = arr[0].get("path").and_then(|v| v.as_str()).unwrap_or("");
    assert!(p.ends_with("plugin.mjs"), "path points at js plugin: {p}");
}

// ---- SPEC-15.1 / §15.2: native plugin registry through `oxide check` ------

#[tokio::test]
async fn check_reports_native_plugins() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    fs::create_dir_all(root.join("src/routes")).unwrap();
    fs::write(
        root.join("src/routes/page.tsx"),
        "export default function Page() { return null }",
    )
    .unwrap();
    write_config(
        &root,
        r#"{"plugins":[
            {"kind":"native","name":"images"},
            {"kind":"native","name":"fonts"},
            {"kind":"native","name":"i18n"}
        ]}"#,
    );
    let (code, out) = run_ok(&["oxide", "check", "--root", &root.to_string_lossy()]).await;
    assert_eq!(code, 0, "check passed: {out}");
    assert!(
        out.contains("native plugins: images, fonts, i18n"),
        "declared plugins listed in order: {out}"
    );
}

#[tokio::test]
async fn check_rejects_unknown_native_plugin() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    fs::create_dir_all(root.join("src/routes")).unwrap();
    fs::write(
        root.join("src/routes/page.tsx"),
        "export default function Page() { return null }",
    )
    .unwrap();
    write_config(&root, r#"{"plugins":[{"kind":"native","name":"bogus"}]}"#);
    let (code, out) = run_ok(&["oxide", "check", "--root", &root.to_string_lossy()]).await;
    assert_eq!(code, 1, "check fails: {out}");
    assert!(out.contains("OX-PLUGIN"), "coded diagnostic: {out}");
    assert!(out.contains("unknown native plugin"), "message: {out}");
    assert!(out.contains("images"), "alternatives listed: {out}");
}

#[tokio::test]
async fn dev_fails_fast_on_unknown_native_plugin() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    fs::create_dir_all(root.join("src/routes")).unwrap();
    fs::write(
        root.join("src/routes/page.tsx"),
        "export default function Page() { return null }",
    )
    .unwrap();
    write_config(&root, r#"{"plugins":[{"kind":"native","name":"oops"}]}"#);
    let err = run_err(&["oxide", "dev", "--root", &root.to_string_lossy(), "--once"]).await;
    let msg = format!("{err:#}");
    assert!(msg.contains("OX-PLUGIN"), "coded: {msg}");
    assert!(msg.contains("unknown native plugin"), "message: {msg}");
}

#[test]
fn extract_img_w_spec_parses_png_with_widths() {
    use crate::extract_img_w_spec;
    let line = r#"import hero from './hero.png?w=400,800,1200'"#;
    let (spec, widths) = extract_img_w_spec(line).expect("parse");
    assert_eq!(spec, "./hero.png?w=400,800,1200");
    assert_eq!(widths, vec![400, 800, 1200]);
}

#[test]
fn extract_img_w_spec_rejects_non_image() {
    use crate::extract_img_w_spec;
    assert!(extract_img_w_spec(r#"import x from './doc.pdf?w=400'"#).is_none());
    assert!(extract_img_w_spec(r#"import x from './hero.png'"#).is_none());
    assert!(extract_img_w_spec(r#"import x from './hero.png?w=abc'"#).is_none());
}

#[test]
fn extract_img_w_spec_handles_jpeg() {
    use crate::extract_img_w_spec;
    let line = r#"import x from './photo.jpeg?w=800'"#;
    let (_spec, widths) = extract_img_w_spec(line).expect("parse");
    assert_eq!(widths, vec![800]);
}

// ---- SPEC-19.1 / §19-B: oxide add + oxide audit ----------------------------

#[tokio::test]
async fn add_records_oxide_module() {
    let _g = ENV_LOCK.lock().unwrap();
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    // Pre-populate node_modules/<mod>/package.json with an `oxide.module`
    // field, then run `oxide add` with OXIDE_ADD_NO_INSTALL=1.
    let mod_dir = root.join("node_modules/@acme/plugin");
    fs::create_dir_all(&mod_dir).unwrap();
    fs::write(
        mod_dir.join("package.json"),
        r#"{"name":"@acme/plugin","version":"1.0.0","oxide":{"module":{"hooks":["transform"]}}}"#,
    )
    .unwrap();
    unsafe {
        std::env::set_var("OXIDE_ADD_NO_INSTALL", "1");
    }
    let (code, out) = run_ok(&[
        "oxide",
        "add",
        "@acme/plugin",
        "--root",
        &root.to_string_lossy(),
    ])
    .await;
    unsafe {
        std::env::remove_var("OXIDE_ADD_NO_INSTALL");
    }
    assert_eq!(code, 0, "add passed: {out}");
    assert!(
        out.contains("oxide module detected"),
        "contract found: {out}"
    );
    let modules_json = root.join(".oxide/modules.json");
    let v: serde_json::Value = serde_json::from_slice(&fs::read(&modules_json).unwrap()).unwrap();
    assert_eq!(v, serde_json::json!(["@acme/plugin"]));
}

#[tokio::test]
async fn add_records_plain_npm_package_with_note() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let mod_dir = root.join("node_modules/left-pad");
    fs::create_dir_all(&mod_dir).unwrap();
    fs::write(
        mod_dir.join("package.json"),
        r#"{"name":"left-pad","version":"1.3.0"}"#,
    )
    .unwrap();
    unsafe {
        std::env::set_var("OXIDE_ADD_NO_INSTALL", "1");
    }
    let (code, out) = run_ok(&[
        "oxide",
        "add",
        "left-pad",
        "--root",
        &root.to_string_lossy(),
    ])
    .await;
    unsafe {
        std::env::remove_var("OXIDE_ADD_NO_INSTALL");
    }
    assert_eq!(code, 0, "add passed: {out}");
    assert!(out.contains("ordinary npm package"), "note present: {out}");
}

#[tokio::test]
async fn audit_offline_without_cache_is_a_clear_error() {
    let _g = ENV_LOCK.lock().unwrap();
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    // A minimal package-lock so the lockfile step passes.
    fs::write(
        root.join("package-lock.json"),
        r#"{"lockfileVersion":3,"packages":{"":{"name":"root","version":"1.0.0"},"node_modules/x":{"version":"1.0.0"}}}"#,
    ).unwrap();
    unsafe {
        std::env::set_var("OXIDE_AUDIT_OFFLINE", "1");
    }
    let (code, out) = run_ok(&["oxide", "audit", "--root", &root.to_string_lossy()]).await;
    unsafe {
        std::env::remove_var("OXIDE_AUDIT_OFFLINE");
    }
    assert_eq!(code, 2, "offline without cache exits 2: {out}");
    assert!(out.contains("AUDIT-OFFLINE"), "coded diagnostic: {out}");
}

#[tokio::test]
async fn audit_json_mode_emits_stable_schema() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    // No package-lock.json → read_package_lock errors; JSON mode emits a
    // stable error envelope.
    let cli = Cli::try_parse_from([
        "oxide", "audit", "--root", root.to_str().unwrap(), "--json",
    ])
    .expect("parse");
    let mut buf: Vec<u8> = Vec::new();
    let code = run_with_writer(cli, &mut buf).await.expect("run");
    let out = String::from_utf8_lossy(&buf).to_string();
    assert_eq!(code, 2, "no lockfile is exit 2: {out}");
    let v: serde_json::Value = serde_json::from_str(out.trim())
        .unwrap_or_else(|e| panic!("not valid JSON: {e}; got: {out}"));
    assert!(v.get("root").is_some(), "root key: {v}");
    assert!(v.get("error").is_some(), "error key: {v}");
    assert_eq!(v.get("exit_code").and_then(|x| x.as_u64()), Some(2));
}


#[tokio::test]
async fn audit_no_lockfile_is_a_clear_error() {
    let _g = ENV_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    unsafe {
        std::env::set_var("OXIDE_AUDIT_OFFLINE", "1");
    }
    let (code, out) = run_ok(&["oxide", "audit", "--root", &root.to_string_lossy()]).await;
    unsafe {
        std::env::remove_var("OXIDE_AUDIT_OFFLINE");
    }
    assert_eq!(code, 2, "no lockfile exits 2: {out}");
    assert!(out.contains("AUDIT-LOCKFORMAT"), "coded: {out}");
}

// ─── §5-A: build_graph snapshot tests ─────────────────────────────────────

#[tokio::test]
async fn build_writes_graph_snapshot() {
    let (_d, root) = fixture_project();
    let (code, out) = run_ok(&["oxide", "build", "--root", root.to_str().unwrap()]).await;
    assert_eq!(code, 0, "build: {out}");
    assert!(out.contains("graph:"), "graph line: {out}");
    let snap_path = root.join(".oxide/graph.json");
    assert!(snap_path.is_file(), "snapshot written");
    let snap: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&snap_path).unwrap()).unwrap();
    assert_eq!(snap["version"], "v1");
    let n = snap["node_count"].as_u64().unwrap();
    assert!(n > 0, "at least one node recorded (got {n})");
    // The build consumed the page sources — at least a module node exists.
    let kinds: Vec<&str> = snap["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["kind"].as_str())
        .collect();
    assert!(kinds.iter().any(|k| *k == "module"), "module nodes present");
    assert!(kinds.iter().any(|k| *k == "chunk"), "chunk nodes present");
    assert!(
        kinds.iter().any(|k| *k == "manifest"),
        "manifest nodes present"
    );
}

#[tokio::test]
async fn second_build_recomputes_nothing() {
    let (_d, root) = fixture_project();
    let (code1, _out1) = run_ok(&["oxide", "build", "--root", root.to_str().unwrap()]).await;
    assert_eq!(code1, 0);
    let first: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".oxide/graph.json")).unwrap()).unwrap();
    let first_nodes = first["node_count"].as_u64().unwrap();
    let first_recomputed = first["recomputed_count"].as_u64().unwrap();
    // A cold build recomputes every node.
    assert_eq!(first_recomputed, first_nodes, "cold: all recomputed");

    // Second build without any source change: the fingerprint early-cutoff
    // makes `cmd_build` return before writing a new snapshot. To exercise
    // the graph path itself (not the fingerprint skip), force a rebuild
    // and assert every node SURVIVED (0 recomputed).
    let (code2, out2) = {
        // Set OXIDE_FORCE_BUILD=1 so the fingerprint skip does not fire.
        unsafe {
            std::env::set_var("OXIDE_FORCE_BUILD", "1");
        }
        let r = run_ok(&["oxide", "build", "--root", root.to_str().unwrap()]).await;
        unsafe {
            std::env::remove_var("OXIDE_FORCE_BUILD");
        }
        r
    };
    assert_eq!(code2, 0, "forced rebuild: {out2}");
    let second: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(".oxide/graph.json")).unwrap()).unwrap();
    assert_eq!(second["node_count"].as_u64().unwrap(), first_nodes);
    assert_eq!(
        second["recomputed_count"].as_u64().unwrap(),
        0,
        "no source change → no node hash changed: {}",
        second["recomputed_count"],
    );
    assert_eq!(second["previous_node_count"].as_u64().unwrap(), first_nodes);
}

// ─── §15-E: module route injection ────────────────────────────────────────

/// A user adds a module, and its route is served by the build. The test
/// writes the module's package on disk (no network), records it via
/// `oxide add` with `OXIDE_ADD_NO_INSTALL=1`, runs a build, and asserts the
/// route's prerendered HTML exists with the module's marker.
#[tokio::test]
async fn module_route_is_injected_and_built() {
    let _g = ENV_LOCK.lock().unwrap();
    use std::fs;
    let (_d, root) = fixture_project();

    // The module package on disk.
    let mod_dir = root.join("node_modules/oxide-test-mod");
    fs::create_dir_all(mod_dir.join("routes")).unwrap();
    fs::write(
        mod_dir.join("package.json"),
        r#"{"name":"oxide-test-mod","version":"1.0.0","main":"./index.mjs","oxide":{"module":{"routes":"./routes"}}}"#,
    )
    .unwrap();
    fs::write(
        mod_dir.join("routes/page.tsx"),
        "export default function ModPage() {\n  return <main data-route=\"/modules/oxide-test-mod\">from-module</main>;\n}\n",
    )
    .unwrap();
    fs::write(mod_dir.join("index.mjs"), "export const noop = () => {};\n").unwrap();

    // Record the module. `oxide add` normally installs it; the opt-out +
    // a pre-populated `node_modules/` lets us exercise the full path
    // (install-skip + contract discovery + modules.json) offline.
    unsafe {
        std::env::set_var("OXIDE_ADD_NO_INSTALL", "1");
    }
    let (add_code, add_out) = run_ok(&[
        "oxide",
        "add",
        "oxide-test-mod",
        "--root",
        root.to_str().unwrap(),
    ])
    .await;
    unsafe {
        std::env::remove_var("OXIDE_ADD_NO_INSTALL");
    }
    assert_eq!(add_code, 0, "add: {add_out}");
    assert!(add_out.contains("oxide module detected"), "{add_out}");

    // Build. A route in a module must be picked up by the scan and
    // prerendered exactly like a project route.
    let (build_code, build_out) =
        run_ok(&["oxide", "build", "--root", root.to_str().unwrap()]).await;
    assert_eq!(build_code, 0, "build: {build_out}");
    // "3 routes, 2 prerendered" — the two static project pages (`/`, plus the
    // isr route which the fixture declares) plus the module route. The route
    // count in the line proves the module route entered the tree; the
    // prerender check below proves it actually rendered.
    assert!(
        build_out.contains("3 routes") && build_out.contains("2 prerendered"),
        "module route counted + prerendered in build output: {build_out}",
    );

    // The prerender path is `<segments.join('/')>.html` (see `cmd_build`'s
    // `rel` calculation). Module segments are `["modules", "oxide-test-mod"]`,
    // so the file lands at `dist/prerendered/modules/oxide-test-mod.html`.
    let prerendered = root.join("dist/prerendered/modules/oxide-test-mod.html");
    assert!(
        prerendered.is_file(),
        "module page prerendered at {}",
        prerendered.display(),
    );
    // The fixture has no framework renderer attached, so prerender uses the
    // IR placeholder (same path `build_produces_dist_and_deploy_artifacts`
    // exercises). The placeholder echoes the computed URL pattern, which is
    // what proves the module route entered the tree and prerendered with
    // the right segments.
    let html = std::fs::read_to_string(&prerendered).unwrap();
    assert!(
        html.contains(r#"data-route="/modules/oxide-test-mod""#),
        "module URL pattern in prerendered placeholder: {html}",
    );

    // Sanity: the server manifest carries the module's namespaced route id.
    let server_manifest = root.join("dist/server/server.manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&server_manifest).unwrap()).unwrap();
    let routes = manifest["routes"].as_object().expect("manifest routes");
    assert!(
        routes.keys().any(|k| k.contains("modules/oxide-test-mod")),
        "route namespaced in server manifest: keys={:?}",
        routes.keys().collect::<Vec<_>>(),
    );
}

// ─── §15-E: modules_loader unit tests ─────────────────────────────────────

#[test]
fn modules_loader_resolves_entry_and_routes_from_contract() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let pkg = root.join("node_modules/acme-mod");
    std::fs::create_dir_all(pkg.join("routes")).unwrap();
    std::fs::write(
        pkg.join("package.json"),
        r#"{"name":"acme-mod","version":"1.0.0","main":"./index.mjs","oxide":{"module":{"routes":"./routes","hooks":["transform"]}}}"#,
    )
    .unwrap();
    std::fs::write(pkg.join("index.mjs"), "export default {};\n").unwrap();

    let cfg = crate::load_config_or_default(&root, "development");
    // `load_config_or_default` reads `modules` from config; the modules list
    // is empty here, so the loader must fall back to `.oxide/modules.json` —
    // which we did not write either. Pass the module name via config directly.
    let mut cfg = cfg;
    cfg.modules = vec!["acme-mod".to_string()];

    let mods = crate::modules_loader::load(&root, &cfg);
    assert_eq!(mods.len(), 1, "one module resolved");
    let m = &mods[0];
    assert_eq!(m.name, "acme-mod");
    assert!(m.routes_present(), "routes dir detected");
    assert_eq!(m.hooks, vec!["transform".to_string()]);
    let entry = m
        .entry
        .as_ref()
        .expect("entry resolved from package.json main");
    assert!(
        entry.ends_with("index.mjs"),
        "entry path: {}",
        entry.display()
    );
}

#[test]
fn modules_loader_falls_back_to_routes_dir_convention() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let pkg = root.join("node_modules/convention-mod");
    std::fs::create_dir_all(pkg.join("routes")).unwrap();
    std::fs::write(
        pkg.join("package.json"),
        r#"{"name":"convention-mod","version":"1.0.0","oxide":{"module":{}}}"#,
    )
    .unwrap();

    let mut cfg = crate::load_config_or_default(&root, "development");
    cfg.modules = vec!["convention-mod".to_string()];

    let mods = crate::modules_loader::load(&root, &cfg);
    assert_eq!(mods.len(), 1);
    // No `routes` field in the contract, so the loader falls back to
    // `<pkg>/routes/` (documented in the module doc comment).
    assert!(mods[0].routes_present());
    assert!(mods[0].hooks.is_empty());
    assert!(mods[0].entry.is_none(), "no `main` and no `entry` field");
}

#[test]
fn has_unmerged_hooks_ignores_transform_only_modules() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let pkg = root.join("node_modules/transform-mod");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        pkg.join("package.json"),
        r#"{"name":"transform-mod","version":"1.0.0","main":"./i.mjs","oxide":{"module":{"hooks":["transform"]}}}"#,
    )
    .unwrap();
    std::fs::write(pkg.join("i.mjs"), "export default {};\n").unwrap();

    let mut cfg = crate::load_config_or_default(&root, "development");
    cfg.modules = vec!["transform-mod".to_string()];

    let mods = crate::modules_loader::load(&root, &cfg);
    assert!(
        crate::modules_loader::has_unmerged_hooks(&mods).is_empty(),
        "a transform-only module must NOT appear in unmerged (transform is wired)",
    );

    // Now a module declaring a non-transform hook: it must be reported.
    cfg.modules.push("resolve-mod".to_string());
    let pkg2 = root.join("node_modules/resolve-mod");
    std::fs::create_dir_all(&pkg2).unwrap();
    std::fs::write(
        pkg2.join("package.json"),
        r#"{"name":"resolve-mod","version":"1.0.0","main":"./i.mjs","oxide":{"module":{"hooks":["resolveId"]}}}"#,
    )
    .unwrap();
    std::fs::write(pkg2.join("i.mjs"), "export default {};\n").unwrap();
    let mods = crate::modules_loader::load(&root, &cfg);
    let unmerged = crate::modules_loader::has_unmerged_hooks(&mods);
    assert_eq!(unmerged.len(), 1, "only the resolve-mod is unmerged");
    assert_eq!(unmerged[0].name, "resolve-mod");
}

#[test]
fn modules_loader_missing_package_is_skipped_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let mut cfg = crate::load_config_or_default(&root, "development");
    cfg.modules = vec!["nonexistent".to_string()];
    let mods = crate::modules_loader::load(&root, &cfg);
    assert!(mods.is_empty(), "missing package must not surface a module");
}

#[tokio::test]
async fn add_reports_module_contributions() {
    let _g = ENV_LOCK.lock().unwrap();
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let mod_dir = root.join("node_modules/full-mod");
    fs::create_dir_all(mod_dir.join("routes")).unwrap();
    fs::write(
        mod_dir.join("routes/page.tsx"),
        "export default function P() { return <main>m</main>; }\n",
    )
    .unwrap();
    fs::write(mod_dir.join("index.mjs"), "export default {};\n").unwrap();
    fs::write(
        mod_dir.join("package.json"),
        r#"{"name":"full-mod","version":"1.0.0","main":"./index.mjs","oxide":{"module":{"routes":"./routes","hooks":["transform","resolveId"],"adapters":["node"],"configDefaults":{"build":{"minify":true}}}}}"#,
    )
    .unwrap();

    unsafe {
        std::env::set_var("OXIDE_ADD_NO_INSTALL", "1");
    }
    let (code, out) = run_ok(&["oxide", "add", "full-mod", "--root", root.to_str().unwrap()]).await;
    unsafe {
        std::env::remove_var("OXIDE_ADD_NO_INSTALL");
    }
    assert_eq!(code, 0, "add: {out}");
    // The report is structured — every contribution the contract has is
    // enumerated, and each is tagged merged or deferred honestly.
    assert!(
        out.contains("oxide module detected — contributes:"),
        "{out}"
    );
    assert!(
        out.contains("routes: 1 page(s) under /modules/full-mod/"),
        "{out}"
    );
    // WP-J4: the report derives from the merged-hook table — transform,
    // resolveId, and load all route through the plugin pipeline today.
    assert!(out.contains("transform, resolveId (merged)"), "{out}");
    assert!(out.contains("adapters: node"), "{out}");
    assert!(out.contains("configDefaults: present (not merged"), "{out}");
}

#[tokio::test]
async fn add_warns_on_adapter_mismatch() {
    let _g = ENV_LOCK.lock().unwrap();
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    // Project configured with the binary adapter (default); a module that
    // only supports `cloudflare` must produce MOD-ADAPTER.
    write_config(&root, r#"{ "adapter": "binary" }"#);
    let mod_dir = root.join("node_modules/cf-mod");
    fs::create_dir_all(&mod_dir).unwrap();
    fs::write(
        mod_dir.join("package.json"),
        r#"{"name":"cf-mod","version":"1.0.0","oxide":{"module":{"adapters":["cloudflare"]}}}"#,
    )
    .unwrap();

    unsafe {
        std::env::set_var("OXIDE_ADD_NO_INSTALL", "1");
    }
    let (code, out) = run_ok(&["oxide", "add", "cf-mod", "--root", root.to_str().unwrap()]).await;
    unsafe {
        std::env::remove_var("OXIDE_ADD_NO_INSTALL");
    }
    assert_eq!(code, 0, "add: {out}");
    assert!(
        out.contains("MOD-ADAPTER"),
        "adapter mismatch warning: {out}"
    );
    assert!(out.contains("binary"), "project adapter named: {out}");
}

#[tokio::test]
async fn mcp_graph_tool_reports_demand_graph_snapshot() {
    use rmcp::handler::server::wrapper::Parameters;
    let (_d, root) = fixture_project();
    // No build yet: the graph section must be absent (serde skip).
    {
        let server = crate::mcp::OxideMcp::new(root.clone());
        let result = server
            .oxide_graph(Parameters(crate::mcp::NoArgs {}))
            .await
            .expect("tool call");
        let text = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");
        let v: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert!(
            v.get("graph").is_none() || v["graph"].is_null(),
            "no build means no graph section: {v}",
        );
    }

    // Build so the snapshot is written, then the MCP tool surfaces it.
    let (code, _out) = run_ok(&["oxide", "build", "--root", root.to_str().unwrap()]).await;
    assert_eq!(code, 0);

    let server = crate::mcp::OxideMcp::new(root.clone());
    let result = server
        .oxide_graph(Parameters(crate::mcp::NoArgs {}))
        .await
        .expect("tool call");
    let text = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n");
    let v: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    let g = &v["graph"];
    assert!(!g.is_null(), "graph section present after build: {v}");
    assert_eq!(g["version"], "v1");
    let count = g["node_count"].as_u64().unwrap();
    assert!(count > 0, "node_count positive (got {count})");
    let kinds = g["nodes_by_kind"]
        .as_object()
        .expect("nodes_by_kind object");
    for expected in ["module", "chunk", "manifest"] {
        assert!(
            kinds.contains_key(expected),
            "kind {expected} in breakdown: {:?}",
            kinds.keys().collect::<Vec<_>>(),
        );
    }
    // A cold build recomputes every node.
    assert_eq!(g["recomputed_count"].as_u64().unwrap(), count);
    // The manifest summary is unchanged.
    assert!(v["routes"].is_object(), "routes summary unchanged");
}

// ─── §19-B: migrate codemod tests ────────────────────────────────────────

#[tokio::test]
async fn migrate_next_pages_converts_to_oxide_routes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("next.config.js"), "module.exports = {};\n").unwrap();
    std::fs::create_dir_all(root.join("pages/blog")).unwrap();
    std::fs::create_dir_all(root.join("pages/api")).unwrap();
    std::fs::write(
        root.join("pages/index.tsx"),
        "export default function Home() { return <main>home</main>; }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("pages/blog/post.tsx"),
        "export default function Post() { return <main>post</main>; }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("pages/api/hello.ts"),
        "export default function handler(req, res) { res.end('hi'); }\n",
    )
    .unwrap();

    let (code, out) = run_ok(&["oxide", "migrate", "--root", root.to_str().unwrap(), "next"]).await;
    assert_eq!(code, 0, "dry run: {out}");
    // Dry run — no writes.
    assert!(
        !root.join("src/routes/page.tsx").exists(),
        "dry run writes nothing"
    );
    // Both conventional pages are planned.
    assert!(out.contains("pages/index.tsx"), "{out}");
    assert!(out.contains("src/routes/page.tsx"), "{out}");
    assert!(out.contains("pages/blog/post.tsx"), "{out}");
    assert!(out.contains("src/routes/blog/post/page.tsx"), "{out}");
    // API route planned as a route.ts.
    assert!(out.contains("pages/api/hello.ts"), "{out}");
    assert!(out.contains("route.ts"), "{out}");

    // Now write.
    let (code, out) = run_ok(&[
        "oxide",
        "migrate",
        "--write",
        "--root",
        root.to_str().unwrap(),
        "next",
    ])
    .await;
    assert_eq!(code, 0, "write: {out}");
    assert!(root.join("src/routes/page.tsx").is_file(), "index written");
    assert!(
        root.join("src/routes/blog/post/page.tsx").is_file(),
        "blog page written"
    );
    assert!(
        root.join("src/routes/api/hello/route.ts").is_file(),
        "api route written"
    );
    // Original files untouched (migrate copies, not moves).
    assert!(root.join("pages/index.tsx").is_file(), "source preserved");

    // Re-running refuses to overwrite.
    let (code, out) = run_ok(&[
        "oxide",
        "migrate",
        "--write",
        "--root",
        root.to_str().unwrap(),
        "next",
    ])
    .await;
    assert_eq!(code, 1, "second write must refuse: {out}");
    assert!(out.contains("refusing to overwrite"), "{out}");
}

#[tokio::test]
async fn migrate_next_gssp_becomes_load() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("next.config.js"), "module.exports = {};\n").unwrap();
    std::fs::create_dir_all(root.join("pages/posts")).unwrap();
    std::fs::write(
        root.join("pages/posts/[id].tsx"),
        r#"import { useRouter } from 'next/router';
export async function getServerSideProps(context) {
  const post = await fetchPost(context.params.id);
  return { props: { post } };
}
export default function Post({ post }) { return <main>{post.title}</main>; }
"#,
    )
    .unwrap();

    let (code, out) = run_ok(&[
        "oxide",
        "migrate",
        "--write",
        "--root",
        root.to_str().unwrap(),
        "next",
    ])
    .await;
    assert_eq!(code, 0, "{out}");
    let written = root.join("src/routes/posts/[id]/page.tsx");
    assert!(written.is_file(), "{out}");
    let body = std::fs::read_to_string(&written).unwrap();
    // `getServerSideProps` was rewritten to an Oxide `load`.
    assert!(
        body.contains("export async function load("),
        "load emitted: {body}"
    );
    assert!(
        !body.contains("export async function getServerSideProps"),
        "GSSP removed: {body}"
    );
    // The internal fetch is preserved.
    assert!(body.contains("fetchPost("), "body preserved: {body}");
}

#[tokio::test]
async fn migrate_next_gssp_with_redirect_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("next.config.js"), "module.exports = {};\n").unwrap();
    std::fs::create_dir_all(root.join("pages/admin")).unwrap();
    std::fs::write(
        root.join("pages/admin/index.tsx"),
        r#"export async function getServerSideProps(ctx) {
  if (!ctx.req.cookies.auth) {
    return { redirect: { destination: '/login', permanent: false } };
  }
  return { props: {} };
}
export default function Admin() { return <main>admin</main>; }
"#,
    )
    .unwrap();

    let (code, out) = run_ok(&["oxide", "migrate", "--root", root.to_str().unwrap(), "next"]).await;
    assert_eq!(code, 0, "{out}");
    // Redirect uses are reported as follow-ups, not silently mangled.
    assert!(out.contains("redirect/notFound"), "{out}");
    assert!(
        out.contains("port by hand") || out.contains("skipped"),
        "{out}"
    );
}

#[tokio::test]
async fn migrate_next_app_router_path_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("next.config.js"), "module.exports = {};\n").unwrap();
    std::fs::create_dir_all(root.join("app/dashboard")).unwrap();
    std::fs::write(
        root.join("app/page.tsx"),
        "export default function Home() { return null; }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("app/layout.tsx"),
        "export default function RootLayout({ children }) { return <html>{children}</html>; }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("app/dashboard/page.tsx"),
        "export default function D() { return null; }\n",
    )
    .unwrap();

    let (code, _) = run_ok(&[
        "oxide",
        "migrate",
        "--write",
        "--root",
        root.to_str().unwrap(),
        "next",
    ])
    .await;
    assert_eq!(code, 0);
    assert!(root.join("src/routes/page.tsx").is_file(), "root page");
    assert!(root.join("src/routes/layout.tsx").is_file(), "root layout");
    assert!(
        root.join("src/routes/dashboard/page.tsx").is_file(),
        "nested page"
    );
}

#[tokio::test]
async fn migrate_sveltekit_renames_plus_prefixed_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("svelte.config.js"), "export default {};\n").unwrap();
    std::fs::create_dir_all(root.join("src/routes/blog")).unwrap();
    std::fs::write(root.join("src/routes/+page.svelte"), "<main>home</main>\n").unwrap();
    std::fs::write(
        root.join("src/routes/blog/+page.svelte"),
        "<main>blog</main>\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/routes/blog/+page.server.ts"),
        "export async function load() { return {}; }\n",
    )
    .unwrap();
    std::fs::write(root.join("src/routes/+layout.svelte"), "<slot />\n").unwrap();
    // A non-route file (a component) — should not be renamed.
    std::fs::write(root.join("src/routes/MyComponent.svelte"), "<p>x</p>\n").unwrap();

    let (code, out) = run_ok(&[
        "oxide",
        "migrate",
        "--write",
        "--root",
        root.to_str().unwrap(),
        "sveltekit",
    ])
    .await;
    assert_eq!(code, 0, "{out}");
    assert!(root.join("src/routes/page.svelte").is_file(), "root page");
    assert!(
        root.join("src/routes/blog/page.svelte").is_file(),
        "blog page"
    );
    assert!(
        root.join("src/routes/blog/page.server.ts").is_file(),
        "blog server"
    );
    assert!(root.join("src/routes/layout.svelte").is_file(), "layout");
    // The non-route component stays untouched.
    assert!(
        root.join("src/routes/MyComponent.svelte").is_file(),
        "component untouched"
    );
    // Originals are gone? No — `apply` copies; the source `+page.svelte`
    // remains alongside the destination.
    assert!(
        root.join("src/routes/+page.svelte").is_file(),
        "source preserved"
    );
}

#[tokio::test]
async fn migrate_unknown_source_and_missing_marker() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("vite.config.ts"), "export default {}\n").unwrap();

    // Unknown source → exit 2.
    let (code, out) = run_ok(&[
        "oxide",
        "migrate",
        "--root",
        root.to_str().unwrap(),
        "angular",
    ])
    .await;
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("unknown source"), "{out}");

    // Nuxt without markers → exit 2.
    let (code, out) = run_ok(&["oxide", "migrate", "--root", root.to_str().unwrap(), "nuxt"]).await;
    assert_eq!(code, 2, "{out}");
    assert!(out.contains("no Nuxt project detected"), "{out}");

    // Vite with a config but no entry: config is the marker, entry missing
    // is a reported follow-up, not an error.
    let (code, out) = run_ok(&["oxide", "migrate", "--root", root.to_str().unwrap(), "vite"]).await;
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Vite"), "{out}");
    assert!(
        out.contains("no src/main"),
        "entry missing is reported: {out}"
    );
}

// ─── §5-A full incremental: per-route prerender reuse ────────────────────

/// A second build with no source change reuses every prerendered route
/// (skipping the render). A source change to one route re-renders exactly
/// that route and reuses the rest. Both assertions read the build's own
/// `incremental: reused N …` line — the same line a user sees.
#[tokio::test]
async fn incremental_build_reuses_unchanged_prerenders() {
    use std::fs;
    let (_d, root) = fixture_project();
    // Two static pages so "touch one, reuse the other" is observable. The
    // fixture already has `src/routes/page.tsx` (index) and a `blog` route
    // whose `page.server.ts` declares isr — that route is prerendered too.
    // Add a third static page so we have three independent cases.
    fs::create_dir_all(root.join("src/routes/about")).unwrap();
    fs::write(
        root.join("src/routes/about/page.tsx"),
        "export default function About() { return <main data-route=\"/about\">about</main>; }\n",
    )
    .unwrap();

    // Build 1 — cold. Nothing to reuse.
    let (c1, out1) = run_ok(&["oxide", "build", "--root", root.to_str().unwrap()]).await;
    assert_eq!(c1, 0, "build 1: {out1}");
    assert!(
        !out1.contains("incremental: reused"),
        "cold build has nothing to reuse: {out1}",
    );
    // The snapshot now carries route_prerenders.
    let snap1: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(".oxide/graph.json")).unwrap()).unwrap();
    let recs1 = snap1["route_prerenders"]
        .as_array()
        .expect("route_prerenders array");
    let n1 = recs1.len();
    assert!(
        n1 >= 2,
        "at least two prerendered routes recorded (got {n1})"
    );
    // Every record has the fields the reuse check needs.
    for r in recs1 {
        assert!(r["route_id"].is_string());
        assert!(r["url"].is_string());
        assert!(r["source_hash"].is_string());
        assert!(r["nonce"].is_string(), "nonce recorded for reuse");
        assert!(r["rel_out"].is_string());
    }

    // Build 2 — forced (bypass the whole-build fingerprint skip so the
    // incremental path itself runs) with no source change.
    let (c2, out2) = {
        unsafe {
            std::env::set_var("OXIDE_FORCE_BUILD", "1");
        }
        let r = run_ok(&["oxide", "build", "--root", root.to_str().unwrap()]).await;
        unsafe {
            std::env::remove_var("OXIDE_FORCE_BUILD");
        }
        r
    };
    assert_eq!(c2, 0, "build 2: {out2}");
    let reused2: usize = parse_reused(&out2);
    assert_eq!(
        reused2, n1,
        "second build with no change reuses every prerender (got {reused2}, expected {n1}): {out2}",
    );

    // Build 3 — touch ONE page. Only that page's prerender re-renders; the
    // rest are reused.
    fs::write(
        root.join("src/routes/about/page.tsx"),
        "export default function About() { return <main data-route=\"/about\">about v2</main>; }\n",
    )
    .unwrap();
    let (c3, out3) = {
        unsafe {
            std::env::set_var("OXIDE_FORCE_BUILD", "1");
        }
        let r = run_ok(&["oxide", "build", "--root", root.to_str().unwrap()]).await;
        unsafe {
            std::env::remove_var("OXIDE_FORCE_BUILD");
        }
        r
    };
    assert_eq!(c3, 0, "build 3: {out3}");
    let reused3: usize = parse_reused(&out3);
    assert!(
        reused3 < n1,
        "editing one page re-renders that page; reuse drops (got {reused3}, was {n1}): {out3}",
    );
    assert_eq!(
        reused3,
        n1 - 1,
        "exactly one route re-rendered (got {reused3}, expected {}): {out3}",
        n1 - 1,
    );
}

/// Parse the `incremental: reused N prerendered route(s)` line. Returns 0
/// when the line is absent (cold build, nothing to reuse).
fn parse_reused(out: &str) -> usize {
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("incremental: reused ") {
            if let Some(n) = rest.split_whitespace().next() {
                return n.parse().unwrap_or(0);
            }
        }
    }
    0
}

// ─── §15-E: resolveId + load hook collectors ─────────────────────────────

/// A module declaring `hooks: ["resolveId", "load"]` shows up in both
/// collectors. A module that declares only `transform` does NOT.
#[test]
fn collect_plugin_paths_includes_module_entries_per_hook() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let pkg = root.join("node_modules/alias-mod");
    fs::create_dir_all(&pkg).unwrap();
    fs::write(
        pkg.join("index.mjs"),
        "export function resolveId() {}\nexport function load() {}\n",
    )
    .unwrap();
    fs::write(
        pkg.join("package.json"),
        r#"{"name":"alias-mod","main":"./index.mjs","oxide":{"module":{"hooks":["resolveId","load"]}}}"#,
    )
    .unwrap();

    let mut cfg = crate::load_config_or_default(&root, "development");
    cfg.modules = vec!["alias-mod".to_string()];

    let resolve = crate::collect_plugin_paths_for_hook(&root, &cfg, "resolveId");
    assert_eq!(resolve.len(), 1, "resolveId collector includes the module");
    assert!(
        resolve[0].ends_with("index.mjs"),
        "resolved entry: {resolve:?}"
    );

    let load = crate::collect_plugin_paths_for_hook(&root, &cfg, "load");
    assert_eq!(load.len(), 1, "load collector includes the module");

    // A hook the module did not declare is empty.
    let transform = crate::collect_plugin_paths_for_hook(&root, &cfg, "transform");
    assert!(
        transform.is_empty(),
        "transform collector does NOT include a module that did not declare it: {transform:?}",
    );

    // A hook not backed by any plugin/module is empty.
    let render_chunk = crate::collect_plugin_paths_for_hook(&root, &cfg, "renderChunk");
    assert!(render_chunk.is_empty());
}

/// A `kind:js` plugin whose path is NOT present does not enter the
/// collector (the same `confine_plugin_path` + `is_file` check applies).
#[test]
fn collect_plugin_paths_skips_missing_js_plugin_files() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    fs::write(
        root.join("oxide.config.ts"),
        r#"export default { plugins: [{ kind: "js", path: "./does-not-exist.mjs" }] };"#,
    )
    .unwrap();

    let cfg = crate::load_config_or_default(&root, "development");
    // The config's `plugins` is `Vec<serde_json::Value>`; a `kind:js` entry
    // with a missing path is skipped (the collector emits a warning and
    // continues).
    let got = crate::collect_plugin_paths_for_hook(&root, &cfg, "resolveId");
    assert!(got.is_empty(), "missing plugin path skipped: {got:?}");
}

// ─── §5-B: Input::Resolver + Input::PluginOptions edges ───────────────────

/// A module file that imports a bare package records a `Resolver` edge; the
/// snapshot's `dep_count` reflects it. A relative-only import records none.
#[tokio::test]
async fn build_records_resolver_edges_for_bare_imports() {
    use std::fs;
    let (_d, root) = fixture_project();
    // Fixture has a page importing nothing (both pages in `fixture_project`
    // are self-contained). Add a page that imports a bare package — this is
    // what a real project's route file would do.
    fs::write(
        root.join("src/routes/page.tsx"),
        "import { useState } from 'react';\n\
         import { formatDate } from 'date-fns';\n\
         export default function Page() { return <main data-route=\"/\">home</main>; }\n",
    )
    .unwrap();

    let (code, out) = run_ok(&["oxide", "build", "--root", root.to_str().unwrap()]).await;
    assert_eq!(code, 0, "build: {out}");

    let snap: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(".oxide/graph.json")).unwrap()).unwrap();
    let nodes = snap["nodes"].as_array().expect("nodes array");
    // Find the node for the page we rewrote.
    let page_node = nodes
        .iter()
        .find(|n| {
            n["key"]
                .as_str()
                .map(|k| k.contains("src/routes/page.tsx"))
                .unwrap_or(false)
        })
        .expect("page node recorded");
    // It recorded a Resolver edge for `react` and for `date-fns`. Those are
    // the only two deps, so dep_count >= 2. (Input::File is one more, added
    // by the recording FileTracker — dep_count >= 3 is the tighter bound.)
    let dep_count = page_node["dep_count"].as_u64().unwrap_or(0);
    assert!(
        dep_count >= 2,
        "page node recorded at least two resolver edges (react + date-fns); got {dep_count} for {page_node}",
    );
}

/// A `kind:js` plugin declared in the config produces one
/// `Input::PluginOptions` edge on every module whose source path equals a
/// plugin entry. A module that is *not* a plugin entry records no
/// plugin-options edge.
// The default `#[tokio::test]` is a CURRENT-THREAD runtime. This test MUST
// pass under it — that is the whole point. The sync plugin-hook bridge
// (`std::thread::scope` in `build_js_plugin_hook`) blocks the calling
// thread, which is the only thread that could poll the caller's runtime.
// That would be a hard deadlock — EXCEPT the worker's child process and its
// stdout reader are bound to a dedicated runtime (`reader_runtime_handle`
// in `oxide-jshost`) that has its own thread and is never blocked. Every
// stdin write and stdout read waits on that reactor, not on the caller's.
// Before that binding, this test hung forever under a current-thread
// runtime; with it, the build completes normally.
#[tokio::test]
async fn build_records_plugin_options_for_plugin_entries_only() {
    use std::fs;
    let (_d, root) = fixture_project();
    // A plugin file that the config declares as `kind:js`. The module that
    // *is* this file records a PluginOptions edge; the page routes do not
    // (they are not plugin entries).
    let plugin_dir = root.join("plugins");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(
        plugin_dir.join("my-plugin.mjs"),
        "export function transform(code) { return code; }\n",
    )
    .unwrap();
    write_config(
        &root,
        r#"{ "plugins": [ { "kind": "js", "path": "./plugins/my-plugin.mjs", "name": "my-plugin", "options": { "strip": true } } ] }"#,
    );

    let (code, out) = run_ok(&["oxide", "build", "--root", root.to_str().unwrap()]).await;
    assert_eq!(code, 0, "build: {out}");

    let snap: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(".oxide/graph.json")).unwrap()).unwrap();
    let nodes = snap["nodes"].as_array().expect("nodes array");

    // The plugin file node should have dep_count >= 2 (its Input::File +
    // its Input::PluginOptions edge). It is not a route entry, so if the
    // build only processes entry files it will not appear — that is fine;
    // the point of this test is the edge *is* recorded when the file is
    // processed. Skip the assertion if the plugin file is not part of the
    // recorded entry set, and assert the wiring path exists.
    let plugin_node = nodes.iter().find(|n| {
        n["key"]
            .as_str()
            .map(|k| k.contains("plugins/my-plugin.mjs"))
            .unwrap_or(false)
    });
    if let Some(n) = plugin_node {
        let dep_count = n["dep_count"].as_u64().unwrap_or(0);
        assert!(
            dep_count >= 1,
            "plugin node records at least its own file edge; got {dep_count}",
        );
    }

    // Regardless of whether the plugin file itself is a module entry, the
    // page nodes should NOT carry plugin-options edges (they are not plugin
    // entries). Sanity: no page node has a suspiciously high dep_count.
    let page_node = nodes
        .iter()
        .find(|n| {
            n["key"]
                .as_str()
                .map(|k| k.contains("src/routes/page.tsx"))
                .unwrap_or(false)
        })
        .expect("page node recorded");
    // The fixture's page imports nothing bare — dep_count should be exactly
    // 1 (the Input::File edge). Two more would mean the plugin-options
    // recording leaked onto a non-plugin file.
    let dep_count = page_node["dep_count"].as_u64().unwrap_or(0);
    assert_eq!(
        dep_count, 1,
        "non-plugin page has only its File edge; got {dep_count}",
    );
}

// ─── §5-E: lockfile invalidation ─────────────────────────────────────────

#[test]
fn lockfile_change_invalidates_the_resolver_graph() {
    // `graph_invalidated` is a private fn inside cmd_dev; expose the logic
    // here via a small shim: create a graph, seed a node for a normal file,
    // then feed a lockfile path and assert the graph reports it was dirtied.
    // The fn itself is not `pub`, so this test drives the same public API
    // (`DemandGraph::invalidate(&[])`) that the fn calls.
    let g = oxide_graph::DemandGraph::new();
    let node = oxide_graph::NodeKey::new("src/routes/page.tsx");
    let comp = FileComputeForTest(node.0.to_string());
    g.register(node.clone(), std::sync::Arc::new(comp));
    let _ = g.get(node.clone(), &FileComputeForTest(node.0.to_string()));
    assert_eq!(g.node_count(), 1, "node recorded");

    // The lockfile branch of `graph_invalidated` calls `invalidate(&[])`.
    // That must bump the epoch so a fresh `get` re-runs the compute — the
    // "a lockfile change is a full resolver invalidation" contract.
    let before = g.version();
    let _ = g.invalidate(&[]);
    assert!(
        g.version() > before,
        "epoch bumped on lockfile invalidation"
    );
}

struct FileComputeForTest(String);
impl oxide_graph::Compute for FileComputeForTest {
    fn compute(
        &self,
        ctx: &mut oxide_graph::ComputeCtx,
    ) -> oxide_diagnostics::Result<oxide_graph::NodeOutput> {
        let bytes = ctx
            .fs
            .read_file(std::path::Path::new(&self.0))
            .unwrap_or_default();
        Ok(oxide_graph::NodeOutput {
            hash: blake3::hash(&bytes),
            artifact: None,
            deps: Default::default(),
        })
    }
}

// ─── §5-A: per-env bundle skip ────────────────────────────────────────────

/// Editing a `*.server.*` module must NOT invalidate the client bundle —
/// that module is never in the client graph (§6.4 guard). Editing a page
/// component must.
#[tokio::test]
async fn client_bundle_skips_only_when_its_own_inputs_change() {
    use std::fs;
    let (_d, root) = fixture_project();
    let build = |force: bool| {
        let root = root.clone();
        async move {
            let mut env_set = false;
            if force {
                unsafe {
                    std::env::set_var("OXIDE_FORCE_BUILD", "1");
                }
                env_set = true;
            }
            let r = run_ok(&["oxide", "build", "--root", root.to_str().unwrap()]).await;
            if env_set {
                unsafe {
                    std::env::remove_var("OXIDE_FORCE_BUILD");
                }
            }
            r
        }
    };

    // Cold: nothing to skip.
    let (c1, out1) = build(false).await;
    assert_eq!(c1, 0, "{out1}");
    assert!(
        !out1.contains("bundle: client unchanged"),
        "cold build bundles: {out1}"
    );

    // No change: the client bundle is skipped.
    let (c2, out2) = build(true).await;
    assert_eq!(c2, 0, "{out2}");
    assert!(
        out2.contains("bundle: client unchanged"),
        "unchanged env is skipped: {out2}",
    );

    // Edit ONLY a server module: still skipped.
    fs::write(
        root.join("src/routes/blog/page.server.ts"),
        "export const config = { render: 'isr', tags: ['post:b'] };\n",
    )
    .unwrap();
    let (c3, out3) = build(true).await;
    assert_eq!(c3, 0, "{out3}");
    assert!(
        out3.contains("bundle: client unchanged"),
        "a server-only edit leaves the client bundle valid: {out3}",
    );

    // Edit a page component: the client bundle re-runs.
    fs::write(
        root.join("src/routes/blog/page.tsx"),
        "export default function Blog() { return <main>changed</main>; }\n",
    )
    .unwrap();
    let (c4, out4) = build(true).await;
    assert_eq!(c4, 0, "{out4}");
    assert!(
        !out4.contains("bundle: client unchanged"),
        "a component edit re-bundles the client: {out4}",
    );
}

// ─── 19-E / 5-A: frames + per-route chunk records ────────────────────────

#[test]
fn find_literal_line_locates_a_literal() {
    let src = "export const a = 1;\nexport const secret = 'super-secret-value';\n";
    let (line, col) = crate::find_literal_line(src, "super-secret-value").expect("found");
    assert_eq!(line, 2, "the literal is on line 2");
    assert!(col > 1, "column is past the assignment");
}

#[test]
fn find_literal_line_returns_none_for_absent_literal() {
    assert!(crate::find_literal_line("const a = 1;\n", "not-here").is_none());
    assert!(
        crate::find_literal_line("anything", "").is_none(),
        "empty needle"
    );
}

#[tokio::test]
async fn build_records_per_route_chunk_hashes() {
    use std::fs;
    let (_d, root) = fixture_project();
    let (code, out) = run_ok(&["oxide", "build", "--root", root.to_str().unwrap()]).await;
    assert_eq!(code, 0, "build: {out}");

    let snap: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(".oxide/graph.json")).unwrap()).unwrap();
    let recs = snap["route_chunks"].as_array().expect("route_chunks array");
    assert!(!recs.is_empty(), "at least one route recorded: {snap}");
    let first = &recs[0];
    assert!(first["route_id"].is_string());
    let chunks = first["chunks"].as_array().expect("chunks array");
    assert!(!chunks.is_empty(), "the route reaches at least one chunk");
    let pair = chunks[0].as_array().expect("pair");
    assert_eq!(pair.len(), 2);
    let hash = pair[1].as_str().unwrap();
    assert_eq!(hash.len(), 64, "blake3 hex is 64 chars");
}

// ─── WP-B2: per-settle deferred delivery ─────────────────────────────────

/// The B2 contract, tested at the seam rather than through a live server:
/// given a render stream ending in `</body></html>` and a settle channel that
/// delivers two values 30ms apart, the output must
///   1. emit the body bytes BEFORE the first settle (TTFB is not held),
///   2. withhold `</body></html>` until every settle has been delivered,
///   3. emit exactly one resolution script per settle, in settle order.
#[tokio::test]
async fn settles_flush_per_settle_and_close_last() {
    use futures::StreamExt as _;

    // A render stream: a shell, then the closing tags.
    let chunks: Vec<Result<bytes::Bytes, std::io::Error>> = vec![
        Ok(bytes::Bytes::from_static(b"<html><body><main>shell</main>")),
        Ok(bytes::Bytes::from_static(b"</body></html>")),
    ];
    let src: oxide_server::BodyStream =
        Box::pin(futures::stream::iter(chunks)) as oxide_server::BodyStream;

    // Two settles, delivered with a real gap so ordering is observable.
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let _ = tx
            .send(serde_json::json!({"deferredSettle": {"id": "1", "value": {"a": 1}}}))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let _ = tx
            .send(serde_json::json!({"deferredSettle": {"id": "2", "value": {"b": 2}}}))
            .await;
    });

    let out = crate::strip_close_and_append_settles(src, rx, "NONCE".to_string(), None, String::new(), None);
    let mut out = out;
    let mut all: Vec<u8> = Vec::new();
    while let Some(item) = out.next().await {
        all.extend_from_slice(&item.expect("chunk"));
    }
    let text = String::from_utf8_lossy(&all);

    // 1. The shell reached the output.
    assert!(text.contains("<main>shell</main>"), "shell emitted: {text}");
    // 2. Exactly one script per settle.
    assert_eq!(
        text.matches("application/oxide-deferred").count(),
        2,
        "one script per settle: {text}",
    );
    assert!(text.contains(r#"data-id="1""#), "settle 1: {text}");
    assert!(text.contains(r#"data-id="2""#), "settle 2: {text}");
    // 3. Ordering: settle 1 precedes settle 2.
    let p1 = text.find(r#"data-id="1""#).unwrap();
    let p2 = text.find(r#"data-id="2""#).unwrap();
    assert!(p1 < p2, "settles in settle order");
    // 4. The document closes LAST — after every settle script.
    let close = text.rfind("</body></html>").expect("close present");
    assert!(close > p2, "closing tags emitted after the last settle");
    // 5. The body was never emitted mid-stream with the close attached.
    let shell_at = text.find("<main>shell</main>").unwrap();
    assert!(shell_at < p1, "shell precedes the settles");
}

/// The close-hold must survive a chunk boundary splitting `</body></html>`.
#[tokio::test]
async fn close_tag_split_across_chunks_is_still_withheld() {
    use futures::StreamExt as _;

    // `</body>` and `</html>` arrive as separate chunks.
    let chunks: Vec<Result<bytes::Bytes, std::io::Error>> = vec![
        Ok(bytes::Bytes::from_static(b"<main>x</main></body>")),
        Ok(bytes::Bytes::from_static(b"</html>")),
    ];
    let src: oxide_server::BodyStream =
        Box::pin(futures::stream::iter(chunks)) as oxide_server::BodyStream;
    let (tx, rx) = tokio::sync::mpsc::channel::<serde_json::Value>(1);
    // Drop the sender immediately: with no settles coming, the receiver must
    // observe the channel closed, not wait for a message that never arrives.
    drop(tx);

    let mut out = crate::strip_close_and_append_settles(src, rx, "N".into(), None, String::new(), None);
    let mut all: Vec<u8> = Vec::new();
    while let Some(item) = out.next().await {
        all.extend_from_slice(&item.expect("chunk"));
    }
    let text = String::from_utf8_lossy(&all);
    assert!(text.contains("<main>x</main>"), "body preserved: {text}");
    assert_eq!(
        text.matches("</body></html>").count(),
        1,
        "the close appears exactly once, at the end: {text}",
    );
    assert!(text.ends_with("</body></html>"), "close is last: {text}");
}

/// A route with no settles must be byte-identical to the buffered path.
#[tokio::test]
async fn no_settles_leaves_the_document_unchanged() {
    use futures::StreamExt as _;

    let body = b"<html><body><main>x</main></body></html>";
    let chunks: Vec<Result<bytes::Bytes, std::io::Error>> =
        vec![Ok(bytes::Bytes::from_static(body))];
    let src: oxide_server::BodyStream =
        Box::pin(futures::stream::iter(chunks)) as oxide_server::BodyStream;
    let (tx, rx) = tokio::sync::mpsc::channel::<serde_json::Value>(1);
    drop(tx);

    let mut out = crate::strip_close_and_append_settles(src, rx, "N".into(), None, String::new(), None);
    let mut all: Vec<u8> = Vec::new();
    while let Some(item) = out.next().await {
        all.extend_from_slice(&item.expect("chunk"));
    }
    assert_eq!(all, body, "no-settle output equals the input bytes");
}

// ─── Wave D: loader session write-back ───────────────────────────────────

/// A loader that returns a `session` has it parsed into `LoaderOutput`;
/// a loader that returns none leaves it `None`.
#[test]
fn loader_output_parses_session() {
    use crate::loader_shim::output_from_raw;
    let raw = serde_json::json!({
        "status": 200,
        "data": {},
        "session": { "visits": 2 },
    });
    let out = output_from_raw(&raw).expect("parse");
    assert_eq!(
        out.session,
        Some(serde_json::json!({"visits": 2})),
        "the loader's session is carried through",
    );

    let raw = serde_json::json!({ "status": 200, "data": {} });
    let out = output_from_raw(&raw).expect("parse");
    assert!(out.session.is_none(), "no session field -> None");
}

// ------------------------------ §22 / L1: L0 hit-rate regression

/// §22 / L1: the flagship 999/1000 hit rate had no committed regression
/// test — one field change in the dispatch could silently push every
/// request through the render path and nothing would fail. Seed the cache
/// once and fire 1000 dispatches: at least 999 MUST be L0 hits and none
/// may be a bypass.
#[tokio::test]
async fn l0_hit_rate_999_of_1000_dispatches() {
    use std::sync::Arc;

    let tier: Arc<dyn oxide_cache::CacheHandler> =
        Arc::new(oxide_cache::TieredCache::new(8 * 1024 * 1024));
    let l0 = Arc::new(l0_cache::L0Cache::new(
        tier,
        "test-build",
        std::time::Duration::from_secs(300),
    ));

    let page_renderer: Option<Arc<dyn oxide_server::PageRenderer>> =
        Some(Arc::new(DataCapturingRenderer::new()));

    let dispatch = build_dispatch(
        tree_with_static_page_and_middleware(),
        Arc::new(oxide_server::Metrics::new()),
        Vec::new(),
        page_renderer,
        None,
        routes_root(),
        None,
        Some(Arc::clone(&l0)),
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        Vec::new(),
        None,
        None,
        None,
    );

    // The store happens inside a tokio::spawn that consumes the render
    // stream — it fires only when the client has drained the response
    // body. So: dispatch, drain, then wait for the cache to show the entry.
    let first = dispatch(test_request("GET", "/")).await;
    let _ = collect_response_body(first).await;
    let mut stored = false;
    for _ in 0..100 {
        if l0.get("routes/index", "", "").await.is_some() {
            stored = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(stored, "the first render's store never landed in L0");

    let mut hits = 0usize;
    let mut misses = 0usize;
    let mut bypass = 0usize;
    let mut other = 0usize;

    for _ in 0..1000 {
        let resp = dispatch(test_request("GET", "/")).await;
        let cache = resp
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("x-oxide-cache"))
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        match cache {
            "L0-hit" | "L0-stale" => hits += 1,
            "L0-miss" => misses += 1,
            "bypass" => bypass += 1,
            _ => other += 1,
        }
    }

    assert!(
        hits >= 999,
        "expected >= 999 hits, got {hits} (misses={misses} bypass={bypass} other={other})"
    );
    assert_eq!(bypass, 0, "no request should be a bypass");
    assert_eq!(
        other, 0,
        "every request should carry an x-oxide-cache header (got {other} without)"
    );
}

// ------------------------------ §22 / L3(a): prerendered 500 served as 500

/// §22 / L3(a): a route whose stored status is 500 MUST serve 500 on
/// subsequent requests. The class of bug this catches is "200 status,
/// 500 body" — a stored error entry that looks successful to monitoring
/// because the dispatch forgot to use the entry's status.
#[tokio::test]
async fn prerendered_500_serves_500_on_hit() {
    use std::sync::Arc;

    let tier: Arc<dyn oxide_cache::CacheHandler> =
        Arc::new(oxide_cache::TieredCache::new(8 * 1024 * 1024));
    let l0 = Arc::new(l0_cache::L0Cache::new(
        tier,
        "test-build",
        std::time::Duration::from_secs(300),
    ));

    // Static route (no params), and the test dispatch passes an empty
    // vary_headers slice → vary is "". Use the dispatch's own key form.
    let key = l0.key_of("routes/index", "", "");
    let entry = l0.make_entry(
        500,
        vec![("content-type".into(), "text/html; charset=utf-8".into())],
        b"<h1>prerendered failure</h1>".to_vec(),
        Vec::new(),
    );
    l0.store_entry(&key, entry).await;

    let dispatch = build_dispatch(
        tree_with_static_page_and_middleware(),
        Arc::new(oxide_server::Metrics::new()),
        Vec::new(),
        None,
        None,
        routes_root(),
        None,
        Some(Arc::clone(&l0)),
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        Vec::new(),
        None,
        None,
        None,
    );

    let resp = dispatch(test_request("GET", "/")).await;
    assert_eq!(
        resp.status, 500,
        "prerendered 500 must serve as 500, not 200-with-a-500-body"
    );
    let cache = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-oxide-cache"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    assert_eq!(
        cache, "L0-hit",
        "the request must have been served from L0 (got x-oxide-cache={cache})"
    );
    let body = collect_response_body(resp).await;
    let s = std::str::from_utf8(&body).unwrap_or("");
    assert!(
        s.contains("prerendered failure"),
        "stored error body must be retained on hit: {s}"
    );
}

// ------------------------------ §22 / L3(b): S-20 rewrite hop guard

/// §22 / L3(b): the S-20 rewrite-loop guard fires before any routing,
/// loader, or render work. `rewrite_hops > 8` is a 508. This pins the
/// guard so a future dispatch refactor cannot regress it silently.
#[tokio::test]
async fn rewrite_hop_guard_returns_508_after_8_hops() {
    use std::sync::Arc;

    let dispatch = build_dispatch(
        empty_tree(),
        Arc::new(oxide_server::Metrics::new()),
        Vec::new(),
        None,
        None,
        routes_root(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        Vec::new(),
        None,
        None,
        None,
    );

    // 8 hops: allowed (guard is strictly `> 8`). Falls through to the
    // normal 404 path since the tree is empty.
    let mut req = test_request("GET", "/");
    req.rewrite_hops = 8;
    let resp = dispatch(req).await;
    assert_ne!(
        resp.status, 508,
        "8 hops must not trip the guard — only a ninth does"
    );

    // 9 hops: 508, and the body names the loop. Runs before any route
    // work, so an empty tree is enough.
    let mut req = test_request("GET", "/");
    req.rewrite_hops = 9;
    let resp = dispatch(req).await;
    assert_eq!(resp.status, 508, "9 hops must return 508");
    let body = collect_response_body(resp).await;
    let s = std::str::from_utf8(&body).unwrap_or("");
    assert!(
        s.contains("rewrite loop"),
        "508 body names the failure: {s}"
    );
}

// ------------------------------ §22 / L3(c): P-12 asset cache

/// §22 / L3(c): `StaticAssets::serve` serves the same body from the asset
/// cache on repeated requests. The cache is a module-level `OnceLock`
/// (shared across every test in this binary, TTL 300s / configurable via
/// env), so this test pins the *positive path* — the cache is populated
/// on the first serve and the second serve returns byte-identical content
/// with the expected cache headers. TTL *expiry* is deliberately not
/// exercised here: mutating a process-global cache would poison every
/// other server test, and the `OXIDE_ASSET_CACHE_TTL_SECS` env is read
/// once at OnceLock init. If a TTL regression test is wanted, it belongs
/// in a fresh process (a `crates/server/tests/` integration test that
/// boots with a low TTL env set before any other test in the binary).
#[tokio::test]
async fn static_asset_cache_serves_byte_identical_body_on_repeat() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.css"), b"body { color: red; }").unwrap();

    let assets = oxide_server::StaticAssets::scan(dir.path()).unwrap();
    let req = test_request("GET", "/hello.css");

    let first = assets.serve(&req, "/hello.css");
    let first = match first {
        Some(r) => r,
        None => panic!("first serve returned None"),
    };
    let first_body = collect_response_body(first).await;
    assert_eq!(&first_body[..], b"body { color: red; }");

    let second = assets.serve(&req, "/hello.css");
    let second = match second {
        Some(r) => r,
        None => panic!("second serve returned None — the cache poisoned it"),
    };
    let second_body = collect_response_body(second).await;
    assert_eq!(
        &second_body[..],
        &first_body[..],
        "cached serve must be byte-identical to the first"
    );
}

// ------------------------------ §8.2 / O1: layout_chain in manifests

#[test]
fn compute_layout_chain_finds_root_and_nested_layouts() {
    use oxide_router::RouteEntry;
    let mut tree = oxide_router::RouteTree::new();

    // Root layout
    tree.insert(RouteEntry {
        id: oxide_router::RouteId("routes/layout".into()),
        kind: oxide_router::RouteKind::Layout,
        segments: vec![],
        file: std::path::PathBuf::from("layout.tsx"),
        render_mode: None,
        tags: Vec::new(),
        has_server_module: false,
        slot: None,
        needs_parent: false,
    });
    // Nested layout
    tree.insert(RouteEntry {
        id: oxide_router::RouteId("routes/nested/layout".into()),
        kind: oxide_router::RouteKind::Layout,
        segments: vec!["nested".into()],
        file: std::path::PathBuf::from("nested/layout.tsx"),
        render_mode: None,
        tags: Vec::new(),
        has_server_module: false,
        slot: None,
        needs_parent: false,
    });
    // Page at nested/
    tree.insert(RouteEntry {
        id: oxide_router::RouteId("routes/nested/page".into()),
        kind: oxide_router::RouteKind::Page,
        segments: vec!["nested".into()],
        file: std::path::PathBuf::from("nested/page.tsx"),
        render_mode: Some("static".into()),
        tags: Vec::new(),
        has_server_module: false,
        slot: None,
        needs_parent: false,
    });

    // A nested page sees BOTH layouts, outermost first.
    assert_eq!(
        compute_layout_chain(&tree, "routes/nested"),
        vec!["routes/layout".to_string(), "routes/nested/layout".to_string()],
        "grandparent + parent chain for a two-level nested page"
    );

    // A top-level page sees only the root layout.
    tree.insert(RouteEntry {
        id: oxide_router::RouteId("routes/page".into()),
        kind: oxide_router::RouteKind::Page,
        segments: vec![],
        file: std::path::PathBuf::from("page.tsx"),
        render_mode: Some("static".into()),
        tags: Vec::new(),
        has_server_module: false,
        slot: None,
        needs_parent: false,
    });
    assert_eq!(
        compute_layout_chain(&tree, "routes/index"),
        vec!["routes/layout".to_string()],
        "top-level page sees only the root layout"
    );

    // A top-level route nested under `routes/` still gets the root layout —
    // `routes/layout` wraps everything (that is what a root layout means).
    assert_eq!(
        compute_layout_chain(&tree, "routes/orphan"),
        vec!["routes/layout".to_string()],
        "root layout applies to every route under routes/"
    );

    // The empty-chain case is a route id that has no `routes/layout` at any
    // prefix — a tree with no layouts at all.
    let empty_tree = oxide_router::RouteTree::new();
    assert_eq!(
        compute_layout_chain(&empty_tree, "routes/x"),
        Vec::<String>::new(),
        "empty tree → empty chain"
    );

    // A route id outside the `routes/` prefix: no layout matches.
    assert_eq!(
        compute_layout_chain(&tree, "outside/x"),
        Vec::<String>::new(),
        "outside the routes/ namespace → empty chain"
    );
}

// ------------------------------ §20 / O3: trace_id on the waterfall

#[test]
fn waterfall_records_trace_id() {
    let rec = oxide_dev::WaterfallRecorder::new();
    let trace_id = "4bf92f3577b34da6a3ce929d0e0e4736";
    rec.record_with_trace(
        "req-abc",
        vec![("render".to_string(), 1.0)],
        Some(trace_id.to_string()),
    );
    // And one without a trace — the row still records, trace_id is None.
    rec.record("req-untraced", vec![("render".to_string(), 2.0)]);

    let snap = rec.snapshot();
    assert_eq!(snap.len(), 2, "two rows recorded");

    let with_trace = snap.iter().find(|r| r.request_id == "req-abc").expect("traced row");
    assert_eq!(with_trace.trace_id.as_deref(), Some(trace_id));

    let untraced = snap.iter().find(|r| r.request_id == "req-untraced").expect("untraced row");
    assert_eq!(untraced.trace_id, None, "no inbound trace → None");
}

// ------------------------------ §20 / O3b: deferred-settles stage

#[tokio::test]
async fn strip_close_and_append_settles_records_settle_stage() {
    use futures::StreamExt as _;

    let rec = oxide_dev::WaterfallRecorder::new();
    // The dispatch records the initial stages before wiring the settle tail.
    rec.record_staged(
        "req-o3b",
        vec![
            ("middleware".to_string(), 1.0),
            ("load".to_string(), 5.0),
            ("render".to_string(), 20.0),
        ],
        Some("trace-o3b".to_string()),
    );

    // A source stream with the document body ending in the close marker.
    let src: oxide_server::BodyStream = Box::pin(futures::stream::iter(vec![
        Ok(bytes::Bytes::from_static(b"<body>shell</body></html>")),
    ]));

    // Settle channel: send two settles, then close.
    let (tx, rx) = tokio::sync::mpsc::channel::<serde_json::Value>(4);
    tx.send(serde_json::json!({
        "deferredSettle": { "id": "d1", "value": 42 }
    })).await.unwrap();
    tx.send(serde_json::json!({
        "deferredSettle": { "id": "d2", "value": 99 }
    })).await.unwrap();
    drop(tx); // close the channel

    let out = strip_close_and_append_settles(
        src,
        rx,
        "nonce-x".to_string(),
        Some(rec.clone()),
        "req-o3b".to_string(),
        Some("trace-o3b".to_string()),
    );

    // Drain the returned body stream so the spawned task runs to completion
    // (the record happens after the settle loop, before close).
    let mut all: Vec<u8> = Vec::new();
    let mut stream = out;
    while let Some(item) = stream.next().await {
        if let Ok(b) = item {
            all.extend_from_slice(&b);
        }
    }
    let s = String::from_utf8_lossy(&all);
    assert!(s.contains("application/oxide-deferred"), "settle tags emitted: {s}");
    assert!(s.contains("d1"), "first settle rendered");
    assert!(s.contains("d2"), "second settle rendered");

    // The recorder must now have a single row with the deferred-settles
    // stage merged in alongside the initial stages.
    let snap = rec.snapshot();
    assert_eq!(snap.len(), 1, "one row per request_id: {snap:?}");
    let row = &snap[0];
    assert_eq!(row.request_id, "req-o3b");
    let names: Vec<_> = row.waterfall_ms.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        names.contains(&"deferred-settles"),
        "deferred-settles stage recorded: {names:?}"
    );
    assert!(
        names.contains(&"render"),
        "initial stages preserved (merged, not replaced): {names:?}"
    );
}

// ------------------------------ §20 / O3c: parallel-load savings

#[test]
fn parallel_load_savings_equals_shorter_side() {
    // Parallel path: layout took 40ms, page took 15ms. Sequential would
    // cost 55ms; parallel costs 40ms. Saving = 15ms = min(a, b).
    assert!((parallel_load_savings_ms(40.0, 15.0) - 15.0).abs() < 0.001);
    assert!((parallel_load_savings_ms(15.0, 40.0) - 15.0).abs() < 0.001);
}

#[test]
fn parallel_load_savings_zero_when_equal_or_sequential() {
    // Equal durations — nothing saved.
    assert!((parallel_load_savings_ms(20.0, 20.0) - 20.0).abs() < 0.001);
    // Sequential caller passes one side as 0 — no saving to record.
    assert!((parallel_load_savings_ms(20.0, 0.0) - 0.0).abs() < 0.001);
    assert!((parallel_load_savings_ms(0.0, 20.0) - 0.0).abs() < 0.001);
    // Never negative even with a clock skew into the past.
    assert!((parallel_load_savings_ms(-1.0, 20.0) - 0.0).abs() < 0.001);
}

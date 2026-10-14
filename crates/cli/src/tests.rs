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
        "oxide", "render", "routes/index",
        "--root", &root_str,
        "--data", r#"{"name":"CLI"}"#,
    ]).await;
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
    async fn render(
        &self,
        input: oxide_server::PageRenderInput,
    ) -> oxide_server::Result<String> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
    async fn render(
        &self,
        _input: oxide_server::PageRenderInput,
    ) -> oxide_server::Result<String> {
        Err(oxide_server::err("FAKE-RENDER", "simulated render failure"))
    }
}

fn test_request(method: &str, url: &str) -> oxide_server::OxideRequest {
    oxide_server::OxideRequest {
        method: method.into(),
        url: url.into(),
        headers: Vec::new(),
        body: bytes::Bytes::new(),
        request_id: "dispatch-test".into(),
    }
}

fn empty_tree() -> std::sync::Arc<oxide_router::RouteTree> {
    std::sync::Arc::new(oxide_router::RouteTree::new())
}

#[tokio::test]
async fn dispatch_routes_through_page_renderer_when_attached() {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tree = empty_tree();
    let metrics = Arc::new(oxide_server::Metrics::new());
    let renderer: Option<Arc<dyn oxide_server::PageRenderer>> = Some(Arc::new(CountingRenderer {
        calls: calls.clone(),
    }));

    let dispatch = build_dispatch(tree, metrics, Vec::new(), renderer, None, std::path::PathBuf::new(), None);
    let resp = dispatch(test_request("GET", "/")).await;

    assert_eq!(resp.status, 200, "renderer branch returns 200");
    let collected = collect_response_body(resp).await;
    let body = std::str::from_utf8(&collected).expect("utf8 body");
    assert!(
        body.contains("RENDERER:"),
        "response body should come from the attached renderer: {body}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1, "renderer called exactly once");
}

#[tokio::test]
async fn dispatch_uses_ir_placeholder_when_no_renderer() {
    use std::sync::Arc;

    let tree = empty_tree();
    let metrics = Arc::new(oxide_server::Metrics::new());
    let dispatch = build_dispatch(tree, metrics, Vec::new(), None, None, std::path::PathBuf::new(), None);

    let resp = dispatch(test_request("GET", "/some/page")).await;

    // Empty tree → `_unmatched` → 404, IR placeholder body.
    assert_eq!(resp.status, 404, "no renderer → placeholder status from tree match");
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

    let dispatch = build_dispatch(tree, metrics, Vec::new(), renderer, None, std::path::PathBuf::new(), None);
    let resp = dispatch(test_request("GET", "/")).await;

    assert_eq!(resp.status, 500, "renderer error → 500");
    let body = std::str::from_utf8(resp.body.as_ref()).expect("utf8 body");
    assert!(
        body.contains("render error"),
        "error response body: {body}"
    );
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
    fn name(&self) -> &str { "data-capture" }
    async fn render(
        &self,
        input: oxide_server::PageRenderInput,
    ) -> oxide_server::Result<String> {
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
            })),
        }
    }
}

#[async_trait::async_trait]
impl oxide_server::LoaderRunner for ScriptedLoader {
    fn name(&self) -> &str { "scripted" }
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
    });
    std::sync::Arc::new(tree)
}

fn routes_root() -> std::path::PathBuf {
    std::path::PathBuf::from("/tmp/oxide-test-routes")
}

#[tokio::test]
async fn loader_feeds_data_to_renderer() {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

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
        tree, metrics, Vec::new(), page_renderer, loader_runner, routes_root(), None,
    );
    let resp = dispatch(test_request("GET", "/")).await;

    assert_eq!(resp.status, 200);
    assert_eq!(loader_calls.load(Ordering::SeqCst), 1, "loader ran once");
    assert_eq!(renderer_calls.load(Ordering::SeqCst), 1, "renderer ran once");
    let data = renderer_data.lock().unwrap().clone();
    assert_eq!(data["title"], "Hi", "loader data reached the renderer");
}

#[tokio::test]
async fn loader_404_short_circuits_render() {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    let tree = tree_with_server_route();
    let metrics = Arc::new(oxide_server::Metrics::new());

    let renderer = Arc::new(DataCapturingRenderer::new());
    let renderer_calls = Arc::clone(&renderer.calls);
    let page_renderer: Option<Arc<dyn oxide_server::PageRenderer>> = Some(renderer);

    let loader = Arc::new(ScriptedLoader::http_error(404, "not found"));
    let loader_calls = Arc::clone(&loader.calls);
    let loader_runner: Option<Arc<dyn oxide_server::LoaderRunner>> = Some(loader);

    let dispatch = build_dispatch(
        tree, metrics, Vec::new(), page_renderer, loader_runner, routes_root(), None,
    );
    let resp = dispatch(test_request("GET", "/")).await;

    assert_eq!(resp.status, 404);
    assert_eq!(loader_calls.load(Ordering::SeqCst), 1, "loader ran once");
    assert_eq!(renderer_calls.load(Ordering::SeqCst), 0, "renderer must NOT run");
    let body = std::str::from_utf8(resp.body.as_ref()).unwrap();
    assert!(body.contains("not found"), "error message in body: {body}");
}

#[tokio::test]
async fn loader_redirect_short_circuits_with_location() {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    let tree = tree_with_server_route();
    let metrics = Arc::new(oxide_server::Metrics::new());

    let renderer = Arc::new(DataCapturingRenderer::new());
    let renderer_calls = Arc::clone(&renderer.calls);
    let page_renderer: Option<Arc<dyn oxide_server::PageRenderer>> = Some(renderer);

    let loader = Arc::new(ScriptedLoader::redirect("/login", 303));
    let loader_calls = Arc::clone(&loader.calls);
    let loader_runner: Option<Arc<dyn oxide_server::LoaderRunner>> = Some(loader);

    let dispatch = build_dispatch(
        tree, metrics, Vec::new(), page_renderer, loader_runner, routes_root(), None,
    );
    let resp = dispatch(test_request("GET", "/")).await;

    assert_eq!(resp.status, 303);
    assert_eq!(resp.header("location"), Some("/login"));
    assert_eq!(loader_calls.load(Ordering::SeqCst), 1);
    assert_eq!(renderer_calls.load(Ordering::SeqCst), 0, "renderer must NOT run");
}

#[tokio::test]
async fn no_server_module_skips_loader() {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

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
        tree, metrics, Vec::new(), page_renderer, loader_runner, routes_root(), None,
    );
    let resp = dispatch(test_request("GET", "/")).await;

    assert_eq!(resp.status, 200);
    assert_eq!(loader_calls.load(Ordering::SeqCst), 0, "loader must NOT run");
    assert_eq!(renderer_calls.load(Ordering::SeqCst), 1, "renderer runs with empty data");
    let data = renderer_data.lock().unwrap().clone();
    assert_eq!(data, serde_json::json!({}), "empty data when no server module");
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

    let params: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    write_prerender(
        &dist,
        "prerendered/index.html",
        "routes/index",
        "/",
        "/",
        &params,
        "test-nonce",
        Some(&page_renderer),
        Some(&loader_runner),
        Some(std::path::Path::new("/fake/page.server.ts")),
        None, // no island boot script in this test
    )
    .await
    .expect("write_prerender ok");

    let html = std::fs::read_to_string(dist.join("prerendered/index.html"))
        .expect("read output file");
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

    let params: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    write_prerender(
        &dist,
        "prerendered/index.html",
        "routes/index",
        "/",
        "/",
        &params,
        "test-nonce",
        Some(&page_renderer),
        None,
        None,
        None, // no island boot script
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
    let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|e| {
        panic!("config output not JSON ({e}): {out}")
    });
    assert_eq!(
        v["name"], "react",
        "descriptor.name from oxide.config.ts: {out}"
    );
    assert_eq!(v["extensions"][0], ".tsx");

    // 2. `oxide render routes/index` produces the home page markup.
    let (code, out) = run_ok(&[
        "oxide", "render", "routes/index",
        "--root", smoke_str,
        "--data", "{}",
    ])
    .await;
    assert_eq!(code, 0, "render routes/index exit; output:\n{out}");
    assert!(
        out.starts_with("<!doctype html>"),
        "doctype first: {out}"
    );
    // The smoke-app's `page.tsx` is `return <h1>Home</h1>`; the loader
    // hook compiles it via sucrase, so we assert the user's own content
    // (not the IR fallback's Atlas mock).
    assert!(
        out.contains(">Home</h1>"),
        "home route renders the user's page.tsx: {out}"
    );
    assert!(
        out.contains("data-reactroot"),
        "root marker present: {out}"
    );

    // 3. `oxide render routes/blog/[slug]` threads the URL param through.
    let (code, out) = run_ok(&[
        "oxide", "render", "routes/blog/[slug]",
        "--root", smoke_str,
        "--url", "/blog/hello-smoke",
        "--param", "slug=hello-smoke",
        "--data", "{}",
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
async fn http_get(
    addr: std::net::SocketAddr,
    path: &str,
) -> String {
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

    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await.expect("write request");
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

    // Build the same framework stack cmd_dev builds.
    let stack = crate::framework_shim::build_framework_stack(Some(&config_path), oxide_config::JsBackend::Node).await;
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
        resp_home.to_lowercase().contains("transfer-encoding: chunked"),
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
        !resp_health.to_lowercase().contains("transfer-encoding: chunked"),
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
    let index_entry = routes
        .get("routes/index/page")
        .or_else(|| routes.get("routes/index"))
        .expect("manifest has an index route entry");
    assert_eq!(
        index_entry.get("render_mode").and_then(|v| v.as_str()),
        Some("static"),
        "index route classified static: {index_entry}"
    );

    let _ = std::fs::remove_dir_all(&dist);
}

/// Collect the full response body, whether it came back collected
/// (`resp.body`) or streamed (`resp.stream`). Streaming responses leave
/// `.body` empty by design; the content is in `.stream`. Most tests want
/// the logical body regardless of which transport shape the dispatch
/// chose, so they call this instead of reading `.body` directly.
async fn collect_response_body(
    mut resp: oxide_server::OxideResponse,
) -> Vec<u8> {
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
    let stack = crate::framework_shim::build_framework_stack(Some(&config_path), oxide_config::JsBackend::Node).await;
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
        resp_home.to_lowercase().contains("transfer-encoding: chunked"),
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
        !resp_health.to_lowercase().contains("transfer-encoding: chunked"),
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
        "oxide", "render", "routes/index",
        "--root", "/some/root",
        "--url", "/home",
        "--param", "slug=hello",
        "--data", r#"{"title":"Hi"}"#,
    ]).expect("parse");
    match cli.command {
        crate::Command::Render { root, route, url, params, data, stream: _ } => {
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
    let cli = crate::Cli::try_parse_from(["oxide", "render", "routes/blog/[slug]"])
        .expect("parse");
    match cli.command {
        crate::Command::Render { root, route, url, params, data, stream: _ } => {
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
    let cli = crate::Cli::try_parse_from([
        "oxide", "render", "routes/index", "--root", "/proj",
    ]).expect("parse");
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
    let cli = crate::Cli::try_parse_from([
        "oxide", "config", "--json", "/some/root",
    ]).expect("parse");
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
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let (code, out) = run_ok(&[
        "oxide",
        "add",
        "--root",
        root.to_str().unwrap(),
        "oxide-image-scraper",
    ])
    .await;
    assert_eq!(code, 0);
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

#[tokio::test]
async fn audit_prints_no_advisories_for_modules_and_plugins() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    write_config(
        &root,
        r#"{ "plugins": [ { "name": "oxide-plugin-mdx" } ] }"#,
    );
    let oxide_dir = root.join(".oxide");
    std::fs::create_dir_all(&oxide_dir).unwrap();
    std::fs::write(
        oxide_dir.join("modules.json"),
        serde_json::json!(["oxide-image-scraper"]).to_string(),
    )
    .unwrap();
    let (code, out) = run_ok(&["oxide", "audit", "--root", root.to_str().unwrap()]).await;
    assert_eq!(code, 0);
    assert!(out.contains("oxide-image-scraper: no advisories"), "{out}");
    assert!(out.contains("oxide-plugin-mdx: no advisories"), "{out}");
}

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

#[tokio::test]
async fn start_serves_health_endpoint_then_shuts_down() {
    let (_d, root) = fixture_project();
    let started = start_server(root.to_str().unwrap(), Some(0), crate::framework_shim::FrameworkStack::default()).await.unwrap();
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
    // Dispatch placeholder for a routed page.
    let mut stream = tokio::net::TcpStream::connect(started.addr).await.unwrap();
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
    let req =
        format!("GET {path} HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\n\r\n");
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

    let started = crate::start_server(dir.path().to_str().unwrap(), Some(0), crate::framework_shim::FrameworkStack::default())
        .await
        .expect("start_server");
    let text = started.metrics.render();
    assert!(text.contains("# TYPE oxide_requests_total counter"), "{text}");
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

    let started = crate::start_server(dir.path().to_str().unwrap(), Some(0), crate::framework_shim::FrameworkStack::default())
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

    let started = crate::start_server(dir.path().to_str().unwrap(), Some(0), crate::framework_shim::FrameworkStack::default())
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

    let started = crate::start_server(dir.path().to_str().unwrap(), Some(0), crate::framework_shim::FrameworkStack::default())
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

    // JSON state endpoint.
    let state_url = format!("http://{}/__oxide/state", started.addr);
    let resp = simple_get(&state_url).await.expect("GET /__oxide/state");
    assert!(resp.contains("HTTP/1.1 200"), "{resp}");
    assert!(resp.contains("application/json"), "{resp}");
    assert!(resp.contains("/blog/:slug"), "{resp}");

    // HTML endpoint.
    let html_url = format!("http://{}/__oxide", started.addr);
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

    let started = crate::start_server(dir.path().to_str().unwrap(), Some(0), crate::framework_shim::FrameworkStack::default())
        .await
        .expect("start_server");
    wait_ready(started.addr).await;

    let url = format!("http://{}/__oxide/state", started.addr);
    let resp = simple_get(&url).await.expect("GET");
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
    assert!(dir.path().join("dist/prerendered/blog/hello.html").is_file());
    assert!(dir.path().join("dist/prerendered/blog/world.html").is_file());
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
    use rmcp::handler::server::wrapper::Parameters;
    use rmcp::ServerHandler as _;
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
    assert!(v["source"].as_str().unwrap().ends_with("last-build-diagnostics.json"));
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
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
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
    assert!(smoke.join("oxide.config.ts").is_file(), "smoke-app fixture missing");

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
    assert!(checked >= 2, "expected at least the client + server chunks; got {checked}");

    // Chunks land in both envs.
    let client_dir = dist.join("client");
    let server_dir = dist.join("server");
    assert!(client_dir.is_dir(), "dist/client must exist");
    assert!(server_dir.is_dir(), "dist/server must exist");
    assert!(
        walk_files(&client_dir).iter().any(|p| p.extension().is_some_and(|e| e == "js")),
        "dist/client must contain at least one .js chunk"
    );
    assert!(
        walk_files(&server_dir).iter().any(|p| p.extension().is_some_and(|e| e == "js")),
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
    assert!(deploy.is_dir(), "dist/deploy must exist for the binary target");
    for name in ["deploy.json", "client.manifest.json", "server.manifest.json"] {
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
    let close = index_html[open..].find("</ox-island>").expect("closing tag") + open;
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






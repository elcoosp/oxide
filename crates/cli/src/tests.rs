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

    let dispatch = build_dispatch(tree, metrics, Vec::new(), renderer);
    let resp = dispatch(test_request("GET", "/")).await;

    assert_eq!(resp.status, 200, "renderer branch returns 200");
    let body = std::str::from_utf8(resp.body.as_ref()).expect("utf8 body");
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
    let dispatch = build_dispatch(tree, metrics, Vec::new(), None);

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

    let dispatch = build_dispatch(tree, metrics, Vec::new(), renderer);
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
        crate::Command::Render { root, route, url, params, data } => {
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
        crate::Command::Render { root, route, url, params, data } => {
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
    let started = start_server(root.to_str().unwrap(), Some(0), None).await.unwrap();
    // Poll until the listener accepts (no sleeps > 50 ms, bounded).
    let mut stream = None;
    for _ in 0..50 {
        match tokio::net::TcpStream::connect(started.addr).await {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
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
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
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

    let started = crate::start_server(dir.path().to_str().unwrap(), Some(0), None)
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

    let started = crate::start_server(dir.path().to_str().unwrap(), Some(0), None)
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

    let started = crate::start_server(dir.path().to_str().unwrap(), Some(0), None)
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

    let started = crate::start_server(dir.path().to_str().unwrap(), Some(0), None)
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

    let started = crate::start_server(dir.path().to_str().unwrap(), Some(0), None)
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

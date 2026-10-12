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
    let started = start_server(root.to_str().unwrap(), Some(0)).await.unwrap();
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

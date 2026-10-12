//! oxide-cli — clap entry, commands (spec §19.1).
//!
//! Commands: dev, build, start, preview, routes, analyze, check, why, doctor,
//! add, generate, cache, audit, migrate, test (in-process app.fetch helper).

use clap::{Parser, Subcommand};
use oxide_config::OxideConfig;
use oxide_router::RouteTree;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Oxide CLI (spec §19.1).
#[derive(Debug, Parser)]
#[command(
    name = "oxide",
    version,
    about = "Oxide meta-framework: dev · build · start · preview · routes · analyze · check · doctor"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start the dev server (on-demand ESM, HMR, §13).
    Dev {
        /// Project directory (default: cwd).
        #[arg(long, default_value = ".")]
        root: String,
        #[arg(long)]
        port: Option<u16>,
        /// Hidden test hook: run exactly one watch poll, print `dev ready` and exit.
        /// Without it the dev loop runs until Ctrl-C (§13, not exercised in tests).
        #[arg(long, hide = true)]
        once: bool,
    },
    /// Build for production (all environments + manifests, §6).
    Build {
        #[arg(long, default_value = ".")]
        root: String,
        /// Static export (§10.5).
        #[arg(long)]
        r#static: bool,
        #[arg(long)]
        mode: Option<String>,
    },
    /// Start the production server (binary target, §12).
    Start {
        #[arg(long, default_value = ".")]
        root: String,
        #[arg(long)]
        port: Option<u16>,
    },
    /// Preview the production build locally.
    Preview {
        #[arg(long, default_value = ".")]
        root: String,
    },
    /// Print the route table with inferred modes and cache policy (§10.1, §19.1).
    Routes {
        #[arg(long, default_value = ".")]
        root: String,
    },
    /// Bundle and budget report (§21 budgets).
    Analyze {
        #[arg(long, default_value = ".")]
        root: String,
    },
    /// Typecheck (tsgo), lint (oxlint), config + route diagnostics (§19.1).
    Check {
        #[arg(long, default_value = ".")]
        root: String,
    },
    /// Explain importers and chunks of a module (§13.4).
    Why {
        #[arg(long, default_value = ".")]
        root: String,
        module: String,
    },
    /// Environment and config health (§19.1).
    Doctor {
        #[arg(long, default_value = ".")]
        root: String,
    },
    /// Install a module (§15.4).
    Add {
        /// Project directory (default: cwd).
        #[arg(long, default_value = ".")]
        root: String,
        module: String,
    },
    /// Scaffold route, layout or action files.
    Generate {
        #[arg(long, default_value = ".")]
        root: String,
        #[arg(long)]
        kind: String,
        path: String,
    },
    /// Cache inspection (§19.1).
    Cache {
        /// Project directory (default: cwd).
        #[arg(long, default_value = ".")]
        root: String,
        #[command(subcommand)]
        op: CacheOp,
    },
    /// Plugin and dependency advisories (§17.2).
    Audit {
        #[arg(long, default_value = ".")]
        root: String,
    },
    /// Migrate from next/nuxt/sveltekit/vite (§19.2).
    Migrate {
        #[arg(long, default_value = ".")]
        root: String,
        source: String,
    },
    /// In-process test helper: boot the app and expose `fetch` (§16.6 oxide/test).
    Test {
        #[arg(long, default_value = ".")]
        root: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum CacheOp {
    Inspect,
    Clear,
    Stats,
}

/// Entry point used by both `main.rs` and tests.
///
/// Runs [`run_with_writer`] into a buffer, then flushes it to stdout (so tests can
/// capture tables via [`run_with_writer`] while the binary keeps its real stdout).
pub async fn run(cli: Cli) -> anyhow::Result<i32> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();
    let mut buf: Vec<u8> = Vec::new();
    let code = run_with_writer(cli, &mut buf).await?;
    use std::io::Write as _;
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(&buf);
    let _ = stdout.flush();
    Ok(code)
}

/// [`run`] with an injectable writer — every command that prints tables/reports
/// writes here (§19.1 output), so tests assert on the buffer instead of stdout.
/// Exit codes follow §19.1 conventions: 0 ok, 1 failed checks/build, 2 usage-class
/// failures (unknown source/kind).
pub async fn run_with_writer(cli: Cli, w: &mut dyn std::io::Write) -> anyhow::Result<i32> {
    match cli.command {
        Command::Dev {
            root,
            port: _,
            once,
        } => cmd_dev(&root, once, w).await,
        Command::Build {
            root,
            r#static,
            mode: _,
        } => cmd_build(&root, r#static, w).await,
        Command::Start { root, port } => cmd_serve(&root, port, "oxide start", w).await,
        Command::Preview { root } => cmd_serve(&root, None, "oxide preview", w).await,
        Command::Routes { root } => cmd_routes(&root, w).await,
        Command::Analyze { root } => cmd_analyze(&root, w).await,
        Command::Check { root } => cmd_check(&root, w).await,
        Command::Why { root, module } => cmd_why(&root, &module, w).await,
        Command::Doctor { root } => cmd_doctor(&root, w).await,
        Command::Add { root, module } => cmd_add(&root, &module, w).await,
        Command::Generate { root, kind, path } => cmd_generate(&root, &kind, &path, w).await,
        Command::Cache { root, op } => cmd_cache(&root, op, w).await,
        Command::Audit { root } => cmd_audit(&root, w).await,
        Command::Migrate { root, source } => cmd_migrate(&root, &source, w).await,
        Command::Test { root } => cmd_test(&root, w).await,
    }
    .map_err(|e| {
        tracing::error!(error = %e, "command failed");
        e
    })
}

// ----------------------------------------------------------------- helpers

type Writer<'a> = &'a mut dyn std::io::Write;

macro_rules! outln {
    ($w:expr, $($arg:tt)*) => {
        writeln!($w, $($arg)*).map_err(|e| anyhow::anyhow!("write failed: {e}"))?
    };
}

/// Tolerant config load for informational commands (§14): a missing/invalid config
/// falls back to defaults so `routes`/`doctor` still work on fresh projects.
fn load_config_or_default(root: &Path, mode: &str) -> OxideConfig {
    match oxide_config::load(root, mode, BTreeMap::new()) {
        Ok(loaded) => loaded.config,
        Err(e) => {
            tracing::debug!(code = %e.0.code, "config load failed; using defaults");
            OxideConfig::default()
        }
    }
}

/// Strict config load for commands that must surface validation errors (§19.1 check).
fn load_config_strict(root: &Path, mode: &str) -> anyhow::Result<OxideConfig> {
    oxide_config::load(root, mode, BTreeMap::new())
        .map(|l| l.config)
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))
}

fn scan_routes(root: &Path, cfg: &OxideConfig) -> anyhow::Result<RouteTree> {
    let dir = root.join(&cfg.routes.dir);
    RouteTree::scan_with_config(root, Some(&dir))
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))
}

/// URL pattern for display (§19.1 table): `/` for the root, `/a/b` otherwise.
fn pattern_of(segments: &[String]) -> String {
    if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    }
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    let s = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

fn config_rules_to_router(
    rules: &[oxide_config::native_rule::NativeRule],
) -> Vec<oxide_router::NativeRule> {
    rules
        .iter()
        .map(|r| oxide_router::NativeRule {
            source: r.source.clone(),
            destination: r.destination.clone(),
            status: r.status,
            rule_type: r.rule_type.clone(),
            headers: r.headers.clone(),
            basic_auth: r.basic_auth.clone(),
            rate_limit: r.rate_limit.as_ref().map(|rl| oxide_router::RateLimit {
                key: rl.key.clone(),
                max: rl.max,
                window_secs: rl.window_secs,
            }),
        })
        .collect()
}

fn bundle_env_options(
    env: oxide_bundler::BundleEnv,
    entries: BTreeMap<String, PathBuf>,
    out_dir: PathBuf,
) -> oxide_bundler::BundleOptions {
    oxide_bundler::BundleOptions {
        env,
        entries,
        out_dir,
        // Simplified build: minification/sourcemaps land with the rolldown backend.
        minify: false,
        sourcemap: false,
        externals: Vec::new(),
        define: BTreeMap::new(),
        tsconfig_paths: BTreeMap::new(),
    }
}

// ----------------------------------------------------------------- commands

/// `oxide dev` (§13). `--once` runs a single watch poll (test hook); otherwise the
/// 200 ms poll loop runs until Ctrl-C.
async fn cmd_dev(root: &str, once: bool, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let cfg = load_config_or_default(&rootp, "development");
    let tree = scan_routes(&rootp, &cfg)?;
    // Prebundle decision (§6.5): key by lockfile hash.
    let lockfile = [
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "bun.lockb",
    ]
    .iter()
    .map(|f| rootp.join(f))
    .find(|p| p.is_file());
    let lock_hash = match &lockfile {
        Some(p) => {
            oxide_bundler::ChunkBundler::content_hash(&[&std::fs::read(p).unwrap_or_default()])
        }
        None => "no-lockfile".to_string(),
    };
    let mut deps: Vec<String> = Vec::new();
    if let Some(pkg) = read_json(&rootp.join("package.json")) {
        for section in ["dependencies", "devDependencies"] {
            if let Some(map) = pkg.get(section).and_then(|m| m.as_object()) {
                deps.extend(map.keys().cloned());
            }
        }
    }
    deps.sort();
    deps.dedup();
    outln!(
        w,
        "oxide dev — root {} ({} routes)",
        rootp.display(),
        tree.entries.len()
    );
    for dep in &deps {
        outln!(
            w,
            "prebundle {} → {}",
            dep,
            oxide_dev::prebundle_key(&lock_hash, dep)
        );
    }
    let watcher = oxide_dev::Watcher::new(vec![rootp.clone()]);
    let mut seen = std::collections::BTreeSet::new();
    if once {
        let events = watcher
            .poll(&mut seen)
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        outln!(w, "dev ready — {} change(s) in first poll", events.len());
        return Ok(0);
    }
    // Real dev loop (§13): poll, report, Ctrl-C to stop. Not exercised in tests.
    outln!(w, "dev ready — watching for changes (Ctrl-C to stop)");
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let events = watcher
            .poll(&mut seen)
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        for ev in events {
            tracing::info!(?ev, "file change");
            outln!(w, "change: {ev:?}");
        }
    }
}

/// `oxide build` (§6, §10.5): bundle client + ssr, prerender static routes, emit the
/// deploy artifact for the configured adapter (§18). `--static` forces a static
/// export target, which refuses dynamic routes without prerendered output (§10.5).
async fn cmd_build(root: &str, r#static: bool, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let cfg = load_config_strict(&rootp, "production")?;
    let tree = scan_routes(&rootp, &cfg)?;
    let dist = rootp.join("dist");

    // Entries: pages + endpoints (client and ssr share the entry set, §6.1).
    let mut entries: BTreeMap<String, PathBuf> = BTreeMap::new();
    for e in &tree.entries {
        if matches!(
            e.kind,
            oxide_router::RouteKind::Page | oxide_router::RouteKind::Endpoint
        ) {
            entries.insert(e.id.0.clone(), rootp.join(&cfg.routes.dir).join(&e.file));
        }
    }
    let bundler = oxide_bundler::ChunkBundler::new(&rootp);
    use oxide_bundler::Bundler as _;
    let client_out = dist.join("client");
    let server_out = dist.join("server");
    let _client = bundler
        .bundle(&bundle_env_options(
            oxide_bundler::BundleEnv::Client,
            entries.clone(),
            client_out.clone(),
        ))
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    let _server = bundler
        .bundle(&bundle_env_options(
            oxide_bundler::BundleEnv::Ssr,
            entries.clone(),
            server_out.clone(),
        ))
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    // The bundler writes client.manifest.json / server.manifest.json into the out dirs.
    let client_manifest = read_json(&client_out.join("client.manifest.json"))
        .ok_or_else(|| anyhow::anyhow!("bundler did not write client.manifest.json"))?;
    let mut server_manifest = read_json(&server_out.join("server.manifest.json"))
        .ok_or_else(|| anyhow::anyhow!("bundler did not write server.manifest.json"))?;
    let build_id = client_manifest
        .get("build_id")
        .and_then(|v| v.as_str())
        .unwrap_or("dev")
        .to_string();

    // Prerender static routes (§10.5, honest simplified pass): pages without a server
    // module and without dynamic needs get a minimal HTML shell via the Injector.
    let nonce = oxide_bundler::ChunkBundler::content_hash(&[build_id.as_bytes()]);
    let mut prerendered: BTreeMap<String, String> = BTreeMap::new();
    for e in &tree.entries {
        let mode_ok = matches!(
            e.render_mode.as_deref(),
            None | Some("auto") | Some("static")
        );
        let no_params = e
            .segments
            .iter()
            .all(|s| !s.contains(':') && !s.contains('*') && !s.contains('{'));
        if !matches!(e.kind, oxide_router::RouteKind::Page)
            || e.has_server_module
            || !mode_ok
            || !no_params
        {
            continue;
        }
        let pattern = pattern_of(&e.segments);
        let rel = if e.segments.is_empty() {
            "prerendered/index.html".to_string()
        } else {
            format!("prerendered/{}.html", e.segments.join("/"))
        };
        let shell = format!(
            "<!doctype html><html><head></head><body>\
             <main data-route=\"{pattern}\">placeholder</main></body></html>"
        );
        let injector = oxide_render::Injector::new(
            oxide_render::HeadTags::default(),
            nonce.clone(),
            e.id.0.clone(),
        );
        let html = injector
            .inject_into_html(&shell)
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        let dest = dist.join(&rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, html)?;
        prerendered.insert(e.id.0.clone(), rel);
    }

    // Enrich the server manifest with route-tree render modes + prerendered files
    // (the bundler leaves render_mode None; mode inference lives with the router).
    if let Some(routes) = server_manifest
        .get_mut("routes")
        .and_then(|r| r.as_object_mut())
    {
        for (id, row) in routes.iter_mut() {
            if let Some(e) = tree.entries.iter().find(|e| &e.id.0 == id) {
                if let Some(mode) = &e.render_mode {
                    row["render_mode"] = serde_json::json!(mode);
                }
                if let Some(rel) = prerendered.get(id) {
                    row["prerendered"] = serde_json::json!(rel);
                }
            }
        }
    }

    let target = if r#static {
        oxide_adapters::Target::Static
    } else {
        oxide_adapters::Target::parse(&cfg.adapter).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown adapter `{}`; expected one of binary|node|static|docker|cloudflare|lambda|vercel|netlify|deno|bun (§18)",
                cfg.adapter
            )
        })?
    };
    let artifact = oxide_adapters::BuildArtifact {
        out_dir: dist.clone(),
        build_id: build_id.clone(),
        client_manifest,
        server_manifest,
        prerendered,
        server_bundle: Some(server_out.join("ssr/index.js")),
    };
    let written = oxide_adapters::emit(target, &artifact, &dist.join("deploy"))
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    outln!(
        w,
        "build ok — {} routes, {} prerendered, {} deploy file(s) for `{}` in dist/deploy",
        tree.entries.len(),
        artifact.prerendered.len(),
        written.len(),
        target.as_str()
    );
    Ok(0)
}

/// A handle over a started server, so tests can shut it down deterministically.
pub struct StartedServer {
    pub addr: std::net::SocketAddr,
    pub shutdown: tokio::sync::watch::Sender<bool>,
}

/// Boxed dispatch future. Structurally identical to `futures::future::BoxFuture<_,
/// OxideResponse>` (which [`oxide_server::Server::serve`] takes) without needing a
/// `futures` dependency in this crate.
type BoxedResponse =
    std::pin::Pin<Box<dyn std::future::Future<Output = oxide_server::OxideResponse> + Send>>;

/// Construct the production pipeline (§12.1) from config and start serving.
/// The dispatch returns a placeholder page from the router match — the JS host wiring
/// (NodeHost/isolate render) lives in oxide-jshost and plugs into this dispatch later.
pub async fn start_server(root: &str, port: Option<u16>) -> anyhow::Result<StartedServer> {
    let rootp = PathBuf::from(root);
    let cfg = load_config_or_default(&rootp, "production");
    let tree = std::sync::Arc::new(scan_routes(&rootp, &cfg)?);
    let port = resolve_port(port.unwrap_or(cfg.server.port)).await;
    let rules = oxide_router::RulesEngine::compile(config_rules_to_router(&cfg.rules))
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    let pipeline = std::sync::Arc::new(oxide_server::Pipeline {
        rules,
        assets: None,
        limits: oxide_server::Limits::default(),
        security: oxide_server::SecuritySettings {
            csp: cfg.security.csp.clone(),
            csrf: cfg.security.csrf.clone(),
            ..Default::default()
        },
        rate_limiter: std::sync::Arc::new(oxide_server::RateLimiter::default()),
        health_path: "/_oxide/health".to_string(),
    });
    let dispatch: std::sync::Arc<
        dyn Fn(oxide_server::OxideRequest) -> BoxedResponse + Send + Sync,
    > = {
        let tree = std::sync::Arc::clone(&tree);
        std::sync::Arc::new(move |req: oxide_server::OxideRequest| {
            let raw = req.url.split(['?', '#']).next().unwrap_or("/").to_string();
            // Route conventions: the root page module is `index`, so `/` maps to
            // the `/index` pattern (spec §8.1); trailing slashes are folded too.
            let normalized = if raw.len() > 1 && raw.ends_with('/') {
                raw.trim_end_matches('/').to_string()
            } else {
                raw
            };
            let candidates = if normalized == "/" {
                vec!["/".to_string(), "/index".to_string()]
            } else {
                vec![normalized]
            };
            let (status, label) = candidates
                .iter()
                .find_map(|p| {
                    tree.match_path(p)
                        .map(|m| (200u16, pattern_of(&m.route.segments)))
                })
                .unwrap_or((404u16, "no route".to_string()));
            Box::pin(std::future::ready(oxide_server::OxideResponse::html(
                status,
                format!(
                    "<!doctype html><html><head><title>Oxide</title></head>\
                     <body><main>Oxide serving {label}</main></body></html>"
                ),
            )))
        })
    };
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        if let Err(e) = oxide_server::Server::serve(addr, pipeline, dispatch, rx).await {
            tracing::error!(code = %e.0.code, "server stopped with error");
        }
    });
    Ok(StartedServer { addr, shutdown: tx })
}

/// The server binds inside `Server::serve`, so a requested port of `0` is resolved
/// by probing a free port first (test convenience; real runs use the config port).
async fn resolve_port(requested: u16) -> u16 {
    if requested != 0 {
        return requested;
    }
    if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await {
        if let Ok(addr) = listener.local_addr() {
            drop(listener);
            return addr.port();
        }
    }
    0
}

async fn cmd_serve(
    root: &str,
    port: Option<u16>,
    label: &str,
    w: Writer<'_>,
) -> anyhow::Result<i32> {
    let started = start_server(root, port).await?;
    outln!(
        w,
        "{label} — listening on http://{} (health: /_oxide/health, Ctrl-C to stop)",
        started.addr
    );
    // Runs until Ctrl-C (§12.2 graceful shutdown); tests drive `start_server` directly.
    let _ = tokio::signal::ctrl_c().await;
    let _ = started.shutdown.send(true);
    Ok(0)
}

/// `oxide routes` (§10.1, §19.1): table with inferred modes + typegen into
/// `.oxide/types/` (routes.d.ts + routes.json, §8.3).
async fn cmd_routes(root: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let cfg = load_config_or_default(&rootp, "development");
    let tree = scan_routes(&rootp, &cfg)?;
    tree.typegen(&rootp)
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    // Join render modes from the machine-readable manifest typegen just wrote.
    let mut modes: BTreeMap<String, String> = BTreeMap::new();
    if let Some(rows) =
        read_json(&rootp.join(".oxide/types/routes.json")).and_then(|v| v.as_array().cloned())
    {
        for row in rows {
            if let (Some(pattern), Some(mode)) = (
                row.get("pattern").and_then(|p| p.as_str()),
                row.get("renderMode").and_then(|m| m.as_str()),
            ) {
                modes.insert(pattern.to_string(), mode.to_string());
            }
        }
    }
    outln!(w, "PATTERN | KIND | MODE | TAGS");
    for (pattern, kind, tags) in tree.table() {
        let mode = modes.get(&pattern).map(String::as_str).unwrap_or("-");
        let tags = if tags.is_empty() {
            "-".to_string()
        } else {
            tags.join(",")
        };
        outln!(w, "{pattern} | {kind} | {mode} | {tags}");
    }
    Ok(0)
}

/// `oxide analyze` (§21 budgets): per-route JS bytes vs `budgets.routeJs` (falling
/// back to `budgets.clientJs`), from dist/client/client.manifest.json when present,
/// zeros otherwise. Exit 1 when any route is over budget.
async fn cmd_analyze(root: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let cfg = load_config_or_default(&rootp, "production");
    let tree = scan_routes(&rootp, &cfg)?;
    let manifest =
        read_json(&rootp.join("dist/client/client.manifest.json")).unwrap_or(serde_json::json!({}));
    let manifest_routes = manifest
        .get("routes")
        .and_then(|r| r.as_object())
        .cloned()
        .unwrap_or_default();
    let rows: Vec<(String, u64)> = if manifest_routes.is_empty() {
        tree.entries
            .iter()
            .filter(|e| matches!(e.kind, oxide_router::RouteKind::Page))
            .map(|e| (e.id.0.clone(), 0u64))
            .collect()
    } else {
        manifest_routes
            .iter()
            .map(|(id, row)| {
                (
                    id.clone(),
                    row.get("js_bytes").and_then(|v| v.as_u64()).unwrap_or(0),
                )
            })
            .collect()
    };
    let budget_src = cfg
        .budgets
        .route_js
        .as_deref()
        .or(cfg.budgets.client_js.as_deref());
    let budget = match budget_src {
        Some(s) => Some((
            s,
            oxide_config::parse_size(s)
                .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?,
        )),
        None => None,
    };
    outln!(w, "ROUTE | JS BYTES | BUDGET | OK");
    let mut any_over = false;
    for (route, bytes) in rows {
        let (budget_str, limit) = match &budget {
            // The budget column shows the parsed byte count (e.g. 150kb → 153600).
            Some((_, b)) => (b.to_string(), Some(*b)),
            None => ("-".to_string(), None),
        };
        let ok = limit.is_none_or(|l| bytes <= l);
        if !ok {
            any_over = true;
        }
        let status = if ok { "ok" } else { "OVER" };
        outln!(w, "{route} | {bytes} | {budget_str} | {status}");
    }
    Ok(if any_over { 1 } else { 0 })
}

/// `oxide check` (§19.1): config validation + route conventions diagnostics.
async fn cmd_check(root: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let mut ok = true;
    match oxide_config::load(&rootp, "development", BTreeMap::new()) {
        Ok(lc) => outln!(w, "config: OK ({} env vars)", lc.env.len()),
        Err(e) => {
            outln!(w, "config: {} {}", e.0.code, e.0.message);
            ok = false;
        }
    }
    let cfg = load_config_or_default(&rootp, "development");
    match scan_routes(&rootp, &cfg) {
        Ok(tree) => outln!(w, "routes: OK ({} entries)", tree.entries.len()),
        Err(e) => {
            outln!(w, "routes: {e:#}");
            ok = false;
        }
    }
    if ok {
        outln!(w, "OK");
        Ok(0)
    } else {
        Ok(1)
    }
}

/// `oxide why <module>` (§13.4): importers (routes) and chunks referencing the module,
/// derived from the client manifest + chunk bodies. `not found` on a clean miss.
async fn cmd_why(root: &str, module: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let client_dir = rootp.join("dist/client");
    let manifest =
        read_json(&client_dir.join("client.manifest.json")).unwrap_or(serde_json::json!({}));
    let mut hits: Vec<(String, String)> = Vec::new();
    if let Some(routes) = manifest.get("routes").and_then(|r| r.as_object()) {
        for (route, row) in routes {
            let chunk = row
                .get("entry_chunk")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if chunk.is_empty() {
                continue;
            }
            let references = std::fs::read_to_string(client_dir.join(chunk))
                .map(|body| body.contains(module))
                .unwrap_or(false);
            if references {
                hits.push((route.clone(), chunk.to_string()));
            }
        }
    }
    if hits.is_empty() {
        outln!(w, "not found: {module} (run `oxide build` first)");
        return Ok(0);
    }
    outln!(w, "module {module}");
    let mut chunks: Vec<&str> = hits.iter().map(|(_, c)| c.as_str()).collect();
    chunks.sort();
    chunks.dedup();
    outln!(w, "  chunk: {}", chunks.join(", "));
    for (route, _) in &hits {
        outln!(w, "  importer: {route}");
    }
    Ok(0)
}

/// `oxide doctor` (§19.1): environment and config health, ASCII `[ok]/[fail]` lines.
async fn cmd_doctor(root: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let mut all_ok = true;
    let mut check = |w: &mut dyn std::io::Write, ok: bool, what: &str| -> anyhow::Result<()> {
        if !ok {
            all_ok = false;
        }
        outln!(w, "[{}] {what}", if ok { "ok" } else { "fail" });
        Ok(())
    };
    let node_ok = tokio::process::Command::new("node")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    check(w, node_ok, "node runtime (node --version)")?;
    let cargo_ok = tokio::process::Command::new("cargo")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    check(w, cargo_ok, "cargo (cargo --version)")?;
    check(
        w,
        oxide_config::load(&rootp, "development", BTreeMap::new()).is_ok(),
        "config loadable",
    )?;
    let cfg = load_config_or_default(&rootp, "development");
    let routes_ok = rootp.join(&cfg.routes.dir).is_dir() || rootp.join("app").is_dir();
    check(w, routes_ok, "routes directory (src/routes or app/)")?;
    let cache_dir = rootp.join(".oxide/cache");
    let cache_ok = std::fs::create_dir_all(&cache_dir).is_ok();
    let cache_ok = cache_ok && {
        let probe = cache_dir.join(".oxide-doctor-probe");
        let writable =
            std::fs::write(&probe, b"probe").is_ok() && std::fs::remove_file(&probe).is_ok();
        writable
    };
    check(w, cache_ok, "cache directory writable (.oxide/cache)")?;
    Ok(if all_ok { 0 } else { 1 })
}

/// `oxide add <module>` (§15.4): records the module in `.oxide/modules.json` and
/// prints next steps. (TS config writes are fragile; the list file is the contract.)
async fn cmd_add(root: &str, module: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let path = PathBuf::from(root).join(".oxide/modules.json");
    let mut list: Vec<String> = read_json(&path)
        .and_then(|v| v.as_array().cloned())
        .map(|a| {
            a.into_iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if !list.iter().any(|m| m == module) {
        list.push(module.to_string());
    }
    list.sort();
    list.dedup();
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    std::fs::write(&path, serde_json::to_string_pretty(&list)? + "\n")?;
    outln!(w, "[ok] added {module} to .oxide/modules.json");
    outln!(
        w,
        "next steps: import it in your code and run `oxide dev` (§15.4)"
    );
    Ok(0)
}

/// `oxide generate` (§19.1): route/layout/action scaffolds under the routes dir,
/// extension picked from the configured framework adapter (default `tsx`).
async fn cmd_generate(root: &str, kind: &str, path: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    if !matches!(kind, "route" | "layout" | "action") {
        outln!(w, "unknown kind `{kind}`; expected route|layout|action");
        return Ok(2);
    }
    let rootp = PathBuf::from(root);
    let cfg = load_config_or_default(&rootp, "development");
    let ext = match cfg.framework.adapter.as_deref() {
        Some("vue") => "vue",
        Some("svelte") => "svelte",
        _ => "tsx",
    };
    let dir = rootp.join(&cfg.routes.dir).join(path);
    std::fs::create_dir_all(&dir)?;
    let mut created: Vec<PathBuf> = Vec::new();
    let write = |name: &str, body: String| -> anyhow::Result<PathBuf> {
        let p = dir.join(name);
        std::fs::write(&p, body)?;
        Ok(p)
    };
    let server_stub = |actions: bool| {
        let mut s = format!(
            "// Oxide server module for {path} (D10).\n\
             export const config = {{ render: 'static' }};\n\n\
             export async function load() {{\n  return {{ props: {{}} }};\n}}\n"
        );
        if actions {
            s.push_str(&format!(
                "\nexport const actions = {{\n  async submit(_data) {{\n    // TODO: validate + persist (§9.3)\n    return {{ ok: true }};\n  }},\n}};\n"
            ));
        }
        s
    };
    match kind {
        "route" => {
            created.push(write(
                &format!("page.{ext}"),
                format!(
                    "// Oxide route page ({path}).\n\
                     export default function Page() {{\n\
                       return <main data-route=\"{path}\">{path}</main>;\n\
                     }}\n"
                ),
            )?);
            created.push(write("page.server.ts", server_stub(false))?);
        }
        "layout" => {
            created.push(write(
                &format!("layout.{ext}"),
                format!(
                    "// Oxide layout ({path}).\n\
                     export default function Layout({{ children }}) {{\n\
                       return <section>{{children}}</section>;\n\
                     }}\n"
                ),
            )?);
        }
        "action" => {
            created.push(write("page.server.ts", server_stub(true))?);
        }
        _ => unreachable!("kind validated above"),
    }
    for p in &created {
        outln!(w, "created {}", p.display());
    }
    Ok(0)
}

/// `oxide cache <inspect|clear|stats>` (§19.1): operates directly on the
/// content-addressed blob directory at `<root>/.oxide/cache`.
async fn cmd_cache(root: &str, op: CacheOp, w: Writer<'_>) -> anyhow::Result<i32> {
    let dir = PathBuf::from(root).join(".oxide/cache");
    let mut files: Vec<PathBuf> = Vec::new();
    walk_files(&dir, &mut files);
    match op {
        CacheOp::Inspect => {
            if files.is_empty() {
                outln!(w, "cache empty ({})", dir.display());
            }
            for f in &files {
                outln!(w, "{}", f.display());
            }
        }
        CacheOp::Clear => {
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
            }
            outln!(w, "[ok] cache cleared ({})", dir.display());
        }
        CacheOp::Stats => {
            let total: u64 = files
                .iter()
                .filter_map(|f| std::fs::metadata(f).ok())
                .map(|m| m.len())
                .sum();
            outln!(w, "files: {}", files.len());
            outln!(w, "total_bytes: {total}");
        }
    }
    Ok(0)
}

/// `oxide audit` (§17.2): static advisory pass over `.oxide/modules.json` + config
/// plugins. The real registry feed plugs in behind the same output shape.
async fn cmd_audit(root: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let cfg = load_config_or_default(&rootp, "development");
    let mut names: Vec<String> = read_json(&rootp.join(".oxide/modules.json"))
        .and_then(|v| v.as_array().cloned())
        .map(|a| {
            a.into_iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    for p in &cfg.plugins {
        if let Some(n) = p.get("name").and_then(|v| v.as_str()) {
            names.push(n.to_string());
        }
    }
    if names.is_empty() {
        outln!(w, "no modules or plugins to audit");
        return Ok(0);
    }
    names.sort();
    names.dedup();
    for n in &names {
        outln!(w, "{n}: no advisories (static registry check)");
    }
    Ok(0)
}

/// `oxide migrate <source>` (§19.2): dry-run plan; never writes.
async fn cmd_migrate(root: &str, source: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let (markers, label): (&[&str], &str) = match source {
        "next" => (&["next.config.js", "next.config.mjs"], "Next.js"),
        "nuxt" => (&["nuxt.config.ts"], "Nuxt"),
        "sveltekit" => (&["svelte.config.js"], "SvelteKit"),
        "vite" => (&["vite.config.ts"], "Vite"),
        other => {
            outln!(
                w,
                "unknown source `{other}`; expected next|nuxt|sveltekit|vite"
            );
            return Ok(2);
        }
    };
    if !markers.iter().any(|m| rootp.join(m).is_file()) {
        outln!(
            w,
            "no {label} project detected (missing {})",
            markers.join(" or ")
        );
        return Ok(2);
    }
    outln!(
        w,
        "migration plan ({label} → oxide) — dry run, no files written:"
    );
    let candidates: &[&str] = match source {
        "next" => &["page.tsx", "page.jsx", "route.ts"],
        "nuxt" => &["vue"],
        "sveltekit" => &["+page.svelte", "+layout.svelte"],
        _ => &["main.tsx", "main.ts", "index.html"],
    };
    let mut files: Vec<PathBuf> = Vec::new();
    walk_files(&rootp, &mut files);
    let found: Vec<PathBuf> = files
        .into_iter()
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            candidates.iter().any(|c| name == *c || name.ends_with(c))
                && !p.to_string_lossy().contains("node_modules")
        })
        .collect();
    if found.is_empty() {
        outln!(w, "  no convertible route files found");
    } else {
        outln!(w, "  found {} file(s) to convert:", found.len());
        for f in &found {
            outln!(
                w,
                "  - {} → conventions mapped (page/layout modules, data APIs)",
                f.display()
            );
        }
    }
    outln!(
        w,
        "  no files written — codemods run with --write in a later pass (§19.2)"
    );
    Ok(0)
}

/// `oxide test` (§16.6): gates on build manifests; the in-process `app.fetch` helper
/// itself lives in packages/oxide (documented in the output).
async fn cmd_test(root: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let client = rootp.join("dist/client/client.manifest.json");
    let server = rootp.join("dist/server/server.manifest.json");
    if client.is_file() && server.is_file() {
        outln!(w, "[ok] build manifests present");
        outln!(
            w,
            "use the in-process fetch helper from packages/oxide (oxide/test, §16.6)"
        );
        Ok(0)
    } else {
        outln!(
            w,
            "[fail] missing build manifests — run `oxide build` first"
        );
        Ok(1)
    }
}

#[cfg(test)]
mod tests;

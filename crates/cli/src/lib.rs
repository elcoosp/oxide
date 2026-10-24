//! oxide-fw-cli — clap entry, commands (spec §19.1).
//!
//! Commands: dev, build, start, preview, routes, analyze, check, why, doctor,
//! add, generate, cache, audit, migrate, test (in-process app.fetch helper).

use clap::{Parser, Subcommand};

/// The wasm-bindgen-processed `oxide-core-wasm` artifact, embedded at
/// CLI build time so a user project without a cargo workspace containing
/// `oxide-core-wasm` still gets the accelerated edge path.
/// Regenerated via `scripts/regen-edge-wasm.sh` (WP-C2, deviation 6).
static EDGE_WASM_BYTES: &[u8] = include_bytes!("../resources/edge.wasm");
use oxide_config::OxideConfig;
use oxide_router::RouteTree;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod audit;
pub mod mcp;
mod native_plugins;

mod build_cache;
mod build_graph;
mod framework_shim;
mod l0_cache;
pub mod loader_shim;
mod migrate;
mod modules_loader;
pub mod precompile;
mod precompile_cache;
/// `oxide secrets <op>` (§9.4 / BUG-05).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum SecretsOp {
    /// Delete `.oxide/secret`; the next build generates a new one.
    Rotate,
}

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
        /// Bind all interfaces + print the LAN URL (§13.5).
        #[arg(long)]
        host: bool,
        /// Serve over HTTPS with a self-signed cert (§13.5).
        #[arg(long)]
        https: bool,
        /// Print the network URL as a QR code (§13.5).
        #[arg(long)]
        qr: bool,
        /// Open the browser on start (§13.5).
        #[arg(long)]
        open: bool,
        /// Reverse-proxy rule, repeatable: `PREFIX=TARGET` (e.g.
        /// `--proxy /api=http://localhost:4000`). Longest prefix wins (§13.5).
        /// Add a leading `!` to strip the prefix: `--proxy '!/api=http://x'`.
        #[arg(long)]
        proxy: Vec<String>,
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
        /// §17.2: deny outbound network from plugin/tooling workers.
        #[arg(long)]
        offline_plugins: bool,
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
    /// Manage the per-project build secret (§9.4 / BUG-05): `rotate` deletes
    /// `.oxide/secret` so the next build mints a fresh one (server-fn ids
    /// change — every cached client bundle referencing an old id is stale).
    Secrets {
        #[arg(long, default_value = ".")]
        root: String,
        #[arg(value_enum)]
        op: SecretsOp,
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
        /// Emit the advisory report as JSON (a stable, machine-parseable
        /// schema: root, package_count, advisories, fatal, exit_code).
        /// Exit code is unchanged — a caller checking CI still gets 1 on
        /// HIGH/CRITICAL.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Migrate from next/nuxt/sveltekit/vite (§19.2).
    Migrate {
        #[arg(long, default_value = ".")]
        root: String,
        source: String,
        /// Actually write the converted files. Default is dry-run (plan only).
        #[arg(long, default_value_t = false)]
        write: bool,
    },
    /// In-process test helper: boot the app and expose `fetch` (§16.6 oxide/test).
    Test {
        #[arg(long, default_value = ".")]
        root: String,
    },
    /// MCP server over stdio (§19.3): exposes routes, cache, graph and
    /// diagnostics as MCP tools so coding agents see accurate project facts.
    Mcp {
        #[arg(long, default_value = ".")]
        root: String,
    },
    /// Print the JSON Schema for `oxide.config.{ts,json}` to stdout (§19.3).
    /// Editors (VS Code, JetBrains) consume this via a `$schema` field.
    Schema,
    /// Load oxide.config.{ts,mjs,js}, evaluate it in a Node worker, and print
    /// the framework adapter descriptor (spec §7). This is the load path the
    /// build/dev pipeline uses; today it proves end-to-end wiring of the
    /// §6.3 / §7 FrameworkHost.
    Config {
        /// Project root that contains oxide.config.ts (default: cwd).
        #[arg(default_value = ".")]
        root: String,
        /// Emit the descriptor as JSON (machine-readable for CI / tooling).
        #[arg(long)]
        json: bool,
    },
    /// Render a route through the framework adapter and print the HTML
    /// (spec §7). Loads oxide.config.{ts,mjs,js}, dispatches a render call
    /// to the FrameworkAdapter the config exports, and writes the resulting
    /// HTML to stdout. This is the read-path proof for §6.3 / §7.
    Render {
        /// Route id, e.g. `routes/index` or `routes/blog/[slug]`.
        route: String,
        /// Project root containing oxide.config.ts (default: cwd).
        #[arg(long, default_value = ".")]
        root: String,
        /// URL the request would have (default: derived from route).
        #[arg(long)]
        url: Option<String>,
        /// URL params as `k=v` pairs (repeatable). Arrays via `k=v1,k=v2`.
        #[arg(long = "param", value_name = "KEY=VALUE")]
        params: Vec<String>,
        /// Loader data as JSON (default: `{}`).
        #[arg(long, default_value = "{}")]
        data: String,
        /// Stream the render to stdout as chunks arrive, instead of
        /// collecting first. Useful for large pages and for verifying that
        /// a framework adapter's streaming path works. Output bytes are
        /// identical to the non-streaming mode.
        #[arg(long)]
        stream: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum CacheOp {
    Inspect,
    Clear,
    Stats,
    /// SPEC-5D: GC idle + over-cap entries from `.oxide/cache/`. Defaults:
    /// idle > 7 days, cap = 1 GiB. `--idle-secs` / `--max-bytes` override.
    Gc {
        #[arg(long, default_value_t = 7 * 24 * 3600)]
        idle_secs: u64,
        #[arg(long, default_value_t = 1024 * 1024 * 1024)]
        max_bytes: u64,
    },
}

/// Entry point used by both `main.rs` and tests.
///
/// Runs [`run_with_writer`] into a buffer, then flushes it to stdout (so tests can
/// capture tables via [`run_with_writer`] while the binary keeps its real stdout).

/// Install the tracing subscriber (§20). Console logging is always on;
/// OpenTelemetry OTLP export is opt-in via `OTEL_EXPORTER_OTLP_ENDPOINT`.
/// When the env var is unset, no OTLP layer is installed and behaviour is
/// unchanged from before.
///
/// The OTLP exporter uses the SDK's default batch processor (spans are
/// shipped asynchronously). The provider is stored in a process-wide
/// `OnceLock` so it lives for the process duration — dropping it would
/// silently close the exporter.
fn init_tracing() {
    use tracing_subscriber::layer::SubscriberExt;

    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
    // SPEC-24 / §20: `OXIDE_LOG=json` selects structured JSON log lines; the
    // default is human-readable. Each formatter is a distinct concrete layer
    // type, so the whole subscriber is built per branch (boxed as a
    // `Subscriber`).
    let json = std::env::var("OXIDE_LOG")
        .map(|v| v.eq_ignore_ascii_case("json"))
        .unwrap_or(false);

    macro_rules! build {
        ($otel:expr) => {{
            let base = tracing_subscriber::registry()
                .with($otel)
                .with(env_filter());
            if json {
                Box::new(
                    base.with(
                        tracing_subscriber::fmt::layer()
                            .with_target(false)
                            .json()
                            .flatten_event(true),
                    ),
                ) as Box<dyn tracing::Subscriber + Send + Sync>
            } else {
                Box::new(base.with(tracing_subscriber::fmt::layer().with_target(false)))
                    as Box<dyn tracing::Subscriber + Send + Sync>
            }
        }};
    }

    let subscriber: Box<dyn tracing::Subscriber + Send + Sync> = match endpoint {
        Some(ep) => match build_otel_layer(&ep) {
            Ok(otel_layer) => build!(otel_layer),
            Err(e) => {
                eprintln!(
                    "oxide: OTLP export disabled ({e}); continuing with console logging only"
                );
                build!(tracing_subscriber::layer::Identity::new())
            }
        },
        None => build!(tracing_subscriber::layer::Identity::new()),
    };

    let _ = tracing::subscriber::set_global_default(subscriber);
}

fn env_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
}

/// SPEC-24 / §20: redact secret-bearing fields before they reach a log line.
/// A field key matching `cookie`/`authorization`/`set-cookie`/dev-token, or
/// containing `secret`/`token`/`password`/`api_key`, is replaced with
/// `[redacted]`. (Applied at the request-log call sites — the formatter emits
/// whatever it is given, so the redaction happens before the event.)
fn is_secret_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    matches!(
        k.as_str(),
        "cookie" | "authorization" | "set-cookie" | "x-oxide-dev-token"
    ) || k.contains("secret")
        || k.contains("token")
        || k.contains("password")
        || k.contains("api_key")
}

/// SPEC-24 / §20: emit one structured request log line (JSON under
/// `OXIDE_LOG=json`) with every secret-bearing header redacted. Called from
/// the CLI's response finisher.
fn log_request(
    request_id: &str,
    method: &str,
    path: &str,
    status: u16,
    headers: &[(String, String)],
) {
    let mut fields = String::new();
    for (k, v) in headers {
        let val = if is_secret_key(k) {
            "[redacted]"
        } else {
            v.as_str()
        };
        fields.push_str(&format!("{k}={val} "));
    }
    tracing::info!(
        request_id = request_id,
        method = method,
        path = path,
        status = status,
        headers = fields.trim_end(),
        "request"
    );
}

/// Build the `tracing-opentelemetry` layer for the given OTLP endpoint.
/// The returned layer is backed by a batch span processor whose lifetime is
/// process-wide via a leaked `TracerProvider` — acceptable for a CLI that
/// exits with the process.
fn build_otel_layer(
    endpoint: &str,
) -> Result<
    tracing_opentelemetry::OpenTelemetryLayer<
        tracing_subscriber::Registry,
        opentelemetry_sdk::trace::Tracer,
    >,
    String,
> {
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_otlp::WithExportConfig;

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .map_err(|e| format!("OTLP exporter build failed: {e}"))?;
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .build();
    // Leak the provider so its batch exporter lives for the process lifetime.
    let provider: &'static opentelemetry_sdk::trace::SdkTracerProvider =
        Box::leak(Box::new(provider));
    let tracer = provider.tracer("oxide");
    Ok(tracing_opentelemetry::layer().with_tracer(tracer))
}

pub async fn run(cli: Cli) -> anyhow::Result<i32> {
    init_tracing();
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
            port,
            host,
            https,
            qr,
            open,
            proxy,
            once,
        } => {
            cmd_dev(
                &root,
                once,
                crate::DevFlags {
                    host,
                    https,
                    qr,
                    open,
                    port,
                    proxy,
                },
                w,
            )
            .await
        }
        Command::Build {
            root,
            r#static,
            mode: _,
            offline_plugins,
        } => {
            if offline_plugins {
                // Read by `PoolConfig::default` for every worker pool the
                // build spawns. Process-global is correct here: the build is
                // one process and the flag applies to all its workers.
                unsafe { std::env::set_var("OXIDE_OFFLINE_PLUGINS", "1") };
            }
            cmd_build(&root, r#static, w).await
        }
        Command::Start { root, port } => cmd_serve(&root, port, "oxide start", w).await,
        Command::Preview { root } => cmd_serve(&root, None, "oxide preview", w).await,
        Command::Routes { root } => cmd_routes(&root, w).await,
        Command::Analyze { root } => cmd_analyze(&root, w).await,
        Command::Check { root } => cmd_check(&root, w).await,
        Command::Why { root, module } => cmd_why(&root, &module, w).await,
        Command::Doctor { root } => cmd_doctor(&root, w).await,
        Command::Secrets { root, op } => cmd_secrets(&root, op, w).await,
        Command::Add { root, module } => cmd_add(&root, &module, w).await,
        Command::Generate { root, kind, path } => cmd_generate(&root, &kind, &path, w).await,
        Command::Cache { root, op } => cmd_cache(&root, op, w).await,
        Command::Audit { root, json } => cmd_audit(&root, json, w).await,
        Command::Migrate {
            root,
            source,
            write,
        } => cmd_migrate(&root, &source, write, w).await,
        Command::Test { root } => cmd_test(&root, w).await,
        Command::Mcp { root } => cmd_mcp(&root, w).await,
        Command::Schema => cmd_schema(w),
        Command::Config { root, json } => cmd_config(&root, json, w).await,
        Command::Render {
            root,
            route,
            url,
            params,
            data,
            stream,
        } => cmd_render(&root, &route, url.as_deref(), &params, &data, stream, w).await,
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
        Ok(loaded) => {
            // BUG-11: a dynamic `oxide.config.ts` (one the static extractor
            // cannot read) means the CLI is running on a static-extracted
            // subset — the JS eval fallback is not wired here yet. Say so
            // loudly instead of silently serving a partial config. The
            // supported remedy is a literal config or a `oxide.config.json`.
            if loaded.used_dynamic_eval {
                eprintln!(
                    "warn: oxide.config.ts needed dynamic evaluation; the CLI is \
                     running on the static-extracted subset. Make the config a \
                     literal object, or provide oxide.config.json (CFG-DYNAMIC)."
                );
            }
            loaded.config
        }
        Err(e) => {
            // A present-but-broken config is not the same as no config. Surface
            // the code so a user sees `CFG-SCHEMA`/`CFG-IO` rather than silent
            // defaults (the debug log alone hides it in a normal run).
            eprintln!("warn: config load failed ({}): {}", e.0.code, e.0.message);
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

/// §10.1: refine each route's render mode with the AST-based inference.
///
/// The scanner sets `render_mode` from a line-oriented regex (fast, and
/// good enough for `oxide routes` without parsing every loader). The CLI
/// has `oxide-compiler`, so before the build consumes the mode it re-runs
/// the inference over each route's `page.server.ts` with the real AST —
/// which does not misfire on a comment mentioning `cookies` or on a local
/// variable named `locals`.
///
/// An explicit `config.render` from the module still wins: the inference
/// only replaces a mode the scanner produced on its own (the scanner writes
/// `static` / `dynamic` for the inferred case, and any other value for an
/// explicit one). If the module does not parse, the AST pass returns
/// `dynamic` — which the scanner's answer already is in that case.
/// `true` when `page.server.ts` declares an explicit `config.render`
/// (same literal-shape regex the route scanner uses in
/// `oxide-router`'s `read_page_config`). Keeps the AST refinement from
/// clobbering an explicit `render: 'dynamic'` with an inferred `static`.
fn has_explicit_render_config(source: &str) -> bool {
    let mut start = 0;
    while let Some(i) = source[start..].find("render") {
        let abs = start + i;
        // `render`, `renderMode` and `render_mode` all declare the mode.
        let rest = &source[abs + "render".len()..];
        let rest = if rest.starts_with("Mode") {
            &rest["Mode".len()..]
        } else if rest.starts_with("_mode") {
            &rest["_mode".len()..]
        } else {
            rest
        };
        // Skip whitespace; require `:` then a quote (a literal value).
        let t = rest.trim_start();
        if let Some(after) = t.strip_prefix(':') {
            let after = after.trim_start();
            if after.starts_with('"') || after.starts_with('\'') {
                return true;
            }
        }
        start = abs + "render".len();
    }
    false
}

fn refine_render_modes_from_ast(root: &Path, cfg: &OxideConfig, mut tree: RouteTree) -> RouteTree {
    let routes_root = root.join(&cfg.routes.dir);
    for e in &mut tree.entries {
        if !matches!(e.kind, oxide_router::RouteKind::Page) {
            continue;
        }
        let server = routes_root.join(e.file.with_file_name("page.server.ts"));
        let Ok(source) = std::fs::read_to_string(&server) else {
            // §10.1: a page route with no `page.server.ts` has no loader and
            // therefore reads no request state — it is `static`. Without this
            // the mode stays `None`, and the §11 L0 layer cannot tell it can
            // be cached. An explicit mode the scanner already set is kept.
            if matches!(
                e.render_mode.as_deref(),
                Some("static") | Some("dynamic") | None
            ) {
                e.render_mode = Some("static".to_string());
            }
            continue;
        };
        // An explicit `config.render` always wins (§10.1): the scanner wrote
        // the literal value into `render_mode`, and it is indistinguishable
        // from an inferred `static`/`dynamic` after the fact — so re-check
        // the source. When the module declares `render` explicitly, the
        // scanner's value stands and the AST inference must not overwrite it
        // (e.g. `render: 'dynamic'` with no request reads stays dynamic).
        if has_explicit_render_config(&source) {
            continue;
        }
        // Only overwrite an *inferred* mode: the scanner's value is `static`
        // or `dynamic` when it inferred, and the literal string from an
        // explicit `config.render` otherwise. An explicit `isr` / `spa` /
        // `islands` / `auto` is left as-is.
        let inferred_by_scanner = matches!(
            e.render_mode.as_deref(),
            Some("static") | Some("dynamic") | None
        );
        if !inferred_by_scanner {
            continue;
        }
        let m = oxide_compiler::infer_render_mode(&source, &server.to_string_lossy());
        e.render_mode = Some(m.mode.to_string());
    }
    tree
}

/// Scan routes with a tooling pool when a plugin declares `routes` or
/// `serverMiddleware`. Those two hooks contribute entries during the scan, so
/// every command that builds a route table must pass a pool — not just serve.
/// The pool is created lazily: a project with no such plugin pays nothing.
async fn scan_routes_with_plugins(root: &Path, cfg: &OxideConfig) -> anyhow::Result<RouteTree> {
    if !plugin_declares_any(root, cfg, &["routes", "serverMiddleware"]) {
        return scan_routes(root, cfg, None).await;
    }
    use oxide_jshost::JsHost as _;
    let host = oxide_jshost::NodeHost::new(oxide_jshost::NodeHost::worker_script_default());
    let pool = host
        .tooling_pool(oxide_jshost::PoolConfig::default())
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    scan_routes(root, cfg, Some(&pool)).await
}

async fn scan_routes(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_jshost::Pool>>,
) -> anyhow::Result<RouteTree> {
    // Canonicalize `root` first so `dir` is absolute. `RouteTree::scan_with_config`
    // joins the override onto `dir`; if the override is already root-relative
    // (which it is when `root` itself is relative), the join produces
    // `root/root/src/routes`. An absolute override short-circuits that:
    // `PathBuf::join` returns the RHS unchanged when it's absolute.
    let root_abs = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let dir = root_abs.join(&cfg.routes.dir);
    let mut tree = RouteTree::scan_with_config(&root_abs, Some(&dir))
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    // §15.2: `serverMiddleware` — a plugin contributes middleware source.
    // Each result is `{ name, source }`. The source is written under
    // `.oxide/plugins/` and registered as a synthetic root middleware
    // entry, so the existing chain (which joins a route's ancestor
    // middleware files) runs it for every route.
    if let Some(pool) = pool {
        if !collect_plugin_paths_for_hook(&root_abs, cfg, "serverMiddleware").is_empty() {
            let results = call_js_plugin_hook(
                &root_abs,
                cfg,
                pool,
                "serverMiddleware",
                serde_json::json!({ "root": root_abs.to_string_lossy() }),
            )
            .await;
            let plugins_dir = root_abs.join(".oxide/plugins");
            let _ = std::fs::create_dir_all(&plugins_dir);
            for (i, r) in results.iter().enumerate() {
                let Some(src) = r.get("source").and_then(|s| s.as_str()) else {
                    continue;
                };
                let name = r
                    .get("name")
                    .and_then(|n| n.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("plugin-{i}"));
                let safe: String = name
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '-' || c == '_' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                let file = plugins_dir.join(format!("{safe}.middleware.mjs"));
                if std::fs::write(&file, src).is_err() {
                    continue;
                }
                tree.insert(oxide_router::RouteEntry {
                    // A root middleware: base dir `routes`, so it wraps every
                    // route (the chain rule is a prefix match).
                    id: oxide_router::RouteId(format!("routes/{safe}.plugin-middleware")),
                    kind: oxide_router::RouteKind::Middleware,
                    segments: Vec::new(),
                    file,
                    render_mode: None,
                    tags: Vec::new(),
                    has_server_module: false,
                    needs_parent: false,
                    slot: None,
                });
            }
        }
    }
    // §15-E: merge every `oxide.module` package's routes into the tree. This
    // is the single seam every command funnels through (build, dev, routes,
    // analyze, check) — patching it here makes a module's routes visible
    // everywhere without further plumbing. A module's route `file` is
    // rewritten to an absolute path under `node_modules/<name>/`, and its
    // `id` is namespaced `modules/<name>/…` so a project route cannot shadow
    // it. A broken module is reported on stderr and skipped — a module the
    // user did not write must never fail their build (a future tightening
    // pass can make `cfg.modules` strict).
    let mods = crate::modules_loader::load(&root_abs, cfg);
    let merged = crate::modules_loader::inject_routes(&mut tree, &mods);
    if merged > 0 {
        eprintln!(
            "modules: merged {merged} route entr(y/ies) from {} module(s)",
            mods.len()
        );
    }
    for m in crate::modules_loader::has_unmerged_hooks(&mods) {
        let unmerged: Vec<&str> = m
            .hooks
            .iter()
            // §15 / WP-r9: keep in sync with the message — transform,
            // resolveId and load all route through the pipeline.
            .filter(|h| !matches!(h.as_str(), "transform" | "resolveId" | "load"))
            .map(|s| s.as_str())
            .collect();
        eprintln!(
            "modules: `{}` declares hook(s) [{}] — transform, resolveId and load are merged; the rest are deferred",
            m.name,
            unmerged.join(", "),
        );
    }
    // §15.2: `routes` hook — a plugin contributes route entries. Each
    // result is `{ entries: [{ id, file, kind?, segments? }] }`. The file
    // must exist inside the project; the id is used as given (a plugin is
    // expected to namespace it). A malformed entry is reported and skipped.
    if let Some(pool) = pool {
        if !collect_plugin_paths_for_hook(&root_abs, cfg, "routes").is_empty() {
            let results = call_js_plugin_hook(
                &root_abs,
                cfg,
                pool,
                "routes",
                serde_json::json!({ "root": root_abs.to_string_lossy() }),
            )
            .await;
            for r in results {
                let Some(list) = r.get("entries").and_then(|e| e.as_array()) else {
                    continue;
                };
                for e in list {
                    let Some(id) = e.get("id").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let Some(rel) = e.get("file").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let file = root_abs.join(rel);
                    if !file.starts_with(&root_abs) || !file.is_file() {
                        eprintln!("warn: routes hook entry `{id}` has no readable file; skipped");
                        continue;
                    }
                    let kind = match e.get("kind").and_then(|v| v.as_str()).unwrap_or("page") {
                        "layout" => oxide_router::RouteKind::Layout,
                        "endpoint" | "route" => oxide_router::RouteKind::Endpoint,
                        "loading" => oxide_router::RouteKind::Loading,
                        "error" => oxide_router::RouteKind::Error,
                        _ => oxide_router::RouteKind::Page,
                    };
                    let segments: Vec<String> = e
                        .get("segments")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    tree.insert(oxide_router::RouteEntry {
                        id: oxide_router::RouteId(id.to_string()),
                        kind,
                        segments,
                        file,
                        render_mode: None,
                        tags: Vec::new(),
                        has_server_module: false,
                        needs_parent: false,
                        slot: None,
                    });
                }
            }
        }
    }

    Ok(tree)
}

/// URL pattern for display (§19.1 table): `/` for the root, `/a/b` otherwise.

/// Extract a param name from a route segment: `:slug` → `slug`, `*rest` →
/// `rest`, `*?opt` → `opt`. Returns `None` for static segments.
fn param_name(segment: &str) -> Option<&str> {
    let s = segment.trim_start_matches(':');
    let s = s.trim_start_matches('*').trim_start_matches('?');
    if s.len() == segment.len() {
        return None; // no prefix consumed
    }
    // Strip regex constraint (`:slug{[a-z]+}` → `slug`).
    let s = s.split('{').next().unwrap_or(s);
    Some(s)
}

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

/// §10.3: generate the synthetic boot module for a project with islands.
///
/// Static-imports each island from `config.framework.islands`, registers
/// them with the framework adapter's `hydrateIsland`, and pulls in the
/// shared boot runtime (`oxide/client/boot`). Bundled as a single client
/// chunk and injected into every SSR'd page as
/// `<script type="module" src="/<boot-url>">`.
fn generate_island_boot_entry(
    root: &Path,
    framework: &str,
    islands: &BTreeMap<String, String>,
    // §8.4 loading boundaries: URL prefix (`/`, `/blog`) → on-disk
    // `loading.tsx` path. Emitted as static imports + a `loading` map passed
    // to `bootWithRouter`.
    loading: &BTreeMap<String, PathBuf>,
    // §16.6 / SPEC-21: whether the project configured a PWA (`manifest`).
    // When true, the boot entry registers the service worker.
    pwa: bool,
) -> String {
    let mut out = String::new();
    out.push_str("// Generated by oxide-fw-cli -- island boot entry. Do not edit.\n");
    out.push_str("// Static imports resolve before any statement runs, so `boot()`\n");
    out.push_str("// can see both the adapter and the boot runtime's registration hook.\n");
    // `clientAdapter` is the browser-side adapter each framework package
    // exports. For React it wires `react-dom/client` hydration (so `useState`
    // survives); for the others it aliases the neutral-IR adapter today.
    // The named `adapter` export stays the neutral-IR form for tests and the
    // §22 conformance suite, so its `hydrateIsland` continues to work against
    // the fake DOM.
    out.push_str(&format!(
        "import {{ clientAdapter }} from '@oxide/{framework}';\n"
    ));
    out.push_str("import { bootWithRouter, installBootRuntime } from 'oxide/client/boot';\n");
    let mut idents: Vec<(String, String)> = Vec::new();
    for (name, src) in islands {
        let abs = root.join(src);
        let ident = format!(
            "__island_{}",
            name.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                })
                .collect::<String>()
        );
        out.push_str(&format!(
            "import {ident} from {:?};\n",
            abs.to_string_lossy()
        ));
        idents.push((name.clone(), ident));
    }
    out.push_str("\nconst modules = {\n");
    for (name, ident) in &idents {
        out.push_str(&format!("  {name:?}: {ident},\n"));
    }
    out.push_str("};\n\n");
    // §8.4 loading boundaries: a `route directory → loading component`
    // map. Each route's directory (e.g. `routes/`, `routes/blog/`) is
    // emitted as a URL path key so the client router can match the URL it
    // is navigating to. The boot entry imports each loading module
    // statically; a project without loading.tsx emits `{}`.
    //
    // KEY CONTRACT (client matcher, §8.4): a key is an ABSOLUTE PATH whose
    // segments may be `[name]` tokens (the literal bracket dir token the
    // scanner keeps in route ids, e.g. `routes/blog/[slug]/loading` →
    // `/blog/[slug]`). A `[name]` key segment matches exactly ONE
    // non-empty URL segment; the longest key (most segments) wins; the
    // root key is `/`. No trailing slash on non-root keys — the matcher
    // compares segment-by-segment, so `/blog/[slug]` matches `/blog/x`
    // (the old literal-prefix key `/blog/[slug]/` matched nothing).
    if !loading.is_empty() {
        let mut loading_idents: Vec<(String, String)> = Vec::new();
        for (route_id, src) in loading {
            // `routes/blog/[slug]/loading` → `/blog/[slug]` (bracket tokens
            // preserved verbatim). `routes/loading` → `/`.
            let dir = route_id
                .strip_suffix("/loading")
                .unwrap_or(route_id)
                .strip_prefix("routes")
                .unwrap_or(route_id)
                .to_string();
            let url_key = if dir.is_empty() { "/".to_string() } else { dir };
            let abs = root.join(src);
            let ident = format!(
                "__loading_{}",
                route_id
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() || c == '_' {
                        c
                    } else {
                        '_'
                    })
                    .collect::<String>()
            );
            out.push_str(&format!(
                "import {ident} from {:?};\n",
                abs.to_string_lossy()
            ));
            loading_idents.push((url_key, ident));
        }
        out.push_str("\nconst __loading_map = {\n");
        for (url_prefix, ident) in &loading_idents {
            out.push_str(&format!("  {url_prefix:?}: {ident},\n"));
        }
        out.push_str("};\n\n");
    } else {
        out.push_str("\nconst __loading_map = {};\n\n");
    }
    out.push_str("installBootRuntime();\n");
    out.push_str(
        "window.__oxideRegisterAdapter(clientAdapter, async (chunk) => {\n  \
         return modules[chunk] ?? (await import(chunk));\n});\n",
    );
    // §8.4: hydrate the SSR root, scan islands, THEN attach the client router.
    // `bootWithRouter` is the sequenced form: it calls `adapter.hydrate(root,
    // input)` (which sets the adapter module's `active` handle that `update()`
    // later needs) BEFORE installing the router's click listener. Attaching
    // `createRouter` without a preceding `adapter.hydrate` was the pre-fix
    // bug — a same-origin link click changed the URL and the DOM did not
    // re-render (worse than no router, because the pre-fix page still worked
    // via full page load).
    //
    // The client manifest is still not consumed (SPA nav fetches `/__data`;
    // manifest-driven chunk preloading is a documented gap, not silently
    // stubbed). A hydrate failure leaves the SSR markup untouched and skips
    // router attachment so anchors fall back to full-page navigation.
    // §6.4: fetch the client manifest at boot so the router can preload a
    // route's chunks in parallel with the /__data fetch. The fetch is
    // best-effort — a missing manifest (dev server, static export without it)
    // leaves `manifest` undefined and the router silently skips preloading.
    out.push_str(
        "fetch('/client/client.manifest.json')\n  \
         .then((r) => (r.ok ? r.json() : undefined))\n  \
         .then((manifest) => bootWithRouter(clientAdapter, { loading: __loading_map, manifest }))\n  \
         .catch(() => bootWithRouter(clientAdapter, { loading: __loading_map }));\n",
    );
    // §16.6 / §20: report Core Web Vitals to `/_oxide/vitals`. Guarded and
    // no-op outside the browser; the boot entry only runs in one.
    out.push_str("import { reportWebVitals } from 'oxide/client';\nreportWebVitals();\n");
    // §16.6 / SPEC-21: auto-register the service worker when the project
    // configured a PWA manifest. `emit_pwa_worker` wrote `/pwa-register.js`;
    // loading it from the boot entry is what the spec's "auto-registration"
    // means (previously the script was emitted but never loaded).
    if pwa {
        // §16.6: register the service worker at runtime, not via a
        // static `import('/pwa-register.js')`. The latter is resolved by
        // rolldown at bundle time, but `/pwa-register.js` is written by
        // `emit_pwa_worker` AFTER bundling — so a clean build fails with
        // "Could not resolve /pwa-register.js". A runtime `<script>`
        // injection sidesteps the bundler entirely and matches what a
        // manual PWA registration does anyway.
        out.push_str(
            "if (typeof navigator !== 'undefined' && 'serviceWorker' in navigator && !globalThis.__oxidePwaRegistered) {\n  \
             globalThis.__oxidePwaRegistered = true;\n  \
             var __oxidePwa = document.createElement('script');\n  \
             __oxidePwa.src = '/pwa-register.js';\n  \
             __oxidePwa.async = true;\n  \
             document.head.appendChild(__oxidePwa);\n}\n",
        );
    }
    out
}

/// §6.3: the JSX runtime for a framework. React uses Oxc's default
/// (`react/jsx-runtime`); Solid 2.0's runtime lives in `@solidjs/web`,
/// whose `jsx-runtime` subpath exports `jsx`/`jsxs`/`Fragment`. Vue and
/// Svelte entries arrive precompiled from the tooling pool, so their
/// `.vue`/`.svelte` files never reach the TS/JSX lowering pass — but a
/// stray `.tsx` in a Vue project still needs the default.
///
/// This mirrors the `jsxImportSource` field the adapter packages declare
/// in their manifests. When the config carries an explicit framework
/// descriptor with a jsxImportSource, prefer that; today the CLI resolves
/// it from the framework name.
fn jsx_import_source_for(framework: &str) -> Option<String> {
    // BUG-13: honour the adapter's `jsxImportSource` (contract §7,
    // `packages/oxide/src/types.ts`). React uses the Oxc default runtime
    // (`react/jsx-runtime`), which is what `None` means. Solid's renderer
    // lives in `@solidjs/web`, not the React factory.
    match framework {
        "solid" => Some("@solidjs/web".to_string()),
        _ => None,
    }
}

/// §6.4 / §21: the framework runtimes must never be inlined into the SSR
/// bundle. Each runtime carries module-level state (React's
/// `ReactCurrentDispatcher`, Vue's current instance, Svelte's component
/// context, Solid's reactive root); two copies silently break hook dispatch
/// (the classic "invalid hook call / reading 'useState' of null" failure).
///
/// The user's `build.ssrExternals` still wins when set; this default only
/// applies when that list is empty. Extra entries are harmless — an external
/// that is never imported is never resolved.
fn default_ssr_externals() -> Vec<String> {
    [
        // React
        "react",
        "react-dom",
        "react/jsx-runtime",
        "react/jsx-dev-runtime",
        "react-dom/server",
        "react-dom/client",
        // Vue
        "vue",
        "vue/server-renderer",
        // Svelte
        "svelte",
        "svelte/internal",
        // Solid: v1 namespace + v2 package (renamed `./web` -> `@solidjs/web`)
        "solid-js",
        "solid-js/web",
        "solid-js/server",
        "@solidjs/web",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn bundle_env_options(
    env: oxide_bundler::BundleEnv,
    entries: BTreeMap<String, PathBuf>,
    out_dir: PathBuf,
    precompiled: BTreeMap<PathBuf, String>,
    jsx_import_source: Option<String>,
    build_secret: String,
    transform_hook: Option<oxide_bundler::TransformHook>,
    css_transform: oxide_bundler::CssTransformHook,
    // §6.1: specifiers left external in the SSR bundle. Empty for client/edge
    // (a browser bundle cannot resolve a bare import) and for the default
    // self-contained server bundle; populated from `build.ssrExternals`.
    ssr_externals: Vec<String>,
    // BUG-11: `build.sourcemap` (`off` | `file` | `hidden`). `off` → no maps.
    sourcemap_on: bool,
    // P-23: production builds minify; the dev client bundle stays unminified
    // for debugging and a faster fresh `oxide dev`.
    minify: bool,
) -> oxide_bundler::BundleOptions {
    // §6.1: `process.env.NODE_ENV` drives React's dev-vs-production
    // bundle selection. Setting it at the CLI level (rather than in the
    // bundler) keeps the bundler agnostic of a project's framework — the
    // CLI is what decides a build is a production build. Without this,
    // React ships the dev build (~1 MB) into every island chunk.
    let mut define: BTreeMap<String, String> = BTreeMap::new();
    match env {
        oxide_bundler::BundleEnv::Client => {
            define.insert(
                "process.env.NODE_ENV".to_string(),
                "\"production\"".to_string(),
            );
        }
        oxide_bundler::BundleEnv::Ssr => {
            // Server rendering must NOT bake in "production": loaders may
            // read `process.env.NODE_ENV` at request time to decide dev
            // diagnostics vs. silent failures, and a worker evaluates them
            // with the real environment. Left unset so the define is a
            // no-op on the server bundle.
        }
        oxide_bundler::BundleEnv::Edge => {
            // Edge runtimes do not have a `process` global; a define
            // baking in "production" would be accurate but the runtime is
            // a browser-like sandbox, so leave it untouched for now.
        }
    }
    oxide_bundler::BundleOptions {
        env,
        entries,
        out_dir,
        minify,
        // BUG-11: honour `build.sourcemap`. `RolldownBundler` supports
        // `SourceMapType::File`; `off` emits no map.
        sourcemap: sourcemap_on,
        // §21: externals only apply to the server bundle; a browser bundle
        // must inline everything.
        externals: match env {
            oxide_bundler::BundleEnv::Ssr => {
                let mut e = ssr_externals;
                if e.is_empty() {
                    e = default_ssr_externals();
                }
                e
            }
            _ => Vec::new(),
        },
        define,
        tsconfig_paths: BTreeMap::new(),
        build_secret,
        precompiled,
        jsx_import_source,
        transform_hook,
        resolve_hook: None,
        load_hook: None,
        render_chunk_hook: None,
        generate_bundle_hook: None,
        transform_html_hook: None,
        css_transform: Some(css_transform),
        inline_dynamic_imports: false,
        resolve_conditions: None,
        virtual_modules: Default::default(),
    }
}

// ----------------------------------------------------------------- commands

/// §13.3 (HMR route-data updates): map a changed `page.server.ts` back to
/// its route's URL path pattern and re-run the route's loader so the
/// published `route-data-update` HMR message carries REAL payload bytes.
///
/// Returns `(route_pattern, odf_b64)`:
///   * `route_pattern` — the same key shape the §8.4 loading map emits: an
///     absolute path whose segments may be `[name]` tokens (e.g.
///     `/blog/[slug]`), derived from the route id; `/` for the root page.
///     The previous publisher sent the FILE path, which no client matcher
///     can compare against a URL.
///   * `odf_b64` — the ODF-encoded `{route, url, params, data}` payload
///     (the same `oxide_odf::encode_to_bytes` shape the live hydration
///     payload and the L0 cache entries use), carrying the loader's FRESH
///     data. Params are empty: a file change has no request context, and
///     the inline HMR client re-fetches `/__data` for the concrete URL
///     anyway — `odf_b64` is the offline fallback.
///
/// Any miss (not a page's server module, no loader host, loader error,
/// encode error) degrades to `(file_path, "")` at the call site — the old
/// shape — so a misfire is never worse than the previous behavior.
async fn hmr_route_data_update(
    changed: &Path,
    rootp: &Path,
    cfg: &OxideConfig,
    tree: &oxide_router::RouteTree,
    loader: Option<&std::sync::Arc<dyn oxide_server::LoaderRunner>>,
) -> (String, String) {
    // Only a PAGE route's `page.server.ts` triggers a data update (a
    // `layout.server.ts` change maps to no single URL; the watcher falls
    // back to the file-path message for those).
    let routes_root = rootp.join(&cfg.routes.dir);
    let owned = match tree.entries.iter().find(|e| {
        matches!(e.kind, oxide_router::RouteKind::Page)
            && routes_root.join(e.file.with_file_name("page.server.ts")) == changed
    }) {
        Some(e) => (
            // Route id → URL path pattern (`routes/blog/[slug]/page` →
            // `/blog/[slug]`), same derivation as the loading-map keys.
            match e
                .id
                .0
                .strip_suffix("/page")
                .unwrap_or(&e.id.0)
                .strip_prefix("routes")
            {
                Some(d) if !d.is_empty() => d.to_string(),
                _ => "/".to_string(),
            },
            routes_root
                .join(e.file.with_file_name("page.server.ts"))
                .to_string_lossy()
                .to_string(),
            e.id.0.strip_suffix("/page").unwrap_or(&e.id.0).to_string(),
        ),
        None => return (String::new(), String::new()),
    };
    let (pattern, module_path, route_for_loader) = owned;
    let Some(loader) = loader else {
        return (pattern, String::new());
    };
    let input = oxide_server::LoaderRunInput::new(module_path, route_for_loader, pattern.clone());
    match loader.run(input).await {
        Ok(out) if out.status == 200 => {
            let payload = serde_json::json!({
                "route": pattern,
                "url": pattern,
                "params": {},
                "data": out.data,
            });
            match oxide_odf::encode_to_bytes(&oxide_odf::Value::from_json(&payload)) {
                Ok(bytes) => {
                    use base64::Engine as _;
                    (
                        pattern,
                        base64::engine::general_purpose::STANDARD.encode(&bytes),
                    )
                }
                Err(_) => (pattern, String::new()),
            }
        }
        _ => (pattern, String::new()),
    }
}

/// `oxide dev` (§13). `--once` runs a single watch poll (test hook); otherwise the
/// 200 ms poll loop runs until Ctrl-C.
/// §13.1 / P0-7: produce `dist/client/client.manifest.json` on a fresh
/// `oxide dev` when it is missing. Without a client bundle the dev server has
/// no boot script: no router, no loading boundaries, no defer glue, no island
/// hydration — only the inline HMR client runs. This runs the client bundler
/// once, **unminified** (fast; dev does not need the production size), into
/// `dist/client/`, then returns. A project that already has a manifest (a
/// `build` ran first) is left alone.
///
/// This is the dev-mode bundling path: a full per-module dev server is
/// post-v0.1. A cold `oxide dev` on a fresh project therefore pays one
/// unminified client build (~1-3s on the smoke fixtures).
/// §6.4 / WP-A2: the canonical route id used by every emitted manifest and
/// by the runtime component resolver. It is the route *directory* form: the
/// scanner's `routes/<...>/page` loses its `/page` suffix; layouts, error,
/// not-found and template ids are already canonical.
/// §8.2 / §16.3 / O1: compute the layout chain for a route id from the
/// router tree. Route ids are the *canonical* form (`/page` stripped), and
/// a `layout.tsx` at each prefix of the id is checked. A page at
/// `routes/nested/about` sees `routes/layout` then `routes/nested/layout`
/// in that order (outermost first) — the same order the router's
/// `layout_chain` produces for a live match.
///
/// Pure and self-contained: walks `tree.entries` looking for `Layout` kind
/// entries whose canonical id matches `<prefix>/layout` at each depth of
/// the route id. No private router state, no lazy-init to trigger — the
/// CLI already has `tree.entries` in scope at every build call site.
/// Rolldown sanitizes `[slug]` → `_slug_` in chunk names. Manifest route
/// keys therefore use the sanitized form while the router tree uses the
/// source form. Map a manifest key back to the canonical route id so
/// layout-chain lookup can find the tree entry.
fn canonicalize_manifest_key(tree: &oxide_router::RouteTree, key: &str) -> String {
    // Direct match: the manifest already carries the source form.
    if tree
        .entries
        .iter()
        .any(|e| e.id.0.strip_suffix("/page").unwrap_or(&e.id.0) == key)
    {
        return key.to_string();
    }
    // Try each tree route's sanitized form against the manifest key.
    for e in &tree.entries {
        let canon = e.id.0.strip_suffix("/page").unwrap_or(&e.id.0);
        let sanitized = canon
            .split('/')
            .map(|seg| {
                if seg.starts_with('[') && seg.ends_with(']') && seg.len() > 2 {
                    format!("_{}_", &seg[1..seg.len() - 1])
                } else {
                    seg.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        if sanitized == key {
            return canon.to_string();
        }
    }
    // Fallback: return as-is. compute_layout_chain will find nothing.
    key.to_string()
}

fn compute_layout_chain(tree: &oxide_router::RouteTree, route_id: &str) -> Vec<String> {
    let segments: Vec<&str> = route_id.split('/').collect();
    let mut chain = Vec::new();
    for i in 0..segments.len() {
        let prefix = segments[..=i].join("/");
        let layout_id = format!("{prefix}/layout");
        if tree
            .entries
            .iter()
            .any(|e| matches!(e.kind, oxide_router::RouteKind::Layout) && e.id.0 == layout_id)
        {
            chain.push(layout_id);
        }
    }
    chain
}

/// §8.2 / §16.3 / O1: enrich a written manifest with layout chains from the
/// router tree. The bundler writes an empty `layout_chain` on every route
/// because it doesn't know the tree; the CLI reads the JSON back, fills in
/// each route's chain, and writes it. Absent/None for older manifests is
/// safe — the field is `#[serde(default)]`.
fn enrich_manifest_layout_chains(
    manifest_path: &std::path::Path,
    tree: &oxide_router::RouteTree,
) -> anyhow::Result<()> {
    let bytes = match std::fs::read(manifest_path) {
        Ok(b) => b,
        Err(_) => return Ok(()), // no manifest yet — nothing to enrich
    };
    let mut doc: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("manifest {} invalid JSON: {e}", manifest_path.display()))?;
    let routes = match doc.get_mut("routes").and_then(|v| v.as_object_mut()) {
        Some(r) => r,
        None => return Ok(()),
    };
    let mut enriched = 0usize;
    for (route_id, entry) in routes.iter_mut() {
        // §8.2 / O1: the manifest key may be in rolldown's sanitized form
        // (`routes/blog/_slug_`), while the router tree uses the source form
        // (`routes/blog/[slug]`). Canonicalize before the layout-chain walk
        // so a param route finds its layouts.
        let canonical = canonicalize_manifest_key(tree, route_id);
        let chain = compute_layout_chain(tree, &canonical);
        if let Some(obj) = entry.as_object_mut() {
            obj.insert(
                "layout_chain".to_string(),
                serde_json::to_value(&chain).unwrap_or(serde_json::Value::Array(Vec::new())),
            );
            if !chain.is_empty() {
                enriched += 1;
            }
        }
    }
    std::fs::write(manifest_path, serde_json::to_string_pretty(&doc)? + "\n")
        .map_err(|e| anyhow::anyhow!("write {}: {e}", manifest_path.display()))?;
    if enriched > 0 {
        tracing::debug!(count = enriched, "enriched manifest layout chains");
    }
    Ok(())
}

/// §9.2 / §20 / O3c: the wall-clock time the parallel load path saved.
/// When the last-layout load and the page load run under `tokio::join!`,
/// the total load phase is `max(a, b)` instead of `a + b`; the saving is
/// `min(a, b)`. Zero when they are the same duration (nothing saved) or
/// when the sequential path ran (caller passes 0.0 for one side).
///
/// Pure so the arithmetic is testable without a real server: the
/// arithmetic *is* the marking, the recorder just carries the number.
fn parallel_load_savings_ms(a_ms: f64, b_ms: f64) -> f64 {
    a_ms.min(b_ms).max(0.0)
}

/// The stage name the parallel arm records. It is a **counterfactual**: the
/// wall-clock the overlapping loads would have cost had they run
/// sequentially (`min(a, b)`), not a duration anyone measured. It is
/// recorded as a saving so the inspector's total reconciles, and the name
/// says so — `overlap-saved` rather than a bare `load-overlap` that reads
/// like a phase.
const LOAD_OVERLAP_STAGE: &str = "overlap-saved";

/// §16.6 / WP-D1: encode a loader's post-run session into a `Set-Cookie`
/// value, but only when it actually changed. Compares against the inbound
/// session (the object decoded from the request's cookie); an untouched
/// session — including the `Some({})` the worker returns for a
/// no-cookie-start project — never re-issues a cookie.
///
/// Returns `Some(cookie)` when a cookie should ride the response, `None`
/// when the session is unchanged or absent. Extracted so the four serve
/// paths (sequential page, parallel page, /__data short-circuit, /__data
/// page) share one implementation instead of four near-copies.
/// §16.6 / WP-r9: session cookie lifetime, sourced from
/// `security.sessionTtlSecs` at server start. Process-global because
/// `build_dispatch`'s closure cannot see `cfg`; the CLI runs one server
/// per process (documented single-server assumption). Tests use the
/// default 1800 until they opt in.
static SESSION_TTL_SECS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1800);

fn session_ttl_secs() -> u64 {
    SESSION_TTL_SECS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Set the session cookie TTL. Called by the CLI at server start from
/// `cfg.security.session_ttl_secs`.
pub(crate) fn set_session_ttl(ttl: u64) {
    SESSION_TTL_SECS.store(ttl, std::sync::atomic::Ordering::Relaxed);
}

fn encode_session_cookie(
    after: Option<&serde_json::Value>,
    before: Option<&serde_json::Value>,
    codec: Option<&oxide_server::SessionCodec>,
) -> Option<String> {
    let codec = codec?;
    let after = after?;
    let ttl_secs = session_ttl_secs();
    // Unchanged object → no cookie. `Some({})` (the worker's default for a
    // project with no inbound cookie) equals `None` before → not a change.
    let is_empty_object = after.as_object().map(|o| o.is_empty()).unwrap_or(false);
    let changed = match (after, before) {
        (a, Some(b)) => a != b,
        (a, None) => !is_empty_object && !a.is_null(),
    };
    if !changed {
        return None;
    }
    let mut sess = oxide_server::Session::new();
    if let Some(obj) = after.as_object() {
        for (k, v) in obj {
            sess.insert(k.clone(), v.clone());
        }
    }
    match sess.encode(codec) {
        Ok(token) => Some(format!(
            "oxide_session={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={ttl_secs}"
        )),
        Err(e) => {
            tracing::warn!(error = %e.0.message, "session encode failed; cookie not set");
            None
        }
    }
}

fn canonical_route_id(id: &str) -> String {
    id.strip_suffix("/page").unwrap_or(id).to_string()
}

async fn ensure_dev_client_bundle(
    rootp: &Path,
    cfg: &oxide_config::OxideConfig,
    tree: &oxide_router::RouteTree,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    let dist = rootp.join("dist");
    let client_out = dist.join("client");
    if client_out.join("client.manifest.json").is_file() {
        return Ok(());
    }

    // Entries: pages + endpoints + loading boundaries (mirrors cmd_build).
    let mut entries: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut loading_entries: BTreeMap<String, PathBuf> = BTreeMap::new();
    for e in &tree.entries {
        match e.kind {
            oxide_router::RouteKind::Loading => {
                loading_entries.insert(e.id.0.clone(), rootp.join(&cfg.routes.dir).join(&e.file));
            }
            // §8.2: layouts must be client entries too — the client hydrate
            // rebuilds the server's `<RootLayout><Page/></RootLayout>` tree
            // via `componentFor`, which needs the layout chunk in
            // `client.manifest.json`. Without this the client can only
            // hydrate the bare page and React reports #418.
            oxide_router::RouteKind::Layout => {
                entries.insert(e.id.0.clone(), rootp.join(&cfg.routes.dir).join(&e.file));
            }
            oxide_router::RouteKind::Page
            | oxide_router::RouteKind::Endpoint
            // §6.4 / WP-A1: error / not-found / template components are part
            // of the generated component surface too — the resolver must be
            // able to find them without a hand-written config map.
            | oxide_router::RouteKind::Error
            | oxide_router::RouteKind::NotFound
            | oxide_router::RouteKind::Template => {
                entries.insert(e.id.0.clone(), rootp.join(&cfg.routes.dir).join(&e.file));
            }
            _ => {}
        }
    }
    let mut client_entries = entries;

    // The boot entry (client router + loading map + defer glue + PWA), when
    // the project has any page route.
    let has_page_routes = tree
        .entries
        .iter()
        .any(|e| matches!(e.kind, oxide_router::RouteKind::Page));
    if has_page_routes {
        let framework = cfg
            .framework
            .adapter
            .clone()
            .or_else(|| cfg.framework.name.clone())
            .unwrap_or_else(|| "react".to_string());
        let mut loading_dir_map: BTreeMap<String, PathBuf> = BTreeMap::new();
        for (route_id, path) in &loading_entries {
            loading_dir_map.insert(route_id.clone(), path.clone());
        }
        let boot_src = generate_island_boot_entry(
            rootp,
            &framework,
            &cfg.framework.islands,
            &loading_dir_map,
            cfg.manifest.is_some(),
        );
        let boot_dir = rootp.join(".oxide");
        std::fs::create_dir_all(&boot_dir)?;
        let boot_path = boot_dir.join("island-boot.mjs");
        std::fs::write(&boot_path, boot_src)?;
        client_entries.insert("__boot__".to_string(), boot_path);
    }

    // The transform + CSS hooks the dev bundle uses.
    let jsx_source = jsx_import_source_for(
        cfg.framework
            .adapter
            .as_deref()
            .or(cfg.framework.name.as_deref())
            .unwrap_or("react"),
    );
    let build_secret = build_secret(rootp).unwrap_or_default();
    let transform_hook = build_transform_hook(rootp, cfg);
    let css_transform = build_css_transform_hook(cfg);

    // §6.5 / §6.1: unminified, sourcemaps on (dev debugging). No precompile
    // pass here — the Oxc path handles TS/JSX; SFC precompile is a build-time
    // optimization, and a dev start should not block on the tooling pool.
    let client_opts = bundle_env_options(
        oxide_bundler::BundleEnv::Client,
        client_entries,
        client_out,
        BTreeMap::new(),
        jsx_source,
        build_secret,
        transform_hook,
        css_transform,
        Vec::new(),
        true,  // sourcemap on for dev
        false, // P-23: unminified dev client bundle
    );
    use oxide_bundler::Bundler as _;
    let rb = oxide_bundler::RolldownBundler::new(rootp);
    match rb.bundle(&client_opts) {
        Ok(outcome) => {
            outln!(
                w,
                "dev: client bundle ready ({} chunks)",
                outcome.chunks.len()
            );
            Ok(())
        }
        Err(e) => {
            // A failed dev bundle is reported but not fatal: the SSR-only
            // page still serves (no client hydration), and the error overlay
            // surfaces the diagnostic. A hard failure here would stop `dev`
            // on a project that only has a syntax error in one component.
            outln!(
                w,
                "dev: client bundle failed ({} {}); pages serve without hydration",
                e.0.code,
                e.0.message
            );
            Ok(())
        }
    }
}

/// True when any event touches a configured content-collection directory.
/// The dev loop rewrites `dist/content` when this is true; `getCollection`
/// then re-reads by content hash, so no worker restart or cache clear is
/// needed.
fn content_source_changed(
    root: &Path,
    cfg: &oxide_config::OxideConfig,
    events: &[oxide_dev::FileEvent],
) -> bool {
    events.iter().any(|ev| {
        let p = match ev {
            oxide_dev::FileEvent::Modified(p)
            | oxide_dev::FileEvent::Created(p)
            | oxide_dev::FileEvent::Removed(p) => p,
        };
        cfg.content
            .collections
            .iter()
            .any(|c| !c.dir.is_empty() && p.starts_with(root.join(&c.dir)))
    })
}

async fn cmd_dev(root: &str, once: bool, flags: DevFlags, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let mut cfg = load_config_or_default(&rootp, "development");
    // SPEC-15.1 / §15.2: fail at load for an unknown `kind:"native"` plugin.
    if let Err(e) = crate::native_plugins::parse_native_plugins(&cfg) {
        anyhow::bail!("{} {}", e.0.code, e.0.message);
    }
    let tree = scan_routes_with_plugins(&rootp, &cfg).await?;
    // §16.5: emit content entries before the server starts so a dev loader's
    // first `getCollection` call works without a prior `oxide build`.
    emit_content_search_indexes(&rootp, &cfg, &rootp.join("dist"), w)?;
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
    // §13.5 dev features: HTTPS, LAN host, QR, browser open.
    // Bind: LAN mode uses 0.0.0.0 so a phone on the same Wi-Fi can reach it;
    // otherwise loopback only.
    let bind_ip: std::net::IpAddr = if flags.host {
        std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
    } else {
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
    };

    // Build the TLS config once when HTTPS is on.
    let tls_config = if flags.https {
        let cert = oxide_dev::generate_dev_cert()
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        let cfg = oxide_server::Server::tls_config_from_pem(
            cert.cert_pem.as_bytes(),
            cert.key_pem.as_bytes(),
        )
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        outln!(
            w,
            "dev HTTPS enabled (self-signed; SANs: {})",
            cert.sans.join(", ")
        );
        Some(cfg)
    } else {
        None
    };

    // §13.1: start the dev HTTP(S) server with the inspector attached, then
    // poll the watcher. On `--once` we still start the server so the test
    // hook can exercise it, report readiness, and shut down.
    let proxy_rules = flags.proxy_rules();
    if !proxy_rules.is_empty() {
        for r in &proxy_rules {
            outln!(
                w,
                "proxy: {} -> {}{}",
                r.prefix,
                r.target,
                if r.strip_prefix { " (strip)" } else { "" }
            );
        }
    }
    // §7 / §9.1: build the framework stack (renderer + loader) from the
    // project's oxide.config.{ts,mjs,js}. Both fall back gracefully when the
    // config is absent or the worker can't start.
    let config_path_opt = find_config_file(&rootp).ok();
    // §6.2: the plugin transform handler, attached to the render pool so the
    // worker's compile_module applies it (reaching the prerender path).
    let transform_handler = framework_shim::build_transform_handler(&rootp, &cfg);
    // §13.3 / SPEC-14: HMR env for the loader hook (`tsx-loader.mjs`).
    // `OXIDE_DEV` gates the versioned-resolve behavior; `OXIDE_ROOT` lets
    // the loader find `.oxide/hmr-generation` (written by the watcher on
    // each `framework:invalidate`). Both are inherited by the worker
    // subprocess at spawn.
    // Edition 2024 makes `set_var` unsafe (env vars are process-global and
    // not thread-safe in general). This runs at cmd_dev entry, before any
    // worker thread is spawned, and the only other reader is the worker
    // subprocess after fork.
    unsafe {
        std::env::set_var("OXIDE_DEV", "1");
        std::env::set_var("OXIDE_ROOT", rootp.to_string_lossy().as_ref());
    }
    // SPEC-15-C: write a plugin registry the loader reads. The
    // `tsx-loader.mjs` runs in the app's Node process — it does NOT have
    // access to the worker's `__oxide_op_request` bridge, so it imports
    // plugin modules directly (they are Node ESM) and calls their
    // `transform` hook on every `.ts/.tsx/.js/.jsx` file under
    // `<root>/src/`. This closes the dev-vs-build gap: a plugin transform
    // now applies in BOTH dev and build (the bundler path uses
    // `BundleOptions::transform_hook`).
    {
        let plugins: Vec<serde_json::Value> = cfg
            .plugins
            .iter()
            .filter(|p| p.get("kind").and_then(|v| v.as_str()) == Some("js"))
            .filter_map(|p| p.get("path").and_then(|v| v.as_str()))
            .map(|rel| {
                let abs = if std::path::Path::new(rel).is_absolute() {
                    std::path::PathBuf::from(rel)
                } else {
                    rootp.join(rel)
                };
                serde_json::json!({ "path": abs.to_string_lossy() })
            })
            .collect();
        let path = rootp.join(".oxide/dev-plugins.json");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&plugins) {
            let _ = std::fs::write(&path, bytes);
        }
    }
    // §11.2: thread the project's `cache.handler` into the stack so the tier
    // the worker (and the L0 cache) actually uses honors the config — the
    // CLI-side `stack.cache_tier == None` arm below is only for stacks built
    // without a config (tests), so this is the only live wiring.
    // §15.2: `cacheHandler` — a plugin may select the cache backend. The
    // chosen config replaces `cfg.cache.handler` for the stack build below
    // (and for the L0 tier the server constructs from the same value).
    if let Ok(Some(chosen)) = resolve_cache_handler_via_plugin(&rootp, &cfg, w).await {
        cfg.cache.handler = chosen;
    }
    let stack = framework_shim::build_framework_stack(
        config_path_opt.as_deref(),
        cfg.server.backend,
        transform_handler,
        Some(&cfg.cache.handler),
    )
    .await;
    // §6.4 / WP-C1: in dev there is no build entry, so hand the worker the
    // route-table → source-file map. TS/JS/JSX go through the worker's
    // loader; SFCs are precompiled once here (and recompiled on watcher
    // invalidation, which re-imports through the bumped generation).
    {
        use oxide_router::RouteKind;
        let routes_root = rootp.join(&cfg.routes.dir);
        let entries: BTreeMap<String, PathBuf> = tree
            .entries
            .iter()
            .filter(|e| {
                matches!(
                    e.kind,
                    RouteKind::Page
                        | RouteKind::Layout
                        | RouteKind::Error
                        | RouteKind::NotFound
                        | RouteKind::Template
                )
            })
            .map(|e| (canonical_route_id(&e.id.0), routes_root.join(&e.file)))
            .collect();
        if !entries.is_empty() {
            let framework = cfg
                .framework
                .adapter
                .as_deref()
                .or(cfg.framework.name.as_deref());
            let sources = framework_shim::dev_component_sources(
                &rootp,
                framework,
                entries,
                stack.tooling_pool.as_ref(),
            )
            .await;
            if let Ok(mut src) = stack.component_source.write() {
                src.sources = sources;
            }
        }
    }
    if let Some(name) = stack.renderer_name() {
        outln!(w, "framework renderer: {}", name);
    }
    // §13.3: keep a pool handle for the watcher. On file change it bumps the
    // worker's module generation, so the next render/load re-evaluates the
    // edited module instead of serving the cached one.
    let hmr_pool: Option<std::sync::Arc<dyn oxide_jshost::Pool>> = stack.pool.clone();
    // §13.3: keep a loader handle for the watcher too — a `page.server.ts`
    // change re-runs the route's loader so the published HMR
    // `route-data-update` message carries fresh ODF bytes (not an empty
    // payload the client has to fetch itself).
    let hmr_loader: Option<std::sync::Arc<dyn oxide_server::LoaderRunner>> =
        stack.loader_runner.clone();
    // WP-B: the rescan arm rebuilds renderer component sources when a route
    // file is added or removed, mirroring the boot-time population above.
    // Clone the handles now so the watcher loop can reach them after `stack`
    // is out of scope for the loop body.
    let hmr_component_source = std::sync::Arc::clone(&stack.component_source);
    let hmr_tooling_pool = stack.tooling_pool.clone();

    // §13.1 / P0-7: a fresh `oxide dev` has no client bundle, so nothing
    // would hydrate, navigate, or load a route's JS. Build it once (unminified)
    // if the manifest is missing. A `dev` after `build` finds it and skips.
    ensure_dev_client_bundle(&rootp, &cfg, &tree, w).await?;
    // §16.4: tell the client whether this project has message
    // catalogs, so it does not fetch one that cannot exist.
    stamp_i18n_locales(&rootp, &cfg);

    // SPEC-14: the HMR hub. Its build id comes from the client manifest
    // when present (a `dev` start after `build`), else `"dev"`.
    let hmr_build_id = std::fs::read_to_string(rootp.join("dist/client/client.manifest.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| {
            v.get("build_id")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "dev".to_string());
    let hmr_hub = std::sync::Arc::new(oxide_dev::HmrHub::with_build_id(hmr_build_id.clone()));
    // §15.2: `configureServer` — a plugin gets the dev server's handles and
    // may contribute HMR behaviour. Each result may carry
    // `{ reload: bool }` to force a full reload on a config-shaped change;
    // the hub already reloads on module updates, so the hook's value today
    // is the notification plus a place for a plugin to install its own
    // listeners against the hub.
    if !collect_plugin_paths_for_hook(&rootp, &cfg, "configureServer").is_empty() {
        if let Some(pool) = hmr_pool.as_ref() {
            let _ = call_js_plugin_hook(
                &rootp,
                &cfg,
                pool,
                "configureServer",
                serde_json::json!({
                    "root": rootp.to_string_lossy(),
                    "hmrUrl": "/_oxide/hmr",
                    "buildId": hmr_build_id.clone(),
                }),
            )
            .await;
        }
    }
    let started = start_dev_server_full(
        root,
        flags.port,
        bind_ip,
        tls_config,
        proxy_rules,
        stack,
        Some(std::sync::Arc::clone(&hmr_hub)),
    )
    .await?;
    let local = oxide_dev::local_url(started.addr.port(), flags.https);
    outln!(w, "dev server listening on {local}");
    match &started.dev_token {
        Some(t) => outln!(w, "inspector: {local}/__oxide?token={t}"),
        None => outln!(w, "inspector: {local}/__oxide"),
    }
    if let Some(net) = oxide_dev::network_url(started.addr.port(), flags.https) {
        if flags.host {
            outln!(w, "network: {net}");
        }
        if flags.qr {
            match oxide_dev::render_qr(&net) {
                Ok(qr) => {
                    outln!(w, "{qr}");
                }
                Err(e) => outln!(w, "QR render failed: {}", e.0.message),
            }
        }
        if flags.open {
            let opened = oxide_dev::open_browser(&local);
            outln!(w, "browser open: {opened}");
        }
    }

    // SPEC-16: watch the app AND any linked workspace packages, so an edit in
    // `packages/ui` (a `file:` dependency) invalidates the app.
    let mut watch_roots: Vec<std::path::PathBuf> = vec![rootp.clone()];
    if let Some(ws) = oxide_config::detect_workspace_root(&rootp) {
        // Watch each workspace package that is not the app itself.
        if let Ok(rd) = std::fs::read_dir(&ws) {
            for e in rd.flatten() {
                let p = e.path();
                if !p.is_dir() {
                    continue;
                }
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if matches!(name, "node_modules" | "target" | ".git" | ".oxide" | "dist") {
                    continue;
                }
                // A package has a package.json or Cargo.toml at its root.
                if p.join("package.json").is_file() || p.join("Cargo.toml").is_file() {
                    if p != rootp && !watch_roots.contains(&p) {
                        watch_roots.push(p);
                    }
                }
            }
        }
    }
    let watcher = oxide_dev::Watcher::with_notify(watch_roots);
    let mut seen = std::collections::BTreeSet::new();

    // PERF-08: a `DemandGraph` over the watched modules. A node per source
    // file whose compute reads that file; a change invalidates only the nodes
    // that observed the changed input (early cutoff spares the rest). The dev
    // loop bumps the worker generation only when a node actually recomputed,
    // so a no-op touch (or a change to an unrelated file) does not churn the
    // framework module graph.
    let dev_graph = std::sync::Arc::new(oxide_fw_graph::DemandGraph::new());

    /// A compute that reads one file, recording the `Input::File` edge. Its
    /// output hash is the file content hash — an unchanged re-read is a
    /// Survivor (early cutoff), so dependents are not re-run.
    struct FileCompute(std::path::PathBuf);
    impl oxide_fw_graph::Compute for FileCompute {
        fn compute(
            &self,
            ctx: &mut oxide_fw_graph::ComputeCtx,
        ) -> oxide_diagnostics::Result<oxide_fw_graph::NodeOutput> {
            let bytes = ctx.fs.read_file(&self.0)?;
            let hash = blake3::hash(&bytes);
            Ok(oxide_fw_graph::NodeOutput {
                hash,
                artifact: None,
                deps: Default::default(),
            })
        }
    }

    // §13.3: on any file-change event, bump the worker's module generation
    // so the next render/load sees fresh code. Best-effort: log a warning
    // if the worker is unavailable; never fails the dev loop.
    async fn invalidate_framework_modules(
        pool: &Option<std::sync::Arc<dyn oxide_jshost::Pool>>,
        root: &std::path::Path,
    ) {
        if let Some(pool) = pool.as_ref() {
            let host = oxide_jshost::FrameworkHost::new(std::sync::Arc::clone(pool));
            match host.invalidate().await {
                Ok(generation) => {
                    // SPEC-14: publish the new generation to disk so the
                    // loader's `resolve()` hook (`tsx-loader.mjs`) can
                    // version the config's transitive component imports
                    // with `?v=<generation>`. The generation number matches
                    // the worker's in-memory counter, so versioned imports
                    // hit the correct eval graph on every edit.
                    let gen_path = root.join(".oxide").join("hmr-generation");
                    if let Some(parent) = gen_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(&gen_path, generation.to_string());
                    tracing::debug!(generation, "framework modules invalidated")
                }
                Err(e) => tracing::warn!(
                    code = %e.0.code,
                    error = %e.0.message,
                    "framework invalidate failed",
                ),
            }
        }
    }

    // PERF-08: register a graph node per changed file and invalidate it. The
    // returned set is non-empty only when a node actually recomputed (early
    // cutoff spares unchanged content), so the worker generation bumps only
    // when there is real new code to see.
    fn graph_invalidated(dev_graph: &oxide_fw_graph::DemandGraph, ev: &oxide_dev::FileEvent) -> bool {
        let path = match ev {
            oxide_dev::FileEvent::Modified(p)
            | oxide_dev::FileEvent::Created(p)
            | oxide_dev::FileEvent::Removed(p) => p.clone(),
        };
        // §5.5 / §5-E: a lockfile change invalidates every resolver edge. A
        // `package-lock.json` / `pnpm-lock.yaml` / `yarn.lock` / `bun.lockb`
        // / `Cargo.lock` change can bump a package version that a module's
        // import resolved against — the file's own hash does not change, so
        // only the resolver edge catches it. `Input::Resolver` collapses to
        // the specifier's identity in `InputId::of`, so there is no
        // specifier to name here; instead the invalidation is broadcast by
        // bumping the graph's version epoch, which the demand graph treats as
        // a revalidation of every node whose deps cannot prove they survived.
        //
        // A directory-scoped invalidation of every `Input::Resolver` is what
        // the spec's §5.5 says a lockfile change must do; the graph's API
        // does not currently expose "invalidate by input kind" (its
        // `invalidate(&[Input])` names a specific input), so the honest
        // shape today is: an empty `invalidate([])` bumps the epoch and
        // forces every node without a proven-survivor dep to recompute.
        let base_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if matches!(
            base_name,
            "package-lock.json"
                | "pnpm-lock.yaml"
                | "yarn.lock"
                | "bun.lockb"
                | "Cargo.lock"
                | "bun.lock"
        ) {
            // Broadcast: bump the epoch, then re-seed the lockfile's own node
            // so the graph has something to invalidate against.
            let _ = dev_graph.invalidate(&[]);
            let lock_key = oxide_fw_graph::NodeKey::new(path.to_string_lossy().as_ref());
            struct LockFileCompute(std::path::PathBuf);
            impl oxide_fw_graph::Compute for LockFileCompute {
                fn compute(
                    &self,
                    ctx: &mut oxide_fw_graph::ComputeCtx,
                ) -> oxide_diagnostics::Result<oxide_fw_graph::NodeOutput> {
                    let bytes = ctx.fs.read_file(&self.0).unwrap_or_default();
                    Ok(oxide_fw_graph::NodeOutput {
                        hash: blake3::hash(&bytes),
                        artifact: None,
                        deps: Default::default(),
                    })
                }
            }
            dev_graph.register(
                lock_key.clone(),
                std::sync::Arc::new(LockFileCompute(path.clone())),
            );
            let _ = dev_graph.get(lock_key, &LockFileCompute(path.clone()));
            return true;
        }
        let key = oxide_fw_graph::NodeKey::new(path.to_string_lossy().as_ref());
        // (Re)register a compute that reads the file.
        struct FileCompute(std::path::PathBuf);
        impl oxide_fw_graph::Compute for FileCompute {
            fn compute(
                &self,
                ctx: &mut oxide_fw_graph::ComputeCtx,
            ) -> oxide_diagnostics::Result<oxide_fw_graph::NodeOutput> {
                let bytes = ctx.fs.read_file(&self.0).unwrap_or_default();
                Ok(oxide_fw_graph::NodeOutput {
                    hash: blake3::hash(&bytes),
                    artifact: None,
                    deps: Default::default(),
                })
            }
        }
        dev_graph.register(key.clone(), std::sync::Arc::new(FileCompute(path.clone())));
        // Seed the node and, on a re-read, let early cutoff decide.
        let _ = dev_graph.get(key.clone(), &FileCompute(path.clone()));
        !dev_graph
            .invalidate(&[oxide_fw_graph::Input::Node(key, blake3::hash(&[]))])
            .is_empty()
    }

    if once {
        let events = watcher
            .poll(&mut seen)
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        if content_source_changed(&rootp, &cfg, &events) {
            emit_content_search_indexes(&rootp, &cfg, &rootp.join("dist"), w)?;
        }
        // §15.2: `handleHotUpdate` gets every changed file and may return
        // `{reload: bool}`. A plugin that wants a full reload (a config
        // change, a new dependency) forces one; the default is the graph
        // invalidation below.
        let mut force_reload = false;
        if let Some(pool) = hmr_pool.as_ref() {
            for ev in &events {
                let p = match ev {
                    oxide_dev::FileEvent::Modified(p)
                    | oxide_dev::FileEvent::Created(p)
                    | oxide_dev::FileEvent::Removed(p) => p.clone(),
                };
                let results = call_js_plugin_hook(
                    &rootp,
                    &cfg,
                    pool,
                    "handleHotUpdate",
                    serde_json::json!({ "path": p.to_string_lossy() }),
                )
                .await;
                for r in results {
                    if r.get("reload").and_then(|b| b.as_bool()) == Some(true) {
                        force_reload = true;
                    }
                }
            }
        }
        if force_reload {
            // The dev loop's own reload path is the `server-module-swap`
            // event; a hook that asked for a reload gets it by invalidating
            // everything (an empty invalidate bumps the epoch).
            let _ = dev_graph.invalidate(&[]);
        }
        let any = events.iter().any(|e| graph_invalidated(&dev_graph, e));
        if any {
            invalidate_framework_modules(&hmr_pool, &rootp).await;
        }
        outln!(w, "dev ready — {} change(s) in first poll", events.len());
        let _ = started.shutdown.send(true);
        return Ok(0);
    }
    // Real dev loop (§13): poll, report, Ctrl-C to stop. Not exercised in tests.
    outln!(w, "dev ready — watching for changes (Ctrl-C to stop)");
    // PERF-11: idle pre-warm (§13.1). After `IDLE_WARM` with no change, issue
    // a request for each page route so the first real hit is warm. The set is
    // warmed once per quiet period; a change resets the timer.
    const IDLE_WARM: std::time::Duration = std::time::Duration::from_secs(2);
    let dev_addr = started.addr;
    let page_routes: Vec<String> = tree
        .entries
        .iter()
        .filter(|e| matches!(e.kind, oxide_router::RouteKind::Page))
        .filter(|e| {
            e.segments
                .iter()
                .all(|s| !s.contains(':') && !s.contains('*'))
        })
        .map(|e| {
            let path = if e.segments.is_empty() {
                "/".to_string()
            } else {
                format!("/{}", e.segments.join("/"))
            };
            path
        })
        .collect();
    let mut idle_since = std::time::Instant::now();
    let mut warmed = true; // nothing to do until the first quiet period
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let events_raw = watcher
            .poll(&mut seen)
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        // SPEC-14: writing `.oxide/hmr-generation` on every invalidate
        // triggers another watcher event — a pathological loop. The notify
        // backend sees every path under the root (SKIP_DIRS only applies
        // to the walk_root path, not the notify callback). Filter BEFORE
        // `invalidate_framework_modules` — filtering after it left the
        // invalidate→write→notify→invalidate loop running.
        let events: Vec<_> = events_raw
            .into_iter()
            .filter(|ev| {
                let p = match ev {
                    oxide_dev::FileEvent::Modified(p)
                    | oxide_dev::FileEvent::Created(p)
                    | oxide_dev::FileEvent::Removed(p) => p,
                };
                !p.components().any(|c| c.as_os_str() == ".oxide")
                    && !p.starts_with(rootp.join("dist"))
            })
            .collect();
        if !events.is_empty() {
            // §10.8: PPR shells are keyed by build id; dev's id is stable, so
            // a source edit must clear the store explicitly or the next
            // visitor is served a shell from the pre-edit render.
            if let Some(shells) = started.ppr_shells.as_ref() {
                if let Ok(mut store) = shells.lock() {
                    store.clear();
                }
            }
            // §16.5: rescan collections before the loaders for this batch run.
            // The write is atomic (`write_content_atomic`) and lands under
            // `dist`, which the filter above ignores to avoid a notify loop.
            if content_source_changed(&rootp, &cfg, &events) {
                emit_content_search_indexes(&rootp, &cfg, &rootp.join("dist"), w)?;
                if let Some(hub) = started.hmr.as_ref() {
                    hub.publish(oxide_dev::HmrMessage::FullReload {
                        reason: "content collection changed".to_string(),
                    });
                }
            }
            // SPEC-5E: a lockfile change is a resolver-epoch change. The
            // worker-generation bump makes every versioned() import pick up
            // a fresh URL, so the next resolve re-reads the lockfile. The
            // explicit log is the audit trail §5.5 asks for.
            let has_lockfile = events.iter().any(|ev| {
                let p = match ev {
                    oxide_dev::FileEvent::Modified(p)
                    | oxide_dev::FileEvent::Created(p)
                    | oxide_dev::FileEvent::Removed(p) => p,
                };
                matches!(
                    p.file_name().and_then(|n| n.to_str()),
                    Some("package-lock.json")
                        | Some("pnpm-lock.yaml")
                        | Some("yarn.lock")
                        | Some("bun.lockb")
                        | Some("Cargo.lock")
                )
            });
            if has_lockfile {
                tracing::info!("lockfile change — resolver epoch bump");
            }
            invalidate_framework_modules(&hmr_pool, &rootp).await;
            // §5.6: bump the stale generation BEFORE releasing the loop,
            // so any in-flight dev dispatch's `select!` (see
            // `wrap_dispatch_with_stale_guard` below) aborts and returns
            // a retryable 503 rather than a result computed from the
            // pre-change source. The bump is also what distinguishes a
            // "file changed" from "in-flight" — the dispatch reads the
            // generation at entry and the exit check is only needed if
            // the bump lands while the dispatch was already running.
            if let Some(hub) = started.hmr.as_ref() {
                let g = hub.bump_stale_generation();
                tracing::debug!(generation = g, "stale generation bumped");
            }
            // §15.2: run every plugin's `handleHotUpdate` for this batch.
            // The one-shot `dev` path already did this, but the long-running
            // `dev` watcher only invalidated the graph — a plugin returning
            // `{ reload: true }` had no effect after the initial build.
            let mut force_reload = false;
            if let Some(pool) = hmr_pool.as_ref() {
                for ev in &events {
                    let p = match ev {
                        oxide_dev::FileEvent::Modified(p)
                        | oxide_dev::FileEvent::Created(p)
                        | oxide_dev::FileEvent::Removed(p) => p.clone(),
                    };
                    let results = call_js_plugin_hook(
                        &rootp,
                        &cfg,
                        pool,
                        "handleHotUpdate",
                        serde_json::json!({ "path": p.to_string_lossy() }),
                    )
                    .await;
                    for r in results {
                        if r.get("reload").and_then(|b| b.as_bool()) == Some(true) {
                            force_reload = true;
                        }
                    }
                }
            }
            if force_reload {
                if let Some(hub) = started.hmr.as_ref() {
                    hub.publish(oxide_dev::HmrMessage::FullReload {
                        reason: "plugin handleHotUpdate requested a reload".to_string(),
                    });
                }
            }
            idle_since = std::time::Instant::now();
            warmed = false;
        }
        for ev in events {
            tracing::info!(?ev, "file change");
            outln!(w, "change: {ev:?}");
            // SPEC-14: emit the HMR message for this change.
            if let Some(hub) = started.hmr.as_ref() {
                use oxide_dev::{HmrBehavior, HmrMessage};
                let path = match &ev {
                    oxide_dev::FileEvent::Modified(p)
                    | oxide_dev::FileEvent::Created(p)
                    | oxide_dev::FileEvent::Removed(p) => p,
                };
                let pstr = path.to_string_lossy().to_string();
                let kind = oxide_dev::classify_path(&pstr);
                match oxide_dev::hmr_behavior(kind) {
                    HmrBehavior::StyleSwap => hub.publish(HmrMessage::CssUpdate {
                        path: pstr,
                        code: String::new(),
                    }),
                    HmrBehavior::ReloadData => {
                        // §13.3: a `page.server.ts` change re-runs the route's
                        // loader and publishes the route's URL PATH PATTERN
                        // (the loading-map key shape: `/blog/[slug]`) plus the
                        // fresh ODF payload bytes. The old message carried the
                        // FILE path + an empty payload — nothing the client
                        // could match or apply. The inline HMR client (injected
                        // with every dev page) fetches `/__data` for the live
                        // URL and re-dispatches the data event; `odf_b64` is
                        // its offline fallback. A non-page server module (e.g.
                        // `layout.server.ts`) keeps the legacy file-path
                        // message — `hmr_route_data_update` returns "" for it.
                        // WP-B: read through the swappable slot so a route
                        // added since the watcher started maps correctly. The
                        // outer `tree` binding is the one captured at cmd_dev
                        // entry and would miss any post-boot addition.
                        let live_tree = started.swappable_tree.load();
                        let (route_pattern, odf_b64) = hmr_route_data_update(
                            path,
                            &rootp,
                            &cfg,
                            &live_tree,
                            hmr_loader.as_ref(),
                        )
                        .await;
                        if route_pattern.is_empty() {
                            hub.publish(HmrMessage::RouteDataUpdate {
                                route: pstr,
                                odf_b64: String::new(),
                            });
                        } else {
                            hub.publish(HmrMessage::RouteDataUpdate {
                                route: route_pattern,
                                odf_b64,
                            });
                        }
                    }
                    HmrBehavior::AdapterHmr => hub.publish(HmrMessage::ModuleUpdate { path: pstr }),
                    HmrBehavior::ServerSwap => {
                        hub.publish(HmrMessage::ServerModuleSwap { path: pstr })
                    }
                    HmrBehavior::RestartSubsystems => {
                        // WP-B (deviation 3): a route-file change re-scans the
                        // route tree and swaps it in place, so a newly added
                        // route becomes reachable without restarting the dev
                        // server. A non-route config change keeps the previous
                        // behaviour (full reload only). A failed rescan keeps
                        // the current tree and falls back to a full reload so
                        // the client still sees something.
                        let routes_dir = rootp.join(&cfg.routes.dir);
                        if path.starts_with(&routes_dir) {
                            match scan_routes(&rootp, &cfg, hmr_pool.as_ref()).await {
                                Ok(fresh) => {
                                    let refined = refine_render_modes_from_ast(&rootp, &cfg, fresh);
                                    let new_tree = std::sync::Arc::new(refined);
                                    // Rebuild renderer component sources BEFORE
                                    // publishing the swap so the next request
                                    // resolves the component in the same tick
                                    // as the tree match. Without this, an added
                                    // route matches the tree but fails at
                                    // render with 'no component resolved'.
                                    {
                                        use oxide_router::RouteKind;
                                        let routes_root = rootp.join(&cfg.routes.dir);
                                        let entries: std::collections::BTreeMap<
                                            String,
                                            std::path::PathBuf,
                                        > = new_tree
                                            .entries
                                            .iter()
                                            .filter(|e| {
                                                matches!(
                                                    e.kind,
                                                    RouteKind::Page
                                                        | RouteKind::Layout
                                                        | RouteKind::Error
                                                        | RouteKind::NotFound
                                                        | RouteKind::Template
                                                )
                                            })
                                            .map(|e| {
                                                (
                                                    canonical_route_id(&e.id.0),
                                                    routes_root.join(&e.file),
                                                )
                                            })
                                            .collect();
                                        if !entries.is_empty() {
                                            let framework = cfg
                                                .framework
                                                .adapter
                                                .as_deref()
                                                .or(cfg.framework.name.as_deref());
                                            let sources = framework_shim::dev_component_sources(
                                                &rootp,
                                                framework,
                                                entries,
                                                hmr_tooling_pool.as_ref(),
                                            )
                                            .await;
                                            if let Ok(mut src) = hmr_component_source.write() {
                                                src.sources = sources;
                                            }
                                        }
                                    }
                                    started.swappable_tree.store(new_tree);
                                    tracing::info!(
                                        "dev: route tree + component sources rebuilt after {pstr} change"
                                    );
                                    hub.publish(HmrMessage::FullReload {
                                        reason: "route tree rebuilt".to_string(),
                                    });
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        "dev: route rescan failed; keeping current tree"
                                    );
                                    hub.publish(HmrMessage::FullReload {
                                        reason: format!("{pstr} changed (route rescan failed)"),
                                    });
                                }
                            }
                        } else {
                            hub.publish(HmrMessage::FullReload {
                                reason: format!("{pstr} changed"),
                            });
                        }
                    }
                }
            }
        }
        if !warmed && idle_since.elapsed() >= IDLE_WARM {
            warmed = true;
            for path in &page_routes {
                // A minimal GET; the response is discarded — the point is to
                // run the render path once so its worker + module cache warm.
                if let Ok(mut s) = tokio::net::TcpStream::connect(dev_addr).await {
                    use tokio::io::AsyncWriteExt as _;
                    let req = format!(
                        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
                    );
                    let _ = s.write_all(req.as_bytes()).await;
                    let mut buf = [0u8; 512];
                    use tokio::io::AsyncReadExt as _;
                    let _ = s.read(&mut buf).await;
                }
            }
            if !page_routes.is_empty() {
                outln!(w, "pre-warmed {} route(s) after idle", page_routes.len());
            }
        }
    }
}

/// `oxide build` (§6, §10.5): bundle client + ssr, prerender static routes, emit the
/// deploy artifact for the configured adapter (§18). `--static` forces a static
/// export target, which refuses dynamic routes without prerendered output (§10.5).

/// Compile the `oxide-core-wasm` crate to wasm32-unknown-unknown (§2 D18).
///
/// Best-effort: returns `Ok(None)` when the target isn't installed (the
/// Worker's JS fallback handles it), `Err` on a genuine compile failure.
/// Output goes to `<dist>/edge.wasm`, which is where the Cloudflare emit
/// branch looks for it.
fn compile_edge_wasm(_root: &Path, dist: &Path) -> anyhow::Result<Option<PathBuf>> {
    // WP-C2: the wasm-bindgen-processed `oxide-core-wasm` artifact is embedded
    // into the CLI binary at compile time (see `EDGE_WASM_BYTES` above and
    // `scripts/regen-edge-wasm.sh`). The previous implementation ran
    // `cargo build -p oxide-core-wasm` inside `root`, which only worked when
    // `root` was the oxide workspace itself. A real user project is a JS/TS
    // tree with no cargo workspace containing `oxide-core-wasm`, so that path
    // failed with a non-zero exit for every real project, silently shipping
    // the JS fallback (deviation 6, docs/deviations.md). Embedding the
    // artifact at CLI build time is exactly what the deviation named as the
    // fix.
    std::fs::create_dir_all(dist)?;
    let out = dist.join("edge.wasm");
    std::fs::write(&out, EDGE_WASM_BYTES)?;
    Ok(Some(out))
}

/// Emit a search index per configured content collection (§16.5). Writes
/// `<dist>/content/<name>.index.json` — a deterministic JSON document the
/// front end fetches lazily.
///
/// A collection whose directory is missing is reported and skipped; the build
/// itself succeeds (content often lives outside the main pipeline's blast
/// radius).
/// Write via a sibling temp file + rename, so a dev-loop rescan cannot
/// expose a half-written entries file to a concurrent `getCollection`.
fn write_content_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

fn emit_content_search_indexes(
    root: &Path,
    cfg: &oxide_config::OxideConfig,
    dist: &Path,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    if cfg.content.collections.is_empty() {
        return Ok(());
    }
    let out_dir = dist.join("content");
    std::fs::create_dir_all(&out_dir)?;
    // §16-C: collect each scanned collection so a typed `content.d.ts` can be
    // emitted once the loop finishes. Config carries no schema field yet, so
    // the schema is inferred from the frontmatter actually on disk.
    let mut scanned_collections: Vec<oxide_assets::Collection> = Vec::new();
    let mut inferred_schemas: Vec<(String, oxide_assets::NativeSchema)> = Vec::new();
    for coll in &cfg.content.collections {
        if coll.name.is_empty() || coll.dir.is_empty() {
            outln!(w, "content collection skipped: name and dir are required");
            continue;
        }
        let dir = root.join(&coll.dir);
        let cfg_in = oxide_assets::CollectionConfig {
            name: coll.name.clone(),
            dir: dir.clone(),
            schema: oxide_assets::SchemaKind::Unchecked,
            indexed_fields: coll.indexed_fields.clone(),
        };
        let scanned = match oxide_assets::scan_collection(&cfg_in) {
            Ok(c) => c,
            Err(e) => {
                outln!(
                    w,
                    "content collection `{}` skipped: {}",
                    coll.name,
                    e.0.message
                );
                continue;
            }
        };
        let idx = oxide_assets::build_search_index(&scanned, &coll.indexed_fields);
        let json = oxide_assets::search_index_to_json(&idx)
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        write_content_atomic(
            &out_dir.join(format!("{}.index.json", coll.name)),
            json.as_bytes(),
        )?;
        // §16.5: emit the full entries too. The search index is a
        // *projection* (terms + slugs + title/fields), too lossy for a
        // loader that needs the body. A page.server.ts's
        // `getCollection(name)` reads this file at request time.
        let entries_json = serde_json::json!({
            "name": scanned.name,
            "entries": scanned.entries.iter().map(|e| serde_json::json!({
                "slug": e.slug,
                "frontmatter": e.frontmatter,
                "body": e.body_markdown,
                "is_mdx": e.is_mdx,
            })).collect::<Vec<_>>(),
        });
        if let Ok(pretty) = serde_json::to_string_pretty(&entries_json) {
            // WP-r9-1: do not swallow the write error. A silent failure
            // here surfaces later as a misleading "no entries file" from
            // `getCollection` at request time; the build should fail loud.
            write_content_atomic(
                &out_dir.join(format!("{}.entries.json", coll.name)),
                pretty.as_bytes(),
            )
            .map_err(|e| anyhow::anyhow!("write content/{}.entries.json: {e}", coll.name))?;
        }
        outln!(
            w,
            "content index: {} ({} entries) -> dist/content/{}.index.json",
            coll.name,
            scanned.entries.len(),
            coll.name
        );
        inferred_schemas.push((
            coll.name.clone(),
            oxide_assets::content_typegen::infer_schema(&scanned),
        ));
        scanned_collections.push(scanned);
    }
    // Pair each scanned collection with its inferred schema.
    let typed_collections: Vec<oxide_assets::content_typegen::TypedCollection<'_>> =
        scanned_collections
            .iter()
            .enumerate()
            .map(|(i, c)| oxide_assets::content_typegen::TypedCollection {
                collection: c,
                schema: Some(&inferred_schemas[i].1),
            })
            .collect();
    // §16-C: emit `.oxide/types/content.d.ts`. A typegen failure is a
    // warning, not a build failure — the app still ships.
    let dts_path = root.join(".oxide/types/content.d.ts");
    match oxide_assets::content_typegen::write_content_dts(&typed_collections, &dts_path) {
        Ok(()) => outln!(
            w,
            "content typegen: {} collection(s) -> .oxide/types/content.d.ts",
            typed_collections.len()
        ),
        Err(e) => outln!(
            w,
            "warn: content.d.ts typegen failed ({} {}); stale",
            e.0.code,
            e.0.message
        ),
    }
    Ok(())
}

/// Derive the request URL from a prerender output path. `prerendered/index.html`
/// → `/`; `prerendered/blog/hello.html` → `/blog/hello`; `prerendered/foo.html`
/// → `/foo`. Used to give the framework renderer a plausible URL for a static
/// route (it may use it for canonical links, caching keys, etc.).
/// §16.3: write `sitemap.xml` and `robots.txt` into `dist/`.
///
/// The sitemap lists every URL-addressable page route. A dynamic route
/// (`[slug]`) is included only when the build resolved concrete paths for
/// it — the enumerations from `generateParams`, or the ones the
/// prerender pass already wrote under `dist/prerendered/`. Slots
/// (`@modal`) and API endpoints are not URLs a crawler visits and are
/// skipped.
///
/// Absolute URLs come from `config.siteUrl` (falling back to the
/// `OXIDE_SITE_URL` env var). Without one, the sitemap still emits — with
/// site-relative `<loc>` values — because a sitemap that is only missing
/// the origin is more useful than no sitemap, and robots.txt simply omits
/// the `Sitemap:` line rather than pointing at a broken URL.
fn emit_sitemap_and_robots(
    cfg: &oxide_config::OxideConfig,
    tree: &oxide_router::RouteTree,
    dist: &Path,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    // Origin: config wins, then env. Normalized to no trailing slash.
    let origin = cfg
        .site_url
        .clone()
        .or_else(|| std::env::var("OXIDE_SITE_URL").ok())
        .map(|s| s.trim_end_matches('/').to_string());

    // Absolute URL for a path, or the path itself when no origin is set.
    let abs = |path: &str| -> String {
        match &origin {
            Some(o) => format!("{o}{path}"),
            None => path.to_string(),
        }
    };

    // Collect the URL list: static routes from segments, plus the dynamic
    // routes that were actually prerendered. `dist/prerendered/**.html` is
    // the ground truth for "which URL exists".
    let mut urls: Vec<String> = Vec::new();
    for e in &tree.entries {
        if !matches!(e.kind, oxide_router::RouteKind::Page) {
            continue;
        }
        if e.slot.is_some() {
            continue;
        }
        // Only routes with no dynamic segment are unconditionally listed;
        // dynamic ones are covered by the prerendered-file walk below.
        let has_dynamic = e
            .segments
            .iter()
            .any(|s| s.contains(':') || s.contains('*') || s.contains('{'));
        if has_dynamic {
            continue;
        }
        let path = url_path_from_rel(&format!("prerendered/{}.html", e.segments.join("/")));
        urls.push(abs(&path));
    }

    // Prerendered files (including dynamic-route enumerations).
    let prerendered_root = dist.join("prerendered");
    let mut all_files: Vec<PathBuf> = Vec::new();
    walk_files(&prerendered_root, &mut all_files);
    for f in all_files {
        if f.extension().is_none_or(|x| x != "html") {
            continue;
        }
        let Ok(rel) = f.strip_prefix(&prerendered_root) else {
            continue;
        };
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let path = url_path_from_rel(&format!("prerendered/{rel_str}"));
        let u = abs(&path);
        if !urls.contains(&u) {
            urls.push(u);
        }
    }
    urls.sort();
    urls.dedup();

    // Write the two files. Both live at `dist/` root so the static-asset
    // store serves them at `/sitemap.xml` and `/robots.txt`.
    if !urls.is_empty() {
        let refs: Vec<&str> = urls.iter().map(String::as_str).collect();
        let xml = oxide_assets::sitemap_xml(&refs);
        std::fs::write(dist.join("sitemap.xml"), xml)
            .map_err(|e| anyhow::anyhow!("write sitemap.xml: {e}"))?;
    }
    let sitemap_url = origin.as_ref().map(|o| format!("{o}/sitemap.xml"));
    // `robots.txt` semantics: allow everything by default, which is what
    // an empty rule list means. A project that wants to disallow adds a
    // rule in the future `robots` config field; today the default is the
    // sensible one — you do not ship a sitemap you want ignored.
    let txt = oxide_assets::robots_txt(&[], sitemap_url.as_deref());
    std::fs::write(dist.join("robots.txt"), txt)
        .map_err(|e| anyhow::anyhow!("write robots.txt: {e}"))?;

    outln!(
        w,
        "sitemap: {} url(s){}{}",
        urls.len(),
        if origin.is_some() {
            ""
        } else {
            " (relative — set siteUrl for absolute URLs)"
        },
        if sitemap_url.is_some() {
            ""
        } else {
            " (no siteUrl; robots has no Sitemap: line)"
        },
    );
    Ok(())
}

/// §16.3: write `manifest.webmanifest` when the config declares a
/// `manifest` block. The emitted JSON is the standard web app manifest
/// shape — the fields a PWA needs to be installable.
///
/// `start_url` defaults to `/`. `name` defaults to `shortName` (and vice
/// versa), because a manifest with neither is rejected by some browsers.
/// An icon `type` is inferred from the extension when omitted.
/// §16.2: bundle every configured font family, emit the face files under
/// `dist/fonts/`, and write a `dist/fonts.css` carrying the `@font-face`
/// rules (including the metric-override fallback faces that eliminate
/// font-swap layout shift). A family whose files cannot be read is reported
/// and skipped; fonts are never load-bearing for a build.
fn emit_fonts(
    cfg: &oxide_config::OxideConfig,
    root: &Path,
    dist: &Path,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    use oxide_assets::{DefaultFontBackend, FontBackend as _, FontSpec};

    if cfg.fonts.families.is_empty() {
        return Ok(());
    }
    let fonts_dir = dist.join("fonts");
    std::fs::create_dir_all(&fonts_dir)
        .map_err(|e| anyhow::anyhow!("create {}: {e}", fonts_dir.display()))?;

    let mut all_css = String::new();
    let mut emitted = 0usize;
    for fam in &cfg.fonts.families {
        let local_paths: Vec<std::path::PathBuf> = fam
            .local_paths
            .iter()
            .map(|p| {
                let pb = std::path::PathBuf::from(p);
                if pb.is_absolute() { pb } else { root.join(pb) }
            })
            .collect();
        let spec = FontSpec {
            family: fam.family.clone(),
            provider: fam.provider.clone(),
            weights: fam.weights.clone(),
            styles: fam.styles.clone(),
            subset: if fam.provider.is_empty() {
                cfg.fonts.subset.clone()
            } else {
                cfg.fonts.subset.clone()
            },
            local_paths,
        };
        match DefaultFontBackend.bundle(&spec) {
            Ok(bundle) => {
                for (name, bytes) in &bundle.files {
                    std::fs::write(fonts_dir.join(name), bytes)
                        .map_err(|e| anyhow::anyhow!("write font {name}: {e}"))?;
                    emitted += 1;
                }
                all_css.push_str(&bundle.css);
                all_css.push('\n');
            }
            Err(e) => {
                eprintln!(
                    "warn: font family '{}' skipped: {} {}",
                    fam.family, e.0.code, e.0.message
                );
            }
        }
    }
    if emitted == 0 && all_css.is_empty() {
        return Ok(());
    }
    std::fs::write(dist.join("fonts.css"), &all_css)
        .map_err(|e| anyhow::anyhow!("write fonts.css: {e}"))?;
    outln!(w, "fonts: {emitted} file(s), dist/fonts.css");
    Ok(())
}

/// §4.4: lint each route's server module for top-level side effects that
/// break V8 snapshots — a `setTimeout`, `setInterval`, `new WebSocket` or
/// `new EventSource` at module scope holds a live handle at snapshot time.
/// The spec requires `oxide build` to report these with source locations so
/// a user can move the work behind a lazy initializer. Advisory: it never
/// fails the build, because the snapshot fallback (a V8 code cache) is a
/// valid degradation.
fn persist_diagnostics(root: &Path, diags: &[oxide_diagnostics::Diagnostic]) {
    let dir = root.join(".oxide");
    let _ = std::fs::create_dir_all(&dir);
    let json = serde_json::to_string_pretty(diags).unwrap_or_else(|_| "[]".to_string());
    let _ = std::fs::write(dir.join("last-build-diagnostics.json"), json);
}

fn emit_snapshot_lint(
    cfg: &oxide_config::OxideConfig,
    root: &Path,
    tree: &oxide_router::RouteTree,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    let routes_root = root.join(&cfg.routes.dir);
    let mut count = 0usize;
    // 19-E: each side effect becomes a real Diagnostic — location plus a
    // code frame — so the persisted diagnostics file (and the editor / MCP
    // tool reading it) shows the offending line, not just a stderr line.
    let mut diags: Vec<oxide_diagnostics::Diagnostic> = Vec::new();
    for e in &tree.entries {
        if !matches!(e.kind, oxide_router::RouteKind::Page) {
            continue;
        }
        let server = routes_root.join(e.file.with_file_name("page.server.ts"));
        let Ok(source) = std::fs::read_to_string(&server) else {
            continue;
        };
        let rel = server
            .strip_prefix(root)
            .unwrap_or(&server)
            .to_string_lossy()
            .to_string();
        for fx in oxide_jshost::lint_top_level_side_effects(&rel, &source) {
            outln!(
                w,
                "warn: snapshot side effect at {}:{} - {}",
                fx.path,
                fx.line,
                fx.message
            );
            let mut d = oxide_diagnostics::Diagnostic::warning(
                "SNP-SIDEEFFECT",
                format!("{}: {}", fx.path, fx.message),
            )
            .with_fix(
                "move the call behind a lazy initializer (a function called on first \
                 request) so the module body has no live timer/socket at snapshot time",
            );
            let line = fx.line as usize;
            if line > 0 {
                d = d.at(oxide_diagnostics::Location::file(&fx.path, fx.line, 1));
                let frame = oxide_diagnostics::render_code_frame(&source, line, None);
                if !frame.is_empty() {
                    d = d.with_code_frame(frame);
                }
            }
            diags.push(d);
            count += 1;
        }
    }
    if count > 0 {
        outln!(
            w,
            "snapshot: {count} top-level side effect(s) - move them behind a lazy initializer for a clean snapshot"
        );
        persist_diagnostics(root, &diags);
    }
    Ok(())
}

/// §4.4: build a V8 startup snapshot from the emitted server bundle and
/// write it to `<root>/.oxide/snapshot.bin`. `oxide start` with
/// `server.backend = "v8"` loads it, restoring the evaluated bundle heap
/// instead of re-evaluating it per worker.
///
/// A snapshot failure (a bundle that does not evaluate cleanly in V8) is
/// reported and skipped — the server still runs without one.
fn emit_snapshot_blob(root: &Path, server_out: &Path, w: Writer<'_>) -> anyhow::Result<()> {
    let entry = server_out.join("ssr/index.js");
    if !entry.is_file() {
        return Ok(());
    }
    // BUG-12: the old bundle walk is gone — the emitted snapshot is the
    // runtime bootstrap only (imported at runtime by `__oxideDispatch`);
    // only `entry` matters (its existence gates the snapshot at all).
    // `create_snapshot` builds its own Tokio runtime internally and drops it;
    // dropping a runtime from inside an async context panics ("Cannot drop a
    // runtime in a context where blocking is not allowed"). `cmd_build` is
    // async, so run the snapshot on a dedicated thread where the inner runtime
    // is dropped outside any async context.
    let built = std::thread::spawn(oxide_jshost::snapshot_build_runtime)
        .join()
        .unwrap_or_else(|_| {
            Err(oxide_diagnostics::OxideError(
                oxide_diagnostics::Diagnostic::error(
                    "SNP-PANIC",
                    "snapshot builder thread panicked",
                ),
            ))
        });

    match built {
        Ok(snap) if !snap.blob.is_empty() => {
            let dir = root.join(".oxide");
            std::fs::create_dir_all(&dir)
                .map_err(|e| anyhow::anyhow!("create {}: {e}", dir.display()))?;
            // BUG-12: versioned `AppSnapshot` envelope (not a raw blob), so
            // `from_snapshot_file` can reject stale/foreign snapshots with
            // `JSH-SNAPVER` instead of an opaque deno_core error at spawn.
            let env = serde_json::json!({
                "format": oxide_jshost::SNAPSHOT_FORMAT,
                "oxide_version": env!("CARGO_PKG_VERSION"),
                "source_hash": snap.source_hash,
                "blob_b64": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &snap.blob,
                ),
            });
            std::fs::write(
                dir.join("snapshot.bin"),
                serde_json::to_vec(&env)
                    .map_err(|e| anyhow::anyhow!("serialize snapshot envelope: {e}"))?,
            )
            .map_err(|e| anyhow::anyhow!("write snapshot.bin: {e}"))?;
            outln!(w, "snapshot: {} bytes", snap.blob.len());
        }
        Ok(snap) => {
            // No blob; the warnings say why. Not fatal.
            for warn in &snap.warnings {
                outln!(w, "snapshot: skipped ({warn})");
            }
        }
        Err(e) => {
            outln!(w, "snapshot: skipped ({} {})", e.0.code, e.0.message);
        }
    }
    Ok(())
}

/// §4.2: for the V8 backend, bundle `oxide.config.ts` (and the framework
/// adapter it imports) into a single self-contained ESM file. Bare V8 ESM has
/// no CommonJS interop — `react` is CJS and `import('react')` fails with
/// "does not provide an export named 'createElement'". Rolldown resolves CJS,
/// so bundling the config graph once at build gives V8 a file it can import.
///
/// Written to `<root>/.oxide/config.server.js`; `build_framework_stack` points
/// the V8 renderer at it. A no-op for the Node backend, which imports the
/// config directly.
fn emit_config_bundle(
    cfg: &oxide_config::OxideConfig,
    root: &Path,
    precompiled: BTreeMap<PathBuf, String>,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    // Always bundle the config — do NOT gate on `cfg.server.backend`. The
    // static extractor's view of a nested `server.backend` is not reliable
    // enough to gate a file another component needs: when extraction returned
    // `Node` for a config that says `v8`, this returned early, no
    // `config.server.js` was written, and the V8 prerender then failed to load
    // it. Bundling is cheap (~30 ms) and only the V8 backend reads the result.
    let Some(config_file) = find_config_file(root).ok() else {
        return Ok(());
    };
    // Bundle into a DEDICATED directory. Rolldown's `write()` may empty its
    // output dir, and `.oxide/` holds the island boot entry + the snapshot the
    // prerender pass reads — a shared dir raced those writes and the config
    // bundle vanished mid-build. A private dir is never touched by another
    // writer.
    let bundle_dir = root.join(".oxide/config-build");
    if bundle_dir.exists() {
        let _ = std::fs::remove_dir_all(&bundle_dir);
    }
    std::fs::create_dir_all(&bundle_dir)
        .map_err(|e| anyhow::anyhow!("create {}: {e}", bundle_dir.display()))?;

    let mut entries: BTreeMap<String, PathBuf> = BTreeMap::new();
    entries.insert("config".to_string(), config_file.clone());
    let opts = oxide_bundler::BundleOptions {
        // §4.6: the V8 sandbox is browser-like (Web globals, no Node built-ins).
        // Resolve with BROWSER conditions so `react-dom/server` picks
        // `server.browser.js` — the `server.node.js` build `require('stream')`
        // and fails with "require() is not available in the V8 runtime".
        env: oxide_bundler::BundleEnv::Client,
        entries,
        out_dir: bundle_dir.clone(),
        minify: false,
        sourcemap: false,
        externals: Vec::new(),
        define: BTreeMap::new(),
        tsconfig_paths: BTreeMap::new(),
        build_secret: "config-bundle".to_string(),
        // The config imports the SFCs directly (`import Home from './page.vue'`),
        // so the precompiled JS must ride along or rolldown parses the raw SFC.
        precompiled,
        jsx_import_source: None,
        transform_hook: None,
        resolve_hook: None,
        load_hook: None,
        render_chunk_hook: None,
        generate_bundle_hook: None,
        transform_html_hook: None,
        css_transform: None,
        // §4.2: one self-contained file. A code-split bundle needs its sibling
        // chunks copied alongside and its `__toESM` dynamic-import wrappers
        // resolved by the loader; a single inlined file is simpler and what the
        // bare V8 host wants.
        inline_dynamic_imports: true,
        // BUG-SOLID: resolve conditions must match the V8 loader
        // (`v8_loader.rs`). `worker` FIRST gives `@solidjs/web` its SSR build
        // (`dist/server.js`); the `browser`-only default gave it the CLIENT
        // build, whose `ssr()` returns undefined → a shell with an empty body.
        // `worker` still yields a browser-compatible `react-dom/server`.
        resolve_conditions: Some(vec![
            "worker".into(),
            "browser".into(),
            "import".into(),
            "module".into(),
            "default".into(),
        ]),
        virtual_modules: Default::default(),
    };
    use oxide_bundler::Bundler as _;
    let backend = oxide_bundler::RolldownBundler::new(root);
    match backend.bundle(&opts) {
        Ok(out) => {
            // The entry chunk is written as `<name>-<hash>.js`; rename it to
            // the stable name the loader expects.
            // Find the entry chunk's written file and copy it to the stable
            // name the V8 renderer imports. `copy` (not `rename`) so a
            // re-run finds the source again; a missing source is a warning,
            // not a build failure — the Node backend never needs this file.
            let entry_file = out
                .chunks
                .iter()
                .find(|c| c.kind == oxide_bundler::ChunkKind::Entry)
                .map(|c| c.file.clone());
            match entry_file {
                Some(file) => {
                    // The bundle is a small graph: the entry chunk imports
                    // sibling chunks (`react-*.js`, …). Copy ALL of them into
                    // `.oxide/` so the entry's relative imports resolve, then
                    // rename the entry to the stable name the V8 loader opens.
                    let mut copied = 0usize;
                    if let Ok(rd) = std::fs::read_dir(&bundle_dir) {
                        for e in rd.flatten() {
                            let src = e.path();
                            if src.extension().is_some_and(|x| x == "js") {
                                if let Some(name) = src.file_name() {
                                    let dst = root.join(".oxide").join(name);
                                    std::fs::copy(&src, &dst).map_err(|err| {
                                        anyhow::anyhow!(
                                            "copy {} -> {}: {err}",
                                            src.display(),
                                            dst.display()
                                        )
                                    })?;
                                    copied += 1;
                                }
                            }
                        }
                    }
                    let from = root.join(".oxide").join(&file);
                    let to = root.join(".oxide/config.server.js");
                    if from != to {
                        std::fs::copy(&from, &to).map_err(|e| {
                            anyhow::anyhow!("copy {} -> config.server.js: {e}", from.display())
                        })?;
                    }
                    outln!(
                        w,
                        "config bundle: .oxide/config.server.js (+{} chunks)",
                        copied.saturating_sub(1)
                    );
                }
                None => {
                    outln!(
                        w,
                        "config bundle: no entry chunk produced; V8 will load the raw config"
                    );
                }
            }
        }
        Err(e) => {
            outln!(w, "config bundle: skipped ({} {})", e.0.code, e.0.message);
        }
    }
    Ok(())
}

/// SPEC-21 / §16.6: emit `sw.js` when a `manifest` (a PWA block) is
/// configured. The worker precaches every client asset in the manifest
/// (cache-first) and serves an offline fallback for a navigation miss. The
/// registration snippet is emitted as `dist/pwa-register.js` so the page can
/// `<script src="/pwa-register.js">` it (or the CLI can inject it).
fn emit_pwa_worker(
    cfg: &oxide_config::OxideConfig,
    dist: &Path,
    client_manifest: &serde_json::Value,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    if cfg.manifest.is_none() {
        return Ok(());
    }
    if !cfg.sw.enabled {
        outln!(w, "pwa: sw disabled by config (sw.enabled = false)");
        return Ok(());
    }
    // Every asset path the client manifest names (entry + preload + css),
    // turned into a URL. Precache is the *shell* — the paths the app needs
    // to boot offline. Route-specific chunks are SWR'd on first use rather
    // than precached (a large site's full chunk set would blow the install
    // window).
    // §16.6 / WP-r9-8: resolve the offline fallback BEFORE building the
    // precache list, so an existing fallback route can be precached. The
    // old order validated the fallback but never added it — `caches.match`
    // missed on every offline navigation and degraded to the `/` shell.
    let offline_fallback = cfg
        .sw
        .offline_fallback
        .clone()
        .unwrap_or_else(|| "/offline".to_string());
    let fallback_exists = dist
        .join("prerendered")
        .join(offline_fallback.trim_start_matches('/'))
        .with_extension("html")
        .is_file()
        || offline_fallback == "/";
    let effective_fallback = if fallback_exists {
        &offline_fallback
    } else {
        "/"
    };

    let mut precache: Vec<String> = vec!["/".to_string()];
    if let Some(routes) = client_manifest.get("routes").and_then(|r| r.as_object()) {
        for row in routes.values() {
            if let Some(entry) = row.get("entry_chunk").and_then(|v| v.as_str()) {
                precache.push(format!("/client/{entry}"));
            }
            if let Some(pre) = row.get("preload").and_then(|v| v.as_array()) {
                for p in pre {
                    if let Some(s) = p.as_str() {
                        precache.push(format!("/client/{s}"));
                    }
                }
            }
            if let Some(css) = row.get("css").and_then(|v| v.as_array()) {
                for c in css {
                    if let Some(s) = c.as_str() {
                        precache.push(format!("/{s}"));
                    }
                }
            }
        }
    }
    if let Some(assets) = client_manifest.get("assets").and_then(|a| a.as_object()) {
        for orig in assets.keys() {
            precache.push(orig.clone());
        }
    }
    // §16.6 / WP-r9-8: precache the resolved fallback route. `install`
    // runs `caches.addAll(PRECACHE)`, so the offline navigation's
    // `caches.match(OFFLINE_FALLBACK)` only hits if the fallback is here.
    if effective_fallback != "/" {
        precache.push(effective_fallback.to_string());
    }
    precache.sort();
    precache.dedup();
    let list = precache
        .iter()
        .map(|s| format!("  {s:?},"))
        .collect::<Vec<_>>()
        .join("\n");

    let sw = format!(
        r#"// Oxide service worker (§16.6). Stale-while-revalidate for
// same-origin assets; network-first with an offline fallback for
// navigations. Cache name is scoped by build id so a rebuild is a new
// cache and a stale deploy self-evicts on activate.
const CACHE = 'oxide-{build}';
const PRECACHE = [
{list}
];
const OFFLINE_FALLBACK = {fallback:?};

self.addEventListener('install', (e) => {{
  e.waitUntil(
    caches.open(CACHE)
      .then((c) => c.addAll(PRECACHE))
      .then(() => self.skipWaiting())
      .catch(() => self.skipWaiting()),
  );
}});

self.addEventListener('activate', (e) => {{
  e.waitUntil(
    caches.keys()
      .then((ks) => Promise.all(ks.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
}});

self.addEventListener('fetch', (e) => {{
  const req = e.request;
  if (req.method !== 'GET') return;
  const url = new URL(req.url);
  // Only same-origin; a cross-origin request (an analytics beacon, a CDN
  // script) is out of scope for the app's cache.
  if (url.origin !== self.location.origin) return;

  if (req.mode === 'navigate') {{
    // Network-first: a fresh deploy must reach the browser. On failure,
    // the offline fallback (or the shell) — never the browser's own
    // offline page.
    e.respondWith(
      fetch(req).catch(() =>
        caches.match(OFFLINE_FALLBACK).then((hit) => hit || caches.match('/') || caches.match(req)),
      ),
    );
    return;
  }}

  // Stale-while-revalidate: serve the cached copy immediately, fetch a
  // fresh one in the background, and store it for the next request. A
  // cached asset is not "never updated" and a network hiccup is not
  // "unavailable" — the cache bridges both.
  e.respondWith(
    caches.match(req).then((cached) => {{
      const network = fetch(req)
        .then((resp) => {{
          if (resp && resp.status === 200 && resp.type === 'basic') {{
            const copy = resp.clone();
            caches.open(CACHE).then((c) => c.put(req, copy)).catch(() => {{}});
          }}
          return resp;
        }})
        .catch(() => cached);
      return cached || network;
    }}),
  );
}});
"#,
        build = client_manifest
            .get("build_id")
            .and_then(|v| v.as_str())
            .unwrap_or("dev"),
        fallback = effective_fallback,
    );
    std::fs::write(dist.join("sw.js"), sw).map_err(|e| anyhow::anyhow!("write sw.js: {e}"))?;

    let reg = "// Oxide PWA registration (§16.6).\n\
if ('serviceWorker' in navigator) {\n\
  window.addEventListener('load', () => navigator.serviceWorker.register('/sw.js').catch(() => {}));\n\
}\n";
    std::fs::write(dist.join("pwa-register.js"), reg)
        .map_err(|e| anyhow::anyhow!("write pwa-register.js: {e}"))?;
    outln!(
        w,
        "pwa: sw.js + pwa-register.js emitted (fallback: {effective_fallback})"
    );
    Ok(())
}

fn emit_web_manifest(
    cfg: &oxide_config::OxideConfig,
    dist: &Path,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    let Some(m) = &cfg.manifest else {
        return Ok(());
    };
    let mut json = serde_json::Map::new();

    // A browser needs at least one of name / short_name.
    let name = m.name.clone().or_else(|| m.short_name.clone());
    let short = m.short_name.clone().or_else(|| m.name.clone());
    if let Some(n) = name {
        json.insert("name".into(), serde_json::Value::String(n));
    }
    if let Some(s) = short {
        json.insert("short_name".into(), serde_json::Value::String(s));
    }
    json.insert(
        "start_url".into(),
        serde_json::Value::String(m.start_url.clone().unwrap_or_else(|| "/".into())),
    );
    if let Some(d) = &m.display {
        json.insert("display".into(), serde_json::Value::String(d.clone()));
    }
    if let Some(c) = &m.background_color {
        json.insert(
            "background_color".into(),
            serde_json::Value::String(c.clone()),
        );
    }
    if let Some(c) = &m.theme_color {
        json.insert("theme_color".into(), serde_json::Value::String(c.clone()));
    }
    let icons: Vec<serde_json::Value> = m
        .icons
        .iter()
        .map(|i| {
            let mut obj = serde_json::Map::new();
            obj.insert("src".into(), serde_json::Value::String(i.src.clone()));
            obj.insert("sizes".into(), serde_json::Value::String(i.sizes.clone()));
            let ty = i.r#type.clone().or_else(|| {
                i.src
                    .rsplit_once('.')
                    .map(|(_, ext)| format!("image/{ext}"))
            });
            if let Some(t) = ty {
                obj.insert("type".into(), serde_json::Value::String(t));
            }
            if let Some(p) = &i.purpose {
                obj.insert("purpose".into(), serde_json::Value::String(p.clone()));
            }
            serde_json::Value::Object(obj)
        })
        .collect();
    if !icons.is_empty() {
        json.insert("icons".into(), serde_json::Value::Array(icons));
    }

    let body = serde_json::to_string_pretty(&serde_json::Value::Object(json))
        .map_err(|e| anyhow::anyhow!("serialize manifest: {e}"))?;
    std::fs::write(dist.join("manifest.webmanifest"), format!("{body}\n"))
        .map_err(|e| anyhow::anyhow!("write manifest.webmanifest: {e}"))?;
    outln!(w, "manifest: manifest.webmanifest emitted");
    Ok(())
}

fn url_path_from_rel(rel: &str) -> String {
    let stripped = rel.strip_prefix("prerendered/").unwrap_or(rel);
    let stripped = stripped.strip_suffix(".html").unwrap_or(stripped);
    if stripped == "index" || stripped.is_empty() {
        "/".to_string()
    } else {
        format!("/{stripped}")
    }
}

/// Write one prerendered HTML file. When `page_renderer` is attached (§7),
/// the shell comes from the framework adapter; otherwise the neutral-IR
/// placeholder. Either way, `oxide_render::Injector` wraps the shell with
/// the CSP nonce + hydration bootstrap (§10.2 step 4).
/// §10.3: inject the boot script into an SSR'd shell. Placed right before
/// `</body>` so the parser sees the DOM the island scanner will walk.
/// Nonce-carrying if the shell already uses one; the boot chunk carries
/// only static imports and a `registerAdapter` call, so it is a module
/// script and inherits the document's script policy.
fn inject_island_boot_script(shell: &str, boot_url: &str, nonce: &str) -> String {
    // §17.1: the CSP uses `strict-dynamic`, which requires a nonce on every
    // script tag — `'self'` alone is not enough. The prerender pass has the
    // build-time nonce (derived from build_id) in scope; the live server
    // path derives a per-request nonce and stamps it the same way.
    let tag = format!(r#"<script type="module" src="/{boot_url}" nonce="{nonce}"></script>"#);
    if let Some(idx) = shell.rfind("</body>") {
        let mut out = String::with_capacity(shell.len() + tag.len() + 1);
        out.push_str(&shell[..idx]);
        out.push_str(&tag);
        out.push_str(&shell[idx..]);
        out
    } else {
        format!("{shell}\n{tag}")
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
/// P-20: one prerender target (a route + one `generateParams` entry).
/// §16.4 / O2: locale set to prerender for. A project without i18n
/// config returns a single empty-string locale; one with i18n returns
/// one entry per declared locale.
fn prerender_locales(cfg: &oxide_config::OxideConfig) -> Vec<String> {
    if cfg.i18n.locales.is_empty() || cfg.i18n.strategy == "none" {
        vec![String::new()]
    } else {
        cfg.i18n.locales.clone()
    }
}

/// §16.4 / O2: on-disk relative path and URL path for a locale's variant.
/// `base_rel` is a `.html`-suffixed path like `index.html` or
/// `nested/page.html`. Returns the path under `prerendered/` (which the
/// caller joins) and the request URL path.
fn prefixed_prerender_path(
    base_rel: &str,
    locale: &str,
    cfg: &oxide_config::OxideConfig,
) -> (String, String) {
    if locale.is_empty() {
        return (
            format!("prerendered/{base_rel}"),
            url_path_from_rel(base_rel),
        );
    }
    let should_prefix = match cfg.i18n.strategy.as_str() {
        "prefix-always" => true,
        "prefix-never" => false,
        _ => locale != cfg.i18n.default_locale, // prefix-except-default
    };
    if should_prefix {
        let url = url_path_from_rel(&format!("{locale}/{base_rel}"));
        (format!("prerendered/{locale}/{base_rel}"), url)
    } else {
        (
            format!("prerendered/{base_rel}"),
            url_path_from_rel(base_rel),
        )
    }
}

struct PrerenderTarget {
    rel: String,
    url_path: String,
    params: BTreeMap<String, String>,
    /// §16.4 / O2: the locale this target renders for. Empty when the
    /// project has no i18n config.
    locale: String,
}

/// P-20: render a route's `generateParams` targets with bounded parallelism.
///
/// Reuse (an unchanged source hash + an existing output file) is cheap and
/// stays synchronous. Only the targets that must actually render go through
/// the worker pool, at most `render_concurrency()` at a time. Results are
/// returned in target order, so seed manifests and graph records stay
/// deterministic regardless of completion order.
#[allow(clippy::too_many_arguments)]
async fn prerender_targets(
    dist: &Path,
    route_id: &str,
    pattern: &str,
    targets: Vec<PrerenderTarget>,
    nonce: &str,
    nonce_for: &str,
    page_renderer: Option<&std::sync::Arc<dyn oxide_server::PageRenderer>>,
    loader_runner: Option<&std::sync::Arc<dyn oxide_server::LoaderRunner>>,
    module_path: Option<&Path>,
    boot_script_url: Option<&str>,
    layout_ids: &[String],
    layout_modules: &[(String, std::path::PathBuf)],
    error_route: Option<&str>,
    plugin_head: &serde_json::Value,
    prev_prerenders: &std::collections::BTreeMap<
        (String, String),
        &crate::build_graph::RoutePrerenderRecord,
    >,
    route_src_hash: &str,
) -> anyhow::Result<(Vec<(PrerenderTarget, String, u16)>, usize)> {
    use futures::StreamExt as _;

    let mut results: Vec<Option<(String, u16)>> = (0..targets.len()).map(|_| None).collect();
    let mut to_render: Vec<usize> = Vec::new();
    let mut reused = 0usize;
    for (i, t) in targets.iter().enumerate() {
        match try_reuse_prerender(
            &dist.join(&t.rel),
            route_id,
            &t.url_path,
            route_src_hash,
            prev_prerenders,
        ) {
            Some((saved, status, old_nonce)) => {
                let fresh = saved.replace(&old_nonce, nonce);
                if std::fs::write(dist.join(&t.rel), &fresh).is_ok() {
                    reused += 1;
                    results[i] = Some((fresh, status));
                } else {
                    to_render.push(i);
                }
            }
            None => to_render.push(i),
        }
    }

    let concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8);
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
    let mut renders = futures::stream::iter(to_render.into_iter().map(|i| {
        let sem = std::sync::Arc::clone(&sem);
        let t = &targets[i];
        let dist = dist;
        let route_id = route_id;
        let pattern = pattern;
        let nonce_for = nonce_for;
        let page_renderer = page_renderer;
        let loader_runner = loader_runner;
        let module_path = module_path;
        let boot_script_url = boot_script_url;
        let layout_ids = layout_ids;
        let layout_modules = layout_modules;
        let error_route = error_route;
        let plugin_head = plugin_head;
        async move {
            let _permit = sem.acquire().await;
            let out = write_prerender(
                dist,
                &t.rel,
                route_id,
                pattern,
                &t.url_path,
                &t.params,
                &t.locale,
                nonce_for,
                page_renderer,
                loader_runner,
                module_path,
                boot_script_url,
                layout_ids,
                layout_modules,
                error_route,
                plugin_head,
            )
            .await;
            (i, out)
        }
    }))
    .buffer_unordered(concurrency);
    while let Some((i, out)) = renders.next().await {
        results[i] = Some(out?);
    }
    drop(renders);

    let mut ordered = Vec::with_capacity(targets.len());
    for (t, r) in targets.into_iter().zip(results.into_iter()) {
        let Some((html, status)) = r else {
            continue;
        };
        ordered.push((t, html, status));
    }
    Ok((ordered, reused))
}

async fn write_prerender(
    dist: &Path,
    rel: &str,
    route_id: &str,
    pattern: &str,
    url_path: &str,
    params: &BTreeMap<String, String>,
    locale: &str,
    nonce: &str,
    page_renderer: Option<&std::sync::Arc<dyn oxide_server::PageRenderer>>,
    loader_runner: Option<&std::sync::Arc<dyn oxide_server::LoaderRunner>>,
    module_path: Option<&Path>,
    boot_script_url: Option<&str>,
    // §8.2: layout chain for this route, outermost first. Populated by the
    // caller from `MatchInfo.layout_chain`; the adapter nests the page
    // inside them. Empty means "no layouts wrap this page".
    layouts: &[String],
    // §9.2: layout `(id, layout.server.ts path)` pairs, outermost first. The
    // prerender path runs each layout's `load()` before the page load and
    // threads `parent_data`, so a prerendered page's `await parent()` sees
    // the real layout data (mirrors the live and /__data paths).
    layout_module_paths: &[(String, std::path::PathBuf)],
    // §10.2 step 5: nearest ancestor `error.tsx` route id, if any. The
    // adapter renders it on an SSR throw. Mirrors the live-dispatch
    // resolution — an error thrown while prerendering has the same
    // fallback path.
    // STATUS NOTE (§10.2 step 5): a LIVE page that renders its error route
    // returns HTTP 500 (the §10.2 error-status contract marker). A
    // PRERENDERED error page is baked at build time; the marker in the
    // adapter's buffered output is captured below and carried through the
    // L0 seed manifest so `oxide start` serves it with the real status.
    // (Wave 3 fix — was a documented "baked 200" deviation.)
    error_route_id: Option<&str>,
    // §15.2: `renderHead` — the neutral head model a plugin contributed for
    // this route. Merged into the model the adapter receives. Empty for a
    // build with no `renderHead` plugin.
    plugin_head: &serde_json::Value,
) -> anyhow::Result<(String, u16)> {
    // §9.1: run the loader for this route (if the module exists and a
    // loader runner is attached) to obtain the data the render phase
    // receives. Prerendered static pages previously always wrote `{}`
    // into PageRenderInput.data; they now run the real `load()`.
    let mut data = serde_json::Value::Object(Default::default());
    // 16-G: the loader's `meta` contribution, if its server module exports
    // one. Merged into the render input's head below.
    let mut route_meta: Option<serde_json::Value> = None;
    // §7.1 / §10.2 step 5: resolved defer() values from the load. Prerender
    // used to discard these — the generated page had defer slots that never
    // resolved. They are injected as the same nonce'd script the live path
    // emits (below).
    let mut deferred_values: std::collections::BTreeMap<String, serde_json::Value> =
        std::collections::BTreeMap::new();
    // §9.2: run the layout chain (outer → inner) first, threading
    // `parent_data`, so the page's `await parent()` sees the real layout
    // data — the same sequence the live and /__data paths run.
    let mut parent_data: serde_json::Value = serde_json::Value::Null;
    if let Some(loader) = loader_runner {
        for (lid, lpath) in layout_module_paths {
            let mut linput = oxide_server::LoaderRunInput::new(
                lpath.to_string_lossy().to_string(),
                lid.clone(),
                url_path,
            );
            linput.params = params.clone();
            linput.parent_data = parent_data.clone();
            linput.locale = locale.to_string();
            // Build time has no request middleware; `locals` is `{}`.
            linput.locals = serde_json::json!({});
            match loader.run(linput).await {
                Ok(out) if out.status == 200 => {
                    parent_data = out.data;
                }
                Ok(out) => {
                    // A layout short-circuit during prerender is the same as
                    // a page short-circuit: write a diagnostic file.
                    let body = format!(
                        "<!doctype html><html><body>\
                         <h1>layout loader short-circuit: {}</h1><pre>{}</pre>\
                         </body></html>",
                        out.status, out.data
                    );
                    let dest = dist.join(rel);
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&dest, &body)?;
                    return Ok((body, out.status));
                }
                Err(e) => {
                    let body = format!(
                        "<!doctype html><html><body>\
                         <h1>layout loader error</h1><pre>{}: {}</pre>\
                         </body></html>",
                        oxide_render::escape_text(&e.0.code),
                        oxide_render::escape_text(&e.0.message)
                    );
                    let dest = dist.join(rel);
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&dest, &body)?;
                    return Ok((body, 500u16));
                }
            }
        }
    }

    if let (Some(loader), Some(mp)) = (loader_runner, module_path) {
        // Match the route-id normalization the renderer will do so user
        // loaders see the same string in `event.route`.
        let route_for_loader = route_id
            .strip_suffix("/page")
            .unwrap_or(route_id)
            .to_string();
        let mut input = oxide_server::LoaderRunInput::new(
            mp.to_string_lossy().to_string(),
            route_for_loader,
            url_path,
        );
        input.params = params.clone();
        // §9.2: the innermost layout's data — what `parent()` reads.
        input.parent_data = parent_data.clone();
        input.locale = locale.to_string();
        match loader.run(input).await {
            Ok(out) if out.status == 200 => {
                data = out.data;
                deferred_values = out.deferred;
                // 16-G: a `meta = defineMeta(fn)` export, evaluated by the
                // worker against this load's data.
                route_meta = out.meta;
            }
            Ok(out) => {
                // Non-200 during build: emit an explicit diagnostic HTML
                // file rather than a silently-empty page. The dev/serve
                // path short-circuits with a real HTTP response; at build
                // time the closest equivalent is a file with the error body.
                let body = format!(
                    "<!doctype html><html><body>\
                     <h1>loader short-circuit: {}</h1><pre>{}</pre>\
                     </body></html>",
                    out.status, out.data
                );
                let dest = dist.join(rel);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dest, &body)?;
                return Ok((body, out.status));
            }
            Err(e) => {
                // SEC-07: escape worker error text (HTML injection).
                let body = format!(
                    "<!doctype html><html><body>\
                     <h1>loader error</h1><pre>{}: {}</pre>\
                     </body></html>",
                    oxide_render::escape_text(&e.0.code),
                    oxide_render::escape_text(&e.0.message)
                );
                let dest = dist.join(rel);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dest, &body)?;
                return Ok((body, 500u16));
            }
        }
    }

    let shell = if let Some(renderer) = page_renderer {
        // §7: the adapter's `componentFor` is keyed on the route directory
        // (`routes/index`), not the router's file-role id (`routes/index/page`).
        // Strip the trailing `/page` so the lookup matches — the same
        // normalization build_dispatch performs for the request path.
        let route_for_render = route_id
            .strip_suffix("/page")
            .unwrap_or(route_id)
            .to_string();
        let mut input = oxide_server::PageRenderInput::new(route_for_render, url_path);
        for (k, v) in params {
            input.params.insert(k.clone(), v.clone());
        }
        input.data = data.clone();
        // §8.2: hand the layout chain to the adapter so it can nest the page.
        // The prerender path is separate from the live-dispatch path; both
        // populate this field from the same `MatchInfo.layout_chain`.
        input.layouts = layouts.to_vec();
        // §10.2 step 5: mirror the live-dispatch resolution on the prerender
        // path so a throw in `page.tsx` renders the project's `error.tsx`.
        input.error_route_id = error_route_id.map(str::to_string);
        // §16.4 / O2: the resolved locale this prerender targets. The
        // adapter forwards it to the render, so a page calling `t()` /
        // `getLocale()` knows its language even at build time.
        if !locale.is_empty() {
            input.locale = Some(locale.to_string());
        }
        // §7.1 / §9.2: hand the loader's resolved `defer()` values to the
        // adapter here too. The live-dispatch path populates this; the
        // prerender path collected the same values for the `<script>` tag
        // but never gave them to the render, so a prerendered deferred
        // boundary resolved against `{__deferred: id}` instead of the value.
        input.deferred = deferred_values.clone();
        // §17.1 / §7.1: the build-time nonce this prerender already stamps
        // on its own injected tags. The adapter needs it for the deferred
        // swapper; a prerendered page served under `strict-dynamic` must
        // carry the same nonce in both places.
        if !nonce.is_empty() {
            input.nonce = Some(nonce.to_string());
        }
        // §15.2: merge the plugin's `renderHead` contribution into the
        // neutral model. The adapter's own `head()` output and the plugin's
        // both land in `PageRenderInput.head`, which the adapter's head
        // merge walks.
        if plugin_head
            .as_object()
            .map(|o| {
                o.values()
                    .any(|v| v.as_array().is_some_and(|a| !a.is_empty()))
            })
            .unwrap_or(false)
        {
            input.head = plugin_head.clone();
        }
        // 16-G: the route's own `meta` (from `page.server.ts`) joins the
        // same neutral model. Route-level head is the more specific intent,
        // so its entries land after a plugin's.
        if let Some(meta) = route_meta.as_ref() {
            input.head = merge_neutral_head(input.head, normalize_route_meta(meta.clone()));
        }
        renderer
            .render(input)
            .await
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?
    } else {
        // No framework renderer attached (a project without a
        // `componentFor`). The placeholder is the whole document — but both
        // a plugin's `renderHead` and the route's own `meta` contribution
        // belong in it, so the head is built from their merged model rather
        // than hardcoded empty.
        let head_model = match route_meta.as_ref() {
            Some(meta) => {
                merge_neutral_head(plugin_head.clone(), normalize_route_meta(meta.clone()))
            }
            None => plugin_head.clone(),
        };
        format!(
            "<!doctype html><html><head>{}</head><body>\
             <main data-route=\"{pattern}\">placeholder</main></body></html>",
            neutral_head_to_html(&head_model),
        )
    };
    // §10.3: `<Island chunk="X">` emits `data-ox-chunk="X"` — a logical island
    // id, not a runtime URL. The client boot module we generated registers
    // each island under its name, so on the wire the id and the URL are the
    // same string. (If a future slice adds per-island chunks, the rewrite
    // hook lives here.)
    // §10.2 step 5 (prerender): the adapter's buffered output may carry
    // the same wire-level error-status markers the streaming path peeks
    // for. Detect them here (before the injector mutates `shell`) so the
    // status the adapter intended rides the L0 seed and is served as a
    // real 500 by `oxide start` — fixing what used to be a baked 200.
    let prerender_status: u16 = if str_signals_error(&shell) { 500 } else { 200 };
    let mut shell = shell;
    if let Some(url) = boot_script_url {
        shell = inject_island_boot_script(&shell, url, nonce);
    }

    let mut injector = oxide_render::Injector::new(
        oxide_render::HeadTags::default(),
        nonce.to_string(),
        route_id.to_string(),
    );
    // PERF-01 / §9.5: embed the page's loader data as an ODF payload so the
    // client's hydration bootstrap has it without a second request. Empty
    // data still encodes (an empty object), matching the live path.
    {
        let payload = serde_json::json!({
            "route": route_id.strip_suffix("/page").unwrap_or(route_id),
            "url": url_path,
            "params": params,
            "data": data,
            // §8.2: the layout chain the client must re-nest on hydration
            // (SSR rendered `<RootLayout><Page/></RootLayout>`; without
            // this, `hydrateRoot` sees the layout markup as unexpected and
            // throws React #418).
            "layouts": layouts,
        });
        let odf =
            oxide_odf::encode_to_bytes(&oxide_odf::Value::from_json(&payload)).unwrap_or_default();
        use base64::Engine as _;
        injector.odf_payload_b64 = Some(base64::engine::general_purpose::STANDARD.encode(&odf));
    }
    // §7.1 / §10.2 step 5: the resolved-deferred script, identical to the
    // live path's (deferred_tag in build_dispatch). Serialized through
    // serde_json (no untrusted concatenation) with `<` escaped so the
    // content cannot break out of the tag. Without this, a prerendered
    // page's DeferredClient slots stayed pending forever (the glue had a
    // payload but no resolution script).
    if !deferred_values.is_empty() {
        let json = serde_json::to_string(&deferred_values).unwrap_or_else(|_| "{}".into());
        // `</script>` and `<!--` inside a JSON string would terminate the
        // tag; `<` escaping is the OWASP-recommended fix and produces valid
        // JS (`\u003c`).
        let safe = json.replace('<', "\\u003c");
        injector.body_extra = format!(
            r#"<script type="application/oxide-deferred" nonce="{nonce}">{safe}</script>"#,
        );
    }
    let html = injector
        .inject_into_html(&shell)
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    let dest = dist.join(rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&dest, &html)?;
    // PERF-04 / §10.2 step 5: return (html, status) so the caller can seed
    // the L0 entry with the status the adapter's marker indicated.
    Ok((html, prerender_status))
}

/// PERF-01 / §8.4: answer a `/__data?route=<path>` request. Returns the
/// ODF-encoded `{route, url, params, data, deferred, locale}` envelope the
/// client router's `normalizeInput` expects (extra keys are ignored by
/// today's client; the resolved-deferred map + locale are what make
/// client-side navigation data MATCH the SSR render — §8.4 parity).
///
/// Parity contract: the dispatch re-matches `?route=` against the route
/// tree BEFORE this runs, so the middleware phase (nearest-ancestor
/// resolution + locals + headers) and the layout-load chain
/// (`parent_data`) that ran for a full SSR render run here too. What is
/// deliberately skipped is the RENDER — no adapter, no HTML, just loaders
/// + ODF encode. Success is 200 with `application/oxide+odf`; a loader
/// short-circuit (`error()`/`redirect()`) propagates its status/location.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
async fn route_data_response(
    req: &oxide_server::OxideRequest,
    loader_runner: Option<&std::sync::Arc<dyn oxide_server::LoaderRunner>>,
    module_path: Option<&std::path::Path>,
    route_id: Option<&str>,
    params: &std::collections::BTreeMap<String, String>,
    metrics: &std::sync::Arc<oxide_server::Metrics>,
    started: std::time::Instant,
    middleware_locals: &serde_json::Value,
    middleware_headers: &[(String, String)],
    layout_module_paths: &[(String, std::path::PathBuf)],
    locale: &str,
    // §16.6 / WP-D1: session codec for re-sealing a loader-mutated
    // session on the data path.
    session_codec: Option<&oxide_server::SessionCodec>,
    // B-17: the `/__data` branch also shows up in the inspector waterfall,
    // split into the middleware phase it shares with SSR and its own load.
    middleware_ms: f64,
    waterfall: Option<&oxide_dev::WaterfallRecorder>,
) -> oxide_server::OxideResponse {
    let request_id = req.request_id.clone();
    let resp = route_data_response_inner(
        req,
        loader_runner,
        module_path,
        route_id,
        params,
        metrics,
        started,
        middleware_locals,
        middleware_headers,
        layout_module_paths,
        locale,
        session_codec,
    )
    .await;
    if let Some(rec) = waterfall {
        let total = started.elapsed().as_secs_f64() * 1000.0;
        // §20 / WP-r9 nit: /__data rows carried `trace_id: null` because
        // this site used the trace-less `record` shim while SSR/action
        // rows use `record_with_trace`. `req.trace` is in scope here.
        rec.record_with_trace(
            &request_id,
            vec![
                ("middleware".to_string(), middleware_ms),
                ("load".to_string(), (total - middleware_ms).max(0.0)),
            ],
            req.trace.as_ref().map(|t| t.trace_id.clone()),
        );
    }
    resp
}

#[allow(clippy::too_many_arguments)]
async fn route_data_response_inner(
    req: &oxide_server::OxideRequest,
    loader_runner: Option<&std::sync::Arc<dyn oxide_server::LoaderRunner>>,
    module_path: Option<&std::path::Path>,
    route_id: Option<&str>,
    params: &std::collections::BTreeMap<String, String>,
    metrics: &std::sync::Arc<oxide_server::Metrics>,
    started: std::time::Instant,
    // §8.4 parity: the middleware phase ran before this branch; its locals
    // feed the loads and its headers ride the response.
    middleware_locals: &serde_json::Value,
    middleware_headers: &[(String, String)],
    // §9.2 parity: layout chain (outermost → innermost) for the data
    // route, each entry `(layout id, layout.server.ts path)`.
    layout_module_paths: &[(String, std::path::PathBuf)],
    // SPEC-10: the resolved request locale.
    locale: &str,
    // §16.6 / WP-D1: session codec (see the wrapper's doc).
    session_codec: Option<&oxide_server::SessionCodec>,
) -> oxide_server::OxideResponse {
    // `?route=<percent-encoded path>` (default `/`).
    let route_q = req
        .url
        .split_once('?')
        .map(|(_, q)| q)
        .and_then(|q| {
            q.split('&')
                .find_map(|kv| kv.strip_prefix("route=").map(oxide_server::percent_decode))
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/".to_string());

    // §9.2: layout loads first (outer → inner), threading `parent_data`
    // exactly like the render path. A layout short-circuit (redirect /
    // error) short-circuits the data response too — client navigation must
    // see what a full-page load would have seen.
    let mut parent_data: serde_json::Value = serde_json::Value::Null;
    // §16.6 / WP-D1: layout loaders share the middleware locals, so a layout
    // that mutates `event.session` must both persist the cookie and make the
    // new state visible to the page loader that runs next. Keep a local
    // copy rather than mutating the borrowed `middleware_locals`.
    let mut locals_for_load = middleware_locals.clone();
    let mut layout_session_cookie: Option<String> = None;
    if let Some(runner) = loader_runner {
        for (lid, lpath) in layout_module_paths {
            let mut linput = oxide_server::LoaderRunInput::new(
                lpath.to_string_lossy().to_string(),
                lid.clone(),
                &route_q,
            );
            linput.method = req.method.clone();
            // §20 / G2: thread the inbound W3C trace into the loader so its
            // server-side fetch calls join the same trace. The worker's
            // `traceparentFromPayload` reads `payload.trace.traceparent` —
            // a W3C wire-form string — so build the dict from the parsed
            // context rather than serializing the internal struct.
            linput.trace = req.trace.as_ref().and_then(|t| {
                // Re-emit the *parent* span as this loader's inbound
                // traceparent — the worker treats it as the upstream hop.
                // The `tracestate` rides along unchanged (W3C 3.3: a
                // participant that does not mutate it must forward it).
                t.to_traceparent(&t.parent_id).map(|tp| {
                    serde_json::json!({
                        "traceparent": tp,
                        "tracestate": t.tracestate,
                    })
                })
            });
            linput.headers = req.headers.as_ref().clone();
            linput.params = params.clone();
            linput.locals = locals_for_load.clone();
            linput.locale = locale.to_string();
            linput.parent_data = parent_data.clone();
            match runner.run(linput).await {
                Ok(out) if out.short_circuits() => {
                    let body = out
                        .data
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let mut resp = oxide_server::OxideResponse::new(out.status, body);
                    if let Some(loc) = out.location {
                        resp = resp.with_header("location", loc);
                    }
                    // §16.6 / WP-D1: a short-circuit (redirect / error)
                    // that also mutated `event.session` must persist it.
                    // §16.6 / WP-r10-1: the baseline must be the inbound
                    // session (post-middleware), not `None` — `None`
                    // re-issues a cookie on every untouched-session
                    // short-circuit. This is the same class of bug
                    // 119b9ec fixed in the /__data page arms; it missed
                    // the layout arm.
                    if let Some(cookie) = encode_session_cookie(
                        out.session.as_ref(),
                        locals_for_load.get("session"),
                        session_codec,
                    ) {
                        resp = resp.with_header("set-cookie", cookie);
                    }
                    metrics.observe("_data", started.elapsed(), out.status);
                    return resp;
                }
                Ok(out) => {
                    parent_data = out.data;
                    // §16.6 / WP-D1: persist a layout mutation on the
                    // non-short-circuit path too, mirroring SSR. The page
                    // loader then starts from the mutated session.
                    if let Some(cookie) = encode_session_cookie(
                        out.session.as_ref(),
                        locals_for_load.get("session"),
                        session_codec,
                    ) {
                        layout_session_cookie = Some(cookie);
                    }
                    if let Some(sess) = out.session {
                        locals_for_load["session"] = sess;
                    }
                }
                Err(e) => {
                    metrics.observe("_data", started.elapsed(), 500);
                    return oxide_server::OxideResponse::html(
                        500,
                        format!(
                            "layout loader error: {}",
                            oxide_render::escape_text(&e.0.message)
                        ),
                    );
                }
            }
        }
    }

    // §16.6 / WP-D1: session cookie for the data path
    // (attached to the envelope after the loads complete).
    let mut data_path_session_cookie: Option<String> = layout_session_cookie.clone();
    let mut data = serde_json::Value::Object(Default::default());
    // §7.1 / §9.2: resolved defer() values — the /__data envelope carries
    // them so the client router can resolve defer slots after a client-side
    // navigation (previously dropped: nav data diverged from SSR data).
    let mut deferred: std::collections::BTreeMap<String, serde_json::Value> =
        std::collections::BTreeMap::new();
    if let (Some(runner), Some(mp), Some(rid)) = (loader_runner, module_path, route_id) {
        let route_for_loader = rid.strip_suffix("/page").unwrap_or(rid).to_string();
        let mut input = oxide_server::LoaderRunInput::new(
            mp.to_string_lossy().to_string(),
            route_for_loader,
            &route_q,
        );
        input.method = req.method.clone();
        input.headers = req.headers.as_ref().clone();
        input.params = params.clone();
        input.locals = locals_for_load.clone();
        input.locale = locale.to_string();
        input.parent_data = parent_data;
        // §20 / G2: thread the inbound W3C trace so the loader's
        // server-side fetch calls join the request trace.
        input.trace = req.trace.as_ref().and_then(|t| {
            t.to_traceparent(&t.parent_id)
                .map(|tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}))
        });
        match runner.run(input).await {
            Ok(out) if out.short_circuits() => {
                let body = out
                    .data
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mut resp = oxide_server::OxideResponse::new(out.status, body);
                if let Some(loc) = out.location {
                    resp = resp.with_header("location", loc);
                }
                if let Some(cookie) = layout_session_cookie.clone() {
                    resp = resp.with_header("set-cookie", cookie);
                }
                // §16.6 / WP-D1: a short-circuit (redirect / error)
                // that also mutated `event.session` must persist it.
                if let Some(cookie) = encode_session_cookie(
                    out.session.as_ref(),
                    // §16.6 / WP-r9-3: pass the request's decoded session
                    // as `before`. `middleware_locals["session"]` is set at
                    // :6735 from the inbound cookie and refined by any
                    // middleware mutation, so it is the exact state the
                    // loader started with — an untouched loader matches it
                    // and no cookie is emitted. Passing `None` here meant
                    // every /__data request from a client holding a session
                    // re-issued the cookie.
                    locals_for_load.get("session"),
                    session_codec,
                ) {
                    resp = resp.with_header("set-cookie", cookie);
                }
                metrics.observe("_data", started.elapsed(), out.status);
                return resp;
            }
            Ok(out) => {
                data = out.data;
                deferred = out.deferred;
                // §16.6 / WP-D1: persist a session mutation on the
                // data path. The response is built below; the cookie
                // rides the envelope.
                if let Some(cookie) = encode_session_cookie(
                    out.session.as_ref(),
                    // §16.6 / WP-r9-3: pass the request's decoded session
                    // as `before`. `middleware_locals["session"]` is set at
                    // :6735 from the inbound cookie and refined by any
                    // middleware mutation, so it is the exact state the
                    // loader started with — an untouched loader matches it
                    // and no cookie is emitted. Passing `None` here meant
                    // every /__data request from a client holding a session
                    // re-issued the cookie.
                    locals_for_load.get("session"),
                    session_codec,
                ) {
                    data_path_session_cookie = Some(cookie);
                }
            }
            Err(e) => {
                metrics.observe("_data", started.elapsed(), 500);
                return oxide_server::OxideResponse::html(
                    500,
                    format!("loader error: {}", oxide_render::escape_text(&e.0.message)),
                );
            }
        }
    }

    let route_label = route_id
        .map(|r| r.strip_suffix("/page").unwrap_or(r).to_string())
        .unwrap_or_else(|| route_q.clone());
    // §8.2: emit the layout-id chain so the client re-render can nest the
    // new page inside the same layout components the SSR rendered it in.
    // Without this, a client nav hydrates a fresh bare `<Page>` against the
    // SSR `<RootLayout><Page/></RootLayout>` tree — the DOM shows the OLD
    // page because `realRoot.update` re-rendered the ORIGINAL Component
    // with new props instead of swapping in the new route.
    let layout_ids: Vec<String> = layout_module_paths
        .iter()
        .map(|(lid, _)| lid.clone())
        .collect();
    let payload = serde_json::json!({
        "route": route_label,
        "url": route_q,
        "params": params,
        "data": data,
        "deferred": deferred,
        "locale": locale,
        "layouts": layout_ids,
    });
    // §9.5 / D11: encode as ODF, the wire format the client decodes.
    let body =
        oxide_odf::encode_to_bytes(&oxide_odf::Value::from_json(&payload)).unwrap_or_default();
    metrics.observe("_data", started.elapsed(), 200);
    let mut resp = oxide_server::OxideResponse::new(200, body)
        .with_header("content-type", "application/oxide+odf");
    // §8.5 / §16.6 / WP-r9-3: middleware headers FIRST, then the loader's
    // session cookie — the loader is the later mutation and wins on the
    // wire (HTTP applies same-name Set-Cookies in arrival order). This
    // matches the render path's order (WP-D1's 663201a fix); the /__data
    // envelope was inverted, so a /__data request from a client with a
    // middleware-mutated session saw the OLD value reach the browser.
    for (k, v) in middleware_headers {
        resp = resp.with_header(k, v.clone());
    }
    if let Some(cookie) = data_path_session_cookie {
        resp = resp.with_header("set-cookie", cookie);
    }
    resp
}

/// §9.4 / BUG-05: read (or create) the per-project build secret.
///
/// Stored at `<root>/.oxide/secret` as 64 hex chars (32 bytes of entropy,
/// blake3-folded). A missing file is created once from the OS CSPRNG; every
/// subsequent build reads the same value, so server-fn ids and manifests are
/// byte-stable across builds. Deleting the file (or `oxide secrets rotate`)
/// rotates the secret — server-fn ids then change, which is the intended
/// effect of a rotation.
///
/// A malformed file (wrong length, non-hex) is a hard error: silently
/// regenerating would churn every fn id without telling anyone.
fn build_secret(rootp: &std::path::Path) -> anyhow::Result<String> {
    let key_file = rootp.join(".oxide/secret");
    if let Ok(existing) = std::fs::read_to_string(&key_file) {
        let s = existing.trim();
        if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(s.to_string());
        }
        anyhow::bail!(
            "{} is malformed (expected 64 hex chars); delete it or run `oxide secrets rotate`",
            key_file.display()
        );
    }
    // Create once. `/dev/urandom` never EOFs, so `read_exact` on a fixed
    // buffer is the bounded form (a plain `read` would block forever).
    use std::io::Read as _;
    let mut rnd = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut rnd))
        .map_err(|e| anyhow::anyhow!("cannot read /dev/urandom for the build secret: {e}"))?;
    let s = blake3::hash(&rnd).to_hex().to_string();
    std::fs::create_dir_all(rootp.join(".oxide"))
        .map_err(|e| anyhow::anyhow!("cannot create .oxide/: {e}"))?;
    std::fs::write(&key_file, &s)
        .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", key_file.display()))?;
    Ok(s)
}

/// §18 / P0-1b: locate the project's edge entry.
///
/// Cloudflare Workers convention: a single `src/edge.{ts,mjs,js,tsx,jsx}`
/// exports `default (req, env, ctx) => Response`. The typed extension is
/// preferred so a project with a stale `.js` and a real `.ts` gets the
/// typed one. Returns `None` when no entry exists -- the emitted
/// `edge_handler.js` catches the resulting `Cannot find module` and falls
/// through to the built-in JS path.
fn find_edge_entry(root: &Path) -> Option<PathBuf> {
    for ext in ["ts", "mjs", "js", "tsx", "jsx"] {
        let candidate = root.join("src").join("edge").with_extension(ext);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// §18 / P0-1b: bundle the edge entry into `dist/deploy/edge_bundle.js`.
///
/// The emitted `edge_handler.js` does `import("./edge_bundle.js")` and
/// calls its default export. This helper produces that file: a
/// single-entry bundle in `BundleEnv::Edge` (production defines,
/// minified, no sourcemap) written next to the handler.
///
/// **Chunking:** rolldown may emit shared chunks alongside the entry when
/// the entry imports common code. All chunks are written to
/// `dist/deploy/` (the whole directory deploys together for Cloudflare
/// Workers); only the entry chunk is renamed to `edge_bundle.js`. Its
/// relative imports of the sibling chunks resolve at their hashed names.
///
/// **Externals:** none. The edge handler must self-contain relative
/// imports; a bare npm import either resolves to the package's edge
/// entry or fails with BLD-RESOLVE at build time. `node:*` imports are
/// not specially handled -- a project using them gets the build-time
/// resolve error, which is the honest signal (Cloudflare Workers'
/// node-compat is a runtime flag the build cannot know about).
async fn bundle_edge_entry(
    root: &Path,
    dist: &Path,
    entry: &Path,
    css_transform: oxide_bundler::CssTransformHook,
    // §18 / D18: specifiers left external in the edge bundle. Empty by
    // default; a Worker declaring node-compat lists what it expects the
    // runtime to provide.
    edge_externals: Vec<String>,
    // §6.3 / §16.3: precompiled sources for SFCs and MDX reachable from
    // the edge entry — the same async tooling-pool pass the client and
    // SSR bundles use. Without it, the bundler refuses a
    // .vue/.svelte/.mdx module reachable from src/edge.ts (BLD-FRAMEWORK).
    precompiled: std::collections::BTreeMap<std::path::PathBuf, String>,
    w: Writer<'_>,
) -> anyhow::Result<Option<PathBuf>> {
    use oxide_bundler::Bundler as _;
    let out_dir = dist.join("deploy");
    std::fs::create_dir_all(&out_dir)?;

    let mut entries: BTreeMap<String, PathBuf> = BTreeMap::new();
    entries.insert("edge".to_string(), entry.to_path_buf());

    let opts = bundle_env_options(
        oxide_bundler::BundleEnv::Edge,
        entries,
        out_dir.clone(),
        precompiled,
        None,
        "edge".to_string(),
        None,
        css_transform,
        Vec::new(),
        false,
        true,
    );

    // `bundle_env_options` only populates externals for the SSR env; the edge
    // env is self-contained by default. Apply the declared set here rather
    // than widening that function's signature for one caller.
    let mut opts = opts;
    opts.externals = edge_externals;
    let bundler = oxide_bundler::RolldownBundler::new(root);
    let out = bundler
        .bundle(&opts)
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;

    let entry_chunks: Vec<_> = out
        .chunks
        .iter()
        .filter(|c| c.kind == oxide_bundler::ChunkKind::Entry)
        .collect();
    if entry_chunks.is_empty() {
        return Ok(None);
    }
    let entry_chunk = entry_chunks[0];
    let produced = out_dir.join(&entry_chunk.file);
    let target = out_dir.join("edge_bundle.js");
    if target.exists() {
        std::fs::remove_file(&target)?;
    }
    std::fs::rename(&produced, &target)?;
    let bytes = std::fs::metadata(&target)?.len();
    outln!(
        w,
        "edge bundle: {} ({} bytes, env=edge)",
        target.display(),
        bytes
    );
    Ok(Some(target))
}

async fn cmd_build(root: &str, r#static: bool, w: Writer<'_>) -> anyhow::Result<i32> {
    // Canonicalize once at entry. Downstream joins (routes scan, bundler,
    // write_prerender) all expect absolute paths so a nested `root.join`
    // cannot double the prefix — `PathBuf::join` returns the RHS unchanged
    // when it is absolute.
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    // §16.5 / WP-r9-1: thread the project root to worker
    // subprocesses so `getCollection` resolves `dist/content`
    // against the project, not the shell's cwd. Edition 2024:
    // set_var is process-global; this runs before any worker
    // thread spawns.
    unsafe {
        std::env::set_var("OXIDE_ROOT", rootp.to_string_lossy().as_ref());
    }
    let dist = rootp.join("dist");

    // §5.1 early cutoff. The fingerprint walk is only paid when the previous
    // output is present — a clean tree short-circuits before any I/O.
    // `OXIDE_FORCE_BUILD=1` bypasses the skip for a build that must run.
    if dist.join("client/client.manifest.json").is_file()
        && dist.join("server/server.manifest.json").is_file()
        && std::env::var_os("OXIDE_FORCE_BUILD").is_none()
    {
        let fp = build_cache::fingerprint(&rootp, "production", r#static);
        if build_cache::is_up_to_date(&rootp, &fp) {
            outln!(w, "build up to date (no tracked input changed) — skipping");
            return Ok(0);
        }
    }

    let mut cfg = load_config_strict(&rootp, "production")?;

    // §16.5 / WP-r9-1: emit content entries BEFORE the
    // prerender phase. A prerendered loader that calls
    // `getCollection` must see the current build's data — not
    // the previous build's (or nothing, on a first build).
    emit_content_search_indexes(&rootp, &cfg, &dist, w)?;
    // §15.2: `config` runs before the build uses the config; a plugin may
    // return a patch object. `configResolved` runs after, read-only. Both
    // need a pool, which the build has not built yet — so they fire through
    // a throwaway tooling pool created here, before the framework stack.
    // The pool is cheap (one Node child) and only built when a plugin
    // declares either hook.
    if plugin_declares_any(&rootp, &cfg, &["config", "configResolved"]) {
        // `tooling_pool` is a method on the `JsHost` trait, not an inherent
        // method — the trait must be in scope.
        use oxide_jshost::JsHost as _;
        let host = oxide_jshost::NodeHost::new(oxide_jshost::NodeHost::worker_script_default());
        if let Ok(pool) = host.tooling_pool(oxide_jshost::PoolConfig::default()) {
            let patches = fire_lifecycle_hook(
                &rootp,
                &cfg,
                Some(&pool),
                "config",
                serde_json::json!({ "config": cfg }),
            )
            .await;
            for patch in patches {
                let _ = apply_config_patch(&mut cfg, &patch, w);
            }
            let _ = fire_lifecycle_hook(
                &rootp,
                &cfg,
                Some(&pool),
                "configResolved",
                serde_json::json!({ "config": cfg }),
            )
            .await;
        }
    }
    // SPEC-15.1 / §15.2: fail at load for an unknown `kind:"native"` plugin.
    // Doing it in build means `oxide build` alone catches a typo'd plugin
    // name, not just `oxide check`.
    if let Err(e) = crate::native_plugins::parse_native_plugins(&cfg) {
        anyhow::bail!("{} {}", e.0.code, e.0.message);
    }
    let tree =
        refine_render_modes_from_ast(&rootp, &cfg, scan_routes_with_plugins(&rootp, &cfg).await?);
    let routes_root = rootp.join(&cfg.routes.dir);
    // §4.4: report top-level side effects that would break a V8 snapshot.
    emit_snapshot_lint(&cfg, &rootp, &tree, w)?;

    // §7 / §9.1: build the framework stack (renderer + loader). Renderer
    // is used for prerendering static routes; loader is present but not
    // consumed during build (build-time loader evaluation is a follow-up).
    let config_path_opt = find_config_file(&rootp).ok();
    // §6.2: the plugin transform handler, attached to the render pool so the
    // worker's compile_module applies it (reaching the prerender path).
    let transform_handler = framework_shim::build_transform_handler(&rootp, &cfg);
    // §11.2: `cache.handler` is honored inside the stack (see cmd_dev).
    // §15.2: `cacheHandler` — a plugin may select the cache backend. The
    // chosen config replaces `cfg.cache.handler` for the stack build below
    // (and for the L0 tier the server constructs from the same value).
    if let Ok(Some(chosen)) = resolve_cache_handler_via_plugin(&rootp, &cfg, w).await {
        cfg.cache.handler = chosen;
    }
    let stack = framework_shim::build_framework_stack(
        config_path_opt.as_deref(),
        cfg.server.backend,
        transform_handler,
        Some(&cfg.cache.handler),
    )
    .await;
    if let Some(name) = stack.renderer_name() {
        outln!(w, "framework renderer: {}", name);
    }

    // Entries: pages + endpoints (client and ssr share the entry set, §6.1).
    // Loading boundaries (§8.4) are also emitted as client entries so their
    // component is bundled and importable from the boot script.
    let mut entries: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut loading_entries: BTreeMap<String, PathBuf> = BTreeMap::new();
    for e in &tree.entries {
        if matches!(
            e.kind,
            oxide_router::RouteKind::Page
                | oxide_router::RouteKind::Endpoint
                | oxide_router::RouteKind::Loading
                // §8.2: layouts are client entries too — the client hydrate
                // rebuilds the server's `<RootLayout><Page/></RootLayout>`
                // tree via `componentFor`, which needs the layout chunk in
                // `client.manifest.json`. Without this the client can only
                // hydrate a bare page and React reports #418.
                | oxide_router::RouteKind::Layout
                // §6.4 / WP-A1: the generated SSR entry is the component
                // surface; it must include the error / not-found / template
                // modules the renderer resolves on failures and navigation.
                | oxide_router::RouteKind::Error
                | oxide_router::RouteKind::NotFound
                | oxide_router::RouteKind::Template
        ) {
            if matches!(e.kind, oxide_router::RouteKind::Loading) {
                loading_entries.insert(e.id.0.clone(), rootp.join(&cfg.routes.dir).join(&e.file));
            } else {
                entries.insert(e.id.0.clone(), rootp.join(&cfg.routes.dir).join(&e.file));
            }
        }
    }
    // WP-A2: the SSR entry and server manifest speak the canonical route id
    // (directory form). The client manifest keeps its historical `/page`
    // keys — the client router/manifest matcher consumes that shape and is
    // already manifest-driven; both surfaces are derived from this same
    // entry set, and the build test asserts their canonical key sets match.
    let server_entries: BTreeMap<String, PathBuf> = entries
        .iter()
        .map(|(id, p)| (canonical_route_id(id), p.clone()))
        .collect();
    let client_out = dist.join("client");
    let server_out = dist.join("server");
    // RolldownBundler handles every project shape: it lowers TS/JSX via
    // Rolldown's built-in Oxc pipeline, and its `PrecompiledPlugin` serves
    // the JS the tooling pool produced for any `.vue`/`.svelte` entries.
    // Real V3 sourcemaps come from `SourceMapType::File`.
    use oxide_bundler::Bundler as _;
    outln!(w, "bundler: rolldown");

    // §10.3 islands + §8.4 client runtime: generate a synthetic boot entry
    // whenever the project has PAGE routes (not only when it registers
    // islands). The boot entry is the delivery vehicle for the client
    // runtime's non-island glue: the client router (`bootWithRouter`),
    // the §8.4 loading map, the defer glue's install (`installBootRuntime`)
    // and the §16.6 PWA import. Gating it on islands left no-islands
    // projects with none of those — a dynamic page hydrated nothing and
    // navigated via full page loads. With zero islands the `modules` map
    // is simply empty and island hydration is a no-op scan.
    // It is added as a `__boot__` client entry only — the server bundle
    // stays island-free (server only needs to SSR the shell markup). The
    // emitted boot chunk URL is injected into every prerendered page as
    // `<script type="module" src="/<boot-url>">`.
    let mut client_entries = entries.clone();
    let boot_chunk_url: Option<String> = {
        let has_page_routes = tree
            .entries
            .iter()
            .any(|e| matches!(e.kind, oxide_router::RouteKind::Page));
        if !has_page_routes {
            None
        } else {
            let framework = cfg
                .framework
                .adapter
                .clone()
                .or_else(|| cfg.framework.name.clone())
                .unwrap_or_else(|| "react".to_string());
            // §8.4 loading boundaries: route directory → on-disk loading.tsx path.
            // `loading_entries` already has the route ids (from the entries scan);
            // the boot generator turns each id into a URL path key.
            let mut loading_dir_map: BTreeMap<String, PathBuf> = BTreeMap::new();
            for (route_id, path) in &loading_entries {
                loading_dir_map.insert(route_id.clone(), path.clone());
            }
            let boot_src = generate_island_boot_entry(
                &rootp,
                &framework,
                &cfg.framework.islands,
                &loading_dir_map,
                cfg.manifest.is_some(),
            );
            let boot_dir = rootp.join(".oxide");
            std::fs::create_dir_all(&boot_dir)?;
            let boot_path = boot_dir.join("island-boot.mjs");
            std::fs::write(&boot_path, boot_src)?;
            client_entries.insert("__boot__".to_string(), boot_path);
            if cfg.framework.islands.is_empty() {
                outln!(
                    w,
                    "boot entry: __boot__ (client router + loading + defer glue, no islands)"
                );
            } else {
                outln!(
                    w,
                    "islands: {} registered (boot entry __boot__)",
                    cfg.framework.islands.len()
                );
            }
            Some("__boot__".to_string())
        }
    };

    // §6.3 precompile pass: run Vue / Svelte SFCs through the tooling pool
    // once per env, then hand the emitted JS to the bundler via
    // `BundleOptions::precompiled`. When the pool is unavailable (no Node,
    // or a project with no SFCs) the map is empty and the bundler falls back
    // to its own Oxc lowering for TS/TSX.
    // §6.3: the framework name decides which non-Oxc compiler runs.
    // Solid needs `babel-preset-solid`; Vue and Svelte need their SFC
    // compilers; React and everything else stay on the Oxc path.
    let framework_name = cfg
        .framework
        .adapter
        .as_deref()
        .or(cfg.framework.name.as_deref());
    // §6.3 / §5.4: the precompile pass is a pure function of
    // (source, framework, mode, compiler version) and is cached in the Tier-B
    // CAS, so an unchanged SFC is not recompiled on the next build.
    // §5.4: a configured `build.remoteCache` makes the precompile cache
    // read-through/write-through against a remote HTTP cache. A missing/failed
    // remote degrades to local-only (open_with_remote falls back).
    // SPEC-5D: gate remote write on the current git branch. Reads `GIT_BRANCH`
    // (falls back to `GITHUB_REF_NAME`, `CI_COMMIT_REF_NAME`) and checks against
    // `build.writeBranches`. When the branch is not trusted, the remote cache is
    // NOT attached — the build uses the local cache only. This prevents an
    // untrusted push from poisoning the shared cache with a bad artifact.
    let branch = std::env::var("GIT_BRANCH")
        .or_else(|_| std::env::var("GITHUB_REF_NAME"))
        .or_else(|_| std::env::var("CI_COMMIT_REF_NAME"))
        .ok();
    let trusted_branch = match (&cfg.build.remote_cache, &cfg.build.write_branches, &branch) {
        (Some(_), Some(list), Some(b)) => list.iter().any(|t| t == b),
        (Some(_), None, _) => {
            // No explicit list — default trusted: main + release/*.
            matches!(branch.as_deref(), Some("main") | Some("master"))
                || branch
                    .as_deref()
                    .map(|b| b.starts_with("release/"))
                    .unwrap_or(false)
        }
        (Some(_), _, None) => {
            eprintln!(
                "warn: build.remoteCache configured but no GIT_BRANCH / GITHUB_REF_NAME \
                 env var; not attaching remote (writeBranches policy)"
            );
            false
        }
        _ => false,
    };
    let precompile_cache = {
        let local = precompile_cache::PrecompileCache::open_soft(&rootp);
        if trusted_branch {
            match (&cfg.build.remote_cache, local) {
                (Some(url), Some(c)) => {
                    let remote: std::sync::Arc<dyn oxide_fw_graph::RemoteCache> =
                        std::sync::Arc::new(oxide_fw_graph::HttpRemoteCache::new(url.clone()));
                    match precompile_cache::PrecompileCache::open_with_remote(&rootp, remote) {
                        Ok(rc) => Some(rc),
                        Err(e) => {
                            eprintln!(
                                "warn: remote build cache unavailable ({}); using local only",
                                e.0.message
                            );
                            Some(c)
                        }
                    }
                }
                (_, l) => l,
            }
        } else {
            if cfg.build.remote_cache.is_some() {
                eprintln!(
                    "cache: remote attached only for trusted branches (writeBranches); \
                     current branch = {:?}, using local only",
                    branch
                );
            }
            local
        }
    };
    let tool_version = env!("CARGO_PKG_VERSION");
    let client_precompiled = precompile::precompile_entries(
        stack.tooling_pool.as_ref(),
        &entries,
        "dom",
        framework_name,
        precompile_cache.as_ref(),
        tool_version,
    )
    .await;
    let server_precompiled = precompile::precompile_entries(
        stack.tooling_pool.as_ref(),
        &server_entries,
        "ssr",
        framework_name,
        precompile_cache.as_ref(),
        tool_version,
    )
    .await;
    // §4.2: bundle the config graph to ESM for the V8 backend, NOW that the
    // SFCs are precompiled (the config imports them directly). V8 cannot import
    // the raw `.ts` config — a CJS framework runtime has no V8 ESM interop.
    {
        let mut merged: BTreeMap<PathBuf, String> = client_precompiled.clone();
        merged.extend(server_precompiled.clone());
        emit_config_bundle(&cfg, &rootp, merged, w)?;
    }
    if !client_precompiled.is_empty() {
        outln!(
            w,
            "precompiled {} SFC(s) for client",
            client_precompiled.len()
        );
    }
    if !server_precompiled.is_empty() {
        outln!(
            w,
            "precompiled {} SFC(s) for server",
            server_precompiled.len()
        );
    }

    // §6.3: JSX runtime, resolved from the framework the config declares.
    // Passed to both envs so a stray `.tsx` on either side lowers against
    // the right runtime; today this only matters for Solid.
    let jsx_source = jsx_import_source_for(
        cfg.framework
            .adapter
            .as_deref()
            .or(cfg.framework.name.as_deref())
            .unwrap_or("react"),
    );
    // §9.4 / BUG-05: a **stable** per-project secret for both environments.
    // The server-fn id is `blake3(file, export, secret)`; if the secret
    // changed per build, every build would produce different fn ids and the
    // manifests could never be byte-stable (§6.4) — chunk caching would be
    // structurally impossible. The secret lives in `.oxide/secret` (rotated
    // only on demand via `oxide secrets rotate`), never in the repo.
    let build_secret = build_secret(&rootp)?;
    // The env fingerprint needs the secret too, but `build_secret` is
    // moved into `server_opts` below. Clone once at creation.
    let build_secret_for_fp = build_secret.clone();

    // §15.1: a WASM component plugin's transform hook, if the config
    // declares one. Built once, shared by both envs.
    // SPEC-17: the JS plugin bridge (kind: js) composes with the WASM hook
    // (kind: wasm). JS runs first (it sees original source), then WASM.
    let wasm_hook = build_transform_hook(&rootp, &cfg);
    let js_hook = build_js_plugin_hook(&rootp, &cfg, stack.tooling_pool.as_ref());
    let transform_hook: Option<oxide_bundler::TransformHook> = match (js_hook, wasm_hook) {
        (Some(js), Some(wasm)) => Some(oxide_bundler::TransformHook::new(move |id, src| {
            let after_js = js.call(id, src)?;
            wasm.call(id, &after_js)
        })),
        (Some(js), None) => Some(js),
        (None, Some(wasm)) => Some(wasm),
        (None, None) => None,
    };
    // §6.6: Lightning CSS for every emitted stylesheet.
    let css_transform = build_css_transform_hook(&cfg);

    let client_opts = bundle_env_options(
        oxide_bundler::BundleEnv::Client,
        client_entries,
        client_out.clone(),
        client_precompiled,
        jsx_source.clone(),
        build_secret.clone(),
        transform_hook.clone(),
        css_transform.clone(),
        Vec::new(),
        cfg.build.sourcemap != "off",
        true, // production build minifies
    );
    let server_opts = bundle_env_options(
        oxide_bundler::BundleEnv::Ssr,
        server_entries.clone(),
        server_out.clone(),
        server_precompiled.clone(),
        jsx_source,
        build_secret,
        transform_hook,
        css_transform.clone(),
        cfg.build.ssr_externals.clone(),
        cfg.build.sourcemap != "off",
        true, // production build minifies
    );
    // §15.2: `buildStart` fires once, before the first module is bundled.
    // `buildEnd` fires after the last, with a summary. Both are
    // notifications — a failure is logged by the dispatcher and skipped.
    if plugin_declares_any(&rootp, &cfg, &["buildStart"]) {
        let _ = fire_lifecycle_hook(
            &rootp,
            &cfg,
            stack.tooling_pool.as_ref(),
            "buildStart",
            serde_json::json!({ "root": rootp.to_string_lossy() }),
        )
        .await;
    }
    let rb = oxide_bundler::RolldownBundler::new(&rootp);
    // SPEC-16.6 / §16-A: scan every route entry for `.svg?react` imports
    // and register a generated React component for each unique file. The
    // import `./icon.svg?react` then resolves to a synthetic path whose
    // content is the generated TSX, which the bundler reads from
    // `precompiled` rather than disk.
    let mut client_opts = client_opts;
    // §15-E: JS-plugin resolveId / load hooks. Build once; both envs share
    // the closures (`ResolveHook`/`LoadHook` are `Arc<dyn Fn>` — cheap to
    // clone; worker discovery is lazy on first call). Assigned by field on
    // `BundleOptions` — no signature change to `bundle_env_options`, so this
    // composes with any in-flight edit elsewhere in that function.
    let js_resolve_hook = build_js_plugin_resolve_hook(&rootp, &cfg, stack.tooling_pool.as_ref());
    let js_load_hook = build_js_plugin_load_hook(&rootp, &cfg, stack.tooling_pool.as_ref());
    client_opts.resolve_hook = js_resolve_hook.clone();
    client_opts.load_hook = js_load_hook.clone();
    let mut server_opts = server_opts;
    server_opts.resolve_hook = js_resolve_hook;
    server_opts.load_hook = js_load_hook;
    // §15-B: the three post-emit / HTML hooks. renderChunk and generateBundle
    // run inside the bundler for both envs; transformHtml runs on each
    // prerendered document.
    let js_render_chunk_hook =
        build_js_plugin_render_chunk_hook(&rootp, &cfg, stack.tooling_pool.as_ref());
    let js_generate_bundle_hook =
        build_js_plugin_generate_bundle_hook(&rootp, &cfg, stack.tooling_pool.as_ref());
    let js_transform_html_hook =
        build_js_plugin_transform_html_hook(&rootp, &cfg, stack.tooling_pool.as_ref());
    client_opts.render_chunk_hook = js_render_chunk_hook.clone();
    client_opts.generate_bundle_hook = js_generate_bundle_hook.clone();
    server_opts.render_chunk_hook = js_render_chunk_hook;
    server_opts.generate_bundle_hook = js_generate_bundle_hook;
    let transform_html_hook = js_transform_html_hook;
    if client_opts.virtual_modules.is_empty() {
        let mut seen: std::collections::BTreeMap<PathBuf, String> =
            std::collections::BTreeMap::new();
        for entry in client_opts.entries.values() {
            // Scan the entry module (and transitively? No — scan every route
            // file referenced by the manifest's `modules` field is better,
            // but a simpler first cut: scan just the immediate entry).
            let Ok(source) = std::fs::read_to_string(entry) else {
                continue;
            };
            for line in source.lines() {
                // Match `import X from './foo.svg?react'` (any quote style).
                if let Some(spec) = extract_svg_react_spec(line) {
                    // Resolve the specifier relative to the entry.
                    let base = entry.parent().unwrap_or(std::path::Path::new("."));
                    let svg_path = base.join(spec.trim_end_matches("?react"));
                    if seen.contains_key(&svg_path) {
                        continue;
                    }
                    let Ok(raw) = std::fs::read_to_string(&svg_path) else {
                        continue;
                    };
                    let display = svg_path.to_string_lossy().to_string();
                    match oxide_bundler::svg_react::svg_to_react_component(&raw, &display) {
                        Ok(tsx) => {
                            // Synthetic path: same dir as the SVG, with
                            // `.react.tsx` appended so the resolver's `.tsx`
                            // extension handler picks it up.
                            let synthetic = svg_path.with_extension("svg.react.tsx");
                            // Register in BOTH `virtual_modules` (for
                            // resolution) and `precompiled` (so the bundler
                            // reads the generated TSX, not the (nonexistent)
                            // disk file).
                            client_opts.precompiled.insert(synthetic.clone(), tsx);
                            seen.insert(svg_path, synthetic.to_string_lossy().to_string());
                        }
                        Err(e) => {
                            tracing::warn!(code = %e.0.code, msg = %e.0.message, "svg?react: generate failed");
                        }
                    }
                }
            }
        }
        // SPEC-16-A: scan for `img?w=400,800,1200` imports and emit a
        // virtual metadata module exporting `{ src, srcset, sizes, width,
        // height }`. The URLs point at the existing `/_oxide/image` endpoint
        // (runtime resize), so the variant modules are pure metadata — no
        // build-time rasterization cost. A user's `<img>` becomes:
        //
        //   import hero from './hero.png?w=400,800,1200'
        //   <img src={hero.src} srcset={hero.srcset} width={hero.width}
        //        height={hero.height} sizes="100vw" />
        let mut seen_imgs: std::collections::BTreeMap<PathBuf, (String, String)> =
            std::collections::BTreeMap::new(); // abs-img-path → (synthetic-path, module-source)
        for entry in client_opts.entries.values() {
            let Ok(source) = std::fs::read_to_string(entry) else {
                continue;
            };
            let base = entry.parent().unwrap_or(std::path::Path::new("."));
            for line in source.lines() {
                if let Some((spec, widths)) = extract_img_w_spec(line) {
                    let img_path = base.join(spec.split('?').next().unwrap_or(spec));
                    if seen_imgs.contains_key(&img_path) {
                        continue;
                    }
                    let Ok(dims) = image::image_dimensions(&img_path) else {
                        continue;
                    };
                    let (w0, h0) = dims;
                    // The runtime URL. The `src` attribute uses the max width;
                    // `srcset` has one entry per requested width.
                    let rel_url = spec.split('?').next().unwrap_or(spec);
                    // Strip leading `./` from the relative URL.
                    let rel_url = rel_url.strip_prefix("./").unwrap_or(rel_url);
                    let url_base = format!("/_oxide/image?src={rel_url}");
                    let srcset = oxide_assets::srcset(&widths, &url_base);
                    let max_w = widths.iter().copied().max().unwrap_or(w0);
                    let src = format!("{url_base}&w={max_w}");
                    let module = format!(
                        "export const src = {};\nexport const srcset = {};\nexport const width = {};\nexport const height = {};\n",
                        serde_json::to_string(&src).unwrap_or_else(|_| "\"\"".into()),
                        serde_json::to_string(&srcset).unwrap_or_else(|_| "\"\"".into()),
                        w0,
                        h0,
                    );
                    // Synthetic path: `.img.js` extension so the bundler
                    // treats it as a normal module (no JSX/TSX precompile
                    // needed).
                    let h = {
                        let mut hh = blake3::Hasher::new();
                        hh.update(img_path.to_string_lossy().as_bytes());
                        hh.update(&[0u8]);
                        for w in &widths {
                            hh.update(&w.to_le_bytes());
                        }
                        hh.finalize().to_hex()[..12].to_string()
                    };
                    let synthetic = img_path.with_extension(format!("img.{h}.js"));
                    client_opts
                        .precompiled
                        .insert(synthetic.clone(), module.clone());
                    seen_imgs.insert(
                        img_path.clone(),
                        (synthetic.to_string_lossy().to_string(), module),
                    );
                    let _ = (w0, h0);
                }
            }
        }

        // Second pass for img?w=: register every specifier form we saw.
        for entry in client_opts.entries.values() {
            let Ok(source) = std::fs::read_to_string(entry) else {
                continue;
            };
            let base = entry.parent().unwrap_or(std::path::Path::new("."));
            for line in source.lines() {
                if let Some((spec, _widths)) = extract_img_w_spec(line) {
                    let img_path = base.join(spec.split('?').next().unwrap_or(spec));
                    if let Some((synth_str, module_src)) = seen_imgs.get(&img_path) {
                        let synth_path = PathBuf::from(synth_str);
                        client_opts
                            .virtual_modules
                            .insert(spec.to_string(), (synth_path, module_src.clone()));
                    }
                }
            }
        }

        // Build the virtual_modules map keyed by the ORIGINAL specifier so
        // the resolve hook (which sees `./icon.svg?react`) finds them.
        // We iterate `seen` here to build specifier keys. `seen` is
        // svg_path → synthetic_path; the specifier is `./<basename>?react`
        // relative to each entry. Since multiple entries can reference the
        // same SVG with different relative paths, we key by BOTH the
        // resolved synthetic path AND the spec forms we encountered.
        // Simplest correct version: re-scan and register every form.
        for entry in client_opts.entries.values() {
            let Ok(source) = std::fs::read_to_string(entry) else {
                continue;
            };
            let base = entry.parent().unwrap_or(std::path::Path::new("."));
            for line in source.lines() {
                if let Some(spec) = extract_svg_react_spec(line) {
                    let svg_path = base.join(spec.trim_end_matches("?react"));
                    if let Some(synthetic_str) = seen.get(&svg_path) {
                        let synthetic_path = PathBuf::from(synthetic_str);
                        if let Some(tsx) = client_opts.precompiled.get(&synthetic_path) {
                            client_opts
                                .virtual_modules
                                .insert(spec.to_string(), (synthetic_path, tsx.clone()));
                        }
                    }
                }
            }
        }
    }
    // §5-A: skip the client bundle when nothing it depends on changed. The
    // env fingerprint excludes `*.server.*` (never in the client bundle), so
    // a server-only edit leaves this env valid. The manifests are read from
    // disk below either way, so a skip is safe only when they exist.
    let client_fp = build_cache::env_fingerprint(
        &rootp,
        "client",
        &client_opts
            .entries
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>(),
        &[&client_out.to_string_lossy(), build_secret_for_fp.as_str()],
    );
    let client_manifest_on_disk = client_out.join("client.manifest.json");
    // `OXIDE_FORCE_BUNDLE` forces a re-bundle; `OXIDE_FORCE_BUILD` forces
    // the whole build (which includes every env). The two are separate
    // because "the whole-build fingerprint said nothing changed" and "this
    // env's bundle is still valid" are different questions — a developer
    // forcing a rebuild to re-run a later phase should not also pay for
    // re-bundling an unchanged env.
    let force_bundle = std::env::var_os("OXIDE_FORCE_BUNDLE").is_some();
    let skip_client = client_manifest_on_disk.is_file()
        && !force_bundle
        && build_cache::env_is_up_to_date(&rootp, "client", &client_fp);
    let client_outcome = if skip_client {
        outln!(
            w,
            "bundle: client unchanged (env fingerprint match) — skipped"
        );
        oxide_bundler::BundleOutput {
            modules: Vec::new(),
            chunks: Vec::new(),
            client_manifest: None,
            server_manifest: None,
            diagnostics: Vec::new(),
        }
    } else {
        rb.bundle(&client_opts).map_err(|e| {
            persist_diagnostics(&rootp, &[e.0.clone()]);
            anyhow::anyhow!("{} {}", e.0.code, e.0.message)
        })?
    };
    // SPEC-02 / §4.6.3: a V8 build whose server modules need Node (a `.node`
    // addon or an unshimmed `node:` builtin) must fail loudly. The bundler
    // externalizes `node:` specifiers, so scanning the emitted JS misses them;
    // scan the source server modules instead.
    if matches!(cfg.server.backend, oxide_config::JsBackend::V8) {
        let mut needs: Vec<String> = Vec::new();
        let mut stack = vec![rootp.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    let n = p.file_name().and_then(|x| x.to_str()).unwrap_or("");
                    if !matches!(n, "node_modules" | "dist" | ".oxide" | "target" | ".git") {
                        stack.push(p);
                    }
                    continue;
                }
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !(name.ends_with(".ts") || name.ends_with(".tsx") || name.ends_with(".js")) {
                    continue;
                }
                let Ok(code) = std::fs::read_to_string(&p) else {
                    continue;
                };
                for quote in ['\'', '"'] {
                    let pat = format!("{quote}node:");
                    let mut rest = code.as_str();
                    while let Some(i) = rest.find(&pat) {
                        let after = &rest[i + pat.len()..];
                        let bname: String = after
                            .chars()
                            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                            .collect();
                        if let Some(reason) =
                            oxide_jshost::v8_loader::requires_node_backend(&format!("node:{bname}"))
                        {
                            needs.push(reason);
                        }
                        rest = &after[bname.len().min(after.len())..];
                    }
                }
                for ext in [".node\"", ".node'"] {
                    if code.contains(ext) {
                        needs.push("a .node native addon".to_string());
                    }
                }
            }
        }
        needs.sort();
        needs.dedup();
        if !needs.is_empty() {
            let msg = format!(
                "server.backend = v8, but the server code needs Node: {}. Set server.backend to node.",
                needs.join(", ")
            );
            outln!(w, "error JSH-V8KIND: {msg}");
            let d = oxide_diagnostics::Diagnostic::error("JSH-V8KIND", msg);
            persist_diagnostics(&rootp, &[d]);
            anyhow::bail!("V8 backend incompatible with the server code (JSH-V8KIND)");
        }
    }

    // SPEC-05 / §6.4: the forbidden-import guard. A `*.server.ts` module must
    // never ship its real body into the client bundle. A legitimate import
    // goes through the compiler's server-fn stub (a `serverFn(...)` export
    // compiles to a `POST /_oxide/fn/<id>` stub with none of the module's own
    // identifiers); a raw import leaks the body. So: collect every non-stub
    // export name from every server module under the routes root, and fail if
    // any appears in ANY emitted client chunk. This is metadata-independent —
    // it reads the actual emitted bytes.
    {
        let mut server_exports: Vec<(std::path::PathBuf, String)> = Vec::new();
        // Scan the WHOLE project, not just the routes dir: a server module
        // can live anywhere (`src/posts.server.ts`). Skip build/vendor dirs.
        let mut stack = vec![rootp.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    let n = p.file_name().and_then(|x| x.to_str()).unwrap_or("");
                    if !matches!(n, "node_modules" | "dist" | ".oxide" | "target" | ".git") {
                        stack.push(p);
                    }
                    continue;
                }
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !(name.ends_with(".server.ts") || name.ends_with(".server.tsx")) {
                    continue;
                }
                let src = std::fs::read_to_string(&p).unwrap_or_default();
                for line in src.lines() {
                    let t = line.trim();
                    // Only a NON-FUNCTION `export const` is a leak risk — that
                    // is a literal value (a secret, a base URL, a table). A
                    // function export (`load`, `getPost`, a `serverFn`) never
                    // ships its body to the client: loaders run server-side,
                    // serverFns compile to stubs, and the compiler's cache
                    // wrapper keeps the function on the server. Flagging a
                    // generic name like `load` would false-positive on every
                    // chunk (it is a common word).
                    if let Some(rest) = t.strip_prefix("export const ") {
                        if t.contains("serverFn(") {
                            continue; // compiles to a stub — legitimate
                        }
                        let ident: String = rest
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_')
                            .collect();
                        // Build-time metadata the compiler consumes and never
                        // ships as a body.
                        if matches!(
                            ident.as_str(),
                            "config" | "prerender" | "generateParams" | "load"
                        ) {
                            continue;
                        }
                        // A function-ish RHS (`= async`, `= (`, `= function`,
                        // `=>` on the same line) is not a literal export.
                        let after = rest[ident.len()..].trim_start();
                        let is_fn = after.starts_with("= async")
                            || after.starts_with("= (")
                            || after.starts_with("= function")
                            || after.starts_with("=>")
                            || after.contains("=> {");
                        if !is_fn && ident.len() >= 3 {
                            // The emitted chunk may inline the value, so look
                            // for any string literal in the RHS (a base URL, a
                            // secret). The bare identifier is NOT pushed: an
                            // object/array export (`actions`, `config`, a route
                            // table) has a common name that appears in every
                            // chunk — only its literal *contents* leak.
                            let rhs = after.splitn(2, '=').nth(1).unwrap_or("");
                            let mut in_str = false;
                            let mut lit = String::new();
                            let mut quote = '\0';
                            for ch in rhs.chars() {
                                if !in_str && (ch == '\'' || ch == '"' || ch == '`') {
                                    in_str = true;
                                    quote = ch;
                                } else if in_str && ch == quote {
                                    if lit.len() >= 8 {
                                        server_exports.push((p.clone(), lit.clone()));
                                    }
                                    in_str = false;
                                    lit.clear();
                                } else if in_str {
                                    lit.push(ch);
                                }
                            }
                        }
                    }
                }
            }
        }
        // Read every emitted client .js once.
        let mut client_code: Vec<(String, String)> = Vec::new();
        let mut stack = vec![client_out.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|x| x.to_str()) != Some("js") {
                    continue;
                }
                if let Ok(code) = std::fs::read_to_string(&p) {
                    client_code.push((p.to_string_lossy().to_string(), code));
                }
            }
        }
        let mut hits: Vec<oxide_diagnostics::Diagnostic> = Vec::new();
        for (sp, ident) in &server_exports {
            for (chunk, code) in &client_code {
                if code.contains(ident.as_str()) {
                    // 19-E: locate the literal in its source file so the
                    // diagnostic can show the offending line. `ident` is the
                    // string literal the guard found (>= 8 chars), so a
                    // substring search for it in the quoted source is exact
                    // enough to name the line.
                    let mut diag = oxide_diagnostics::Diagnostic::error(
                        "CMP-GUARD",
                        format!(
                            "server-only `{ident}` from `{}` shipped into client chunk `{chunk}`",
                            sp.display()
                        ),
                    )
                    .with_fix(
                        "import it via a server function (9.4) or move the value \
                         into a non-server module",
                    );
                    if let Ok(src) = std::fs::read_to_string(sp) {
                        if let Some((line_no, col)) = find_literal_line(&src, ident) {
                            diag = diag.at(oxide_diagnostics::Location::file(
                                sp.to_string_lossy(),
                                line_no as u32,
                                col as u32,
                            ));
                            let frame =
                                oxide_diagnostics::render_code_frame(&src, line_no, Some(col));
                            if !frame.is_empty() {
                                diag = diag.with_code_frame(frame);
                            }
                        }
                    }
                    hits.push(diag);
                    break;
                }
            }
        }
        if !hits.is_empty() {
            for d in &hits {
                outln!(w, "error {}: {}", d.code, d.message);
            }
            persist_diagnostics(&rootp, &hits);
            anyhow::bail!(
                "{} server-only export(s) reached the client bundle (CMP-GUARD)",
                hits.len()
            );
        }
    }

    // SEC-08: fail the build if a secret-looking `process.env.*` read shipped
    // into the client bundle.
    let secret_hits = oxide_bundler::scan_client_secrets(&client_outcome.chunks);
    if !secret_hits.is_empty() {
        for d in &secret_hits {
            outln!(w, "error {}: {}", d.code, d.message);
        }
        persist_diagnostics(&rootp, &secret_hits);
        anyhow::bail!(
            "{} client secret(s) would leak to the browser (CMP-SECRET)",
            secret_hits.len()
        );
    }
    // Only record the client env fingerprint AFTER the client bundle passed
    // every post-bundle gate (secret scan included). Storing it before the
    // scan let a failed `CMP-SECRET` build look "unchanged" on the next run:
    // `skip_client` was true, the scan saw no chunks, and the build went
    // green without ever re-checking the leaking bundle.
    if !skip_client {
        if let Err(e) = build_cache::env_store(&rootp, "client", &client_fp) {
            eprintln!("warn: could not store client env fingerprint: {e}");
        }
    }
    let server_outcome = rb.bundle(&server_opts).map_err(|e| {
        let mut diags = client_outcome.diagnostics.clone();
        diags.push(e.0.clone());
        persist_diagnostics(&rootp, &diags);
        anyhow::anyhow!("{} {}", e.0.code, e.0.message)
    })?;

    // §8.2 / §16.3 / O1: enrich both manifests with layout chains from the
    // router tree. The bundler doesn't know the tree, so its layout_chain
    // is []; the CLI fills it in here. Best-effort — a missing manifest
    // (an older build, or a non-SSR env) is a no-op.
    let _ = enrich_manifest_layout_chains(&client_out.join("client.manifest.json"), &tree);
    let _ = enrich_manifest_layout_chains(&server_out.join("server.manifest.json"), &tree);
    // BUG-06: persist build diagnostics so MCP `oxide_errors` has data.
    // Written on success too (possibly `[]`) so stale errors don't linger.
    {
        let mut all = client_outcome.diagnostics.clone();
        all.extend(server_outcome.diagnostics.clone());
        persist_diagnostics(&rootp, &all);
    }
    // The bundler writes client.manifest.json / server.manifest.json into the out dirs.
    let client_manifest = read_json(&client_out.join("client.manifest.json"))
        .ok_or_else(|| anyhow::anyhow!("bundler did not write client.manifest.json"))?;
    let client_manifest_snapshot = client_manifest.clone();
    // §10.3: the boot chunk URL, if islands were registered. `RolldownBundler`
    // emits `client.manifest.json.routes["__boot__"].entry_chunk` as the
    // hashed file name; the shell references `/client/<file>`.
    let boot_script_url: Option<String> = if boot_chunk_url.is_some() {
        client_manifest
            .get("routes")
            .and_then(|r| r.get("__boot__"))
            .and_then(|r| r.get("entry_chunk"))
            .and_then(|v| v.as_str())
            .map(|file| format!("client/{file}"))
    } else {
        None
    };
    let mut server_manifest = read_json(&server_out.join("server.manifest.json"))
        .ok_or_else(|| anyhow::anyhow!("bundler did not write server.manifest.json"))?;
    let build_id = client_manifest
        .get("build_id")
        .and_then(|v| v.as_str())
        .unwrap_or("dev")
        .to_string();

    // Prerender static routes (§10.5). Two cases:
    //   1. Page routes with no params and no server module → one HTML shell.
    //   2. Page routes whose `page.server.ts` exports a **literal**
    //      `generateParams` array → one HTML per param entry.
    // Runtime-valued `generateParams` cannot be evaluated at build time without
    // a JS worker (post-MLP) and is skipped with a printed notice.
    let nonce = oxide_bundler::ChunkBundler::content_hash(&[build_id.as_bytes()]);
    let mut prerendered: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // PERF-04: (route_id, url_path, sentineled html) for the L0 seed manifest.
    // (route_id, url, sentineled_html, status, locale) — the locale is
    // the last element so the L0-seed key can carry it once the dispatch
    // appends the resolved locale to the vary (a follow-up; see the seed
    // write below).
    let mut seed_bodies: Vec<(String, String, String, u16, String)> = Vec::new();
    // §5-A full incremental: load the previous build's per-route prerender
    // records, indexed by (route_id, url). On this build, a route whose
    // source hash matches AND whose output file still exists is reused
    // (skipping the render). The saved HTML's `nonce="…"` is rewritten to
    // the current build's nonce so the CSP header the server emits still
    // matches. A missing or old-format snapshot yields an empty map — every
    // route renders, which is the safe default.
    let prev_prerender_snapshot = crate::build_graph::load_snapshot(&rootp);
    let prev_prerenders: std::collections::BTreeMap<
        (String, String),
        &crate::build_graph::RoutePrerenderRecord,
    > = prev_prerender_snapshot
        .route_prerenders
        .iter()
        .map(|r| ((r.route_id.clone(), r.url.clone()), r))
        .collect();
    let mut new_prerender_records: Vec<crate::build_graph::RoutePrerenderRecord> = Vec::new();
    let mut reused_prerender_count: usize = 0;
    // Routes with literal generateParams are effectively static regardless of
    // whether they also have a server module.
    let mut static_override: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for e in &tree.entries {
        if !matches!(e.kind, oxide_router::RouteKind::Page) {
            continue;
        }

        // Load the accompanying `page.server.ts` (if any) for generateParams.
        let server_mod = rootp
            .join(&cfg.routes.dir)
            .join(e.file.with_file_name("page.server.ts"));
        let gen_params = if server_mod.is_file() {
            let src = std::fs::read_to_string(&server_mod).unwrap_or_default();
            oxide_compiler::extract_generate_params(&src)
        } else {
            oxide_compiler::GenerateParamsResult::Absent
        };

        let pattern = pattern_of(&e.segments);
        let nonce_for = nonce.clone();
        // WP-A2: prerender speaks the same canonical id the live pipeline
        // hands the renderer, so the adapter's resolver hit is identical.
        let route_id = canonical_route_id(&e.id.0);
        // §8.2: the layout chain for this route. `e` is a scanned entry;
        // asking the tree to match this route's own id via a synthetic path
        // is overkill — compute the chain directly from the entry list using
        // the same rule the matcher uses (any layout whose id is a prefix
        // directory of this route id).
        let layout_ids: Vec<String> = {
            let mut chain: Vec<(String, usize)> = Vec::new();
            for candidate in &tree.entries {
                if !matches!(candidate.kind, oxide_router::RouteKind::Layout) {
                    continue;
                }
                // A layout `routes/<a>/<b>/layout` wraps any route whose id
                // starts with `routes/<a>/<b>/`. The root layout
                // `routes/layout` wraps everything.
                let layout_base = candidate
                    .id
                    .0
                    .strip_suffix("/layout")
                    .unwrap_or(&candidate.id.0);
                let wraps = if layout_base == "routes" || layout_base.is_empty() {
                    true
                } else {
                    route_id.starts_with(&format!("{layout_base}/"))
                };
                if wraps {
                    chain.push((candidate.id.0.clone(), layout_base.len()));
                }
            }
            // Outermost first: shortest base wins.
            chain.sort_by_key(|(_, len)| *len);
            chain.into_iter().map(|(id, _)| id).collect()
        };

        // §9.2: layout `(id, layout.server.ts path)` pairs for the prerender
        // layout-load chain. Only layouts that actually have a
        // `layout.server.ts` participate.
        let prerender_layout_modules: Vec<(String, std::path::PathBuf)> = layout_ids
            .iter()
            .filter_map(|lid| {
                tree.entries
                    .iter()
                    .find(|le| {
                        le.id.0 == *lid && matches!(le.kind, oxide_router::RouteKind::Layout)
                    })
                    .map(|le| {
                        (
                            lid.clone(),
                            rootp
                                .join(&cfg.routes.dir)
                                .join(le.file.with_file_name("layout.server.ts")),
                        )
                    })
            })
            .filter(|(_, p)| p.is_file())
            .collect();
        // §10.2 step 5: nearest ancestor error.tsx for this route.
        // Innermost wins — a `routes/blog/error` shadows `routes/error`.
        let prerender_error_route_id: Option<String> = {
            let mut best: Option<(String, usize)> = None;
            for candidate in &tree.entries {
                if !matches!(candidate.kind, oxide_router::RouteKind::Error) {
                    continue;
                }
                let base = candidate
                    .id
                    .0
                    .strip_suffix("/error")
                    .unwrap_or(&candidate.id.0);
                let wraps = if base == "routes" || base.is_empty() {
                    true
                } else {
                    route_id.starts_with(&format!("{base}/"))
                };
                if !wraps {
                    continue;
                }
                let len = base.len();
                match &best {
                    Some((_, prev)) if *prev >= len => {}
                    _ => best = Some((candidate.id.0.clone(), len)),
                }
            }
            best.map(|(id, _)| id)
        };
        // §9.1: if the route declares a server module, resolve its path so
        // the loader can run during prerender. Uses the same
        // `page.server.ts` sibling convention as build_dispatch.
        let module_path: Option<std::path::PathBuf> = if e.has_server_module {
            let p = routes_root.join(e.file.with_file_name("page.server.ts"));
            if p.is_file() { Some(p) } else { None }
        } else {
            None
        };

        // §5-A: hash every source that feeds this route. Reuse fires only when
        // this hash matches the previous build's record for the same
        // (route_id, url). Computed once per entry — the hash covers file
        // contents + config, not the per-URL params.
        // §15.2: `renderHead` — a plugin's contribution to the neutral head
        // model for THIS route. Called once per route entry (not per
        // generateParams path): the head is route-shaped, not URL-shaped.
        let plugin_head =
            plugin_render_head(&rootp, &cfg, stack.tooling_pool.as_ref(), &route_id).await;
        let route_src_hash = route_source_hash(
            &rootp,
            &cfg,
            &tree,
            e,
            &layout_ids,
            &prerender_layout_modules,
            prerender_error_route_id.as_deref(),
        );
        match gen_params {
            oxide_compiler::GenerateParamsResult::Literal(params) => {
                // Prerendered output exists → effective mode `static` for the
                // manifest, even if the route also has a server module.
                static_override.insert(route_id.clone());
                let locales = prerender_locales(&cfg);
                let mut targets = Vec::with_capacity(params.len() * locales.len());
                for params_map in &params {
                    // Substitute params into the segment list to build a path.
                    let mut segs: Vec<String> = e.segments.clone();
                    for s in segs.iter_mut() {
                        if let Some(name) = param_name(s) {
                            if let Some(v) = params_map.get(name) {
                                *s = v.clone();
                            }
                        }
                    }
                    let base_rel = if segs.is_empty() {
                        "index.html".to_string()
                    } else {
                        format!("{}.html", segs.join("/"))
                    };
                    for locale in &locales {
                        let (rel, url_path) = prefixed_prerender_path(&base_rel, locale, &cfg);
                        targets.push(PrerenderTarget {
                            rel,
                            url_path,
                            params: params_map.clone(),
                            locale: locale.clone(),
                        });
                    }
                }
                // P-20: bounded-parallel render across this route's params.
                let (rows, reused) = prerender_targets(
                    &dist,
                    &route_id,
                    &pattern,
                    targets,
                    &nonce,
                    &nonce_for,
                    stack.page_renderer.as_ref(),
                    stack.loader_runner.as_ref(),
                    module_path.as_deref(),
                    boot_script_url.as_deref(),
                    &layout_ids,
                    &prerender_layout_modules,
                    prerender_error_route_id.as_deref(),
                    &plugin_head,
                    &prev_prerenders,
                    &route_src_hash,
                )
                .await?;
                reused_prerender_count += reused;
                let mut outs: Vec<String> = Vec::with_capacity(rows.len());
                for (t, html, prerender_status) in rows {
                    let PrerenderTarget {
                        rel,
                        url_path,
                        locale,
                        ..
                    } = t;
                    seed_bodies.push((
                        route_id.clone(),
                        url_path.clone(),
                        html.replace(&nonce, l0_cache::NONCE_SENTINEL),
                        prerender_status,
                        locale,
                    ));
                    new_prerender_records.push(crate::build_graph::RoutePrerenderRecord {
                        route_id: route_id.clone(),
                        url: url_path.clone(),
                        status: prerender_status,
                        rel_out: format!("dist/{rel}"),
                        source_hash: route_src_hash.clone(),
                        nonce: nonce.clone(),
                    });
                    outs.push(rel);
                }
                if !outs.is_empty() {
                    prerendered.insert(route_id.clone(), outs);
                }
            }
            oxide_compiler::GenerateParamsResult::Runtime { .. } => {
                // Runtime evaluate `generateParams` via the worker (spec
                // §10.5). The route's `page.server.ts` module is imported by
                // the same worker pool that backs the loader and renderer;
                // `LoaderHost::generate_params` calls the export and returns
                // the enumerated entries.
                //
                // When the pool isn't available (no Node), fall back to the
                // previous honest notice — the route is skipped, not failed.
                let params_via_worker: Option<Vec<std::collections::BTreeMap<String, String>>> =
                    match (stack.pool.as_ref(), module_path.as_deref()) {
                        (Some(pool), Some(mp)) => {
                            let host = oxide_jshost::LoaderHost::new(std::sync::Arc::clone(pool));
                            match host.generate_params(&mp.to_string_lossy()).await {
                                Ok(list) => Some(list),
                                Err(e) => {
                                    outln!(
                                        w,
                                        "skip prerender for `{route_id}`: generateParams runtime evaluation failed: {} {}",
                                        e.0.code,
                                        e.0.message
                                    );
                                    None
                                }
                            }
                        }
                        _ => None,
                    };

                match params_via_worker {
                    Some(params) if !params.is_empty() => {
                        // Same shape as the Literal arm: enumerate, batch
                        // render, mark the route static in the manifest.
                        static_override.insert(route_id.clone());
                        let locales = prerender_locales(&cfg);
                        let mut targets = Vec::with_capacity(params.len() * locales.len());
                        for params_map in &params {
                            let mut segs: Vec<String> = e.segments.clone();
                            for s in segs.iter_mut() {
                                if let Some(name) = param_name(s) {
                                    if let Some(v) = params_map.get(name) {
                                        *s = v.clone();
                                    }
                                }
                            }
                            let base_rel = if segs.is_empty() {
                                "index.html".to_string()
                            } else {
                                format!("{}.html", segs.join("/"))
                            };
                            for locale in &locales {
                                let (rel, url_path) =
                                    prefixed_prerender_path(&base_rel, locale, &cfg);
                                targets.push(PrerenderTarget {
                                    rel,
                                    url_path,
                                    params: params_map.clone(),
                                    locale: locale.clone(),
                                });
                            }
                        }
                        let (rows, reused) = prerender_targets(
                            &dist,
                            &route_id,
                            &pattern,
                            targets,
                            &nonce,
                            &nonce_for,
                            stack.page_renderer.as_ref(),
                            stack.loader_runner.as_ref(),
                            module_path.as_deref(),
                            boot_script_url.as_deref(),
                            &layout_ids,
                            &prerender_layout_modules,
                            prerender_error_route_id.as_deref(),
                            &plugin_head,
                            &prev_prerenders,
                            &route_src_hash,
                        )
                        .await?;
                        reused_prerender_count += reused;
                        let mut outs: Vec<String> = Vec::with_capacity(rows.len());
                        for (t, html, prerender_status) in rows {
                            let PrerenderTarget {
                                rel,
                                url_path,
                                locale,
                                ..
                            } = t;
                            seed_bodies.push((
                                route_id.clone(),
                                url_path.clone(),
                                html.replace(&nonce, l0_cache::NONCE_SENTINEL),
                                prerender_status,
                                locale,
                            ));
                            new_prerender_records.push(crate::build_graph::RoutePrerenderRecord {
                                route_id: route_id.clone(),
                                url: url_path.clone(),
                                status: prerender_status,
                                rel_out: format!("dist/{rel}"),
                                source_hash: route_src_hash.clone(),
                                nonce: nonce.clone(),
                            });
                            outs.push(rel);
                        }
                        if !outs.is_empty() {
                            prerendered.insert(route_id.clone(), outs);
                        }
                    }
                    Some(_) => {
                        // Empty array: generateParams ran but produced no
                        // paths. Not an error; the route stays unprerendered.
                        outln!(
                            w,
                            "route `{route_id}`: generateParams returned no entries; skipping prerender",
                            route_id = e.id.0
                        );
                    }
                    None => {
                        // No worker, or module path unresolved.
                        outln!(
                            w,
                            "skip prerender for `{route_id}`: generateParams requires runtime evaluation and no worker is available",
                            route_id = e.id.0
                        );
                    }
                }
            }
            oxide_compiler::GenerateParamsResult::Absent => {
                // §10.1: three signals decide whether a route with no
                // `generateParams` is prerendered.
                //
                //   1. An explicit `config.render` wins outright.
                //      - `static`  → prerender, even with a `page.server.ts`.
                //      - `dynamic`, `isr`, `spa`, `islands` → do not
                //        prerender at build time (their modes are set by
                //        their own machinery).
                //   2. No explicit mode and a `page.server.ts` present →
                //      the loader runs per-request (dynamic); the route
                //      gets no prerendered file.
                //   3. No explicit mode and no server module → static.
                //
                // The mode value comes from the scanner's §10.1 inference
                // (cookies/headers/locals/searchParams markers).
                let explicit = e.render_mode.as_deref();
                let mode_ok = match explicit {
                    Some("static") => true,
                    Some("dynamic") | Some("isr") | Some("spa") | Some("islands") => false,
                    // `auto` (or unset) fall through to the server-module check.
                    _ => !e.has_server_module,
                };
                let no_params = e
                    .segments
                    .iter()
                    .all(|s| !s.contains(':') && !s.contains('*') && !s.contains('{'));
                if !mode_ok || !no_params {
                    continue;
                }
                // §16.4 / O2: a static route without generateParams still
                // needs one output per locale under the project's i18n
                // strategy. Base path (no `prerendered/` prefix) is shared
                // across variants; the locale loop adds the prefix.
                let base_rel = if e.segments.is_empty() {
                    "index.html".to_string()
                } else {
                    format!("{}.html", e.segments.join("/"))
                };
                let empty_params: BTreeMap<String, String> = BTreeMap::new();
                let locales = prerender_locales(&cfg);
                let mut outs: Vec<String> = Vec::with_capacity(locales.len());
                for locale in &locales {
                    let (rel, url_path) = prefixed_prerender_path(&base_rel, locale, &cfg);
                    let (html, prerender_status) = match try_reuse_prerender(
                        &dist.join(&rel),
                        &route_id,
                        &url_path,
                        &route_src_hash,
                        &prev_prerenders,
                    ) {
                        Some((saved, status, old_nonce)) => {
                            let fresh = saved.replace(&old_nonce, &nonce);
                            if std::fs::write(dist.join(&rel), &fresh).is_ok() {
                                reused_prerender_count += 1;
                                (fresh, status)
                            } else {
                                write_prerender(
                                    &dist,
                                    &rel,
                                    &route_id,
                                    &pattern,
                                    &url_path,
                                    &empty_params,
                                    locale,
                                    &nonce_for,
                                    stack.page_renderer.as_ref(),
                                    stack.loader_runner.as_ref(),
                                    module_path.as_deref(),
                                    boot_script_url.as_deref(),
                                    &layout_ids,
                                    &prerender_layout_modules,
                                    prerender_error_route_id.as_deref(),
                                    &plugin_head,
                                )
                                .await?
                            }
                        }
                        None => {
                            write_prerender(
                                &dist,
                                &rel,
                                &route_id,
                                &pattern,
                                &url_path,
                                &empty_params,
                                locale,
                                &nonce_for,
                                stack.page_renderer.as_ref(),
                                stack.loader_runner.as_ref(),
                                module_path.as_deref(),
                                boot_script_url.as_deref(),
                                &layout_ids,
                                &prerender_layout_modules,
                                prerender_error_route_id.as_deref(),
                                &plugin_head,
                            )
                            .await?
                        }
                    };
                    seed_bodies.push((
                        route_id.clone(),
                        url_path.clone(),
                        html.replace(&nonce, l0_cache::NONCE_SENTINEL),
                        prerender_status,
                        locale.clone(),
                    ));
                    new_prerender_records.push(crate::build_graph::RoutePrerenderRecord {
                        route_id: route_id.clone(),
                        url: url_path.clone(),
                        status: prerender_status,
                        rel_out: format!("dist/{rel}"),
                        source_hash: route_src_hash.clone(),
                        nonce: nonce.clone(),
                    });
                    outs.push(rel);
                }
                if !outs.is_empty() {
                    prerendered.insert(route_id.clone(), outs);
                }
            }
        }
    }

    if reused_prerender_count > 0 {
        outln!(
            w,
            "incremental: reused {} prerendered route(s) (source hash unchanged)",
            reused_prerender_count,
        );
    }
    // §15-B: run `transformHtml` on every prerendered document. Fresh and
    // reused documents both flow through here, so a plugin sees every
    // emitted HTML file — the same contract Vite's transformIndexHtml has.
    if let Some(hook) = transform_html_hook.as_ref() {
        let mut touched = 0usize;
        for (route_id, rels) in &prerendered {
            for rel in rels {
                let path = dist.join(rel);
                let Ok(html) = std::fs::read_to_string(&path) else {
                    continue;
                };
                match hook.call(route_id, &html) {
                    Ok(Some(new_html)) if new_html != html => {
                        if std::fs::write(&path, &new_html).is_ok() {
                            touched += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
        if touched > 0 {
            outln!(
                w,
                "transformHtml: rewrote {touched} prerendered document(s)"
            );
        }
    }
    // PERF-07: serialize the route tree to the rkyv archive so `oxide start`
    // can mmap it instead of re-scanning the routes directory.
    {
        let archive_path = rootp.join(".oxide/routes.manifest");
        if let Some(parent) = archive_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = tree.archive_to(&archive_path);
    }

    // §16.5: emit the route search index. Same source as the L0 seed
    // manifest — the `prerendered` map (route_id → output paths) — but a
    // different shape and consumer: a client-side island reads it to
    // answer a query. Extracting from the emitted HTML (not the source
    // AST) means a loader-injected `<title>`, a plugin-added heading, or
    // a `defineMeta` override all reach the index as the user sees them.
    if cfg.build.route_search && !prerendered.is_empty() {
        let search_index =
            oxide_assets::route_search::build_route_search_index(&prerendered, &dist);
        if let Ok(json) = oxide_assets::route_search::route_search_index_to_json(&search_index) {
            let path = dist.join("client/search-index.json");
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let _ = std::fs::write(&path, json);
            outln!(
                w,
                "search index: {} route(s) -> dist/client/search-index.json",
                search_index.entries.len()
            );
        }
    }

    // PERF-04: write the L0 seed manifest. Each prerendered page's sentineled
    // HTML is stored so `oxide start` can insert it into L0 at boot — a
    // seeded route is an L0 hit on the very first request (no warm-up miss).
    if !seed_bodies.is_empty() {
        let mut entries = Vec::with_capacity(seed_bodies.len());
        for (rid, url, sentineled, status, locale) in &seed_bodies {
            // §16.4 / O2: only seed the *default-locale* variant. The L0 key
            // is route + params + vary; the runtime's `l0_vary` is header-only
            // today (accept-language), so a locale-prefixed URL like `/fr/about`
            // and `/about` collapse to the same key. Seeding a non-default
            // locale would write an entry the runtime never hits, and would
            // collide with the default-locale entry. The non-default pages
            // serve from `prerendered/<locale>/<route>.html` via the normal
            // static-asset path. Making per-locale L0 seeding useful needs a
            // dispatch change (append the resolved locale to `l0_vary`);
            // tracked as a §21 follow-up.
            let is_default_variant = locale.is_empty() || locale == &cfg.i18n.default_locale;
            if !is_default_variant {
                continue;
            }
            // The key matches what the live server computes for a request
            // with no `accept-language`: route id (no `/page` suffix), no
            // params, vary `lang=`.
            let rid = rid.strip_suffix("/page").unwrap_or(rid);
            let key = l0_cache::L0Cache::key_for(rid, "", "lang=", &build_id);
            entries.push(serde_json::json!({
                "key": key,
                "url": url,
                "body": sentineled,
                "status": status,
            }));
        }
        let seed = serde_json::json!({ "build_id": build_id, "entries": entries });
        let path = dist.join("server/l0-seed.json");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&seed) {
            let _ = std::fs::write(&path, bytes);
        }
    }

    // SPEC-09 / §8.3: regenerate `.oxide/types/routes.d.ts` from the build's
    // route tree so `href()`/`PageProps` stay in sync with the routes without
    // a separate `oxide routes` run. A typegen failure is a warning, not a
    // build failure — the app still ships.
    // SPEC-16.4: when i18n is configured, typegen emits the locale-aware
    // `href` overloads (route, params, locale) + a Locale union.
    let i18n_for_typegen: Option<&[String]> = match cfg.i18n.strategy.as_str() {
        "none" => None,
        _ if cfg.i18n.locales.is_empty() => None,
        _ => Some(&cfg.i18n.locales),
    };
    let default_for_typegen: Option<&str> = if i18n_for_typegen.is_some() {
        Some(cfg.i18n.default_locale.as_str())
    } else {
        None
    };
    if let Err(e) = tree.typegen_with_root_and_i18n(
        &rootp,
        &rootp.join(&cfg.routes.dir),
        i18n_for_typegen,
        default_for_typegen,
    ) {
        outln!(
            w,
            "warn: typegen failed ({} {}); routes.d.ts is stale",
            e.0.code,
            e.0.message
        );
    }
    // SPEC-16.4 / §16-D: load the message catalogs from `locales/` and
    // write `.oxide/types/messages.d.ts` (typed MessageKey union + per-
    // locale LocaleMessages + DefaultLocale). No-op when there's no
    // `locales/` dir; a parse error is a warning (same convention as
    // routes.d.ts).
    {
        let locales_root = rootp.join("locales");
        match oxide_assets::i18n::load_catalog_from_dir(&locales_root) {
            Ok(cat) if !cat.entries.is_empty() => {
                let out_path = rootp.join(".oxide/types/messages.d.ts");
                if let Err(e) = oxide_assets::i18n::write_messages_dts(&cat, &out_path) {
                    outln!(
                        w,
                        "warn: messages.d.ts typegen failed ({} {}); stale",
                        e.0.code,
                        e.0.message,
                    );
                }
                // §16-D: ship the catalog. Without this a client island has
                // no messages at all — the typed keys exist, the values do
                // not reach the browser. One JSON file per locale under
                // `dist/client/messages/`; the client loader fetches the
                // active locale's file lazily.
                //
                // Per-route splitting (only the keys a route uses) is a
                // size optimization on top: a route that names its keys can
                // ship a subset. The full-catalog file is the baseline that
                // makes the feature work; splitting lands when a route's
                // key usage is recorded at scan time.
                let messages_dir = dist.join("client/messages");
                if let Err(e) = std::fs::create_dir_all(&messages_dir) {
                    outln!(w, "warn: cannot create dist/client/messages: {e}");
                } else {
                    let mut emitted = 0usize;
                    for (locale, entries) in &cat.entries {
                        let body = serde_json::json!({
                            "locale": locale,
                            "messages": entries,
                        });
                        let path = messages_dir.join(format!("{locale}.json"));
                        if std::fs::write(&path, serde_json::to_vec(&body).unwrap_or_default())
                            .is_ok()
                        {
                            emitted += 1;
                        }
                    }
                    outln!(
                        w,
                        "i18n: emitted {emitted} message catalog(s) -> dist/client/messages/"
                    );
                }
                // §16-D per-route splitting: scan each route's sources for
                // `t('key')` calls and emit only the keys that route names.
                // The client loader prefers `/client/messages/<locale>/<route>.json`
                // and falls back to the full catalog, so a route that names
                // no keys costs nothing extra and one that names a few ships
                // a few.
                let per_route_dir = messages_dir.clone();
                let mut routes_with_keys = 0usize;
                for e in &tree.entries {
                    if !matches!(e.kind, oxide_router::RouteKind::Page) {
                        continue;
                    }
                    let route_file = rootp.join(&cfg.routes.dir).join(&e.file);
                    let server_file = route_file.with_file_name("page.server.ts");
                    let mut keys: std::collections::BTreeSet<String> =
                        std::collections::BTreeSet::new();
                    for f in [&route_file, &server_file] {
                        if let Ok(src) = std::fs::read_to_string(f) {
                            for k in scan_message_keys(&src) {
                                keys.insert(k);
                            }
                        }
                    }
                    if keys.is_empty() {
                        continue;
                    }
                    // The route id used by the client loader is the same
                    // `componentFor` key the adapter sees: the route
                    // directory with any trailing `/page` stripped.
                    let route_key =
                        e.id.0
                            .strip_suffix("/page")
                            .unwrap_or(&e.id.0)
                            .trim_start_matches("routes/")
                            .replace('/', "_");
                    if route_key.is_empty() {
                        continue;
                    }
                    routes_with_keys += 1;
                    for (locale, entries) in &cat.entries {
                        // The catalog keys are namespaced by the file they
                        // came from (`locales/en/common.json` → `common.`),
                        // but an author writes `t('blog.title')` without the
                        // namespace — it is a file-location artifact, not
                        // part of the message id. Match on the suffix: a
                        // route key `blog.title` selects `common.blog.title`.
                        let subset: std::collections::BTreeMap<&String, &String> = entries
                            .iter()
                            .filter(|(k, _)| {
                                keys.contains(*k)
                                    || keys.iter().any(|want| {
                                        k.as_str() == want.as_str()
                                            || k.ends_with(&format!(".{want}"))
                                    })
                            })
                            .collect();
                        if subset.is_empty() {
                            continue;
                        }
                        let dir = per_route_dir.join(locale);
                        let _ = std::fs::create_dir_all(&dir);
                        let body = serde_json::json!({
                            "locale": locale,
                            "route": route_key,
                            "messages": subset,
                        });
                        let _ = std::fs::write(
                            dir.join(format!("{route_key}.json")),
                            serde_json::to_vec(&body).unwrap_or_default(),
                        );
                    }
                }
                if routes_with_keys > 0 {
                    outln!(
                        w,
                        "i18n: per-route message subsets for {routes_with_keys} route(s)"
                    );
                }
            }
            Ok(_) => { /* no locales/ dir — nothing to typegen */ }
            Err(e) => outln!(
                w,
                "warn: catalog load failed ({} {}); messages.d.ts skipped",
                e.0.code,
                e.0.message,
            ),
        }
    }

    // Enrich the server manifest with inferred render modes + prerendered
    // files. The bundler leaves `render_mode` unset; §10.1 mode inference
    // lives with the route scanner, and the pragmatic rule at build time is:
    //   - explicit `render_mode` on the route wins
    //   - otherwise: has_server_module → `dynamic`
    //   - otherwise: `static`
    // This is the rule the §10.5 static-export refusal relies on.
    if let Some(routes) = server_manifest
        .get_mut("routes")
        .and_then(|r| r.as_object_mut())
    {
        for (id, row) in routes.iter_mut() {
            if let Some(e) = tree
                .entries
                .iter()
                .find(|e| canonical_route_id(&e.id.0) == *id)
            {
                // WP-A1: error / not-found / template rows have no render
                // mode of their own — they are rendering infrastructure, not
                // routable pages.
                if matches!(
                    e.kind,
                    oxide_router::RouteKind::Error
                        | oxide_router::RouteKind::NotFound
                        | oxide_router::RouteKind::Template
                ) {
                    row["render_mode"] = serde_json::Value::Null;
                    continue;
                }
                let mode = e.render_mode.clone().unwrap_or_else(|| {
                    if static_override.contains(id) {
                        "static".to_string()
                    } else if e.has_server_module {
                        "dynamic".to_string()
                    } else {
                        "static".to_string()
                    }
                });
                row["render_mode"] = serde_json::json!(mode);
                if let Some(rels) = prerendered.get(id) {
                    row["prerendered"] = serde_json::json!(rels);
                }
            }
        }
    }

    // Persist the enriched manifest. `server_manifest` was loaded from
    // disk, modified above (render_mode + prerendered inference), and
    // handed to `emit()` — but the adapters write to `dist/deploy/`, not
    // `dist/server/`, so without this write the enriched version lives
    // only in memory. Downstream consumers (the conformance suite, the
    // E2E tests, custom tooling) read `dist/server/server.manifest.json`
    // and need to see the same enriched data.
    std::fs::write(
        server_out.join("server.manifest.json"),
        serde_json::to_string_pretty(&server_manifest)
            .map_err(|e| anyhow::anyhow!("serialize enriched manifest: {e}"))?,
    )?;

    // §15.2: `adapter` hook — a plugin may force an adapter target (a
    // deploy plugin that knows the platform). The LAST plugin to return a
    // target wins. An unknown target is reported and the config's choice
    // stands.
    let mut plugin_adapter: Option<String> = None;
    if plugin_declares_any(&rootp, &cfg, &["adapter"]) {
        let results = fire_lifecycle_hook(
            &rootp,
            &cfg,
            stack.tooling_pool.as_ref(),
            "adapter",
            serde_json::json!({ "target": cfg.adapter }),
        )
        .await;
        for r in results {
            if let Some(t) = r.get("target").and_then(|v| v.as_str()) {
                plugin_adapter = Some(t.to_string());
            }
        }
    }
    let effective_adapter = plugin_adapter.as_deref().unwrap_or(&cfg.adapter);
    let target = if r#static {
        oxide_adapters::Target::Static
    } else {
        oxide_adapters::Target::parse(effective_adapter).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown adapter `{}`; expected one of binary|node|static|docker|cloudflare|lambda|vercel|netlify|deno|bun (§18)",
                cfg.adapter
            )
        })?
    };
    let static_fallback = cfg.static_export.fallback.clone();
    let artifact = oxide_adapters::BuildArtifact {
        out_dir: dist.clone(),
        build_id: build_id.clone(),
        client_manifest,
        server_manifest,
        prerendered,
        static_fallback,
        server_bundle: Some(server_out.join("ssr/index.js")),
    };
    // §18 / §2 D18: for the Cloudflare target, best-effort compile the core
    // subset to wasm32 and hand it to the adapter. A missing wasm32 target is
    // not fatal — the Worker's JS fallback covers the same functions.
    if target == oxide_adapters::Target::Cloudflare {
        match compile_edge_wasm(&rootp, &dist) {
            Ok(Some(path)) => {
                outln!(w, "edge wasm compiled: {}", path.display());
            }
            Ok(None) => {
                outln!(
                    w,
                    "edge wasm skipped (wasm32-unknown-unknown target not installed); \
                     the Worker falls back to JS — run `rustup target add wasm32-unknown-unknown` \
                     to enable the accelerated path (spec D18)"
                );
            }
            Err(e) => {
                outln!(
                    w,
                    "edge wasm build failed: {e}; the Worker falls back to JS"
                );
            }
        }
    }

    // §18 / P0-1b: for the two Cloudflare targets, bundle the project's
    // edge entry (src/edge.{ts,mjs,js,tsx,jsx}) into
    // dist/deploy/edge_bundle.js BEFORE emit writes the handler that
    // imports it. A project without an edge entry is fine -- the handler's
    // try/catch on `Cannot find module` falls through to the built-in JS
    // path.
    if matches!(
        target,
        oxide_adapters::Target::Cloudflare | oxide_adapters::Target::CloudflarePages
    ) {
        if let Some(edge_entry) = find_edge_entry(&rootp) {
            match bundle_edge_entry(
                &rootp,
                &dist,
                &edge_entry,
                css_transform,
                cfg.build.edge_externals.clone(),
                server_precompiled.clone(),
                w,
            )
            .await
            {
                Ok(Some(path)) => {
                    outln!(w, "edge_bundle.js: {}", path.display());
                }
                Ok(None) => {
                    outln!(
                        w,
                        "edge entry `{}` produced no entry chunk; edge_handler.js will use its JS fallback",
                        edge_entry.display()
                    );
                }
                Err(e) => {
                    anyhow::bail!("edge bundle failed: {e}");
                }
            }
        }
    }

    let written = oxide_adapters::emit(target, &artifact, &dist.join("deploy"))
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    // under `.oxide/types/`. A failure is a warning (the app still ships).
    if plugin_declares_any(&rootp, &cfg, &["typegen"]) {
        let results = fire_lifecycle_hook(
            &rootp,
            &cfg,
            stack.tooling_pool.as_ref(),
            "typegen",
            serde_json::json!({
                "outDir": rootp.join(".oxide/types").to_string_lossy(),
                "routes": tree.entries.len(),
            }),
        )
        .await;
        let types_dir = rootp.join(".oxide/types");
        let mut written = 0usize;
        for r in results {
            let Some(files) = r.get("files").and_then(|f| f.as_object()) else {
                continue;
            };
            for (rel, body) in files {
                let Some(src) = body.as_str() else { continue };
                // Refuse a path that escapes `.oxide/types/`.
                let target = types_dir.join(rel);
                if !target.starts_with(&types_dir) {
                    outln!(
                        w,
                        "warn: typegen path `{rel}` escapes .oxide/types; skipped"
                    );
                    continue;
                }
                if let Some(parent) = target.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if std::fs::write(&target, src).is_ok() {
                    written += 1;
                }
            }
        }
        if written > 0 {
            outln!(w, "typegen plugin: {written} file(s) -> .oxide/types/");
        }
    }
    // §16.3: sitemap.xml + robots.txt from the route table.
    emit_sitemap_and_robots(&cfg, &tree, &dist, w)?;
    // §16.3: manifest.webmanifest, when a `manifest` block is configured.
    emit_web_manifest(&cfg, &dist, w)?;
    // SPEC-21 / §16.6: the PWA service worker, when a manifest is configured.
    emit_pwa_worker(&cfg, &dist, &client_manifest_snapshot, w)?;
    // §16.2: bundle configured font families into dist/fonts + fonts.css.
    emit_fonts(&cfg, &rootp, &dist, w)?;
    // §4.4: build a V8 startup snapshot from the server bundle.
    emit_snapshot_blob(&rootp, &server_out, w)?;
    // §15.2: `buildEnd` — the build finished successfully; a plugin gets the
    // summary. Fired before the "build ok" line so a plugin's own output
    // appears above the summary.
    if plugin_declares_any(&rootp, &cfg, &["buildEnd"]) {
        let _ = fire_lifecycle_hook(
            &rootp,
            &cfg,
            stack.tooling_pool.as_ref(),
            "buildEnd",
            serde_json::json!({
                "routes": tree.entries.len(),
                "prerendered": artifact.prerendered.len(),
                "target": target.as_str(),
            }),
        )
        .await;
    }
    // §16.4: stamp the client manifest now that catalogs are emitted — the
    // stamp keys off `dist/client/messages/`, so it must run after that.
    stamp_i18n_locales(&rootp, &cfg);
    outln!(
        w,
        "build ok — {} routes, {} prerendered, {} deploy file(s) for `{}` in dist/deploy",
        tree.entries.len(),
        artifact.prerendered.len(),
        written.len(),
        target.as_str()
    );
    // §5-A: record a DemandGraph over this build's inputs and persist it so
    // `oxide analyze` and the MCP `oxide_graph` tool can report real
    // recompute counts (node granularity: module / chunk / manifest /
    // typegen). The current shape: the pipeline still runs to
    // completion — the graph is what a future incremental build diffs
    // against. Best-effort: a persistence failure logs, does not fail.
    {
        let mut client_chunks: Vec<std::path::PathBuf> = Vec::new();
        {
            let mut stack = vec![client_out.clone()];
            while let Some(d) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else {
                    continue;
                };
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.extension().and_then(|x| x.to_str()) == Some("js") {
                        client_chunks.push(p);
                    }
                }
            }
        }
        let mut server_chunks: Vec<std::path::PathBuf> = Vec::new();
        {
            let mut stack = vec![server_out.clone()];
            while let Some(d) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else {
                    continue;
                };
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.extension().and_then(|x| x.to_str()) == Some("js") {
                        server_chunks.push(p);
                    }
                }
            }
        }
        let cm_path = client_out.join("client.manifest.json");
        let sm_path = server_out.join("server.manifest.json");
        let inputs = crate::build_graph::RecordInputs {
            rootp: &rootp,
            entries: &entries,
            client_chunks: &client_chunks,
            server_chunks: &server_chunks,
            client_manifest_path: &cm_path,
            server_manifest_path: &sm_path,
            build_id: &build_id,
            route_prerenders: std::mem::take(&mut new_prerender_records),
            plugin_options: collect_plugin_options(&rootp, &cfg),
            // `client_manifest` was moved into the build artifact above;
            // the snapshot clone taken right after it was read is the same
            // value.
            route_chunks: collect_route_chunks(&rootp, &client_manifest_snapshot),
        };
        match crate::build_graph::record_and_persist(inputs) {
            Ok(snap) => outln!(
                w,
                "graph: {} nodes, {} recomputed (previous snapshot: {} nodes)",
                snap.node_count,
                snap.recomputed_count,
                snap.previous_node_count,
            ),
            Err(e) => eprintln!("warn: graph snapshot could not be written: {e}"),
        }
    }
    // §5.1: remember this build's inputs so an unchanged re-run can skip.
    let fp = build_cache::fingerprint(&rootp, "production", r#static);
    if let Err(e) = build_cache::store(&rootp, &fp) {
        eprintln!("warn: could not write build fingerprint: {e}");
    }
    // SPEC-5D: GC the on-disk artifact cache after every build. Default
    // policy: idle > 7 days, cap 1 GiB. Overridable via env (a long-running
    // CI image would set `OXIDE_CACHE_GC_IDLE_SECS` / `OXIDE_CACHE_GC_MAX_BYTES`).
    // Best-effort — a gc failure logs but does not fail the build.
    {
        let idle_secs: u64 = std::env::var("OXIDE_CACHE_GC_IDLE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(7 * 24 * 3600);
        let max_bytes: u64 = std::env::var("OXIDE_CACHE_GC_MAX_BYTES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1024 * 1024 * 1024);
        let cache_dir = rootp.join(".oxide/cache");
        if cache_dir.is_dir() {
            let mut files = Vec::new();
            walk_files(&cache_dir, &mut files);
            let now = std::time::SystemTime::now();
            let idle = std::time::Duration::from_secs(idle_secs);
            let mut removed = 0usize;
            let mut kept: Vec<(std::path::PathBuf, u64, std::time::SystemTime)> = Vec::new();
            for f in &files {
                if let Ok(meta) = std::fs::metadata(f) {
                    let is_idle = meta
                        .modified()
                        .ok()
                        .and_then(|m| now.duration_since(m).ok())
                        .map(|d| d > idle)
                        .unwrap_or(false);
                    if is_idle && std::fs::remove_file(f).is_ok() {
                        removed += 1;
                    } else if let Ok(m) = meta.modified() {
                        kept.push((f.clone(), meta.len(), m));
                    }
                }
            }
            let mut total: u64 = kept.iter().map(|(_, b, _)| *b).sum();
            if total > max_bytes {
                kept.sort_by_key(|(_, _, m)| *m);
                for (path, bytes, _) in &kept {
                    if total <= max_bytes {
                        break;
                    }
                    if std::fs::remove_file(path).is_ok() {
                        total = total.saturating_sub(*bytes);
                        removed += 1;
                    }
                }
            }
            if removed > 0 {
                tracing::info!(removed, "cache gc: evicted idle/over-cap entries");
            }
        }
    }
    Ok(0)
}

/// A handle over a started server, so tests can shut it down deterministically.
/// Optional dev-server feature flags (§13.5).
#[derive(Debug, Clone, Default)]
pub struct DevFlags {
    pub host: bool,
    pub https: bool,
    pub qr: bool,
    pub open: bool,
    pub port: Option<u16>,
    pub proxy: Vec<String>,
}

impl DevFlags {
    /// Parse the `--proxy` strings into `ProxyRule`s. Format:
    /// `[!]PREFIX=TARGET`, where a leading `!` requests prefix stripping.
    pub fn proxy_rules(&self) -> Vec<oxide_dev::ProxyRule> {
        self.proxy
            .iter()
            .filter_map(|s| {
                let (strip, rest) = match s.strip_prefix('!') {
                    Some(r) => (true, r),
                    None => (false, s.as_str()),
                };
                let (prefix, target) = rest.split_once('=')?;
                let rule = oxide_dev::ProxyRule::new(prefix, target);
                Some(if strip { rule.with_strip(true) } else { rule })
            })
            .collect()
    }
}

pub struct StartedServer {
    pub addr: std::net::SocketAddr,
    pub shutdown: tokio::sync::watch::Sender<bool>,
    /// Metrics registry (§20) shared with the pipeline and the dispatch
    /// delegate; exposed so tests and observability tooling can read it.
    pub metrics: std::sync::Arc<oxide_server::Metrics>,
    /// SEC-09: the dev inspector token, when the inspector is attached (dev
    /// only). The dev banner prints it so the developer can open
    /// `/__oxide?token=…`; `None` in production.
    pub dev_token: Option<String>,
    /// SPEC-14: the dev HMR hub, when dev. The dev loop publishes file
    /// changes to it; `/_oxide/hmr` subscribers receive them.
    pub hmr: Option<std::sync::Arc<oxide_dev::HmrHub>>,
    /// §10.8: the PPR shell store, when `render.ppr` is on. The dev watcher
    /// clears it on every change so a shell captured before an edit cannot
    /// outlive the source it was rendered from.
    pub(crate) ppr_shells: Option<PprShells>,
    /// WP-B: the route tree the dispatch reads through. The dev watcher
    /// stores a fresh tree here after re-scanning; `cmd_serve` never swaps.
    pub(crate) swappable_tree: SwappableRouteTree,
}

/// Boxed dispatch future. Structurally identical to `futures::future::BoxFuture<_,
/// OxideResponse>` (which [`oxide_server::Server::serve`] takes) without needing a
/// `futures` dependency in this crate.
type BoxedResponse =
    std::pin::Pin<Box<dyn std::future::Future<Output = oxide_server::OxideResponse> + Send>>;

/// Inspector adapter for the dev server: converts the CLI's
/// `RouteTableInspector` into the `oxide_server::InspectorSnapshot` trait
/// (§13.4). The trait lives in oxide-server so the pipeline can render it
/// without depending on oxide-dev.
#[derive(Debug)]
struct CliInspector {
    state: oxide_dev::InspectorState,
}

impl oxide_server::InspectorSnapshot for CliInspector {
    fn state_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.state).unwrap_or(serde_json::Value::Null)
    }
    fn render_html(&self) -> String {
        oxide_dev::render_inspector_html(&self.state)
    }
}

/// Construct the production pipeline (§12.1) from config and start serving.
/// The dispatch returns a placeholder page from the router match — the JS host wiring
/// (NodeHost/isolate render) lives in oxide-jshost and plugs into this dispatch later.
/// Production server entry (§12.1): no inspector is attached.
pub async fn start_server(
    root: &str,
    port: Option<u16>,
    stack: crate::framework_shim::FrameworkStack,
) -> anyhow::Result<StartedServer> {
    let bind = port.map(|p| (std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), p));
    start_server_inner_full(root, bind, false, None, Vec::new(), stack, None).await
}

/// Dev server entry (§13): identical pipeline but with the `/__oxide`
/// inspector attached, populated with the route table.
/// Full dev-server starter with optional bind IP + TLS.
/// Dispatch delegate: routing, proxy (§13.5), and — when a `page_renderer` is
/// attached (§7) — framework SSR. Extracted from `start_dev_server_full` so
/// tests can drive it with a fake renderer, and so the framework-adapter
/// wiring has one integration point instead of nesting inside the server
/// startup function.
/// Bridge an `oxide_server::RenderStream` (a `BoxStream` that promises
/// only `Send`) into an `oxide_server::BodyStream` (which promises
/// `Send + Sync` because `BodyExt::boxed` demands it).
///
/// The bridge is an mpsc channel: a spawned task pumps each source chunk
/// into the channel; the channel's `Receiver` is `Sync` (its `recv` takes
/// `&self`), so the resulting body stream satisfies the hyper transport's
/// bound. The alternative — an `UnsyncBoxBody` — would ripple through
/// every serve signature in `server`, `dev`, and `cli`.
///
/// Error conversion: `OxideError` → `std::io::Error`. The code and message
/// are preserved in the io error's text so a hyper-layer log line names
/// the framework error. The kind is `Other` — there is no precise
/// `ErrorKind` for "the worker crashed mid-render".
fn render_stream_to_body_stream(src: oxide_server::RenderStream) -> oxide_server::BodyStream {
    render_stream_with_recovery(src, None, None, String::new())
}

/// SPEC-11 / §10.2.5: the streaming bridge with mid-stream recovery.
///
/// If the shell already committed (we have sent ≥1 chunk) and the render
/// errors, emitting nothing more would truncate the response — the browser
/// shows a half page. Instead we append the `ox-error` recovery boundary (a
/// `data-ox-c` template + self-contained swapper that replaces the matching
/// `data-ox-b` placeholder), then close cleanly. The status stays 200: the
/// shell is valid; only a boundary failed, and the client recovers it.
///
/// `boundary_id`/`nonce` are `Some` for a streamed page; `None` (the
/// `render_stream_to_body_stream` shim) keeps the old abort behaviour for
/// callers that have no boundary context.
fn render_stream_with_recovery(
    src: oxide_server::RenderStream,
    boundary_id: Option<String>,
    nonce: Option<String>,
    route_id: String,
) -> oxide_server::BodyStream {
    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<std::result::Result<bytes::Bytes, std::io::Error>>(8);
    tokio::spawn(async move {
        use futures::StreamExt as _;
        let mut src = src;
        let mut sent_any = false;
        while let Some(item) = src.next().await {
            match item {
                Ok(chunk) => {
                    sent_any = true;
                    if tx.send(Ok(chunk)).await.is_err() {
                        // Receiver dropped — client disconnected.
                        return;
                    }
                }
                Err(e) => {
                    if let (true, Some(id), Some(n)) =
                        (sent_any, boundary_id.as_deref(), nonce.as_deref())
                    {
                        // Shell committed: emit the recovery boundary, then
                        // close cleanly (no io::Error → no connection reset).
                        let html = oxide_render::stream_error_recovery(id, &e.0, n);
                        let _ = tx.send(Ok(bytes::Bytes::from(html))).await;
                        return;
                    }
                    // No shell yet (or no boundary context): the old abort.
                    let _ = tx
                        .send(Err(std::io::Error::new(
                            std::io::ErrorKind::Other,
                            format!("{}: {}", e.0.code, e.0.message),
                        )))
                        .await;
                    return;
                }
            }
        }
        let _ = route_id;
        // `tx` drops here, closing the stream cleanly.
    });
    Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)))
}

/// §10.2 step 5 (error-status contract): peek the render stream's FIRST
/// chunk and scan it for an error-status marker. Returns
/// `(500 | 0, reassembled stream)` — the stream is byte-identical to the
/// input (the peeked chunk is chained back in front).
///
/// Why only the first chunk: the response status is fixed before the body
/// streams; HTTP cannot retroactively change a 200 to a 500 once bytes are
/// on the wire. The adapter therefore announces an error render in the
/// document's FIRST chunk:
///   * `<!--oxide-status:500-->` — the canonical contract marker the
///     adapter emits at the top of an error document (both the project's
///     `error.tsx` render and the generic OX-500 fallback);
///   * `data-ox-error='OX-500'` — the generic fallback boundary the adapter
///     already emits (observable when the whole document lands in one
///     chunk, e.g. the buffered `render_streaming` default).
///
/// TTFB cost: one first-chunk await — the shell chunk real adapters yield
/// before awaiting the body stream, so the shell flush timing is unchanged.
fn probe_render_error_status(
    mut src: oxide_server::RenderStream,
) -> impl std::future::Future<Output = (u16, oxide_server::RenderStream)> + Send {
    use futures::StreamExt as _;
    async move {
        match src.next().await {
            Some(Ok(first)) => {
                let status = if chunk_signals_error(&first) { 500 } else { 0 };
                let reassembled = futures::stream::once(async move { Ok(first) })
                    .chain(src)
                    .boxed();
                (status, reassembled)
            }
            Some(Err(e)) => {
                let reassembled = futures::stream::once(async move { Err(e) })
                    .chain(src)
                    .boxed();
                (0, reassembled)
            }
            None => (0, src),
        }
    }
}

/// The error markers `probe_render_error_status` scans the first chunk for.
fn chunk_signals_error(chunk: &bytes::Bytes) -> bool {
    const CONTRACT_MARKER: &[u8] = b"<!--oxide-status:500-->";
    const GENERIC_MARKER: &[u8] = b"data-ox-error='OX-500'";
    fn has(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }
    has(chunk, CONTRACT_MARKER) || has(chunk, GENERIC_MARKER)
}

/// Same contract as [`chunk_signals_error`], for the buffered (prerender)
/// path where the adapter's output is a `String` rather than a stream of
/// `Bytes`.
fn str_signals_error(s: &str) -> bool {
    chunk_signals_error(&bytes::Bytes::copy_from_slice(s.as_bytes()))
}

/// §10.3: splice `<script type="module" src="/<url>">` into a streaming HTML
/// response right before the closing `</body>` tag. Works when `</body>` is
/// split across chunk boundaries by keeping a rolling tail of the last
/// `CLOSE.len() - 1` bytes and scanning for the tag on each new chunk.
///

#[allow(clippy::too_many_arguments)]
/// §9.3: parse an `application/x-www-form-urlencoded` body into the
/// `[key, value]` entry list the worker's `action:run` op accepts. Percent
/// decoding handles `+` as space. A malformed pair is skipped, not fatal.
fn parse_form_urlencoded(body: &[u8]) -> Vec<(String, String)> {
    let s = String::from_utf8_lossy(body);
    s.split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (
                oxide_server::percent_decode(&k.replace('+', " ")),
                oxide_server::percent_decode(&v.replace('+', " ")),
            )
        })
        .collect()
}

/// §9.3: the action name from a form-action query. `?/comment` → `comment`.
/// The SvelteKit convention: a query that begins with `/` names an action.
fn action_from_query(url: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    let q = q.split('#').next().unwrap_or(q);
    let rest = q.strip_prefix('/')?;
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    Some(oxide_server::percent_decode(rest))
}

/// §5.6: wrap a dispatch so a mid-flight file change aborts instead of
/// serving a result computed from pre-change source.
///
/// Behaviour:
///   - Record the stale generation at entry.
///   - `tokio::select!` races the dispatch future against
///     `hub.wait_stale_generation()`.
///   - If the dispatch completes first AND the generation is unchanged:
///     serve normally.
///   - If the dispatch completes first BUT the generation changed during
///     the window (a race the select! poll order can produce): 503 + retry.
///   - If the change notification fires first: abort the dispatch (drop
///     the future, releasing the renderer / loader resources it held) and
///     503 + retry.
///
/// The 503 response carries:
///   - `retry-after: 0` — a browser fetch with default redirect mode will
///     not auto-retry, but the dev HMR client (which reloads on every file
///     change event) will. A manual reload also gets fresh content.
///   - `x-oxide-stale-generation: <new>` — audit trail: the request was
///     refused because the world advanced past its entry generation.
///
/// Streaming responses are unaffected: a streaming dispatch completes
/// its `OxideResponse` (with a live body stream) early, so the select!
/// window is a few milliseconds — the abort almost never fires. Non-
/// streaming (SSR, `/__data`, loaders) get the full guard.
pub(crate) fn wrap_dispatch_with_stale_guard(
    dispatch: std::sync::Arc<dyn Fn(oxide_server::OxideRequest) -> BoxedResponse + Send + Sync>,
    hub: std::sync::Arc<oxide_dev::HmrHub>,
) -> std::sync::Arc<dyn Fn(oxide_server::OxideRequest) -> BoxedResponse + Send + Sync> {
    std::sync::Arc::new(move |req: oxide_server::OxideRequest| {
        let hub = std::sync::Arc::clone(&hub);
        let inner = std::sync::Arc::clone(&dispatch);
        Box::pin(async move {
            let start = hub.stale_generation();
            let fut = inner(req);
            let result = tokio::select! {
                r = fut => r,
                _ = hub.wait_stale_generation() => {
                    return stale_503(start, hub.stale_generation());
                }
            };
            // Exit check: the select! poll order can let the dispatch
            // future complete on the first tick without polling the
            // notification future — a bump between entry and completion
            // would then be missed. This catches that narrow race.
            let now = hub.stale_generation();
            if now != start {
                return stale_503(start, now);
            }
            result
        })
    })
}

/// §5.6: the retryable 503 a mid-flight file change produces.
fn stale_503(start: u64, now: u64) -> oxide_server::OxideResponse {
    let body = format!(
        "<!doctype html><html><head><title>stale</title></head><body>\
         <p>file changed during dispatch (generation {} to {}); retry</p>\
         </body></html>",
        start, now
    );
    let mut resp = oxide_server::OxideResponse::html(503, body);
    resp.headers
        .push(("retry-after".to_string(), "0".to_string()));
    resp.headers
        .push(("x-oxide-stale-generation".to_string(), now.to_string()));
    resp
}

/// WP-B (deviation 3): a swappable slot for the route tree. The dispatch
/// closure reads through this on every request, so a dev-watcher rescan can
/// publish a fresh `RouteTree` without restarting the server. `build_dispatch`
/// (used by every test) wraps a one-shot swappable that is never swapped; dev
/// passes the live one to `build_dispatch_swappable` directly.
#[derive(Clone)]
pub(crate) struct SwappableRouteTree {
    inner: std::sync::Arc<std::sync::RwLock<std::sync::Arc<oxide_router::RouteTree>>>,
}

impl SwappableRouteTree {
    pub fn new(tree: std::sync::Arc<oxide_router::RouteTree>) -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::RwLock::new(tree)),
        }
    }

    pub fn load(&self) -> std::sync::Arc<oxide_router::RouteTree> {
        std::sync::RwLock::read(&self.inner)
            .expect("route tree lock poisoned")
            .clone()
    }

    pub fn store(&self, tree: std::sync::Arc<oxide_router::RouteTree>) {
        *std::sync::RwLock::write(&self.inner).expect("route tree lock poisoned") = tree;
    }
}

fn build_dispatch(
    tree: std::sync::Arc<oxide_router::RouteTree>,
    metrics: std::sync::Arc<oxide_server::Metrics>,
    proxy_rules: Vec<oxide_dev::ProxyRule>,
    page_renderer: Option<std::sync::Arc<dyn oxide_server::PageRenderer>>,
    loader_runner: Option<std::sync::Arc<dyn oxide_server::LoaderRunner>>,
    routes_root: std::path::PathBuf,
    boot_script_url: Option<String>,
    l0: Option<std::sync::Arc<l0_cache::L0Cache>>,
    actions: Option<std::sync::Arc<dyn oxide_server::ActionDispatcher>>,
    archived: Option<std::sync::Arc<oxide_router::ArchivedTree>>,
    pool: Option<std::sync::Arc<dyn oxide_jshost::Pool>>,
    i18n_cfg: Option<oxide_assets::i18n::I18nConfig>,
    endpoint_dispatcher: Option<std::sync::Arc<dyn oxide_server::EndpointDispatcher>>,
    middleware_dispatcher: Option<std::sync::Arc<dyn oxide_server::MiddlewareDispatcher>>,
    dev_hmr: bool,
    vary_headers: Vec<String>,
    waterfall: Option<oxide_dev::WaterfallRecorder>,
    session_codec: Option<std::sync::Arc<oxide_server::SessionCodec>>,
    ppr_shells: Option<PprShells>,
) -> std::sync::Arc<dyn Fn(oxide_server::OxideRequest) -> BoxedResponse + Send + Sync> {
    build_dispatch_swappable(
        SwappableRouteTree::new(tree),
        metrics,
        proxy_rules,
        page_renderer,
        loader_runner,
        routes_root,
        boot_script_url,
        l0,
        actions,
        archived,
        pool,
        i18n_cfg,
        endpoint_dispatcher,
        middleware_dispatcher,
        dev_hmr,
        vary_headers,
        waterfall,
        session_codec,
        ppr_shells,
    )
}

/// §9.3 / B6: copy an action response's Set-Cookie headers into the
/// middleware-headers list, which the render path applies to every response
/// it produces. Before this helper existed, the 200 re-render path dropped
/// the action response entirely (extracting only the body into action_form),
/// so a cookie-setting action that re-rendered lost its cookie.
///
/// Kept as a named helper so a unit test can exercise the pattern without
/// building a full dispatch fixture (an action fires only for a POST with
/// `?/<action>` on a route that has a server module — a shape that takes a
/// lot of scaffolding to reach).
fn forward_action_set_cookies(
    resp_headers: &[(String, String)],
    middleware_headers: &mut Vec<(String, String)>,
) {
    for (k, v) in resp_headers {
        if k.eq_ignore_ascii_case("set-cookie") {
            middleware_headers.push((k.clone(), v.clone()));
        }
    }
}

fn build_dispatch_swappable(
    swappable: SwappableRouteTree,
    metrics: std::sync::Arc<oxide_server::Metrics>,
    proxy_rules: Vec<oxide_dev::ProxyRule>,
    page_renderer: Option<std::sync::Arc<dyn oxide_server::PageRenderer>>,
    loader_runner: Option<std::sync::Arc<dyn oxide_server::LoaderRunner>>,
    routes_root: std::path::PathBuf,
    boot_script_url: Option<String>,
    l0: Option<std::sync::Arc<l0_cache::L0Cache>>,
    actions: Option<std::sync::Arc<dyn oxide_server::ActionDispatcher>>,
    // PERF-07: when `Some`, route matching reads the mmap'd rkyv archive
    // instead of the in-memory tree. Tests pass `None`.
    archived: Option<std::sync::Arc<oxide_router::ArchivedTree>>,
    // PERF-02: the render/loader pool, so the dispatch can surface its live
    // gauges into metrics. Tests pass `None`.
    pool: Option<std::sync::Arc<dyn oxide_jshost::Pool>>,
    // SPEC-10: the i18n config, when the project declares locales. `None`
    // disables locale resolution (every loader sees `locale = ""`).
    i18n_cfg: Option<oxide_assets::i18n::I18nConfig>,
    // §8.1 route.ts / §10.2 step 7: dispatch a matched Endpoint route.
    // `None` means no `route.ts` execution is possible (a project without
    // a worker pool, or a build-time call site).
    endpoint_dispatcher: Option<std::sync::Arc<dyn oxide_server::EndpointDispatcher>>,
    // §8.1 middleware.ts / §8.5: dispatch a route's middleware before the
    // loader/render phase. `None` means middleware is not executed.
    middleware_dispatcher: Option<std::sync::Arc<dyn oxide_server::MiddlewareDispatcher>>,
    // §13.3: dev-only. When true, inject a self-contained HMR client script
    // (connects to `/_oxide/hmr`, swaps CSS, reloads on server updates, shows
    // the error overlay). `false` in production.
    dev_hmr: bool,
    // §11.2 / §14.1: the request headers the L0 key is fingerprinted by
    // (`cache.l0.vary`). Empty disables header variance.
    vary_headers: Vec<String>,
    // §20: the per-request waterfall recorder. `None` for a non-inspector
    // server; the dispatch then skips recording.
    waterfall: Option<oxide_dev::WaterfallRecorder>,
    // §16.6: the session codec, built from `security.sessionKeys`. `None`
    // when the project configures no keys — session cookies then carry no
    // `locals.session` (the signed-only `sign_cookie` primitive remains for
    // a project that wants a tamper-evident, non-confidential cookie).
    session_codec: Option<std::sync::Arc<oxide_server::SessionCodec>>,
    // §10.8 / B-27: the PPR shell cache. `Some` only when `render.ppr` is on;
    // the dispatch then serves a cached shell and streams holes into it.
    ppr_shells: Option<PprShells>,
) -> std::sync::Arc<dyn Fn(oxide_server::OxideRequest) -> BoxedResponse + Send + Sync> {
    let boot_script_url_for_render = boot_script_url.clone();
    let ppr_for_render = ppr_shells.clone();
    let dev_hmr_for_render = dev_hmr;
    let actions_for_dispatch = actions.clone();
    let endpoint_for_dispatch = endpoint_dispatcher.clone();
    let middleware_for_dispatch = middleware_dispatcher.clone();
    // PERF-04: single-flight coalescing. Concurrent misses for the same L0
    // key run one render; the rest await the owner's entry. One map per
    // process, shared across every dispatch call.
    let flight_map: std::sync::Arc<
        dashmap::DashMap<
            String,
            std::sync::Arc<tokio::sync::broadcast::Sender<std::sync::Arc<oxide_cache::Entry>>>,
        >,
    > = std::sync::Arc::new(dashmap::DashMap::new());
    // §8.5: a middleware `{ rewrite }` re-routes the request to another path.
    // The cleanest way to honour that (without duplicating the 280-line route
    // precompute) is a bounded self-redispatch: after middleware returns a
    // rewrite, this closure re-invokes itself with the rewritten URL and a
    // hop header, so the new URL goes through the normal match → loader →
    // render path. The hop header prevents an infinite rewrite loop (a
    // middleware that always rewrites to itself would otherwise spin).
    let self_dispatch: std::sync::Arc<
        std::sync::OnceLock<
            std::sync::Arc<dyn Fn(oxide_server::OxideRequest) -> BoxedResponse + Send + Sync>,
        >,
    > = std::sync::Arc::new(std::sync::OnceLock::new());
    let self_dispatch_for_closure = std::sync::Arc::clone(&self_dispatch);
    let dispatch_fn: std::sync::Arc<
        dyn Fn(oxide_server::OxideRequest) -> BoxedResponse + Send + Sync,
    > = std::sync::Arc::new(move |req: oxide_server::OxideRequest| {
        let tree = swappable.load();
        let started = std::time::Instant::now();
        // Rewrite-hop guard (§8.5 / S-20): the hop count is tracked on the
        // internal `rewrite_hops` field, never a client-supplied header — a
        // client cannot pre-set it to exhaust the rewrite budget. Inbound
        // `x-oxide-rewrite-hop` headers are stripped at `from_hyper_request`.
        let rewrite_hop: u32 = req.rewrite_hops as u32;
        let _is_rewritten = rewrite_hop > 0;
        if rewrite_hop > 8 {
            return Box::pin(async move {
                oxide_server::OxideResponse::html(
                    508,
                    "middleware rewrite loop detected (too many hops)",
                )
            });
        }
        // PERF-02: surface the pool's live gauges into the metrics registry.
        if let Some(p) = pool.as_ref() {
            let s = p.stats();
            metrics.observe_pool(s.workers, s.in_flight, s.queue_wait_ms_p99);
        }
        let raw = req.url.split(['?', '#']).next().unwrap_or("/").to_string();

        // §13.5 proxy: if the request matches a rule, forward upstream and
        // skip routing entirely. Response is passed through unchanged.
        if !proxy_rules.is_empty() {
            let query = req.url.split_once('?').map(|(_, q)| q.to_string());
            if let Some(m) = oxide_dev::apply_proxy(&proxy_rules, &raw, query.as_deref()) {
                let url = m.url.clone();
                let m_rule = m.rule.prefix.clone();
                let metrics_proxy = std::sync::Arc::clone(&metrics);
                let req_owned = req.clone();
                return Box::pin(async move {
                    match oxide_server::forward_proxy(&url, req_owned).await {
                        Ok(r) => {
                            metrics_proxy.observe("_proxy", started.elapsed(), r.status);
                            r
                        }
                        Err(e) => {
                            tracing::warn!(rule = %m_rule, error = %e, "proxy forward failed");
                            let r = oxide_server::OxideResponse::new(
                                502,
                                format!("proxy upstream error: {}", e.0.message),
                            );
                            metrics_proxy.observe("_proxy", started.elapsed(), 502);
                            r
                        }
                    }
                });
            }
        }
        // Route conventions: the root page module is `index`, so `/` maps to
        // the `/index` pattern (spec §8.1); trailing slashes are folded too.
        let normalized = if raw.len() > 1 && raw.ends_with('/') {
            raw.trim_end_matches('/').to_string()
        } else {
            raw
        };
        // §11.3 / BUG-01: remember the request path. The L0 entry for
        // this request is tagged `path:<req_path>` so a `revalidate.path`
        // event (which reaches the dispatch via `x-oxide-revalidate`)
        // actually invalidates it. Without this, entries are tagged with
        // the *route id* only, and path events match nothing.
        let req_path = normalized.clone();
        // SPEC-10 / §16.4: resolve the request locale when the project
        // configures i18n — path prefix first, then `Accept-Language`,
        // then the default. Empty when no i18n is configured.
        // `i18n_cfg` is captured by the dispatch closure (built from config
        // before any request); `req` supplies the header.
        let resolved_locale: String = match i18n_cfg.as_ref() {
            Some(a) => {
                let stripped = oxide_assets::i18n::strip_locale_prefix(a, &req_path);
                let from_path = stripped.0 != a.default && stripped.1 != req_path.as_str();
                if from_path {
                    stripped.0
                } else {
                    oxide_assets::i18n::negotiate(a, req.header("accept-language").unwrap_or(""))
                }
            }
            None => String::new(),
        };
        // PERF-01 / §8.4: a `/__data` GET names its target route via
        // `?route=<path>`. The endpoint re-matches THAT path against the
        // route tree (below), so the whole prelude — route id, params,
        // layout chain, nearest middleware, error boundary — describes
        // the route whose data the client router is fetching. That is
        // what makes client-side navigation data match the SSR render
        // (§8.4 parity); previously /__data matched nothing, ran no
        // middleware, no layout loads, and no defer resolution.
        let data_route_q: String =
            if req.method.eq_ignore_ascii_case("GET") && req_path == "/__data" {
                let raw_q = req.url.split_once('?').map(|(_, q)| q).unwrap_or("");
                raw_q
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("route=").map(oxide_server::percent_decode))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "/".to_string())
            } else {
                String::new()
            };
        let is_data_request = !data_route_q.is_empty();
        // §8.4: for a data request, match the TARGET route (trailing
        // slash folded like a real URL path); otherwise the request path.
        let match_path = if is_data_request {
            let stripped = data_route_q.trim_end_matches('/');
            if stripped.is_empty() {
                "/".to_string()
            } else {
                stripped.to_string()
            }
        } else {
            normalized.clone()
        };
        let candidates = if match_path == "/" {
            vec!["/".to_string(), "/index".to_string()]
        } else {
            vec![match_path]
        };
        let matched: Option<oxide_router::MatchResult> = match &archived {
            Some(a) => candidates.iter().find_map(|p| a.match_path(p)),
            None => candidates.iter().find_map(|p| tree.match_path(p)),
        };
        let (status, label) = match &matched {
            Some(m) => (200u16, pattern_of(&m.route.segments)),
            None => (404u16, "_unmatched".to_string()),
        };

        // Pre-compute the loader inputs synchronously so the async block
        // owns plain data (no borrow of `matched`).
        let (module_path, route_id, params, has_server_module, render_mode, needs_parent): (
            Option<std::path::PathBuf>,
            Option<String>,
            std::collections::BTreeMap<String, String>,
            bool,
            Option<String>,
            bool,
        ) = match &matched {
            Some(m) => {
                let has_srv = m.route.has_server_module;
                // BUG: only produce a module_path when the route actually
                // declares a server module. Setting it unconditionally made
                // `route_data_response` (and any consumer that keyed on
                // `module_path.is_some()` rather than `has_server_module`)
                // fire the loader for routes whose sibling `page.server.ts`
                // does not exist — a 500 with `ENOENT` from `/__data`.
                let mp = if has_srv {
                    Some(routes_root.join(m.route.file.with_file_name("page.server.ts")))
                } else {
                    None
                };
                (
                    mp,
                    Some(m.route.id.0.clone()),
                    m.params.clone(),
                    has_srv,
                    m.route.render_mode.clone(),
                    m.route.needs_parent,
                )
            }
            None => (
                None,
                None,
                std::collections::BTreeMap::new(),
                false,
                None,
                false,
            ),
        };

        // §8.1 route.ts / §10.2 step 7: pre-compute whether this is an
        // Endpoint route and, if so, the on-disk path to its `route.ts`.
        // Endpoints bypass the loader/render phases entirely: a `route.ts`
        // returns a Web `Response` directly.
        let endpoint_module_path: Option<std::path::PathBuf> = match &matched {
            Some(m) if matches!(m.route.kind, oxide_router::RouteKind::Endpoint) => {
                Some(routes_root.join(&m.route.file))
            }
            _ => None,
        };

        // §8.1 middleware.ts / §8.5: resolve the full ancestor middleware
        // chain for the matched route, OUTERMOST first. Every middleware
        // whose directory is an ancestor of the route's directory runs, in
        // outer→inner order — a `routes/middleware` wraps
        // `routes/blog/middleware`, which wraps the page. (Previously only
        // the nearest ancestor ran, which dropped outer auth/security
        // middleware; §8.5 requires the chain.)
        //
        // Ordering key: ancestor-directory length. `routes` (len 6) sorts
        // before `routes/blog` (len 11), i.e. outer before inner. Ties
        // cannot happen (each directory yields at most one middleware
        // file). An unmatched URL still gets the root middleware, matching
        // the previous behaviour.
        // P-06: served from the tree's precomputed prefix index — no full
        // entry scan, no per-candidate `format!`, no sort per request.
        let middleware_module_paths: Vec<std::path::PathBuf> = tree
            .middleware_files_for(route_id.as_deref().unwrap_or(""))
            .into_iter()
            .map(|f| routes_root.join(f))
            .collect();
        let endpoint_params_json: serde_json::Map<String, serde_json::Value> = params
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        // P-07: header/method/url are borrowed from `req_owned` at the
        // dispatcher call sites instead of cloned per request.
        let endpoint_body: bytes::Bytes = req.body.clone();

        // §10.2 step 5 / §8.3: an unmatched URL gets a 404 status (the
        // `status` label above). To render a user-visible 404 page the
        // request must resolve to the project's `not-found.tsx` route id
        // (with its layout chain), not to a placeholder component that
        // `componentFor` cannot find. Precompute both here, before
        // `matched` is consumed, so the async render block below can use
        // them.
        // P-06: not-found route + layout chain, precomputed by the tree index.
        let (unmatched_route_id, unmatched_layout_ids): (Option<String>, Vec<String>) =
            match tree.not_found_route() {
                Some((id, chain)) => (Some(id), chain.into_iter().map(|r| r.0).collect()),
                None => (None, Vec::new()),
            };

        // §10.2 step 5: resolve the NEAREST ancestor `error.tsx` for the
        // matched route (innermost boundary wins — a `routes/blog/error`
        // shadows `routes/error`). Precomputed here so the async render
        // block can pass it as `input.error_route_id`. The adapter
        // renders this component when the page throws.
        // P-06: nearest error boundary from the precomputed index.
        let error_route_id: Option<String> =
            tree.error_route_for(route_id.as_deref().unwrap_or(""));
        // The URL pattern (`pattern_of(segments)`) is for metrics labels;
        // the ROUTE ID is what the framework adapter's `componentFor`
        // expects. Two conventions to reconcile here:
        //
        //   • Router ids include the file role: `routes/index/page`,
        //     `routes/blog/[slug]/page`. That's the shape the scanner
        //     produces (see crates/router/src/scan.rs) and what
        //     `RouteEntry.id.0` carries.
        //   • Adapter keys (`PAGES` in the IR renderer, `componentFor`
        //     in a user's config) are keyed on the route directory:
        //     `routes/index`, `routes/blog/[slug]`. Trailing role
        //     suffix (`/page`, `/layout`) is not part of the key.
        //
        // Normalize by stripping a trailing `/page` (and only `/page`)
        // before handing the id to the renderer. Layout-only ids, slot
        // ids (`routes/@modal/page`), and group-prefixed ids all pass
        // through untouched.
        let route_id_for_render: Option<String> = route_id
            .as_deref()
            .map(|s| s.strip_suffix("/page").unwrap_or(s).to_string());

        // §8.2 / §16.3: precompute the layout chain (outermost first)
        // before `matched` is dropped. Ids carry the role suffix the
        // scanner emits (`routes/layout`), which is also the key
        // adapters use for `componentFor` (same convention as the page
        // id after `/page` is stripped). An unmatched URL gets an empty
        // chain.
        let layout_ids: Vec<String> = match &matched {
            Some(m) => m.layout_chain.iter().map(|id| id.0.clone()).collect(),
            None => Vec::new(),
        };
        // §9.2: layout module paths for the parallel layout-loads phase.
        // Each layout id (`routes/blog/layout`) maps to its
        // `layout.server.ts` on disk, in outermost → innermost order.
        let layout_module_paths: Vec<(String, std::path::PathBuf)> = layout_ids
            .iter()
            .filter_map(|lid| {
                tree.entries
                    .iter()
                    .find(|e| e.id.0 == *lid && matches!(e.kind, oxide_router::RouteKind::Layout))
                    .map(|e| {
                        (
                            lid.clone(),
                            routes_root.join(e.file.with_file_name("layout.server.ts")),
                        )
                    })
            })
            .filter(|(_, p)| p.is_file())
            .collect();

        // §9.3: a form POST to `?/<name>` is an action invocation. Only a
        // POST with a named action on a route that has a server module is
        // eligible; everything else falls through to the loader/render path.
        let action_name: Option<String> = if req.method.eq_ignore_ascii_case("POST") {
            action_from_query(&req.url)
        } else {
            None
        };
        let action_dispatcher = actions_for_dispatch.clone();
        let action_params: serde_json::Map<String, serde_json::Value> = params
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        let action_route = route_id_for_render.clone().unwrap_or_default();

        let page_renderer = page_renderer.clone();
        let loader_runner = loader_runner.clone();
        let metrics = std::sync::Arc::clone(&metrics);
        // P-07: `req` is this invocation's own parameter, so move it into
        // the async block instead of cloning the whole request per call.
        let req_owned = req;
        // Shadow captured values that the async block consumes (an `Fn`
        // closure cannot surrender a captured variable — E0507).
        let boot_script_url_for_render = boot_script_url_for_render.clone();
        // §8.1 route.ts / §10.2 step 7: same pattern for the endpoint
        // dispatcher — the async block below moves this per-call clone.
        let endpoint_for_dispatch = endpoint_for_dispatch.clone();
        // §8.1 middleware.ts / §8.5: same shadow for middleware.
        let middleware_for_dispatch = middleware_for_dispatch.clone();
        // §8.5: same shadow for the self-dispatch handle used by a
        // middleware rewrite (the async block moves it per-call).
        let self_dispatch_for_closure = self_dispatch_for_closure.clone();
        // §20: the waterfall recorder (Arc inside, cheap to clone) is moved
        // by the async block; shadow it per call like the dispatchers above.
        let waterfall = waterfall.clone();
        // §16.6: same shadow for the session codec.
        let session_codec = session_codec.clone();
        // §11 L0: only a GET on a cacheable-mode route is a candidate.
        let l0 = l0.clone();
        // §11.1: L0 caches the FULL response for cacheable modes. The
        // layer table puts no server-module restriction on L0 ("L0 | Full
        // response (HTML, headers, tags) | Rust, no JS on hit" — spec
        // §11.1), and loader routes invalidate through the same
        // revalidate.tag/path channel as every other entry (BUG-01), so
        // the previous `!has_server_module` exclusion is gone. The
        // render mode is the only gate: dynamic routes opt out via
        // `mode_is_cacheable` (static | isr — §10.1). Caveat noted for
        // reviewers: the Vary fingerprint today covers Accept-Language
        // only (§11.2 also lists declared cookies — a follow-up).
        // A /__data request is NEVER L0-eligible: it must re-run the
        // loaders, not serve a route's cached HTML.
        // §11.1 / §10.1: normalize `render_mode` the same way the persisted
        // server manifest does (cmd_build line ~3681): `None` + no server
        // module → static, `None` + server module → dynamic. Without this,
        // a page route whose scanner left `render_mode = None` (no
        // page.server.ts AND no explicit config.render) reaches dispatch
        // with `None`; `mode_is_cacheable(None)` returns false → the L0
        // branch is skipped entirely and every request answers
        // `x-oxide-cache: bypass` (the manifest, meanwhile, shows `static`).
        // Making dispatch agree with the manifest is the fix.
        let effective_mode: &str = match render_mode.as_deref() {
            Some(m) => m,
            None => {
                if has_server_module {
                    "dynamic"
                } else {
                    "static"
                }
            }
        };
        let l0_eligible = req_owned.method.eq_ignore_ascii_case("GET")
            && l0_cache::mode_is_cacheable(Some(effective_mode))
            && route_id.is_some()
            && !is_data_request;
        let l0_route = route_id_for_render.clone().unwrap_or_default();
        let l0_params = {
            let mut ks: Vec<(&String, &String)> = params.iter().collect();
            ks.sort();
            ks.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&")
        };
        // §11.2 / §14.1: fingerprint exactly the headers `cache.l0.vary` names.
        let l0_vary = l0_cache::vary_of(&req_owned.headers, &vary_headers);
        // BUG-01: the path tag the L0 put site stamps alongside the route
        // tag, so `revalidate.path` can invalidate it.
        let l0_path = req_path;
        // PERF-04: the single-flight key (matches the L0 key shape) and the
        // shared map handle.
        let flight_key = match l0.as_ref() {
            Some(c) => l0_cache::L0Cache::key_for(&l0_route, &l0_params, &l0_vary, c.build_id()),
            None => String::new(),
        };
        let flight_map = std::sync::Arc::clone(&flight_map);
        let ppr_for_request = ppr_for_render.clone();

        Box::pin(async move {
            // §8.1 middleware.ts / §8.5: run the route's FULL ancestor
            // middleware chain, OUTERMOST first, BEFORE the loader/render
            // phase — and before the /__data branch (§8.4 parity: client
            // nav data must see the same middleware locals/headers/
            // short-circuits as SSR). Each middleware can:
            //   * short-circuit with a `Response` or `{ redirect }`;
            //   * inject `locals` (merged into the accumulated map,
            //     delivered to the next middleware AND the loader via
            //     `LoaderRunInput.locals`);
            //   * add response headers (accumulated).
            // A `{ rewrite }` re-dispatches through the normal path. An
            // error surfaces as a 500 with a `MIDDLEWARE`-coded envelope.
            let stage_middleware_start = std::time::Instant::now();
            let mut middleware_locals: serde_json::Value = serde_json::json!({});
            // §16.6: decode the encrypted session cookie into
            // `locals.session` so a loader/action reads it as an object. A
            // missing or tampered cookie is an empty session, never an error.
            // B-05: the sealed token rides the `Cookie` header, never a
            // header named `oxide_session`; read it through the cookie parser.
            if let Some(codec) = session_codec.as_deref() {
                if let Some(token) = req_owned
                    .header("cookie")
                    .and_then(|h| oxide_server::cookie_value(h, "oxide_session"))
                {
                    if let Ok(sess) = oxide_server::Session::decode(codec, token) {
                        middleware_locals["session"] = sess.to_value();
                    }
                }
            }
            // B-05: baseline for the session write-back. Middleware is the
            // only phase that can refine `locals.session` (the loader gets a
            // deserialized copy), so diffing around it captures every change.
            let session_before = middleware_locals.get("session").cloned();
            let mut middleware_headers: Vec<(String, String)> = Vec::new();
            // B-22: resume after the layer that issued the rewrite.
            let resume_after = req_owned.mw_resume_after.clone();
            let mut pending_resume = resume_after
                .as_ref()
                .map(|_| {
                    // Only honour the marker when the target layer is present in
                    // this route's chain; otherwise run the chain normally.
                    let present = middleware_module_paths
                        .iter()
                        .any(|p| p.to_string_lossy() == resume_after.as_deref().unwrap_or(""));
                    present.then(|| resume_after.clone().unwrap_or_default())
                })
                .flatten();
            if let Some(dispatcher) = middleware_for_dispatch.as_deref() {
                for mp in &middleware_module_paths {
                    let module_path_s = mp.to_string_lossy().to_string();
                    if let Some(target) = pending_resume.as_ref() {
                        if &module_path_s == target {
                            // The rewriting layer itself must not run twice.
                            pending_resume = None;
                        }
                        continue;
                    }
                    // Pass the full request (headers + body) so an auth /
                    // upload middleware sees what it needs, plus the
                    // locals accumulated by outer middlewares (None when
                    // nothing has accumulated — see the trait doc).
                    let locals_for_call: Option<&serde_json::Value> = if middleware_locals
                        .as_object()
                        .map_or(false, |o| !o.is_empty())
                    {
                        Some(&middleware_locals)
                    } else {
                        None
                    };
                    let mw_res = dispatcher
                        .dispatch(
                            &module_path_s,
                            &req_owned.method,
                            &req_owned.url,
                            &endpoint_params_json,
                            &req_owned.headers,
                            endpoint_body.as_ref(),
                            locals_for_call,
                        )
                        .await;
                    match mw_res {
                        Ok(out) => {
                            if out.short {
                                metrics.observe(&label, started.elapsed(), out.status);
                                let mut resp = oxide_server::OxideResponse::new(
                                    out.status,
                                    bytes::Bytes::from(out.body),
                                );
                                for (k, v) in &out.headers {
                                    resp = resp.with_header(k, v.clone());
                                }
                                // §8.5: the worker returns a middleware
                                // redirect as a sibling `redirect` field
                                // (`{ short: true, status, redirect }`), not
                                // as a `location` header. Without this the
                                // browser got a 302 with no destination.
                                if let Some(loc) = out.redirect.as_deref() {
                                    if !resp
                                        .headers
                                        .iter()
                                        .any(|(k, _)| k.eq_ignore_ascii_case("location"))
                                    {
                                        resp = resp.with_header("location", loc);
                                    }
                                }
                                // §16.6 / WP-r10-1 / B-05: a middleware that
                                // mutates `locals.session` AND short-circuits
                                // must persist the mutation. The post-loop
                                // write-back (below) never runs for a
                                // short-circuit, so this return path is the
                                // only place the cookie can ride.
                                if let Some(codec) = session_codec.as_deref() {
                                    let after =
                                        out.locals.as_ref().and_then(|l| l.get("session")).cloned();
                                    if after.is_some() && after != session_before {
                                        let mut sess = oxide_server::Session::new();
                                        if let Some(obj) =
                                            after.as_ref().and_then(|v| v.as_object())
                                        {
                                            for (k, v) in obj {
                                                sess.insert(k.clone(), v.clone());
                                            }
                                        }
                                        if let Ok(token) = sess.encode(codec) {
                                            resp = resp.with_header(
                                                "set-cookie",
                                                format!(
                                                    "oxide_session={token}; Path=/; \
                                                     HttpOnly; SameSite=Lax; Max-Age={}",
                                                    session_ttl_secs()
                                                )
                                                .replace(
                                                    "\
", "",
                                                )
                                                .replace("  ", " "),
                                            );
                                        }
                                    }
                                }
                                return resp;
                            }
                            // B-06: last-writer-wins per header name — an
                            // inner middleware refines what an outer one
                            // set, matching the locals merge below. Appending
                            // both produced duplicate header lines.
                            for (k, v) in &out.headers {
                                let lower = k.to_ascii_lowercase();
                                match middleware_headers
                                    .iter_mut()
                                    .find(|(ek, _)| ek.eq_ignore_ascii_case(&lower))
                                {
                                    Some((_, ev)) => *ev = v.clone(),
                                    None => middleware_headers.push((k.clone(), v.clone())),
                                }
                            }
                            // P-08: consume the locals by value — the merge
                            // used to deep-clone every key/value per hop.
                            if let Some(serde_json::Value::Object(src)) = out.locals {
                                let dst = middleware_locals
                                    .as_object_mut()
                                    .expect("middleware_locals is always an object");
                                for (k, v) in src {
                                    dst.insert(k, v);
                                }
                            }
                            if let Some(rw) = out.rewrite.as_deref() {
                                // §8.5: honour the rewrite by re-dispatching
                                // the request to the new URL through the
                                // normal match → loader → render path. The
                                // rewrite-hop header prevents an infinite
                                // loop when a middleware always rewrites.
                                if let Some(re_dispatch) = self_dispatch_for_closure.get() {
                                    let mut new_req = req_owned.clone();
                                    new_req.url = rw.to_string();
                                    // S-20: bump the internal counter, not a
                                    // header, so it cannot be forged.
                                    new_req.rewrite_hops = (rewrite_hop + 1) as u8;
                                    new_req.mw_resume_after = Some(module_path_s.clone());
                                    return re_dispatch(new_req).await;
                                }
                                tracing::warn!(
                                    route = %label,
                                    rewrite = %rw,
                                    "middleware rewrite: self-dispatch unavailable",
                                );
                            }
                        }
                        Err(e) => {
                            metrics.observe(&label, started.elapsed(), e.status);
                            return oxide_server::OxideResponse::json(
                                e.status,
                                serde_json::json!({"error": {
                                    "code": e.code,
                                    "message": e.message,
                                    "route": label,
                                }}),
                            );
                        }
                    }
                }
            }
            // B-05: write-back. If a middleware layer changed
            // `locals.session`, seal it and ride a `Set-Cookie` on every
            // response path via `middleware_headers`.
            if let Some(codec) = session_codec.as_deref() {
                let after = middleware_locals.get("session").cloned();
                if after.is_some() && after != session_before {
                    let mut sess = oxide_server::Session::new();
                    if let Some(obj) = after.as_ref().and_then(|v| v.as_object()) {
                        for (k, v) in obj {
                            sess.insert(k.clone(), v.clone());
                        }
                    }
                    match sess.encode(codec) {
                        Ok(token) => middleware_headers.push((
                            "set-cookie".to_string(),
                            format!(
                                "oxide_session={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
                                session_ttl_secs()
                            ),
                        )),
                        Err(e) => tracing::warn!(
                            error = %e.0.message,
                            "session encode failed; cookie not set",
                        ),
                    }
                }
            }
            // PERF-06: the middleware phase is done — record its duration
            // so `Server-Timing` reports the real cost (§12.1 step 8).
            let middleware_ms = stage_middleware_start.elapsed().as_secs_f64() * 1000.0;

            // PERF-01 / §8.4: `/__data` is the client router's route-data
            // endpoint. The target route was re-matched in the prelude, so
            // this branch runs with the SAME middleware locals/headers the
            // SSR render saw, the layout-load parent chain, the page
            // loader's data + resolved defers, and the resolved locale —
            // no render, just loaders + ODF encode.
            if is_data_request {
                return route_data_response(
                    &req_owned,
                    loader_runner.as_ref(),
                    module_path.as_deref(),
                    route_id.as_deref(),
                    &params,
                    &metrics,
                    started,
                    &middleware_locals,
                    &middleware_headers,
                    &layout_module_paths,
                    &resolved_locale,
                    session_codec.as_deref(),
                    middleware_ms,
                    waterfall.as_ref(),
                )
                .await;
            }

            // §9.3: a form action POST runs the route's `actions[name]`.
            // A successful action does NOT return its JSON to the browser —
            // the spec's form-first contract is "runs, and re-renders the
            // page with `form` result data". The result is carried into the
            // render phase through this variable; a redirect or error status
            // still returns directly (POST/redirect/GET, and a failed action
            // must not render a success page).
            let mut action_form: Option<serde_json::Value> = None;
            if let (Some(action), Some(module_path)) =
                (action_name.as_deref(), module_path.as_deref())
            {
                if let Some(dispatcher) = action_dispatcher.as_deref() {
                    let form_json = {
                        let entries = parse_form_urlencoded(&req_owned.body);
                        serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string())
                    };
                    let action_trace = req_owned.trace.as_ref().and_then(|t| {
                        t.to_traceparent(&t.parent_id).map(
                            |tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}),
                        )
                    });
                    let resp = oxide_server::run_action(
                        Some(dispatcher),
                        &module_path.to_string_lossy(),
                        &action_route,
                        action,
                        &form_json,
                        &action_params,
                        &req_owned.url,
                        &req_owned.method,
                        &req_owned.headers,
                        &middleware_locals,
                        action_trace.as_ref(),
                    )
                    .await;
                    // B-17: actions get their own waterfall row so the
                    // inspector shows form-POST latency next to renders.
                    if let Some(rec) = waterfall.as_ref() {
                        let total = started.elapsed().as_secs_f64() * 1000.0;
                        rec.record_with_trace(
                            &req_owned.request_id,
                            vec![
                                ("middleware".to_string(), middleware_ms),
                                ("action".to_string(), (total - middleware_ms).max(0.0)),
                            ],
                            req_owned.trace.as_ref().map(|t| t.trace_id.clone()),
                        );
                    }
                    // §11.3: apply any tag/path invalidations the action
                    // asked for, then return the response.
                    if let Some(rev) = resp.header("x-oxide-revalidate") {
                        let events: Vec<(String, String)> = rev
                            .split(',')
                            .filter_map(|e| {
                                e.split_once(':')
                                    .map(|(k, v)| (k.to_string(), v.to_string()))
                            })
                            .collect();
                        if let Some(cache) = l0.as_ref() {
                            // BUG-01: before this fix, `tag` events were
                            // dropped and `path` events were passed to
                            // `invalidate_tag` raw (no `path:` prefix), so
                            // neither ever matched a cached entry.
                            for (kind, value) in &events {
                                match kind.as_str() {
                                    "tag" => {
                                        cache.invalidate_tag(value, true).await;
                                    }
                                    "path" => {
                                        cache.invalidate_path(value, true).await;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        // PPR shells carry the same tags; drop them with the
                        // L0 entries so a revalidate action cannot leave a
                        // stale shell serving the old document.
                        if let Some(shells) = ppr_for_request.as_ref() {
                            for (kind, value) in &events {
                                if let Ok(mut store) = shells.lock() {
                                    match kind.as_str() {
                                        "tag" => {
                                            store.invalidate_tag(value);
                                        }
                                        "path" => {
                                            store.invalidate_path(value);
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    // §8.5: headers the middleware phase added ride the
                    // action response (they were accumulated but never
                    // applied — the previous drop made a CSP/security
                    // header set in middleware vanish on POSTs).
                    let mut resp = resp;
                    for (k, v) in &middleware_headers {
                        resp = resp.with_header(k, v.clone());
                    }
                    // §9.3: 200 → carry the result into the render and fall
                    // through. Anything else (a redirect, a validation
                    // failure) is the action's own answer and returns.
                    if resp.status == 200 {
                        // B6 fix: copy the action's Set-Cookie headers into
                        // middleware_headers before the response is dropped —
                        // the render path applies middleware_headers to every
                        // response it produces, so the cookie rides the
                        // re-rendered response. Non-200 responses return `resp`
                        // directly (they already carry the headers).
                        forward_action_set_cookies(&resp.headers, &mut middleware_headers);
                        action_form = serde_json::from_slice::<serde_json::Value>(&resp.body).ok();
                    } else {
                        metrics.observe(&label, started.elapsed(), resp.status);
                        return resp;
                    }
                }
            }

            // §11.1 L0: a cache hit serves the stored response with the
            // live request's CSP nonce substituted in, no JS on the path.
            if l0_eligible {
                if let Some(cache) = l0.as_ref() {
                    if let Some(hit) = cache.get(&l0_route, &l0_params, &l0_vary).await {
                        // PERF-04: a conditional request whose validator
                        // matches the stored ETag gets a 304 (no body). The
                        // ETag is weak-formatted so a cache can still serve
                        // it after a nonce-only body change.
                        let etag = format!("W/\"{}\"", hit.etag());
                        let inm = req_owned.header("if-none-match").map(str::to_string);
                        if inm.as_deref().is_some_and(|v| {
                            v.split(',')
                                .map(str::trim)
                                .any(|t| t == etag || t == hit.etag() || t == "*")
                        }) {
                            let mut resp = oxide_server::OxideResponse::new(304, "");
                            resp = resp.with_header("etag", etag.clone());
                            resp = resp.with_header("x-oxide-cache", "L0-hit");
                            // P0-5: middleware runs per request and its
                            // headers must reflect THIS request, not the
                            // request that stored the entry. Apply before
                            // returning so an L0 hit carries them too.
                            for (k, v) in &middleware_headers {
                                resp = resp.with_header(k, v.clone());
                            }
                            metrics.observe(&label, started.elapsed(), 304);
                            if let Some(rec) = waterfall.as_ref() {
                                rec.record(
                                    &req_owned.request_id,
                                    vec![(
                                        "cache-hit".to_string(),
                                        started.elapsed().as_secs_f64() * 1000.0,
                                    )],
                                );
                            }
                            return resp;
                        }
                        // P-05: stale-while-revalidate. A stale hit is served
                        // immediately, and the entry is dropped in the
                        // background so the *next* request misses, re-renders
                        // and re-stores (single-flight coalesces concurrent
                        // misses). Previously the entry served stale forever
                        // within the SWR window with nobody refreshing it.
                        if hit.stale {
                            let bg = cache.clone();
                            let bg_path = l0_path.clone();
                            if let Some(shells) = ppr_for_request.as_ref() {
                                let bg_shells = std::sync::Arc::clone(shells);
                                let bg_shell_path = bg_path.clone();
                                tokio::spawn(async move {
                                    if let Ok(mut store) = bg_shells.lock() {
                                        store.invalidate_path(&bg_shell_path);
                                    }
                                });
                            }
                            tokio::spawn(async move {
                                let _ = bg.invalidate_path(&bg_path, false).await;
                            });
                        }
                        let nonce = oxide_server::nonce_for(&req_owned.request_id);
                        let body = hit.materialize(&nonce);
                        let mut resp = oxide_server::OxideResponse::new(hit.status(), body);
                        for (k, v) in &hit.headers() {
                            resp = resp.with_header(k, v.clone());
                        }
                        resp = resp.with_header("etag", etag);
                        // §20: an explicit cache-outcome header makes the L0
                        // layer observable and testable. A stale serve
                        // (within the SWR window) is labelled so the caller
                        // can see it; background regeneration is scoped.
                        resp = resp.with_header(
                            "x-oxide-cache",
                            if hit.stale { "L0-stale" } else { "L0-hit" },
                        );
                        // P0-5: middleware headers reflect this request; a
                        // cached body must not suppress an auth/security
                        // header the middleware just set.
                        for (k, v) in &middleware_headers {
                            resp = resp.with_header(k, v.clone());
                        }
                        metrics.observe(&label, started.elapsed(), hit.status());
                        // B-17: hit rows sit next to miss rows in the
                        // inspector so cache-hit rate is visible.
                        if let Some(rec) = waterfall.as_ref() {
                            rec.record_with_trace(
                                &req_owned.request_id,
                                vec![(
                                    if hit.stale {
                                        "cache-stale"
                                    } else {
                                        "cache-hit"
                                    }
                                    .to_string(),
                                    started.elapsed().as_secs_f64() * 1000.0,
                                )],
                                req_owned.trace.as_ref().map(|t| t.trace_id.clone()),
                            );
                        }
                        return resp;
                    }
                }
            }

            // PERF-04: single-flight. A concurrent miss for the same L0
            // key awaits the in-flight owner instead of rendering again.
            // S-15: a guard (not a bare sender) so *every* owner exit path —
            // loader error/redirect, cookie short-circuit, render error —
            // clears the in-flight map; the guard's Drop does it.
            let mut flight_owner: Option<oxide_cache::FlightOwnerGuard> = None;
            if l0_eligible && l0.is_some() {
                use oxide_cache::FlightSlot;
                match oxide_cache::SingleFlight::new()
                    .acquire(&flight_map, &flight_key)
                    .await
                {
                    FlightSlot::Follower(mut rx) => {
                        if let Ok(Ok(entry)) =
                            tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
                                .await
                        {
                            let nonce = oxide_server::nonce_for(&req_owned.request_id);
                            let body = String::from_utf8_lossy(&entry.body)
                                .replace(l0_cache::NONCE_SENTINEL, &nonce)
                                .into_bytes();
                            let mut resp = oxide_server::OxideResponse::new(entry.status, body);
                            for (k, v) in &entry.headers {
                                resp = resp.with_header(k, v.clone());
                            }
                            resp = resp.with_header("x-oxide-cache", "L0-coalesced");
                            // P0-5: a coalesced follower gets the same
                            // per-request middleware headers as a direct hit.
                            for (k, v) in &middleware_headers {
                                resp = resp.with_header(k, v.clone());
                            }
                            metrics.observe(&label, started.elapsed(), entry.status);
                            return resp;
                        }
                    }
                    FlightSlot::Owner(tx) => {
                        flight_owner = Some(oxide_cache::FlightOwnerGuard::new(
                            std::sync::Arc::clone(&flight_map),
                            flight_key.clone(),
                            tx,
                        ))
                    }
                }
            }

            // §9.1: loader phase. When the matched route declares a server
            // module and a LoaderRunner is attached, run it. A non-200
            // output (error(...) or redirect(...)) short-circuits the
            // response; a 200 output threads `data` into the render phase.
            let mut data = serde_json::Value::Object(Default::default());
            // B2: the page load's per-settle stream, when the runner streams.
            // `Some` only when the loader declared at least one `defer()`;
            // the render arm then holds `</body>` until the settles land.
            let mut page_settles: Option<tokio::sync::mpsc::Receiver<serde_json::Value>> = None;
            // 16-G: the route's `meta = defineMeta(fn)` contribution, if its
            // server module exports one. Merged into the render input's head.
            let mut route_meta: Option<serde_json::Value> = None;
            // §16.6 / §9.2: cookies and headers a load set on the success
            // path. Collected here and applied to whichever response the
            // render phase produces (stream or error).
            let mut loader_set_cookies: Vec<String> = Vec::new();
            let mut loader_headers: Vec<(String, String)> = Vec::new();
            // §7.1 / §9.2: resolved defer() values from the loader.
            let mut deferred_values: std::collections::BTreeMap<String, serde_json::Value> =
                std::collections::BTreeMap::new();
            // PERF-06: per-stage timing marks. `started` is the request
            // entry; these bracket load / render so `Server-Timing` can
            // report real per-stage durations instead of all-zeros.
            let stage_load_start = std::time::Instant::now();
            let mut load_ms = 0.0f64;

            // §9.2: layout loads run in chain order (outer → inner); each
            // layout gets the pre-resolved data of its own outer layout as
            // `parent_data` (which a load reads via `await parent()`). The
            // innermost layout's data feeds the page's `parent_data`.
            //
            // **Parallelism (v0.1):** when `RouteEntry.needs_parent` is
            // `false` for the matched page (the scanner detected no
            // `parent` reference in `page.server.ts`), the LAST layout load
            // and the page load run concurrently via `tokio::join!` — the
            // page does not read layout data, so the ordering has no
            // observable effect. Earlier layouts still resolve outer-first.
            // Pages that call `parent()` (needs_parent = true) keep the
            // full sequential chain. Layout-then-page precedence on
            // short-circuit matches the sequential order.
            let mut layout_data_chain: Vec<serde_json::Value> = Vec::new();
            let mut layout_side_effects: Vec<(Vec<String>, Vec<(String, String)>)> = Vec::new();
            // §9.2 parallel path (v0.1): when the page does not call
            // `parent()` (`RouteEntry.needs_parent == false`), the last
            // layout load and the page load overlap via `tokio::join!`.
            // Earlier layouts stay sequential (each needs its own parent's
            // data); only the (last-layout, page) pair overlaps.
            let run_parallel = !needs_parent
                && has_server_module
                && loader_runner.is_some()
                && module_path.is_some()
                && route_id.is_some()
                && !layout_module_paths.is_empty();
            let layouts_to_run: &[(String, std::path::PathBuf)] = if run_parallel {
                &layout_module_paths[..layout_module_paths.len() - 1]
            } else {
                &layout_module_paths[..]
            };
            if let (Some(runner), true) = (loader_runner.as_ref(), !layouts_to_run.is_empty()) {
                for (lid, lpath) in layouts_to_run {
                    let mut linput = oxide_server::LoaderRunInput::new(
                        lpath.to_string_lossy().to_string(),
                        lid.clone(),
                        req_owned.url.clone(),
                    );
                    linput.method = req_owned.method.clone();
                    linput.headers = req_owned.headers.as_ref().clone();
                    linput.params = params.clone();
                    linput.locals = middleware_locals.clone();
                    linput.locale = resolved_locale.clone();
                    // §20 / G2: thread the inbound W3C trace so the loader's
                    // server-side fetch calls join the request trace.
                    linput.trace = req_owned.trace.as_ref().and_then(|t| {
                        t.to_traceparent(&t.parent_id).map(
                            |tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}),
                        )
                    });
                    linput.parent_data = layout_data_chain
                        .last()
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    match runner.run(linput).await {
                        Ok(out) if out.short_circuits() => {
                            let body = out
                                .data
                                .get("message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let mut resp = oxide_server::OxideResponse::new(out.status, body);
                            if let Some(loc) = out.location {
                                resp = resp.with_header("location", loc);
                            }
                            for c in &out.set_cookies {
                                resp = resp.with_header("set-cookie", c.clone());
                            }
                            for (k, v) in &out.headers {
                                resp = resp.with_header(k, v.clone());
                            }
                            // §16.6 / WP-D1 / WP-r10-1: a short-circuit that
                            // also mutated `event.session` must persist it.
                            // `session_before` is the inbound (post-middleware)
                            // session snapshot, so an untouched loader does
                            // not re-issue a cookie.
                            if let Some(cookie) = encode_session_cookie(
                                out.session.as_ref(),
                                session_before.as_ref(),
                                session_codec.as_deref(),
                            ) {
                                resp = resp.with_header("set-cookie", cookie);
                            }
                            metrics.observe(&label, started.elapsed(), out.status);
                            return resp;
                        }
                        Ok(out) => {
                            layout_data_chain.push(out.data);
                            // §16.6 / WP-r10-1 / D1: a layout loader that
                            // mutates `event.session` must persist it. The
                            // short-circuit arm (above) already does; this
                            // Ok arm used to drop `out.session` entirely,
                            // so a layout mutation was lost on the common
                            // non-short-circuit path. Encode and append to
                            // the same cookies vec that flattens below.
                            let mut cookies = out.set_cookies;
                            if let Some(cookie) = encode_session_cookie(
                                out.session.as_ref(),
                                session_before.as_ref(),
                                session_codec.as_deref(),
                            ) {
                                cookies.push(cookie);
                            }
                            layout_side_effects.push((cookies, out.headers));
                        }
                        Err(e) => {
                            tracing::error!(code = %e.0.code, error = %e.0.message, "layout loader failed");
                            metrics.observe(&label, started.elapsed(), 500);
                            return oxide_server::OxideResponse::html(
                                500,
                                format!(
                                    "layout loader error: {}",
                                    oxide_render::escape_text(&e.0.message)
                                ),
                            );
                        }
                    }
                }
                // Apply outer-layout side effects to the eventual response
                // (cookies/headers accumulate outward-in).
                for (cookies, headers) in &layout_side_effects {
                    for c in cookies {
                        loader_set_cookies.push(c.clone());
                    }
                    for (k, v) in headers {
                        loader_headers.push((k.clone(), v.clone()));
                    }
                }
            }
            if run_parallel {
                // §9.2 parallel: last layout + page run concurrently.
                // Layout takes precedence (matches sequential order): if
                // either short-circuits we return its response; prior
                // layouts' side effects live in loader_set_cookies/headers
                // and are only applied on the eventual render response.
                let runner = loader_runner.as_ref().unwrap();
                let (lid, lpath) = &layout_module_paths[layout_module_paths.len() - 1];
                let mut linput = oxide_server::LoaderRunInput::new(
                    lpath.to_string_lossy().to_string(),
                    lid.clone(),
                    req_owned.url.clone(),
                );
                linput.method = req_owned.method.clone();
                linput.headers = req_owned.headers.as_ref().clone();
                linput.params = params.clone();
                linput.locals = middleware_locals.clone();
                linput.locale = resolved_locale.clone();
                // §20 / G2: thread the inbound W3C trace so the loader's
                // server-side fetch calls join the request trace.
                linput.trace = req_owned.trace.as_ref().and_then(|t| {
                    t.to_traceparent(&t.parent_id).map(
                        |tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}),
                    )
                });
                linput.parent_data = layout_data_chain
                    .last()
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);

                let mp = module_path.as_ref().unwrap();
                let rid = route_id.as_ref().unwrap();
                let route_for_loader_page = rid.strip_suffix("/page").unwrap_or(rid).to_string();
                let mut pinput = oxide_server::LoaderRunInput::new(
                    mp.to_string_lossy().to_string(),
                    route_for_loader_page,
                    req_owned.url.clone(),
                );
                pinput.method = req_owned.method.clone();
                pinput.headers = req_owned.headers.as_ref().clone();
                pinput.params = params.clone();
                pinput.locals = middleware_locals.clone();
                pinput.locale = resolved_locale.clone();
                // Page does not call `parent()`, so this value is unused.
                pinput.parent_data = serde_json::Value::Null;
                // §20 / G2: thread the inbound W3C trace so the loader's
                // server-side fetch calls join the request trace.
                pinput.trace = req_owned.trace.as_ref().and_then(|t| {
                    t.to_traceparent(&t.parent_id).map(
                        |tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}),
                    )
                });

                // §20 / O3c: time each side of the join. The wall-clock
                // saving vs a sequential layout→page chain is
                // `min(layout_ms, page_ms)`; recording it on the request's
                // waterfall lets a user tell whether the route's schema
                // actually exercised the parallel path or whether `needs_
                // parent` silently forced a sequential chain.
                let (layout_res, layout_ms, page_res, page_ms) = {
                    let layout_start = std::time::Instant::now();
                    let (lr, pr) = tokio::join!(
                        async {
                            let r = runner.run(linput).await;
                            (r, layout_start.elapsed().as_secs_f64() * 1000.0)
                        },
                        async {
                            let start = std::time::Instant::now();
                            // B2 / P0-2: use the streaming entry so a page
                            // that does not call parent() still flushes its
                            // defer() settles per-settle instead of
                            // buffering the whole document in this arm.
                            let r = runner.run_streaming(pinput).await;
                            (r, start.elapsed().as_secs_f64() * 1000.0)
                        }
                    );
                    let (layout_res, layout_ms) = lr;
                    let (page_res, page_ms) = pr;
                    (layout_res, layout_ms, page_res, page_ms)
                };
                if let Some(rec) = waterfall.as_ref() {
                    let saved = parallel_load_savings_ms(layout_ms, page_ms);
                    rec.record_staged(
                        &req_owned.request_id,
                        vec![(LOAD_OVERLAP_STAGE.to_string(), saved)],
                        req_owned.trace.as_ref().map(|t| t.trace_id.clone()),
                    );
                }

                // Layout result first (precedence: matches sequential).
                match layout_res {
                    Ok(out) if out.short_circuits() => {
                        let body = out
                            .data
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let mut resp = oxide_server::OxideResponse::new(out.status, body);
                        if let Some(loc) = out.location {
                            resp = resp.with_header("location", loc);
                        }
                        for c in &out.set_cookies {
                            resp = resp.with_header("set-cookie", c.clone());
                        }
                        for (k, v) in &out.headers {
                            resp = resp.with_header(k, v.clone());
                        }
                        // §16.6 / WP-D1 / WP-r10-1: a layout short-circuit
                        // (parallel arm) must persist any session mutation.
                        if let Some(cookie) = encode_session_cookie(
                            out.session.as_ref(),
                            session_before.as_ref(),
                            session_codec.as_deref(),
                        ) {
                            resp = resp.with_header("set-cookie", cookie);
                        }
                        metrics.observe(&label, started.elapsed(), out.status);
                        return resp;
                    }
                    Ok(out) => {
                        layout_data_chain.push(out.data);
                        for c in &out.set_cookies {
                            loader_set_cookies.push(c.clone());
                        }
                        for (k, v) in &out.headers {
                            loader_headers.push((k.clone(), v.clone()));
                        }
                        // §16.6 / WP-r10-1 / D1: the parallel layout Ok arm
                        // also dropped `out.session`. Same fix as the
                        // sequential arm — encode and push through the
                        // cookies stream that lands on the response.
                        if let Some(cookie) = encode_session_cookie(
                            out.session.as_ref(),
                            session_before.as_ref(),
                            session_codec.as_deref(),
                        ) {
                            loader_set_cookies.push(cookie);
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            code = %e.0.code,
                            error = %e.0.message,
                            "layout loader failed",
                        );
                        metrics.observe(&label, started.elapsed(), 500);
                        return oxide_server::OxideResponse::html(
                            500,
                            format!(
                                "layout loader error: {}",
                                oxide_render::escape_text(&e.0.message)
                            ),
                        );
                    }
                }

                match page_res {
                    Ok((out, _settles)) if out.short_circuits() => {
                        let body = out
                            .data
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let mut resp = oxide_server::OxideResponse::new(out.status, body);
                        if let Some(loc) = out.location {
                            resp = resp.with_header("location", loc);
                        }
                        for c in &out.set_cookies {
                            resp = resp.with_header("set-cookie", c.clone());
                        }
                        for (k, v) in &out.headers {
                            resp = resp.with_header(k, v.clone());
                        }
                        // §16.6 / WP-D1 / WP-r10-1: a page short-circuit
                        // (parallel arm) must persist any session mutation.
                        if let Some(cookie) = encode_session_cookie(
                            out.session.as_ref(),
                            session_before.as_ref(),
                            session_codec.as_deref(),
                        ) {
                            resp = resp.with_header("set-cookie", cookie);
                        }
                        metrics.observe(&label, started.elapsed(), out.status);
                        return resp;
                    }
                    Ok((out, settles)) => {
                        data = out.data;
                        page_settles = Some(settles);
                        // 16-G: the route's `meta = defineMeta(fn)` export,
                        // evaluated by the worker against this load's data.
                        route_meta = out.meta;
                        // NOTE (preexisting, preserved): overwrite, not
                        // extend — matches the sequential-path behavior.
                        loader_set_cookies = out.set_cookies;
                        loader_headers = out.headers;
                        deferred_values = out.deferred;
                        // §16.6 / WP-D1: the parallel arm (page + last
                        // layout run under tokio::join!) had no session
                        // encode. Any page that does not call parent() —
                        // the default — dropped its session mutation. This
                        // is the common case, so this is the highest-impact
                        // of the four D1 gaps.
                        if let Some(cookie) = encode_session_cookie(
                            out.session.as_ref(),
                            session_before.as_ref(),
                            session_codec.as_deref(),
                        ) {
                            loader_set_cookies.push(cookie);
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            code = %e.0.code,
                            error = %e.0.message,
                            route = %label,
                            "loader failed",
                        );
                        metrics.observe(&label, started.elapsed(), 500);
                        return oxide_server::OxideResponse::html(
                            500,
                            format!("loader error: {}", oxide_render::escape_text(&e.0.message)),
                        );
                    }
                }
            } else if has_server_module {
                if let (Some(runner), Some(module_path), Some(route_id)) =
                    (loader_runner.as_ref(), module_path, route_id)
                {
                    // Pass the normalized id (no `/page` suffix) so user
                    // `load(event)` implementations see the route
                    // directory they expect.
                    let route_for_loader = route_id
                        .strip_suffix("/page")
                        .unwrap_or(&route_id)
                        .to_string();
                    let mut input = oxide_server::LoaderRunInput::new(
                        module_path.to_string_lossy().to_string(),
                        route_for_loader,
                        req_owned.url.clone(),
                    );
                    input.method = req_owned.method.clone();
                    input.headers = req_owned.headers.as_ref().clone();
                    input.params = params.clone();
                    // §8.1 middleware.ts / §9.1: request-scoped locals
                    // injected by the middleware that just ran. `{}` when
                    // no middleware executed.
                    input.locals = middleware_locals.clone();
                    // SPEC-10: expose the resolved locale to `load()`.
                    input.locale = resolved_locale.clone();
                    // §20 / G2: thread the inbound W3C trace so the loader's
                    // server-side fetch calls join the request trace.
                    input.trace = req_owned.trace.as_ref().and_then(|t| {
                        t.to_traceparent(&t.parent_id).map(
                            |tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}),
                        )
                    });
                    // §9.2: the innermost layout's data — what `parent()`
                    // reads in a page load.
                    input.parent_data = layout_data_chain
                        .last()
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);

                    match runner.run_streaming(input.clone()).await {
                        Ok((out, settles)) => {
                            if out.short_circuits() {
                                // fall through to the short-circuit arm below
                                // by re-dispatching on the same shape.
                                let body = out
                                    .data
                                    .get("message")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let mut resp = oxide_server::OxideResponse::new(out.status, body);
                                if let Some(loc) = out.location {
                                    resp = resp.with_header("location", loc);
                                }
                                for c in &out.set_cookies {
                                    resp = resp.with_header("set-cookie", c.clone());
                                }
                                for (k, v) in &out.headers {
                                    resp = resp.with_header(k, v.clone());
                                }
                                // §16.6 / WP-D1 / WP-r10-1: a page
                                // short-circuit (sequential arm, streaming
                                // path) must persist any session mutation.
                                if let Some(cookie) = encode_session_cookie(
                                    out.session.as_ref(),
                                    session_before.as_ref(),
                                    session_codec.as_deref(),
                                ) {
                                    resp = resp.with_header("set-cookie", cookie);
                                }
                                metrics.observe(&label, started.elapsed(), out.status);
                                return resp;
                            }
                            data = out.data;
                            loader_set_cookies = out.set_cookies;
                            loader_headers = out.headers;
                            deferred_values = out.deferred;
                            page_settles = Some(settles);
                            // §16.6 / WP-D1: a loader that mutated
                            // `event.session` has its change persisted. The
                            // helper compares against the inbound session so
                            // an untouched one does not re-issue a cookie.
                            if let Some(cookie) = encode_session_cookie(
                                out.session.as_ref(),
                                session_before.as_ref(),
                                session_codec.as_deref(),
                            ) {
                                loader_set_cookies.push(cookie);
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                code = %e.0.code,
                                error = %e.0.message,
                                route = %label,
                                "loader failed",
                            );
                            metrics.observe(&label, started.elapsed(), 500);
                            return oxide_server::OxideResponse::html(
                                500,
                                format!(
                                    "loader error: {}",
                                    oxide_render::escape_text(&e.0.message)
                                ),
                            );
                        }
                    }
                }
            }

            // §8.1 route.ts / §10.2 step 7: an Endpoint route returns a
            // Web `Response` directly, bypassing the loader + render
            // phases. The dispatcher is a worker op (`endpoint:run`),
            // async but bounded; the response bytes are the raw body
            // from the `Response` (a `route.ts` can serve a download).
            let endpoint_for_dispatch_inner = endpoint_for_dispatch.clone();
            if let (Some(dispatcher), Some(mp)) = (
                endpoint_for_dispatch_inner.as_deref(),
                endpoint_module_path.as_ref(),
            ) {
                let module_path_s = mp.to_string_lossy().to_string();
                let endpoint_trace = req_owned.trace.as_ref().and_then(|t| {
                    t.to_traceparent(&t.parent_id).map(
                        |tp| serde_json::json!({"traceparent": tp, "tracestate": t.tracestate}),
                    )
                });
                match dispatcher
                    .dispatch(
                        &module_path_s,
                        &req_owned.method,
                        &req_owned.url,
                        &endpoint_params_json,
                        &req_owned.headers,
                        endpoint_body.as_ref(),
                        &middleware_locals,
                        endpoint_trace.as_ref(),
                    )
                    .await
                {
                    Ok(out) => {
                        metrics.observe(&label, started.elapsed(), out.status);
                        // §9.6: a streaming route.ts (SSE, downloads)
                        // drives the response body from `out.stream`;
                        // a buffered one uses `out.body`.
                        let mut resp = match out.stream {
                            Some(stream) => oxide_server::OxideResponse::stream(out.status, stream),
                            None => oxide_server::OxideResponse::new(
                                out.status,
                                bytes::Bytes::from(out.body),
                            ),
                        };
                        for (k, v) in &out.headers {
                            resp = resp.with_header(k, v.clone());
                        }
                        // §8.5: headers the middleware phase added ride the
                        // endpoint response (previously accumulated and
                        // dropped).
                        for (k, v) in &middleware_headers {
                            resp = resp.with_header(k, v.clone());
                        }
                        return resp;
                    }
                    Err(e) => {
                        metrics.observe(&label, started.elapsed(), e.status);
                        return oxide_server::OxideResponse::json(
                            e.status,
                            serde_json::json!({"error": {
                                "code": e.code,
                                "message": e.message,
                                "route": label,
                            }}),
                        );
                    }
                }
            }

            // §7: render phase. Attached renderer wins; otherwise IR placeholder.
            // PERF-06: the loader phase is done; record its duration.
            load_ms = stage_load_start.elapsed().as_secs_f64() * 1000.0;
            let stage_render_start = std::time::Instant::now();

            if let Some(renderer) = page_renderer.as_ref() {
                // §10.2 step 5: an unmatched URL is a 404 with the
                // project's `not-found.tsx` as the body, when one exists.
                // Matched routes resolve to their own id + layout chain;
                // only the no-match branch substitutes the not-found id.
                let (route_for_render, effective_layouts) = match &route_id_for_render {
                    Some(id) => (id.clone(), layout_ids.clone()),
                    None => match &unmatched_route_id {
                        Some(nf) => (nf.clone(), unmatched_layout_ids.clone()),
                        None => ("_unmatched".to_string(), Vec::new()),
                    },
                };
                // PERF-01 / §8.4 / P-11: the ODF hydration payload needs the
                // same route/url/params/data the render input carries. Encode
                // it directly from borrowed fields *before* they move into
                // `PageRenderInput`, so `data` is traversed once instead of
                // being deep-cloned and then traversed again.
                let route_for_odf = route_for_render.clone();
                let odf_url = req_owned.url.clone();
                let odf_bytes = {
                    use oxide_odf::Value as OdfValue;
                    let mut obj = std::collections::BTreeMap::new();
                    obj.insert("route".to_string(), OdfValue::String(route_for_odf.clone()));
                    obj.insert("url".to_string(), OdfValue::String(odf_url.clone()));
                    obj.insert(
                        "params".to_string(),
                        OdfValue::Object(
                            params
                                .iter()
                                .map(|(k, v)| (k.clone(), OdfValue::String(v.clone())))
                                .collect(),
                        ),
                    );
                    obj.insert("data".to_string(), OdfValue::from_json(&data));
                    obj.insert(
                        "layouts".to_string(),
                        OdfValue::Array(
                            effective_layouts
                                .iter()
                                .map(|l| OdfValue::String(l.clone()))
                                .collect(),
                        ),
                    );
                    oxide_odf::encode_to_bytes(&OdfValue::Object(obj)).unwrap_or_default()
                };
                let mut input =
                    oxide_server::PageRenderInput::new(route_for_render, req_owned.url.clone());
                input.params = params;
                // §9.3: a form POST re-renders the page with the action's
                // result in `data.form`. Injected here rather than seeded
                // before the loader because the loader assigns `data`
                // wholesale; this runs after it, so both survive.
                if let Some(form) = action_form.take() {
                    if let Some(obj) = data.as_object_mut() {
                        obj.insert("form".to_string(), form);
                    }
                }
                input.data = data;
                // §8.2: hand the layout chain to the adapter so it can
                // nest the page. The adapter renders
                // `L1( L2( ...( page ) ) )` using `componentFor(layoutId)`.
                input.layouts = effective_layouts.clone();
                // §10.2 step 5: nearest ancestor `error.tsx` — the adapter
                // renders it on an SSR throw instead of the generic 500.
                input.error_route_id = error_route_id.clone();
                // §16.4 / O2: thread the resolved request locale into the
                // live render, exactly as prerender does. Without it the
                // adapter cannot emit `<html lang>` and the client catalog
                // loader falls back to `en`.
                input.locale = Some(resolved_locale.clone());
                // §7.1 / §9.2: hand the loader's resolved `defer()` values to
                // the adapter so it can replace `{__deferred: id}` in `data`
                // before the component tree is built. Without this a
                // component reads the placeholder object, never the value.
                input.deferred = deferred_values.clone();
                // §17.1 / §7.1: the request's CSP nonce. The adapter stamps it
                // on any script it emits — including the deferred-boundary
                // swapper, which is an inline IIFE and would be blocked under
                // `strict-dynamic` without it. Same value the pipeline uses
                // for its own injected tags.
                input.nonce = Some(oxide_server::nonce_for(&req_owned.request_id));
                // 16-G: the route's own `meta` joins the neutral head model.
                // The live path has no plugin `renderHead` contribution to
                // merge with here (that is a build-time hook); the route's
                // meta is the whole contribution.
                if let Some(meta) = route_meta.as_ref() {
                    input.head = merge_neutral_head(input.head, normalize_route_meta(meta.clone()));
                }
                match renderer.render_streaming(input).await {
                    Ok(render_stream) => {
                        // §10.2 step 5 (error-status contract): peek the
                        // FIRST chunk and scan it for an error-status
                        // marker BEFORE the response status is fixed. The
                        // adapter renders the error route with a 200 on
                        // the wire today (no status channel in the worker
                        // protocol); the contract is that an error
                        // document announces itself in its first chunk —
                        // `<!--oxide-status:500-->` (canonical) or the
                        // generic OX-500 boundary attribute. HTTP cannot
                        // change the status after the first byte is sent,
                        // so a marker in a LATER chunk is unobservable
                        // here (documented; the adapter must emit the
                        // marker before the shell).
                        let (error_status, render_stream) =
                            probe_render_error_status(render_stream).await;
                        let final_status = if error_status == 500 { 500 } else { status };
                        metrics.observe(&label, started.elapsed(), final_status);
                        // Bridge the renderer's Send-only stream to the
                        // transport's Send + Sync body stream. If the
                        // project registered islands, splice the boot
                        // script tag right before `</body>` — the sliding
                        // window in `oxide_server::inject_before_close` buffers
                        // at most `len("</body>") - 1` bytes so a tag
                        // split across chunks still lands in the right
                        // place and every other chunk flushes untouched.
                        // SPEC-11: pass boundary + nonce so a mid-stream
                        // error emits the recovery boundary instead of
                        // truncating the response.
                        let bridged = render_stream_with_recovery(
                            render_stream,
                            Some(label.clone()),
                            Some(oxide_server::nonce_for(&req_owned.request_id)),
                            label.clone(),
                        );

                        // B2: when the load declared at least one
                        // `defer()`, hold the document's closing tags and
                        // emit a resolution script per settle as it arrives,
                        // instead of one batched tag after the slowest.
                        let nonce_for_settles = oxide_server::nonce_for(&req_owned.request_id);
                        let bridged = match page_settles.take() {
                            Some(rx) => {
                                // §20 / O3b: pass the recorder + request id
                                // so the settle drain records a
                                // `deferred-settles` stage once the last
                                // one lands.
                                strip_close_and_append_settles(
                                    bridged,
                                    rx,
                                    nonce_for_settles,
                                    waterfall.clone(),
                                    req_owned.request_id.clone(),
                                    req_owned.trace.as_ref().map(|t| t.trace_id.clone()),
                                )
                            }
                            None => bridged,
                        };
                        // §17.1: a script without the matching nonce is
                        // blocked by the response's `strict-dynamic` CSP.
                        // The pipeline derives a per-request nonce in
                        // `finish_response`; we compute the same value here
                        // from the request id. Both the deferred script and
                        // (when present) the boot module share it.
                        let nonce = oxide_server::nonce_for(&req_owned.request_id);
                        // PERF-01 / §8.4: the SAME `window.__oxide` ODF
                        // payload prerendered pages embed, injected into
                        // the live streamed shell (the sliding-window
                        // injector above). Previously only write_prerender
                        // set it, so a dynamic page's boot module found no
                        // payload and skipped hydration + the router
                        // entirely (`if (root && payload)` in boot.ts).
                        let odf_tag = {
                            use base64::Engine as _;
                            let odf_b64 =
                                base64::engine::general_purpose::STANDARD.encode(&odf_bytes);
                            oxide_render::hydration_bootstrap(&odf_b64, &route_for_odf, &nonce)
                        };
                        // §7.1 / §9.2: resolved defer() values ride a JSON
                        // script element so the boot module can dispatch
                        // `ox:swap` for each. Serialized through serde_json
                        // (no untrusted string concatenation) and `<` escaped
                        // so the content cannot break out of the tag.
                        let deferred_tag = if deferred_values.is_empty() {
                            String::new()
                        } else {
                            let json = serde_json::to_string(&deferred_values)
                                .unwrap_or_else(|_| "{}".into());
                            // `</script>` and `<!--` inside a JSON string
                            // would terminate the tag; `<` escaping is the
                            // OWASP-recommended fix and produces valid JS
                            // (`\u003c`).
                            let safe = json.replace('<', "\\u003c");
                            format!(
                                r#"<script type="application/oxide-deferred" nonce="{nonce}">{safe}</script>"#,
                            )
                        };
                        // §13.3: a self-contained dev HMR client. It
                        // connects to `/_oxide/hmr`, swaps the matching
                        // <link> on a CSS update, and reloads the page on
                        // a module/server update. Inlined (not a bundled
                        // module) so it works in dev without a build step.
                        let hmr_tag = if dev_hmr_for_render {
                            format!(
                                concat!(
                                    r#"<script nonce="{n}">(function(){{"#,
                                    r#"try{{var ws=new WebSocket((location.protocol==='https:'?'wss':'ws')+'://'+location.host+'/_oxide/hmr');"#,
                                    r#"ws.onmessage=function(ev){{try{{var m=JSON.parse(ev.data);"#,
                                    // B-21: a tab open across a server restart holds
                                    // assets from the old build_id — hard reload.
                                    r#"if(m.type==='connected'){{if(window.__oxideBuildId&&window.__oxideBuildId!==m.build_id){{location.reload();return;}}window.__oxideBuildId=m.build_id;}}"#,
                                    r#"if(m.type==='css-update'){{var p=(m.path||'').split('?')[0];var ls=document.querySelectorAll('link[rel=stylesheet]');"#,
                                    r#"for(var i=0;i<ls.length;i++){{var h=(ls[i].getAttribute('href')||'').split('?')[0];"#,
                                    r#"if(h===p||h.slice(-p.length)===p){{ls[i].setAttribute('href',p+'?t='+Date.now());break;}}}}}}"#,
                                    r#"else if(m.type==='module-update'||m.type==='server-module-swap'||m.type==='full-reload'){{location.reload();}}"#,
                                    r#"else if(m.type==='route-data-update'){{var ad=function(b64){{try{{if(window.__oxide)window.__oxide.data=b64;}}catch(e){{}}"#,
                                    r#"document.dispatchEvent(new CustomEvent('ox:data',{{detail:{{route:m.route||location.pathname,odf:b64}}}}));}};"#,
                                    r#"fetch('/__data?route='+encodeURIComponent(location.pathname),{{headers:{{accept:'application/oxide+odf'}}}})"#,
                                    r#".then(function(r){{return r.ok?r.arrayBuffer():Promise.reject(0)}})"#,
                                    r#".then(function(buf){{var u=new Uint8Array(buf);var s='';for(var i=0;i<u.length;i++)s+=String.fromCharCode(u[i]);var b64=btoa(s);ad(b64);}})"#,
                                    r#".catch(function(){{ad(m.odf_b64||'');}});}}"#,
                                    r#"else if(m.type==='error'){{var d=document.createElement('div');"#,
                                    r#"d.setAttribute('data-oxide-error','');d.style.cssText='position:fixed;inset:0;background:rgba(0,0,0,.85);color:#fff;font:14px/1.4 monospace;padding:2rem;z-index:2147483647;overflow:auto';"#,
                                    r#"d.textContent=(m.diagnostic&&(m.diagnostic.code+': '+m.diagnostic.message))||'Oxide error';document.body.appendChild(d);"#,
                                    r#"d.addEventListener('click',function(){{d.remove();}});}}}}catch(e){{}}}};"#,
                                    r#"ws.onclose=function(){{setTimeout(function(){{location.reload();}},2000);}};}}catch(e){{}}}})();</script>"#,
                                ),
                                n = nonce,
                            )
                        } else {
                            String::new()
                        };
                        let body = match boot_script_url_for_render.as_deref() {
                            Some(url) => {
                                let tag = format!(
                                    r#"{odf_tag}{deferred_tag}{hmr_tag}<script type="module" src="/{url}" nonce="{nonce}"></script>"#,
                                );
                                oxide_server::inject_before_close(
                                    bridged,
                                    b"</body>",
                                    tag.as_bytes(),
                                )
                            }
                            None => {
                                let extra = format!("{odf_tag}{deferred_tag}{hmr_tag}");
                                if extra.is_empty() {
                                    bridged
                                } else {
                                    oxide_server::inject_before_close(
                                        bridged,
                                        b"</body>",
                                        extra.as_bytes(),
                                    )
                                }
                            }
                        };
                        // §10.8 / B-27: serve a previously captured static
                        // shell at once and stream only the dynamic boundary
                        // fragments into it; the first request taps the full
                        // render and stores its shell on completion.
                        // §10.8 / P0-4 / r8-P0.3: a PPR shell is a *shared*
                        // asset (the static frame every visitor reuses), so
                        // it must never be captured from a response that
                        // varies per user. The concrete case the audit found:
                        // a login page (or any loader that sets a session
                        // cookie) freezes its user-specific body into a
                        // shell served to every subsequent visitor. The L0
                        // store 30 lines below correctly refuses
                        // cookie-setting responses; this is the same guard.
                        // An error-status render is likewise never a shell.
                        // §10.8 / WP-r9-4: the tap guard must also refuse
                        // a middleware Set-Cookie. `middleware_headers` is
                        // applied at :8029 (after this decision) — without
                        // this check a middleware-mutating route freezes its
                        // shell exactly as a loader-mutating route did
                        // before the r8-P0.3 guard. Both PPR and the L0
                        // store shared this blind spot.
                        let middleware_sets_cookie = middleware_headers
                            .iter()
                            .any(|(k, _)| k.eq_ignore_ascii_case("set-cookie"));
                        let body = if error_status == 0
                            && loader_set_cookies.is_empty()
                            && !middleware_sets_cookie
                        {
                            match ppr_for_request.as_ref() {
                                Some(shells) => {
                                    // §10.8 / WP-r9: the shell key must
                                    // match the L0 key's identity, not the
                                    // raw URL. Two bugs the raw-URL form
                                    // had:
                                    //   1. No build id — a rebuild served
                                    //      stale shells until the next
                                    //      clear-all.
                                    //   2. Query-string explosion —
                                    //      ?utm=twitter and ?utm=facebook
                                    //      were distinct shells on a site
                                    //      with search, so 256 distinct
                                    //      queries cleared the map.
                                    // The shell key is the canonical L0 key
                                    // itself, not a hand-rolled join: same
                                    // build id, route id, sorted params and
                                    // normalized Vary, hashed by
                                    // `CacheKey::canonical_parts`. Dev has no
                                    // L0 object, so use the stable `"dev"`
                                    // namespace; the watcher clears shells on
                                    // every edit.
                                    let ppr_build_id =
                                        l0.as_ref().map(|c| c.build_id()).unwrap_or("dev");
                                    let pkey = l0_cache::L0Cache::key_for(
                                        &l0_route,
                                        &l0_params,
                                        &l0_vary,
                                        ppr_build_id,
                                    );
                                    let ppr_tags =
                                        vec![l0_route.clone(), format!("path:{l0_path}")];
                                    let cached =
                                        shells.lock().ok().and_then(|s| s.get(&pkey).cloned());
                                    match cached {
                                        Some(shell) => ppr_filter_stream(
                                            shell.replace(l0_cache::NONCE_SENTINEL, &nonce),
                                            body,
                                            nonce.clone(),
                                        ),
                                        None => ppr_tap_stream(
                                            body,
                                            std::sync::Arc::clone(shells),
                                            pkey,
                                            nonce.clone(),
                                            ppr_tags,
                                        ),
                                    }
                                }
                                None => body,
                            }
                        } else {
                            // No PPR on a per-user / error response: the
                            // body is served live to this caller, never
                            // tapped for a shell, never filtered from one.
                            body
                        };
                        // §11 L0: when this response is cacheable, collect
                        // the streamed body once so it can be stored, then
                        // serve it from memory. The body was rendered with
                        // the live request's nonce; replace it with the
                        // sentinel before storing so any request can reuse
                        // it. A response that set a cookie is NOT cached
                        // (it varies per user).
                        // PERF-04: streaming-preserving fill. Tee the
                        // rendered body: each chunk flushes to the client
                        // immediately AND accumulates. On stream end the
                        // sender is dropped FIRST (so the client sees EOF
                        // at once, not after the store), then the body is
                        // stored (live nonce → sentinel) and published for
                        // single-flight followers. The next request is an
                        // L0 hit.
                        if l0_eligible
                                && loader_set_cookies.is_empty()
                                // §10.2 step 5: an error-status render is never
                                // cached — a 500 is per-request state, not a
                                // shareable response.
                                && error_status == 0
                                // §10.8 / WP-r10-3: a middleware Set-Cookie
                                // response also varies per user — the L0
                                // store shared PPR's old blind spot. Refuse
                                // both. `middleware_sets_cookie` is defined
                                // above for the PPR guard.
                                && !middleware_sets_cookie
                        {
                            if let Some(cache) = l0.as_ref() {
                                let nonce = oxide_server::nonce_for(&req_owned.request_id);
                                let cache = std::sync::Arc::clone(cache);
                                let l0_route = l0_route.clone();
                                let l0_path = l0_path.clone();
                                let headers_for_store = loader_headers.clone();
                                // P-04: reuse the key already computed for the
                                // flight instead of re-deriving it at store time.
                                let store_key = flight_key.clone();
                                let flight_owner = flight_owner.take();
                                let render_ms = stage_render_start.elapsed().as_secs_f64() * 1000.0;
                                let (tx, mut rx) = tokio::sync::mpsc::channel::<
                                    std::result::Result<bytes::Bytes, std::io::Error>,
                                >(8);
                                tokio::spawn(async move {
                                    use futures::StreamExt as _;
                                    let mut src = body;
                                    let mut collected: Vec<u8> = Vec::new();
                                    while let Some(item) = src.next().await {
                                        match item {
                                            Ok(chunk) => {
                                                collected.extend_from_slice(&chunk);
                                                if tx.send(Ok(chunk)).await.is_err() {
                                                    // Client gone: no store
                                                    // (the render did not
                                                    // finish).
                                                    return;
                                                }
                                            }
                                            Err(e) => {
                                                let _ = tx.send(Err(e)).await;
                                                return;
                                            }
                                        }
                                    }
                                    // Close the client stream NOW — the
                                    // store below must not delay EOF.
                                    drop(tx);
                                    let sentineled = String::from_utf8_lossy(&collected)
                                        .replace(&nonce, l0_cache::NONCE_SENTINEL)
                                        .into_bytes();
                                    let mut store_headers = vec![(
                                        "content-type".to_string(),
                                        "text/html; charset=utf-8".to_string(),
                                    )];
                                    for (k, v) in &headers_for_store {
                                        store_headers.push((k.clone(), v.clone()));
                                    }
                                    let entry = cache.make_entry(
                                        200,
                                        store_headers,
                                        sentineled,
                                        vec![l0_route.clone(), format!("path:{}", l0_path)],
                                    );
                                    cache.store_entry(&store_key, entry.clone()).await;
                                    if let Some(guard) = flight_owner {
                                        guard.publish(std::sync::Arc::new(entry));
                                    }
                                });
                                let stream: oxide_server::BodyStream =
                                    Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)));
                                let t = oxide_render::RequestTimings {
                                    middleware_ms,
                                    cache_ms: 0.0,
                                    load_ms,
                                    render_ms,
                                    inject_ms: 0.0,
                                };
                                if let Some(rec) = waterfall.as_ref() {
                                    // §20 / O3: attach the inbound W3C
                                    // trace id so this row correlates with
                                    // the request's spans downstream.
                                    rec.record_with_trace(
                                        &req_owned.request_id,
                                        vec![
                                            ("middleware".to_string(), middleware_ms),
                                            ("load".to_string(), load_ms),
                                            ("render".to_string(), render_ms),
                                        ],
                                        req_owned.trace.as_ref().map(|t| t.trace_id.clone()),
                                    );
                                    // SPEC-19.3 / §19-D: also persist the
                                    // snapshot to disk so the MCP server (a
                                    // separate process) can read the live
                                    // waterfalls via `oxide_state`. The
                                    // dispatch closure doesn't have `rootp`
                                    // in scope (it's captured at server
                                    // construction), so we capture the path
                                    // via the existing `routes_root` prefix
                                    // resolution — but that's per-dispatch.
                                    // Simpler: use the CWD, which is the
                                    // project root for the dev server.
                                    let waterfall_path =
                                        std::path::PathBuf::from(".oxide/last-waterfalls.json");
                                    rec.persist_to(&waterfall_path);
                                }
                                let mut resp =
                                    oxide_server::OxideResponse::stream(final_status, stream)
                                        .with_header("content-type", "text/html; charset=utf-8")
                                        .with_header("x-oxide-cache", "L0-miss")
                                        .with_header("Server-Timing", t.server_timing());
                                for (k, v) in &loader_headers {
                                    resp = resp.with_header(k, v.clone());
                                }
                                // §8.5: middleware headers ride the teed
                                // response too.
                                for (k, v) in &middleware_headers {
                                    resp = resp.with_header(k, v.clone());
                                }
                                return resp;
                            }
                        }
                        // §10.2 step 5 / §8.3: the framework render path
                        // always used to emit a hardcoded 200 regardless of
                        // the computed `status`. That made an unmatched URL
                        // return `HTTP 200` with a page body, breaking both
                        // the status code and any client HTTP check
                        // (curl, monitoring, browser dev tools). Use the
                        // computed status: 200 for a matched route, 404 for
                        // `_unmatched` — overridden to 500 when the error
                        // document announced itself in the first chunk
                        // (§10.2 error-status contract).
                        let mut resp = oxide_server::OxideResponse::stream(final_status, body)
                            .with_header("content-type", "text/html; charset=utf-8")
                            .with_header("x-oxide-cache", "bypass");
                        // PERF-06: real per-stage Server-Timing.
                        let render_ms = stage_render_start.elapsed().as_secs_f64() * 1000.0;
                        let t = oxide_render::RequestTimings {
                            middleware_ms,
                            cache_ms: 0.0,
                            load_ms,
                            render_ms,
                            inject_ms: 0.0,
                        };
                        resp = resp.with_header("Server-Timing", t.server_timing());
                        // §16.6 / WP-D1: middleware side effects apply
                        // BEFORE the loader's. The loader runs after
                        // middleware and its session mutation is the later
                        // intent; on the wire, HTTP applies same-name
                        // Set-Cookies in arrival order, so loader-after-
                        // middleware means the loader's value survives.
                        for (k, v) in &middleware_headers {
                            resp = resp.with_header(k, v.clone());
                        }
                        for c in &loader_set_cookies {
                            resp = resp.with_header("set-cookie", c.clone());
                        }
                        for (k, v) in &loader_headers {
                            resp = resp.with_header(k, v.clone());
                        }
                        resp
                    }
                    Err(e) => {
                        tracing::error!(
                            code = %e.0.code,
                            error = %e.0.message,
                            route = %label,
                            "framework render failed",
                        );
                        metrics.observe(&label, started.elapsed(), 500);
                        oxide_server::OxideResponse::html(
                            500,
                            format!("render error: {}", oxide_render::escape_text(&e.0.message)),
                        )
                    }
                }
            } else {
                metrics.observe(&label, started.elapsed(), status);
                let mut resp = oxide_server::OxideResponse::html(
                    status,
                    format!(
                        "<!doctype html><html><head><title>Oxide</title></head>\
                             <body><main>Oxide serving {label}</main></body></html>"
                    ),
                );
                // D1: the loader's side effects belong on the response
                // whichever render path produced the body.
                // §16.6 / WP-D1 precedence: middleware applies first, then
                // the loader — see the render path above for the reasoning.
                for (k, v) in &middleware_headers {
                    resp = resp.with_header(k, v.clone());
                }
                for c in &loader_set_cookies {
                    resp = resp.with_header("set-cookie", c.clone());
                }
                for (k, v) in &loader_headers {
                    resp = resp.with_header(k, v.clone());
                }
                resp
            }
        })
    });
    // §8.5: publish the dispatch closure so middleware `{ rewrite }` can
    // re-enter it with a rewritten URL. The OnceLock is set once; subsequent
    // calls are no-ops (a running dispatch is already published).
    let _ = self_dispatch.set(std::sync::Arc::clone(&dispatch_fn));
    dispatch_fn
}

pub async fn start_dev_server_full(
    root: &str,
    port: Option<u16>,
    bind_ip: std::net::IpAddr,
    tls: Option<std::sync::Arc<tokio_rustls::rustls::ServerConfig>>,
    proxy_rules: Vec<oxide_dev::ProxyRule>,
    stack: crate::framework_shim::FrameworkStack,
    hmr: Option<std::sync::Arc<oxide_dev::HmrHub>>,
) -> anyhow::Result<StartedServer> {
    start_server_inner_full(
        root,
        Some((bind_ip, port.unwrap_or(0))),
        true,
        tls,
        proxy_rules,
        stack,
        hmr,
    )
    .await
}

pub async fn start_dev_server(root: &str, port: Option<u16>) -> anyhow::Result<StartedServer> {
    start_server_inner(root, port, true).await
}

async fn start_server_inner(
    root: &str,
    port: Option<u16>,
    with_inspector: bool,
) -> anyhow::Result<StartedServer> {
    let bind = port.map(|p| (std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), p));
    start_server_inner_full(
        root,
        bind,
        with_inspector,
        None,
        Vec::new(),
        crate::framework_shim::FrameworkStack::default(),
        None,
    )
    .await
}

/// §12.1 step 3: load `dist/client/` into the pipeline's static asset store so
/// the shell's `<script type="module" src="/client/...">` resolves. Returns
/// `None` when the directory is missing (dev before a build, or an app with
/// no client bundle) — the pipeline then 404s those paths, which is honest.
///
/// `StaticAssets::scan` indexes every file by its URL path relative to the
/// root, so `<root>/client/counter-abc.js` is reachable at
/// `/client/counter-abc.js` when the root itself is `dist/`. Since our
/// directory is `dist/client` and the URL prefix is `/client`, we scan
/// `dist/` — then the boot chunk's `/client/...` URL matches the file path.
/// §16.1: adapt `oxide_assets::ImageBackend` to the `oxide-server` seam.
///
/// The two crates do not know about each other — `oxide-server` names only
/// the trait, `oxide-assets` names only its own request/result types — so
/// the CLI is where the two shapes are joined. The transform itself is
/// unchanged; only the field names differ.
struct AssetsImageBackend;

impl oxide_server::ImageEndpointBackend for AssetsImageBackend {
    fn process(
        &self,
        req: oxide_server::ImageEndpointRequest,
    ) -> std::result::Result<oxide_server::ImageEndpointResult, String> {
        use oxide_assets::{ImageBackend as _, ImageFormat, ImageRequest, RustImageBackend};

        // Detect the source format from the bytes. The `image` crate's
        // guess is what the backend already relies on for decode; this is
        // a header sniff so the request carries a format the backend can
        // assert against.
        let source_format = if req.source.starts_with(b"\x89PNG") {
            ImageFormat::Png
        } else if req.source.starts_with(b"\xFF\xD8") {
            ImageFormat::Jpeg
        } else if req.source.starts_with(b"RIFF") {
            ImageFormat::Webp
        } else {
            // Default to PNG; the backend re-sniffs on decode anyway.
            ImageFormat::Png
        };

        let format = match req.format {
            Some("image/avif") => ImageFormat::Avif,
            Some("image/webp") => ImageFormat::Webp,
            Some("image/jpeg") => ImageFormat::Jpeg,
            Some("image/png") => ImageFormat::Png,
            _ => source_format,
        };

        let ireq = ImageRequest {
            source: req.source,
            source_format,
            width: req.width,
            height: req.height,
            format,
            quality: req.quality.unwrap_or(75),
            placeholder: req.placeholder.as_deref().and_then(|p| match p {
                "blur" => Some(oxide_assets::PlaceholderKind::Blur),
                "pixelate" => Some(oxide_assets::PlaceholderKind::Lqip),
                _ => None,
            }),
        };

        RustImageBackend
            .process(ireq)
            .map(|r| oxide_server::ImageEndpointResult {
                bytes: r.bytes,
                content_type: r.format.mime(),
                cache_key: r.cache_key,
                placeholder: r.placeholder,
            })
            .map_err(|e| format!("{}: {}", e.0.code, e.0.message))
    }
}

/// §16.1: construct the endpoint from config. Returns `None` when the
/// project declares no remote image patterns AND has no `public/` or
/// `dist/client/` directory to serve from — a project with images off
/// wants a 404, not an endpoint that always errors.
/// §16.3: the OG-image endpoint. Enabled by default — the `oxide-assets`
/// backend is pure-Rust (resvg + taffy + parley), so there is no external
/// process and no reason to gate it. A project that does not want the
/// endpoint can turn it off with a config flag (post-v0.1).
struct SvgOgBackendImpl;

impl oxide_server::OgEndpointBackend for SvgOgBackendImpl {
    fn render(
        &self,
        tree: &serde_json::Value,
        width: u32,
        height: u32,
    ) -> std::result::Result<Vec<u8>, String> {
        use oxide_assets::OgBackend as _;
        oxide_assets::SvgOgBackend
            .render(tree, width, height)
            .map_err(|e| format!("{} {}", e.0.code, e.0.message))
    }
}

fn build_og_endpoint() -> Option<std::sync::Arc<oxide_server::OgEndpoint>> {
    Some(std::sync::Arc::new(oxide_server::OgEndpoint {
        backend: std::sync::Arc::new(SvgOgBackendImpl),
    }))
}

fn build_image_endpoint(
    rootp: &Path,
    cfg: &oxide_config::OxideConfig,
) -> Option<std::sync::Arc<oxide_server::ImageEndpoint>> {
    // Asset root: the same `dist/` the static-asset store scans, so an
    // image imported at build time is reachable at the URL the bundler
    // emitted it under.
    let dist = rootp.join("dist");
    let public = rootp.join("public");
    let asset_root = if dist.is_dir() {
        dist
    } else if public.is_dir() {
        public
    } else {
        // No asset directory means no local sources to transform; the
        // endpoint would 404 every request. Leave it unset.
        return None;
    };

    // Signing: opt-in. A `OXIDE_IMAGE_SECRET` environment variable turns
    // the check on; without it the endpoint is open, which is the right
    // default for a dev server and for a project that serves its own
    // images. Production deployments that proxy third-party URLs set it.
    // SEC-08: the image-signing secret defaults to the project's build
    // secret (deterministic, `.oxide/secret`) unless `OXIDE_IMAGE_SECRET`
    // overrides it or the operator explicitly disables signing. Before this
    // the endpoint was unsigned by default.
    let signing_secret = std::env::var("OXIDE_IMAGE_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("OXIDE_IMAGE_UNSIGNED")
                .is_ok()
                .then(|| String::new())
        })
        .or_else(|| build_secret(&rootp).ok());
    let _ = &cfg.images; // reserved: remotePatterns gating lands with the fetch path

    // §16.1: remote sources are gated by `config.images.remotePatterns`.
    // The config type carries `host` + optional `protocol`; the server's
    // `RemoteImagePattern` is the same shape, so a straight map.
    let remote_patterns: Vec<oxide_server::RemoteImagePattern> = cfg
        .images
        .remote_patterns
        .iter()
        .map(|p| oxide_server::RemoteImagePattern {
            host: p.host.clone(),
            protocol: p.protocol.clone(),
        })
        .collect();

    Some(std::sync::Arc::new(oxide_server::ImageEndpoint {
        backend: std::sync::Arc::new(AssetsImageBackend),
        asset_root,
        signing_secret,
        remote_patterns,
        // §17.1: a project that puts its CDN on a private network adds the
        // host here. Empty by default — the SSRF check blocks the ranges
        // that matter (loopback, RFC1918, link-local).
        ssrf_allow_list: Vec::new(),
        // §16.1: a source larger than 32 MiB is almost certainly a mistake
        // or a hostile payload; the fetch refuses it.
        remote_max_bytes: 32 * 1024 * 1024,
        result_cache: std::sync::Arc::new(std::sync::Mutex::new(Default::default())),
        max_concurrency: cfg.images.max_concurrency,
        in_flight: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        max_dimension: cfg.images.max_dimension,
    }))
}

/// §9.4: the `ServerFnDispatcher` impl backed by the render worker.
///
/// Wraps `oxide_jshost::ServerFnHost`, which forwards `(id, args)` to the
/// worker's `server-fn:run` op. `entry_path` is the SSR adapter entry
/// (`dist/server/ssr/index.js`) — the worker imports it once to populate
/// the `__oxide_fn_registry` global that the compiler's `__oxide_register_fn`
/// lines fill in.
///
/// The pipeline's `NativePrefix::ServerFn` arm calls this; the JSON body
/// the function returns becomes the `{ok, data}` envelope's `data`.
struct ServerFnDispatcherImpl {
    host: oxide_jshost::ServerFnHost,
    entry_path: String,
}

#[async_trait::async_trait]
impl oxide_server::ServerFnDispatcher for ServerFnDispatcherImpl {
    async fn dispatch(
        &self,
        id: &str,
        args_json: &[u8],
    ) -> std::result::Result<Vec<u8>, oxide_server::DispatchError> {
        match self.host.run_coded(id, args_json, &self.entry_path).await {
            Ok(v) => {
                // The worker op returns `{value: <fn result>}` — a stable
                // envelope it can extend later (streaming, diagnostics). The
                // HTTP envelope wraps the *value*, not the worker's
                // container, so the client sees `data` = the fn's own return
                // and not `{value: …}`.
                let value = v.get("value").cloned().unwrap_or(v);
                serde_json::to_vec(&value).map_err(|e| {
                    oxide_server::DispatchError::internal(format!("serialize fn result: {e}"))
                })
            }
            Err(e) => {
                // The worker's JSH-FN-NOT-FOUND maps to a 404 envelope; a
                // function that threw maps to a 400 with the message; any
                // other failure is a 500.
                let err = match e.code.as_str() {
                    "JSH-FN-NOT-FOUND" => oxide_server::DispatchError {
                        status: 404,
                        code: "FN-NOT-FOUND".into(),
                        message: e.message,
                    },
                    "JSH-FN-THREW" => oxide_server::DispatchError {
                        status: 400,
                        code: "FN-THREW".into(),
                        message: e.message,
                    },
                    _ => oxide_server::DispatchError::internal(e.message),
                };
                Err(err)
            }
        }
    }
}

/// Build the dispatcher when a pool and an SSR entry are both present.
/// Otherwise `None`, and the pipeline answers every `/_oxide/fn/*` request
/// with a clean `FN-NO-DISPATCHER` 404.
fn build_server_fn_dispatcher(
    rootp: &Path,
    pool: Option<&std::sync::Arc<dyn oxide_jshost::Pool>>,
) -> Option<std::sync::Arc<dyn oxide_server::ServerFnDispatcher>> {
    let pool = pool?;
    // The SSR entry is written by `oxide build`; in dev without a prior
    // build there is none, which is correct — the dev render path uses
    // the source graph, not the emitted bundle.
    let entry = rootp.join("dist/server/ssr/index.js");
    if !entry.is_file() {
        return None;
    }
    let entry_path = entry.to_string_lossy().to_string();
    Some(std::sync::Arc::new(ServerFnDispatcherImpl {
        host: oxide_jshost::ServerFnHost::new(std::sync::Arc::clone(pool)),
        entry_path,
    }))
}

/// §9.3: a `LoaderRunner`-shaped bridge to the worker's `action:run` op.
struct ActionDispatcherImpl {
    host: oxide_jshost::ActionHost,
}

#[async_trait::async_trait]
impl oxide_server::ActionDispatcher for ActionDispatcherImpl {
    async fn dispatch(
        &self,
        module_path: &str,
        action: &str,
        form_json: &str,
        params: &serde_json::Map<String, serde_json::Value>,
        url: &str,
        method: &str,
        headers: &[(String, String)],
        locals: &serde_json::Value,
        trace: Option<&serde_json::Value>,
    ) -> std::result::Result<oxide_server::ActionRunResult, oxide_server::DispatchError> {
        match self
            .host
            .run(
                module_path,
                action,
                form_json,
                params,
                url,
                method,
                headers,
                locals,
                trace,
            )
            .await
        {
            Ok(out) => Ok(oxide_server::ActionRunResult {
                status: out.status,
                location: out.location,
                data: out.data,
                set_cookies: out.set_cookies,
                headers: out.headers,
                revalidate: out
                    .revalidate
                    .into_iter()
                    .map(|ev| match ev {
                        oxide_jshost::RevalidateEvent::Tag { tag } => ("tag".to_string(), tag),
                        oxide_jshost::RevalidateEvent::Path { path } => ("path".to_string(), path),
                    })
                    .collect(),
            }),
            Err(e) => {
                let code = e.0.code.to_string();
                let status = if code.contains("NOT-FOUND") { 404 } else { 500 };
                Err(oxide_server::DispatchError {
                    status,
                    code: code.replace("JSH-ACTION", "ACT"),
                    message: e.0.message.to_string(),
                })
            }
        }
    }
}

/// §9.3: build the action dispatcher when a pool is present.
fn build_action_dispatcher(
    pool: Option<&std::sync::Arc<dyn oxide_jshost::Pool>>,
) -> Option<std::sync::Arc<dyn oxide_server::ActionDispatcher>> {
    let pool = pool?;
    Some(std::sync::Arc::new(ActionDispatcherImpl {
        host: oxide_jshost::ActionHost::new(std::sync::Arc::clone(pool)),
    }))
}

/// §8.1 route.ts / §10.2 step 7: a bridge to the worker's `endpoint:run` op.
struct EndpointDispatcherImpl {
    host: oxide_jshost::EndpointHost,
}

#[async_trait::async_trait]
impl oxide_server::EndpointDispatcher for EndpointDispatcherImpl {
    async fn dispatch(
        &self,
        module_path: &str,
        method: &str,
        url: &str,
        params: &serde_json::Map<String, serde_json::Value>,
        headers: &[(String, String)],
        body: &[u8],
        locals: &serde_json::Value,
        trace: Option<&serde_json::Value>,
    ) -> std::result::Result<oxide_server::EndpointRunResult, oxide_server::EndpointRunError> {
        match self
            .host
            .run(
                module_path,
                method,
                url,
                params,
                headers,
                body,
                locals,
                trace,
            )
            .await
        {
            Ok(out) => {
                // §9.6: a streaming outcome's receiver becomes a `BodyStream`.
                let stream: Option<oxide_server::BodyStream> = out.stream.map(|mut rx| {
                    // The worker's chunk receiver carries `OxideError`; the
                    // transport body stream carries `std::io::Error`. Pump
                    // through a fresh channel that maps at the boundary,
                    // preserving the diagnostic code/message.
                    let (tx, mut out_rx) = tokio::sync::mpsc::channel::<
                        std::result::Result<bytes::Bytes, std::io::Error>,
                    >(8);
                    tokio::spawn(async move {
                        while let Some(item) = rx.recv().await {
                            let mapped = item.map_err(|e| {
                                std::io::Error::other(format!("{}: {}", e.0.code, e.0.message))
                            });
                            if tx.send(mapped).await.is_err() {
                                return;
                            }
                        }
                    });
                    Box::pin(futures::stream::poll_fn(move |cx| out_rx.poll_recv(cx)))
                        as oxide_server::BodyStream
                });
                Ok(oxide_server::EndpointRunResult {
                    status: out.status,
                    headers: out.headers,
                    body: out.body,
                    stream,
                })
            }
            Err(e) => Err(oxide_server::EndpointRunError {
                status: 500,
                code: e.0.code.to_string().replace("JSH-ENDPOINT", "ENDPOINT"),
                message: e.0.message.to_string(),
            }),
        }
    }
}

/// §9.6: the URL-pattern → `socket.ts` map for the WebSocket service. A
/// `socket.ts` at `routes/chat/[room]/socket` maps to `/chat/:room`; the
/// root `routes/socket` maps to `/`. Params become `:name`; catch-all `[...x]`
/// becomes `*`.
fn cfg_socket_routes(
    rootp: &std::path::Path,
    cfg: &oxide_config::OxideConfig,
    tree: &oxide_router::RouteTree,
) -> std::collections::BTreeMap<String, std::path::PathBuf> {
    let routes_root = rootp.join(&cfg.routes.dir);
    let mut out = std::collections::BTreeMap::new();
    for e in &tree.entries {
        if !matches!(e.kind, oxide_router::RouteKind::Socket) {
            continue;
        }
        // `routes/chat/[room]/socket` → segs `chat`, `[room]` → `/chat/:room`.
        let base =
            e.id.0
                .strip_prefix("routes")
                .unwrap_or(&e.id.0)
                .trim_matches('/')
                .strip_suffix("socket")
                .unwrap_or("")
                .trim_matches('/');
        let mut path = String::from("/");
        if !base.is_empty() {
            let segments: Vec<String> = base
                .split('/')
                .map(|seg| {
                    if let Some(rest) = seg.strip_prefix("[...").and_then(|s| s.strip_suffix(']')) {
                        format!("*{rest}")
                    } else if let Some(name) =
                        seg.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
                    {
                        format!(":{name}")
                    } else if let Some(name) =
                        seg.strip_prefix("[[").and_then(|s| s.strip_suffix("]]"))
                    {
                        format!(":{name}")
                    } else {
                        seg.to_string()
                    }
                })
                .collect();
            path.push_str(&segments.join("/"));
        }
        out.insert(path, routes_root.join(&e.file));
    }
    out
}

/// §8.1 route.ts / §10.2 step 7: build the endpoint dispatcher when a pool is present.
fn build_endpoint_dispatcher(
    pool: Option<&std::sync::Arc<dyn oxide_jshost::Pool>>,
) -> Option<std::sync::Arc<dyn oxide_server::EndpointDispatcher>> {
    let pool = pool?;
    Some(std::sync::Arc::new(EndpointDispatcherImpl {
        host: oxide_jshost::EndpointHost::new(std::sync::Arc::clone(pool)),
    }))
}

/// §8.1 middleware.ts / §8.5: a bridge to the worker's `middleware:run` op.
struct MiddlewareDispatcherImpl {
    host: oxide_jshost::MiddlewareHost,
}

#[async_trait::async_trait]
impl oxide_server::MiddlewareDispatcher for MiddlewareDispatcherImpl {
    async fn dispatch(
        &self,
        module_path: &str,
        method: &str,
        url: &str,
        params: &serde_json::Map<String, serde_json::Value>,
        headers: &[(String, String)],
        body: &[u8],
        locals: Option<&serde_json::Value>,
    ) -> std::result::Result<oxide_server::MiddlewareRunResult, oxide_server::MiddlewareRunError>
    {
        match self
            .host
            .run(module_path, method, url, params, headers, body, locals)
            .await
        {
            Ok(out) => Ok(oxide_server::MiddlewareRunResult {
                short: out.short,
                status: out.status,
                headers: out.headers,
                body: out.body,
                redirect: out.redirect,
                rewrite: out.rewrite,
                locals: out.locals,
            }),
            Err(e) => Err(oxide_server::MiddlewareRunError {
                status: 500,
                code: e.0.code.to_string().replace("JSH-MIDDLEWARE", "MIDDLEWARE"),
                message: e.0.message.to_string(),
            }),
        }
    }
}

/// §8.1 middleware.ts / §8.5: build the middleware dispatcher when a pool is present.
fn build_middleware_dispatcher(
    pool: Option<&std::sync::Arc<dyn oxide_jshost::Pool>>,
) -> Option<std::sync::Arc<dyn oxide_server::MiddlewareDispatcher>> {
    let pool = pool?;
    Some(std::sync::Arc::new(MiddlewareDispatcherImpl {
        host: oxide_jshost::MiddlewareHost::new(std::sync::Arc::clone(pool)),
    }))
}

/// §9.6: a bridge to the worker's `socket:run` op.
struct SocketDispatcherImpl {
    host: oxide_jshost::SocketHost,
}

#[async_trait::async_trait]
impl oxide_server::SocketDispatcher for SocketDispatcherImpl {
    async fn run(
        &self,
        module_path: &str,
        phase: &str,
        payload: serde_json::Value,
        socket_id: &str,
    ) -> std::result::Result<oxide_server::SocketRunResult, oxide_server::SocketRunError> {
        match self.host.run(module_path, phase, payload, socket_id).await {
            Ok(out) => Ok(oxide_server::SocketRunResult {
                sends: out.sends,
                broadcasts: out.broadcasts,
                targets: out.targets,
                close: out.close,
            }),
            Err(e) => Err(oxide_server::SocketRunError {
                status: 500,
                code: e.0.code.to_string().replace("JSH-SOCKET", "SOCKET"),
                message: e.0.message.to_string(),
            }),
        }
    }
}

/// §9.6: build the socket dispatcher when a pool is present.
fn build_socket_dispatcher(
    pool: Option<&std::sync::Arc<dyn oxide_jshost::Pool>>,
) -> Option<std::sync::Arc<dyn oxide_server::SocketDispatcher>> {
    let pool = pool?;
    Some(std::sync::Arc::new(SocketDispatcherImpl {
        host: oxide_jshost::SocketHost::new(std::sync::Arc::clone(pool)),
    }))
}

/// §15.1 kind 2: load the WASM component plugin the config declares, if
/// any, and wrap its `transform` hook as a closure the bundler can call.
///
/// The config's `plugins` array is `Vec<serde_json::Value>` (its shape is
/// S-21.5: resolve a plugin's declared `path` against the project root and
/// confine the result to that root. A plugin entry is config-controlled, but a
/// config can arrive from an untrusted template/PR, and an absolute path or a
/// `..`-escaping relative path would let it load code from anywhere on the
/// host. Absolute paths are refused; a relative path is joined to the
/// canonical root and required to stay under it (symlinks resolved first).
/// Returns `None` (with a warning) when the entry escapes or is unreadable.
fn confine_plugin_path(root: &Path, rel: &str, allow_absolute: bool) -> Option<PathBuf> {
    if Path::new(rel).is_absolute() {
        if allow_absolute {
            return Some(PathBuf::from(rel));
        }
        eprintln!(
            "warn: plugin path {rel:?} is absolute; refusing (set plugins.allowAbsolutePaths = true to permit)"
        );
        return None;
    }
    let root_abs = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let candidate = root.join(rel);
    // Canonicalize to resolve `..`/symlinks; fall back to the lexical join when
    // the target does not exist (the caller reports "not readable").
    let resolved = std::fs::canonicalize(&candidate).unwrap_or(candidate);
    if !resolved.starts_with(&root_abs) {
        eprintln!("warn: plugin path {rel:?} escapes the project root; refusing");
        return None;
    }
    Some(resolved)
}

/// S-21.5: whether the operator has opted out of plugin path confinement.
/// `plugins` is an array, so the flag is read from any entry that sets
/// `allowAbsolutePaths: true` (a config-wide opt-in).
fn plugins_allow_absolute(cfg: &OxideConfig) -> bool {
    cfg.plugins.iter().any(|p| {
        p.get("allowAbsolutePaths")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    })
}

/// the plugin manifest plus a path). A WASM entry looks like:
///
/// ```text
///     { "kind": "wasm", "path": "./plugins/my-plugin.wasm",
///       "name": "my-plugin", "witWorld": "oxide:plugin/hooks",
///       "capabilities": [], "hooks": ["transform"] }
/// ```
///
/// Returns `None` when no WASM plugin is configured, when the path does not
/// exist, or when the component fails to load — a broken plugin is reported
/// on stderr and the build continues without it, matching how the framework
/// stack degrades. Making a broken plugin fail the build is a policy choice
/// for a follow-up.
/// §6.6: the CSS transform the bundler calls for every emitted stylesheet.
/// Wraps `LightningCssBackend` (Lightning CSS with the project's browser
/// target) so the bundler does not depend on `oxide-assets`.
fn build_css_transform_hook(cfg: &OxideConfig) -> oxide_bundler::CssTransformHook {
    use oxide_assets::{CssBackend as _, CssInput, CssKind, LightningCssBackend};

    // `build.target` is the browserslist expression (`baseline-widely-available`
    // by default); lightningcss accepts the same shape.
    let targets = vec![cfg.build.target.clone()];
    oxide_bundler::CssTransformHook::new(move |path, source, is_module, minify| {
        let input = CssInput {
            code: source.to_string(),
            file: Some(path.to_string()),
            kind: if is_module {
                CssKind::Module
            } else {
                CssKind::Global
            },
            minify,
            targets: targets.clone(),
        };
        LightningCssBackend
            .transform(&input)
            .map(|out| (out.code, out.exports))
            .map_err(|e| {
                oxide_bundler::err(
                    "BLD-CSS",
                    format!("lightningcss failed for {path}: {}", e.0.message),
                )
            })
    })
}

/// SPEC-17 / §15.1: run the JS plugins' `transform` hook through the tooling
/// worker bridge. Each `kind: js` plugin is imported once (`plugin:load`),
/// then its `transform(code, id)` runs per module. A plugin that does not
/// export `transform` is skipped for that hook.
fn build_js_plugin_hook(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_jshost::Pool>>,
) -> Option<oxide_bundler::TransformHook> {
    let pool = pool?;
    let mut plugins: Vec<String> = Vec::new();
    for p in &cfg.plugins {
        if p.get("kind").and_then(|v| v.as_str()) != Some("js") {
            continue;
        }
        let Some(rel) = p.get("path").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(abs) = confine_plugin_path(root, rel, plugins_allow_absolute(cfg)) else {
            continue;
        };
        if !abs.is_file() {
            eprintln!("warn: js plugin {} not found; skipping", abs.display());
            continue;
        }
        plugins.push(abs.to_string_lossy().to_string());
    }
    // §15-E: modules declaring `hooks: ["transform"]` contribute their entry
    // to the same chain — a module is a plugin that happens to also ship
    // routes. Each is loaded the same way `plugin:call` loads a kind:js
    // plugin (`<entry>` imported, `<hook>` invoked). `load_quiet` keeps the
    // missing-package / no-contract warnings from printing twice:
    // `scan_routes` already warned once during the route-merge pass.
    for m in crate::modules_loader::load_quiet(root, cfg) {
        if !m.hooks.iter().any(|h| h == "transform") {
            continue;
        }
        let Some(ref entry) = m.entry else {
            eprintln!(
                "warn: module `{}` declares `transform` but has no entry file; skipping",
                m.name,
            );
            continue;
        };
        if !entry.is_file() {
            eprintln!(
                "warn: module `{}` entry {} is missing; skipping transform",
                m.name,
                entry.display(),
            );
            continue;
        }
        plugins.push(entry.to_string_lossy().to_string());
    }
    if plugins.is_empty() {
        return None;
    }
    let pool = std::sync::Arc::clone(pool);
    let loaded = std::sync::Arc::new(std::sync::Mutex::new(false));

    // The hook is sync; the bundler calls it from inside its own tokio
    // runtime. Calling `Runtime::block_on` there deadlocks the worker (the
    // outer runtime never polls). Run the bridge on a scoped thread instead —
    // the same pattern `emit_snapshot_blob` uses.
    Some(oxide_bundler::TransformHook::new(move |id, source| {
        let pool = std::sync::Arc::clone(&pool);
        let loaded = std::sync::Arc::clone(&loaded);
        let plugins = plugins.clone();
        let id = id.to_string();
        let source = source.to_string();
        std::thread::scope(|scope| {
            scope
                .spawn(|| -> oxide_diagnostics::Result<String> {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| {
                            oxide_diagnostics::OxideError(oxide_diagnostics::Diagnostic::error(
                                "PLG-BRIDGE",
                                format!("bridge runtime: {e}"),
                            ))
                        })?;
                    rt.block_on(async move {
                        let need_load = {
                            let mut g = loaded.lock().unwrap();
                            if *g {
                                false
                            } else {
                                *g = true;
                                true
                            }
                        };
                        let mut code = source;
                        for path in &plugins {
                            if need_load {
                                let load = oxide_jshost::CallInput {
                                    payload: serde_json::json!({ "path": path }),
                                    headers: Default::default(),
                                    has_body: false,
                                    request_id: format!("plugin:load:{path}"),
                                    route: None,
                                };
                                let entry = oxide_jshost::EntryRef {
                                    kind: "plugin:load".into(),
                                    module: path.clone(),
                                    export: "default".into(),
                                };
                                let _ = pool.call(entry, load).await;
                            }
                            let call = oxide_jshost::CallInput {
                                payload: serde_json::json!({
                                    "path": path,
                                    "hook": "transform",
                                    "args": { "code": code, "id": id },
                                }),
                                headers: Default::default(),
                                has_body: false,
                                request_id: format!("plugin:call:{path}:transform"),
                                route: None,
                            };
                            let entry = oxide_jshost::EntryRef {
                                kind: "plugin:call".into(),
                                module: path.clone(),
                                export: "transform".into(),
                            };
                            let mut stream = pool.call(entry, call).await.map_err(|e| {
                                oxide_diagnostics::OxideError(oxide_diagnostics::Diagnostic::error(
                                    "PLG-CALL",
                                    format!("plugin transform failed: {}", e.0.message),
                                ))
                            })?;
                            let bytes = stream.collect().await.map_err(|e| {
                                oxide_diagnostics::OxideError(oxide_diagnostics::Diagnostic::error(
                                    "PLG-CALL",
                                    format!("plugin transform read failed: {}", e.0.message),
                                ))
                            })?;
                            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                                if let Some(s) = hook_result_str(&v, "code") {
                                    code = s;
                                }
                            }
                        }
                        Ok(code)
                    })
                })
                .join()
                .unwrap_or_else(|_| {
                    Err(oxide_diagnostics::OxideError(
                        oxide_diagnostics::Diagnostic::error(
                            "PLG-BRIDGE",
                            "plugin bridge thread panicked",
                        ),
                    ))
                })
        })
    }))
}

// ─── SPEC-15.1 / §15.2: JS plugin chain ──────────────────────────────────
//
// A JS plugin is a Node ESM module exporting hook functions by name
// (`transform`, `resolveId`, `load`, …). The worker (`dispatchPluginCall`
// in `oxide-worker.mjs`) is a generic dispatcher — it takes `{ path, hook,
// args }` and calls `mod[hook](ctx, args)`. What lives HERE is the Rust
// side: load N plugin modules, validate their hook declarations at load
// time (SPEC-15.2 "fail at load"), and call a hook across the chain in
// the order the config declares them.
//
// This is the infrastructure §15-B hooks the bundler and prerender
// pipeline against; the wiring itself (BundleOptions.resolve_hook etc.)
// lands in a follow-up commit.

/// One loaded JS plugin: its pool (shared) + absolute path + declared hooks.
struct JsPluginHandle {
    pool: std::sync::Arc<dyn oxide_jshost::Pool>,
    path: std::path::PathBuf,
    hooks: std::collections::BTreeSet<oxide_plugin::Hook>,
}

/// An ordered list of loaded JS plugins. Empty when `config.plugins` has no
/// `kind: "js"` entries — `call` and `has` are then cheap no-ops.
struct JsPluginChain {
    plugins: Vec<JsPluginHandle>,
}

impl JsPluginChain {
    /// True when at least one loaded plugin declares `hook`. Callers use
    /// this to skip the `call` machinery entirely for a hook no plugin
    /// implements — the bundler/prerender wiring checks this before doing
    /// any per-module work.
    fn has(&self, hook: oxide_plugin::Hook) -> bool {
        self.plugins.iter().any(|p| p.hooks.contains(&hook))
    }

    /// Call `hook` across the chain, in declaration order. Returns:
    ///   * `None` when no plugin declares `hook` — the caller skips entirely.
    ///   * `Some(Ok(values))` when all plugins that declared `hook` returned
    ///     a value. `values` is one entry per plugin, in order. Chaining
    ///     semantics (feed output A to input B, first-non-null wins, …) are
    ///     the CALLER's job — different hooks have different rules
    ///     (`transform` chains code; `resolveId` first-wins; `load` first-
    ///     non-null wins; lifecycle hooks just need to run).
    ///   * `Some(Err(_))` when a plugin call failed.
    async fn call(
        &self,
        hook: oxide_plugin::Hook,
        args: serde_json::Value,
    ) -> Option<anyhow::Result<Vec<serde_json::Value>>> {
        use futures::StreamExt as _;
        if !self.has(hook) {
            return None;
        }
        let hook_name = js_hook_name(hook);
        let mut results = Vec::new();
        for p in &self.plugins {
            if !p.hooks.contains(&hook) {
                continue;
            }
            let req = oxide_jshost::CallInput {
                payload: serde_json::json!({
                    "path": p.path.to_string_lossy(),
                    "hook": hook_name,
                    "args": args,
                }),
                headers: Default::default(),
                has_body: false,
                request_id: format!("plugin:call:{}:{}", p.path.display(), hook_name),
                route: None,
            };
            let entry = oxide_jshost::EntryRef {
                kind: "plugin:call".into(),
                module: String::new(),
                export: String::new(),
            };
            let mut stream = match p.pool.call(entry, req).await {
                Ok(s) => s,
                Err(e) => {
                    return Some(Err(anyhow::anyhow!(
                        "plugin:call failed for {} ({}): {} {}",
                        p.path.display(),
                        hook_name,
                        e.0.code,
                        e.0.message
                    )));
                }
            };
            let bytes = match stream.collect().await {
                Ok(b) => b,
                Err(e) => {
                    return Some(Err(anyhow::anyhow!(
                        "plugin:call read failed for {} ({}): {}",
                        p.path.display(),
                        hook_name,
                        e.0.message
                    )));
                }
            };
            let v: serde_json::Value = match serde_json::from_slice(&bytes) {
                Ok(v) => v,
                Err(e) => {
                    return Some(Err(anyhow::anyhow!(
                        "plugin:call returned non-JSON for {} ({}): {}",
                        p.path.display(),
                        hook_name,
                        e
                    )));
                }
            };
            results.push(v);
        }
        Some(Ok(results))
    }
}

/// Reverse of [`parse_hook_name`] — the JS-side property name for each
/// hook. Matches Vite's camelCase convention so a plugin author writes
/// `export function transform(...)` and the dispatch looks up `mod.transform`.
fn js_hook_name(h: oxide_plugin::Hook) -> &'static str {
    use oxide_plugin::Hook;
    match h {
        Hook::Config => "config",
        Hook::ConfigResolved => "configResolved",
        Hook::BuildStart => "buildStart",
        Hook::ResolveId => "resolveId",
        Hook::Load => "load",
        Hook::Transform => "transform",
        Hook::RenderChunk => "renderChunk",
        Hook::GenerateBundle => "generateBundle",
        Hook::BuildEnd => "buildEnd",
        Hook::TransformHtml => "transformHtml",
        Hook::HandleHotUpdate => "handleHotUpdate",
        Hook::ConfigureServer => "configureServer",
        Hook::Routes => "routes",
        Hook::Adapter => "adapter",
        Hook::ServerMiddleware => "serverMiddleware",
        Hook::RenderHead => "renderHead",
        Hook::CacheHandler => "cacheHandler",
        Hook::Typegen => "typegen",
        Hook::Diagnostics => "diagnostics",
    }
}

/// Parse a camelCase hook name from the JS side into the `Hook` enum.
/// Accepts both camelCase (Vite style) and snake_case (Oxide's internal
/// spelling) so a plugin author can use either.
fn parse_hook_name(s: &str) -> Option<oxide_plugin::Hook> {
    use oxide_plugin::Hook;
    Some(match s {
        "config" => Hook::Config,
        "configResolved" | "config_resolved" => Hook::ConfigResolved,
        "buildStart" | "build_start" => Hook::BuildStart,
        "resolveId" | "resolve_id" => Hook::ResolveId,
        "load" => Hook::Load,
        "transform" => Hook::Transform,
        "renderChunk" | "render_chunk" => Hook::RenderChunk,
        "generateBundle" | "generate_bundle" => Hook::GenerateBundle,
        "buildEnd" | "build_end" => Hook::BuildEnd,
        "transformHtml" | "transform_html" => Hook::TransformHtml,
        "handleHotUpdate" | "handle_hot_update" => Hook::HandleHotUpdate,
        "configureServer" | "configure_server" => Hook::ConfigureServer,
        "routes" => Hook::Routes,
        "adapter" => Hook::Adapter,
        "middleware" | "serverMiddleware" | "server_middleware" => Hook::ServerMiddleware,
        "renderHead" | "render_head" => Hook::RenderHead,
        "cacheHandler" | "cache_handler" => Hook::CacheHandler,
        "typegen" => Hook::Typegen,
        "diagnostics" => Hook::Diagnostics,
        _ => return None,
    })
}

/// Load every `kind: "js"` plugin in `config.plugins`, validate its
/// declared hooks, and return the chain. A plugin that declares an
/// unknown hook fails the load (SPEC-15.2) — the build stops rather than
/// silently ignoring the hook. A plugin whose `plugin:load` call fails
/// (syntax error, missing export) also fails the load with the plugin's
/// path named.
async fn build_js_plugin_chain(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_jshost::Pool>>,
) -> anyhow::Result<JsPluginChain> {
    use futures::StreamExt as _;
    let Some(pool) = pool else {
        return Ok(JsPluginChain {
            plugins: Vec::new(),
        });
    };
    let mut plugins = Vec::new();
    for (idx, p) in cfg.plugins.iter().enumerate() {
        if p.get("kind").and_then(|v| v.as_str()) != Some("js") {
            continue;
        }
        let Some(rel) = p.get("path").and_then(|v| v.as_str()) else {
            eprintln!("warn: js plugin #{} has no `path`; skipping", idx);
            continue;
        };
        let Some(abs) = confine_plugin_path(root, rel, plugins_allow_absolute(cfg)) else {
            continue;
        };
        if !abs.is_file() {
            eprintln!("warn: js plugin {} not readable; skipping", abs.display());
            continue;
        }
        // Call `plugin:load` — the worker imports the module and reports
        // which hook exports it has.
        let req = oxide_jshost::CallInput {
            payload: serde_json::json!({ "path": abs.to_string_lossy() }),
            headers: Default::default(),
            has_body: false,
            request_id: format!("plugin:load:{idx}"),
            route: None,
        };
        let entry = oxide_jshost::EntryRef {
            kind: "plugin:load".into(),
            module: String::new(),
            export: String::new(),
        };
        let mut stream = pool.call(entry, req).await.map_err(|e| {
            anyhow::anyhow!(
                "plugin:load failed for {}: {} {}",
                abs.display(),
                e.0.code,
                e.0.message
            )
        })?;
        let bytes = stream.collect().await.map_err(|e| {
            anyhow::anyhow!(
                "plugin:load read failed for {}: {}",
                abs.display(),
                e.0.message
            )
        })?;
        let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
            anyhow::anyhow!("plugin:load returned non-JSON for {}: {}", abs.display(), e)
        })?;
        let hook_names: Vec<String> = v
            .get("hooks")
            .and_then(|h| h.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        // SPEC-15.2: fail at load for an unsupported hook.
        let mut hooks = std::collections::BTreeSet::new();
        for name in &hook_names {
            match parse_hook_name(name) {
                Some(h) => {
                    hooks.insert(h);
                }
                None => {
                    anyhow::bail!(
                        "plugin {} declares unknown hook `{}`. Supported hooks: see docs/plugin-compat.md",
                        abs.display(),
                        name
                    );
                }
            }
        }
        plugins.push(JsPluginHandle {
            pool: std::sync::Arc::clone(pool),
            path: abs,
            hooks,
        });
    }
    Ok(JsPluginChain { plugins })
}

fn build_transform_hook(root: &Path, cfg: &OxideConfig) -> Option<oxide_bundler::TransformHook> {
    for p in &cfg.plugins {
        let kind = p.get("kind").and_then(|v| v.as_str());
        if kind != Some("wasm") {
            continue;
        }
        let Some(rel) = p.get("path").and_then(|v| v.as_str()) else {
            eprintln!("warn: wasm plugin entry has no `path`; skipping");
            continue;
        };
        let Some(abs) = confine_plugin_path(root, rel, plugins_allow_absolute(cfg)) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(&abs) else {
            eprintln!("warn: wasm plugin {} not readable; skipping", abs.display());
            continue;
        };

        let manifest = oxide_plugin::WasmPluginManifest {
            name: p
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("wasm-plugin")
                .to_string(),
            wit_world: p
                .get("witWorld")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            capabilities: p
                .get("capabilities")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            hooks: p
                .get("hooks")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .filter_map(|s| match s {
                            "transform" => Some(oxide_plugin::Hook::Transform),
                            "load" => Some(oxide_plugin::Hook::Load),
                            "resolveId" | "resolve_id" => Some(oxide_plugin::Hook::ResolveId),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };

        let plugin = match oxide_plugin::WasmPlugin::load(&bytes, manifest, Some(&abs)) {
            Ok(p) => p,
            Err(e) => {
                eprintln!(
                    "warn: wasm plugin {} failed to load: {} {}; skipping",
                    abs.display(),
                    e.0.code,
                    e.0.message
                );
                continue;
            }
        };
        // Only a plugin that declares *and* exports `transform` gets a hook.
        if !plugin
            .available_hooks()
            .contains(&oxide_plugin::Hook::Transform)
        {
            continue;
        }
        let plugin = std::sync::Mutex::new(plugin);
        return Some(oxide_bundler::TransformHook::new(move |_id, source| {
            // `call_string_hook` takes `&mut self` (the store is stateful);
            // a `Mutex` makes the closure `Sync`. Plugin transforms are not
            // hot enough for the lock to matter.
            let mut guard = plugin.lock().unwrap_or_else(|p| p.into_inner());
            guard.call_string_hook(&oxide_plugin::Hook::Transform, source)
        }));
    }
    None
}

/// §16.4: stamp the project's locales into `dist/client/client.manifest.json`.
///
/// The client fetches `/client/messages/<locale>.json` on every page load to
/// warm the catalog. A project with no i18n has no such file, so that fetch
/// is a guaranteed 404 — which Chromium reports as a console error and which
/// breaks any test asserting a clean console.
///
/// The bundler does not know about i18n (it is framework- and feature-
/// agnostic), so the CLI — which owns the config — stamps the answer into the
/// manifest it already post-processes. The client then skips the fetch when
/// the list is empty.
fn stamp_i18n_locales(root: &Path, cfg: &oxide_config::OxideConfig) {
    let manifest_path = root.join("dist/client/client.manifest.json");
    let Ok(raw) = std::fs::read_to_string(&manifest_path) else {
        return;
    };
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };
    let Some(obj) = v.as_object_mut() else { return };
    // The signal is "were catalogs emitted", not "what does config say":
    // `i18n.locales` defaults to `["en"]` even for a project that defines no
    // messages, so a config-driven check would claim catalogs exist and the
    // client would 404 on every load. `dist/client/messages/` is the artifact
    // that decides it.
    let emitted = root.join("dist/client/messages");
    let has_catalogs = std::fs::read_dir(&emitted)
        .map(|rd| {
            rd.flatten()
                .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        })
        .unwrap_or(false);
    if has_catalogs {
        obj.insert(
            "i18n_locales".to_string(),
            serde_json::json!(cfg.i18n.locales),
        );
    } else {
        obj.remove("i18n_locales");
    }
    if let Ok(out) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::write(&manifest_path, out + "\n");
    }
}

fn load_client_assets(rootp: &Path) -> Option<oxide_server::StaticAssets> {
    let dist = rootp.join("dist");
    if !dist.is_dir() {
        return None;
    }
    oxide_server::StaticAssets::scan(dist).ok()
}

/// BUG-11: turn `server.host` into a bind IP. `localhost` (and `""`) → the
/// loopback v4 address; an IP literal → itself; `0.0.0.0`/`::` → unspecified
/// (all interfaces). A hostname that is not an IP literal is a config error a
/// operator should see, but binding must still work — we warn and fall back to
/// loopback rather than fail to start.
/// SPEC-16.6: extract the `.svg?react` specifier from an import line.
/// Matches `import X from './foo.svg?react'` and `import './foo.svg?react'`
/// in either quote style, then returns the specifier (still with `?react`).
/// SPEC-16-A: extract `./hero.png?w=400,800` from an import line, returning
/// `(specifier, widths)`. Only matches relative specifiers ending in an
/// image extension (`.png`, `.jpg`, `.jpeg`, `.webp`, `.avif`, `.gif`)
/// followed by `?w=<comma-separated ints>`.
fn extract_img_w_spec(line: &str) -> Option<(&str, Vec<u32>)> {
    let trimmed = line.trim();
    if !trimmed.starts_with("import") {
        return None;
    }
    for q in ['"', '\''] {
        if let Some(start) = line.find(&format!("{q}.")) {
            let after = &line[start + 1..];
            if let Some(end) = after.find(q) {
                let candidate = &after[..end];
                if let Some((path, query)) = candidate.split_once("?w=") {
                    let is_img = path.ends_with(".png")
                        || path.ends_with(".jpg")
                        || path.ends_with(".jpeg")
                        || path.ends_with(".webp")
                        || path.ends_with(".avif")
                        || path.ends_with(".gif");
                    if is_img {
                        let widths: Vec<u32> = query
                            .split(',')
                            .filter_map(|s| s.trim().parse::<u32>().ok())
                            .collect();
                        if !widths.is_empty() {
                            return Some((candidate, widths));
                        }
                    }
                }
            }
        }
    }
    None
}

fn extract_svg_react_spec(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if !trimmed.starts_with("import") {
        return None;
    }
    for q in ['"', '\''] {
        // Find last `q` before a closing `q`; the specifier is between them
        // ONLY if it ends with `.svg?react`.
        if let Some(start) = line.find(&format!("{q}.")) {
            let after = &line[start + 1..];
            if let Some(end) = after.find(q) {
                let candidate = &after[..end];
                if candidate.ends_with(".svg?react") {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn resolve_bind_host(host: &str) -> std::net::IpAddr {
    let h = host.trim();
    if h.is_empty() || h.eq_ignore_ascii_case("localhost") {
        return std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
    }
    if let Ok(ip) = h.parse::<std::net::IpAddr>() {
        return ip;
    }
    tracing::warn!(
        host = h,
        "server.host is not an IP literal; binding loopback (use an IP like 0.0.0.0 to expose)"
    );
    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
}

async fn start_server_inner_full(
    root: &str,
    bind: Option<(std::net::IpAddr, u16)>,
    with_inspector: bool,
    tls: Option<std::sync::Arc<tokio_rustls::rustls::ServerConfig>>,
    proxy_rules: Vec<oxide_dev::ProxyRule>,
    stack: crate::framework_shim::FrameworkStack,
    hmr: Option<std::sync::Arc<oxide_dev::HmrHub>>,
) -> anyhow::Result<StartedServer> {
    // Canonicalize so every path derived here (route modules, the SSR entry,
    // the client manifest) is absolute — the worker resolves a module path
    // against its own cwd, so a relative root breaks `action:run` / `loader:run`.
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    // §16.5 / WP-r9-1: thread the project root to worker
    // subprocesses so `getCollection` resolves `dist/content`
    // against the project, not the shell's cwd. Edition 2024:
    // set_var is process-global; this runs before any worker
    // thread spawns.
    unsafe {
        std::env::set_var("OXIDE_ROOT", rootp.to_string_lossy().as_ref());
    }
    // §16.6 / WP-r10-1: the config env must match the server mode.
    // Before this fix, dev loaded "production" and so ignored
    // `security.sessionTtlSecs` set in the dev config (a common
    // testing knob — set a 60s TTL in dev, 1800s in prod). This is the
    // only place the TTL is published, so fixing the env here is the
    // whole fix.
    let cfg_env = if with_inspector {
        "development"
    } else {
        "production"
    };
    let mut cfg = load_config_or_default(&rootp, cfg_env);
    set_session_ttl(cfg.security.session_ttl_secs);
    // §10.1: refine inferred render modes from the AST, exactly as `oxide build`
    // does — otherwise the live server sees the scanner's line-oriented guess
    // (and `None` for a route with no `page.server.ts`), and the §11 L0 layer
    // cannot tell a static route from a dynamic one.
    let tree = std::sync::Arc::new(refine_render_modes_from_ast(
        &rootp,
        &cfg,
        scan_routes(&rootp, &cfg, stack.tooling_pool.as_ref()).await?,
    ));
    let (bind_ip, requested_port) = match bind {
        // A caller-supplied port is honoured verbatim — including 0, which
        // means "ephemeral, OS chooses" (the test and CI convention).
        Some((ip, p)) => (ip, p),
        None => (resolve_bind_host(&cfg.server.host), cfg.server.port),
    };
    let port = resolve_port(requested_port).await;
    let rules = oxide_router::RulesEngine::compile(config_rules_to_router(&cfg.rules))
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    // §20 metrics: ONE registry shared by the pipeline (native branches) and
    // the dispatch delegate (per-route labels). Cardinality is bounded by the
    // route pattern (e.g. `/blog/[slug]`), never the resolved URL.
    let metrics = std::sync::Arc::new(oxide_server::Metrics::new());

    let assets = load_client_assets(&rootp);
    let image_endpoint = if cfg.images_endpoint.enabled {
        build_image_endpoint(&rootp, &cfg)
    } else {
        None
    };
    // S-09: the OG rasterization endpoint is opt-in (`og.enabled: true`) — it
    // is CPU-bound and, when reachable, unauthenticated.
    let og_endpoint = if cfg.og.enabled {
        build_og_endpoint()
    } else {
        None
    };
    let server_fns = build_server_fn_dispatcher(&rootp, stack.pool.as_ref());
    let actions = build_action_dispatcher(stack.pool.as_ref());
    let endpoint_dispatcher = build_endpoint_dispatcher(stack.pool.as_ref());
    let middleware_dispatcher = build_middleware_dispatcher(stack.pool.as_ref());
    // §9.6: the WebSocket route service. Built only when the project has
    // any `socket.ts` routes AND a dispatcher (a pool). URL segments →
    // `socket.ts` path, so the hot path is a map probe.
    let socket_service: Option<std::sync::Arc<oxide_server::SocketService>> = match (
        build_socket_dispatcher(stack.pool.as_ref()),
        cfg_socket_routes(&rootp, &cfg, &tree),
    ) {
        (Some(dispatcher), routes) if !routes.is_empty() => {
            Some(std::sync::Arc::new(oxide_server::SocketService {
                dispatcher,
                routes: std::sync::Arc::new(routes),
                registry: std::sync::Arc::new(std::sync::Mutex::new(
                    std::collections::HashMap::new(),
                )),
            }))
        }
        _ => None,
    };
    // SEC-09: mint the dev inspector token when the inspector is attached
    // (dev only). `serve` (production) never sets an inspector, so this is
    // `None` there.
    let dev_token: Option<String> = with_inspector.then(oxide_server::dev_token);
    let client_manifest_json: Option<serde_json::Value> = {
        let client_manifest = rootp.join("dist/client/client.manifest.json");
        std::fs::read_to_string(&client_manifest)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
    };
    let boot_script_url: Option<String> = client_manifest_json.as_ref().and_then(|v| {
        v.get("routes")
            .and_then(|r| r.get("__boot__"))
            .and_then(|r| r.get("entry_chunk"))
            .and_then(|e| e.as_str())
            .map(|f| format!("client/{f}"))
    });
    // §11 L0: a response cache for static/isr routes. The build id scopes
    // entries to this deployment, so a rebuild invalidates them; it comes from
    // the client manifest, falling back to a source-derived hash when no build
    // is present (a dev server with no `dist/` still serves, just uncached).
    let build_id = client_manifest_json
        .as_ref()
        .and_then(|v| v.get("build_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| "dev".to_string());

    // §20: the per-request waterfall recorder the inspector renders. Created
    // here so the dispatch closure (which records) and the inspector (which
    // reads) share one instance. `None` for a non-inspector server — the
    // dispatch then skips recording (a branch, not an allocation).
    let waterfall_recorder: Option<oxide_dev::WaterfallRecorder> =
        with_inspector.then(oxide_dev::WaterfallRecorder::new);
    // §16.6: the session codec from `security.sessionKeys`. A malformed key
    // (too short) is a hard config error — silently falling back to an
    // unsigned cookie would be a security downgrade.
    let session_codec: Option<std::sync::Arc<oxide_server::SessionCodec>> = {
        let keys: Vec<Vec<u8>> = cfg
            .security
            .session_keys
            .iter()
            .map(|k| k.as_bytes().to_vec())
            .collect();
        if keys.is_empty() {
            None
        } else {
            match oxide_server::SessionCodec::new(&keys) {
                Ok(c) => Some(std::sync::Arc::new(c)),
                Err(e) => {
                    eprintln!(
                        "error: security.sessionKeys is invalid: {} {}",
                        e.0.code, e.0.message
                    );
                    return Err(anyhow::anyhow!("invalid session keys"));
                }
            }
        }
    };

    // §6.4 / Wave M: capture the renderer handle for warmup BEFORE it
    // moves into the pipeline. Arc clone is cheap; the renderer itself
    // lives in the pipeline but we need a second reference for the task.
    let renderer_to_warm = stack.page_renderer.clone();
    let render_ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
        renderer_to_warm.is_none(),
    ));
    let pipeline = std::sync::Arc::new(oxide_server::Pipeline {
        // A3: OG endpoint rate limit. The rasterization is CPU-bound and the
        // endpoint is unauthenticated; 10 renders per peer per minute bounds
        // the worst case without blocking a legitimate OG card.
        og_rate_limiter: oxide_server::RateLimiter::default(),
        og_rate_max: cfg.og.rate_limit.max,
        og_rate_window_secs: cfg.og.rate_limit.window_secs,

        rules,
        assets,
        image_endpoint,
        og_endpoint,
        server_fns,
        actions: actions.clone(),
        limits: oxide_server::Limits::default(),
        // SPEC-10: locale policy for the pipeline (None when unconfigured).
        i18n: {
            let c = &cfg.i18n;
            (!c.locales.is_empty()).then(|| oxide_server::I18nPolicy {
                locales: c.locales.clone(),
                default: c.default_locale.clone(),
                strategy: c.strategy.clone(),
            })
        },
        security: oxide_server::SecuritySettings {
            csp: cfg.security.csp.clone(),
            csrf: cfg.security.csrf.clone(),
            // SEC-05: populate the allow-lists from config — they were never
            // read, so a configured Host/SSRF list silently did nothing.
            host_allow_list: cfg.security.host_allow_list.clone(),
            ssrf_allow_list: cfg.security.ssrf_allow_list.clone(),
            cookie_keys: {
                // SEC-10: an empty `security.cookieKeys` must not leave
                // cookies signed with the empty key. Derive from the project
                // secret (`.oxide/secret`, BUG-05), with a warn so a multi-node
                // deployment knows to set explicit keys.
                let configured: Vec<Vec<u8>> = cfg
                    .security
                    .cookie_keys
                    .iter()
                    .map(|k| k.as_bytes().to_vec())
                    .collect();
                if configured.is_empty() {
                    match build_secret(&rootp) {
                        Ok(s) => {
                            eprintln!(
                                "warn: security.cookieKeys is empty; signing cookies with \
                                 the project secret. Set security.cookieKeys for multi-node."
                            );
                            vec![s.into_bytes()]
                        }
                        Err(_) => Vec::new(),
                    }
                } else {
                    configured
                }
            },
            // §16.6: session-encryption keys. Empty -> the pipeline's
            // session helpers fall back to signed-only cookies (documented).
            session_keys: cfg
                .security
                .session_keys
                .iter()
                .map(|k| k.as_bytes().to_vec())
                .collect(),
            // S-10: trust X-Forwarded-For only behind these proxies.
            trusted_proxies: cfg.security.trusted_proxies.clone(),
            // S-21.3: whether the `x-oxide-cache` status header is emitted.
            expose_cache_header: cfg.security.expose_cache_header,
        },
        rate_limiter: std::sync::Arc::new(oxide_server::RateLimiter::default()),
        health_path: "/_oxide/health".to_string(),
        ready_path: "/_oxide/ready".to_string(),
        render_ready: std::sync::Arc::clone(&render_ready),
        metrics_path: "/_oxide/metrics".to_string(),
        metrics: Some(std::sync::Arc::clone(&metrics)),
        inspector: if with_inspector {
            // §20: with a recorder wired, the inspector's waterfall table
            // reflects the live per-request timings instead of staying empty.
            let rt = match waterfall_recorder.clone() {
                Some(rec) => oxide_dev::RouteTableInspector::from_router_table_with_recorder(
                    tree.table(),
                    rec,
                ),
                None => oxide_dev::RouteTableInspector::from_router_table(tree.table()),
            };
            Some(std::sync::Arc::new(CliInspector { state: rt.state() }))
        } else {
            None
        },
        page_renderer: None,
        // SPEC-14: the HMR hub's broadcast sender, dev only.
        hmr: hmr.as_ref().map(|h| h.sender()),
        // §13.3: advertise the client manifest's build id so a stale tab can
        // detect a rebuild and reload.
        hmr_build_id: build_id.clone(),
        // SEC-02: HSTS only on TLS (§17.1) — set where the listener is built.
        tls: tls.is_some(),
        // SEC-09: the dev inspector token (see below).
        dev_token: dev_token.clone(),
        // S-03/S-04/S-21.1: dev servers get the DNS-rebinding Host gate on
        // the HMR/socket upgrade arms. Production never sets it.
        dev_bind_ip: with_inspector.then_some(bind_ip),
        // §9.6: WebSocket route dispatch (None when the project has no
        // `socket.ts` routes or no pool).
        sockets: socket_service,
        // §12.1: advertise HTTP/3 (Alt-Svc) only when it is actually enabled.
        // The h3 listener shares the TCP port over UDP. `port` is resolved
        // above (a probe picks a free port for a requested 0); the h3 bind
        // uses the same value.
        http3_advertise: cfg.server.h3_advertise,
        http3_port: if cfg.server.http3 && tls.is_some() {
            port
        } else {
            0
        },
    });
    let proxy_rules: Vec<oxide_dev::ProxyRule> = proxy_rules.clone();
    let routes_root = rootp.join(&cfg.routes.dir);
    // §10.3: the same boot URL that the build-time prerender injects also
    // has to be spliced into every live request's shell — otherwise dev /
    // serve would ship pages whose `<ox-island>` markup is inert.

    // BUG-02: wrap the framework stack's cache tier, not a fresh one. The
    // worker's `'use cache'` entries and the L0 response cache now share one
    // handler, so BUG-01's invalidation reaches both. A stack built without a
    // cache (tests, `FrameworkStack::empty()`) gets a private tier.
    // BUG-11: honour `cache.l0.maxSize` (parsed: "512mb" | "64kb" | bytes).
    // A malformed value falls back to 512 MiB with a warning rather than
    // refusing to start.
    // §14.1: `cache.l0.maxSize`.
    let l0_bytes = oxide_config::parse_size(&cfg.cache.l0.max_size).unwrap_or_else(|_| {
        tracing::warn!(
            value = %cfg.cache.l0.max_size,
            "cache.l0.maxSize is malformed; using 512mb"
        );
        512 * 1024 * 1024
    });
    // §11.2: `cache.handler` selects the L2 remote. The LIVE wiring is in
    // `build_framework_stack` (every `cmd_*` site threads `cfg.cache.handler`
    // in, so the worker's tier honors the config); this arm only fires for a
    // stack built WITHOUT a tier (tests, `FrameworkStack::empty()`) — it
    // keeps the same handler mapping so a test stack still exercises the
    // config path. Memory/disk are the local tiers every handler gets.
    let l0_tier: std::sync::Arc<dyn oxide_cache::CacheHandler> = match stack.cache_tier.clone() {
        Some(tier) => tier,
        None => {
            let mut tier = oxide_cache::TieredCache::new(l0_bytes);
            match &cfg.cache.handler {
                oxide_config::CacheHandlerConfig::Memory => {}
                oxide_config::CacheHandlerConfig::Disk => {
                    // §11.2 L1: a durable local disk tier so a restart keeps
                    // its cache. Failure (permissions) degrades to memory-only
                    // with a warning rather than failing the boot.
                    let dir = rootp.join(".oxide").join("cache").join("l1");
                    // `with_disk` consumes `tier`; on failure it owns it. Use
                    // a placeholder so both arms leave a usable value.
                    match tier.with_disk(dir) {
                        Ok(t) => tier = t,
                        Err(e) => {
                            tracing::warn!(
                                error = %e.0.message,
                                "cache.handler=disk: opening the disk tier failed; \
                                 continuing memory-only",
                            );
                            tier = oxide_cache::TieredCache::new(l0_bytes);
                        }
                    }
                }
                oxide_config::CacheHandlerConfig::Http { url } => {
                    tier = tier.with_remote(std::sync::Arc::new(
                        oxide_cache::HttpCacheHandler::new(url.clone()),
                    ));
                }
                oxide_config::CacheHandlerConfig::Redis { url } => {
                    // §11.2: a real Redis L2 tier (oxide-cache/redis). A
                    // connection failure logs and stays memory-only — the
                    // build must not fail because a cache is down.
                    match oxide_cache::RedisCacheHandler::new(url.clone()).await {
                        Ok(h) => {
                            tier = tier.with_remote(std::sync::Arc::new(h));
                            tracing::info!(
                                url = %oxide_cache::redacted_url(&url),
                                "cache.handler=redis: attached remote tier",
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                url = %oxide_cache::redacted_url(&url),
                                error = %e.0.message,
                                "cache.handler=redis: connection failed; \
                                 continuing with local tier only",
                            );
                        }
                    }
                }
                oxide_config::CacheHandlerConfig::S3 { bucket } => {
                    // §11.2: a real S3 L2 tier (oxide-cache/s3). Same
                    // failure-tolerance as Redis: a bad bucket logs and stays
                    // memory-only. Credentials/region come from AWS_* env vars.
                    match oxide_cache::S3CacheHandler::new(bucket.clone()) {
                        Ok(h) => {
                            tier = tier.with_remote(std::sync::Arc::new(h));
                            tracing::info!(
                                bucket = %bucket,
                                "cache.handler=s3: attached remote tier",
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                bucket = %bucket,
                                error = %e.0.message,
                                "cache.handler=s3: client build failed; \
                                 continuing with local tier only",
                            );
                        }
                    }
                }
            }
            std::sync::Arc::new(tier)
        }
    };
    // §13 / HMR: `dev` must always re-render — a stale L0 entry would shadow
    // every file edit (the watcher bumps the worker generation but L0 short-
    // circuits the render path entirely). `hmr: Some(_)` is the "dev mode"
    // signal in this function (production call sites pass `None`).
    let is_dev = hmr.is_some();
    let l0 = if is_dev {
        None
    } else {
        // P-05: configurable freshness window (`cache.l0.ttlSecs`, default
        // 300) instead of the old hard-coded 60 s.
        Some(std::sync::Arc::new(l0_cache::L0Cache::new(
            l0_tier,
            build_id.clone(),
            std::time::Duration::from_secs(cfg.cache.l0.ttl_secs.unwrap_or(300)),
        )))
    };
    // PERF-04: seed L0 from the build manifest so a prerendered route is an
    // L0 hit on the very first request (no warm-up miss). A missing manifest
    // (dev, or a build with no prerendered pages) is a no-op.
    if let Some(cache) = l0.as_ref() {
        let seed_path = rootp.join("dist/server/l0-seed.json");
        if let Ok(bytes) = std::fs::read(&seed_path) {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                // Only seed a manifest from THIS build.
                let same_build =
                    v.get("build_id").and_then(|b| b.as_str()) == Some(build_id.as_str());
                if same_build {
                    if let Some(entries) = v.get("entries").and_then(|e| e.as_array()) {
                        for e in entries {
                            let key = e.get("key").and_then(|k| k.as_str()).unwrap_or("");
                            let body = e.get("body").and_then(|b| b.as_str()).unwrap_or("");
                            let status =
                                e.get("status").and_then(|s| s.as_u64()).unwrap_or(200) as u16;
                            if !key.is_empty() && !body.is_empty() {
                                cache
                                    .seed_entry(key, body.as_bytes().to_vec(), status)
                                    .await;
                            }
                        }
                    }
                }
            }
        }
    }
    // PERF-07: prefer the mmap'd rkyv route archive when the build wrote one.
    // A missing/stale archive falls back to the in-memory tree (dev, or a
    // build with no archive).
    // WP-B: in dev the route table is swappable, so a stale on-disk archive
    // (written by a previous `oxide build`) must NOT shadow the live tree —
    // otherwise a route added after that build 404s until the archive is
    // rebuilt. Only the production (`serve`) path uses the mmap'd archive.
    let archived = if is_dev {
        None
    } else {
        rootp
            .join(".oxide/routes.manifest")
            .exists()
            .then(|| {
                oxide_router::ArchivedTree::load_archived(&rootp.join(".oxide/routes.manifest"))
                    .ok()
                    .map(std::sync::Arc::new)
            })
            .flatten()
    };
    // §10.8: PPR shells exist only when `render.ppr` is enabled. Keep a
    // handle for the dev watcher (cleared on every edit) and for tests.
    let ppr_shells = cfg
        .render
        .ppr
        .then(|| std::sync::Arc::new(std::sync::Mutex::new(PprShellStore::default())));
    let swappable_tree = SwappableRouteTree::new(std::sync::Arc::clone(&tree));
    let dispatch = build_dispatch_swappable(
        swappable_tree.clone(),
        std::sync::Arc::clone(&metrics),
        proxy_rules.clone(),
        stack.page_renderer,
        stack.loader_runner,
        routes_root,
        boot_script_url,
        l0,
        actions,
        archived,
        stack.pool.clone(),
        {
            let c = &cfg.i18n;
            (!c.locales.is_empty()).then(|| oxide_assets::i18n::I18nConfig {
                locales: c.locales.clone(),
                default: c.default_locale.clone(),
                strategy: c.strategy.clone(),
            })
        },
        endpoint_dispatcher,
        middleware_dispatcher,
        // §13.3: HMR client injection is dev-only. `hmr` is `Some` exactly when
        // the caller wired a dev hub (cmd_dev); serve/prod pass `None`.
        hmr.is_some(),
        // §11.2: the L0 Vary fingerprint set from `cache.l0.vary`.
        cfg.cache.l0.vary.clone(),
        // §20: the waterfall recorder (None for a non-inspector server).
        waterfall_recorder.clone(),
        // §16.6: the session codec (None when no keys are configured).
        session_codec,
        // §10.8: PPR shells exist only when `render.ppr` is enabled.
        ppr_shells.clone(),
    );
    // §5.6: dev-mode stale guard. When a HMR hub is wired, wrap the
    // dispatch so a mid-flight file change aborts the render (no serving
    // a result computed from pre-change source). Prod (no hub) is
    // untouched — zero overhead.
    let dispatch = match hmr.as_ref() {
        Some(hub) => wrap_dispatch_with_stale_guard(dispatch, std::sync::Arc::clone(hub)),
        None => dispatch,
    };
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let (tx, rx) = tokio::sync::watch::channel(false);
    // §12.1: HTTP/3 rides QUIC over UDP on the same port. Enabled by
    // `server.http3` + TLS (QUIC has no plaintext mode). A second listener
    // task drives it; the HTTP/1+2 server is untouched.
    let http3_cfg: Option<oxide_server::H3ServerConfig> = if cfg.server.http3 {
        match tls.as_ref() {
            Some(tls_cfg) => match oxide_server::h3_config_from_tls(tls_cfg.as_ref()) {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::warn!(
                        code = %e.0.code,
                        msg = %e.0.message,
                        "http3: could not build the QUIC config; continuing HTTP/1+2 only",
                    );
                    None
                }
            },
            None => {
                tracing::warn!("http3: requires TLS; continuing HTTP/1+2 only");
                None
            }
        }
    } else {
        None
    };
    let pipeline_h3 = std::sync::Arc::clone(&pipeline);
    let dispatch_h3 = std::sync::Arc::clone(&dispatch);
    let rx_h3 = rx.clone();
    let h3_bind = std::net::SocketAddr::new(bind_ip, port);
    let rx_main = rx;
    // §6.4 / Wave M: warm the renderer on a background task. Socket is
    // already bound (health → 200); ready → 503 until the synthetic render
    // completes. Single-node traffic continues unaffected while this runs.
    if let Some(renderer) = renderer_to_warm {
        let ready = std::sync::Arc::clone(&render_ready);
        tokio::spawn(async move {
            match renderer.warm().await {
                Ok(()) => tracing::info!("renderer warm; /_oxide/ready → 200"),
                Err(e) => tracing::warn!(
                    code = %e.0.code,
                    "warmup failed (readiness flips anyway): {}",
                    e.0.message
                ),
            }
            ready.store(true, std::sync::atomic::Ordering::SeqCst);
        });
    }
    tokio::spawn(async move {
        let bind_addr = std::net::SocketAddr::new(bind_ip, port);
        if let Some(h3_cfg) = http3_cfg {
            let h3_task = tokio::spawn(async move {
                if let Err(e) =
                    oxide_server::serve_http3(h3_bind, pipeline_h3, dispatch_h3, rx_h3, h3_cfg)
                        .await
                {
                    tracing::warn!(code = %e.0.code, "http3 listener stopped: {}", e.0.message);
                }
            });
            let _ = h3_task;
        }
        if let Err(e) =
            oxide_server::Server::serve_with_tls(bind_addr, pipeline, dispatch, rx_main, tls).await
        {
            tracing::error!(code = %e.0.code, "server stopped with error");
        }
    });
    Ok(StartedServer {
        addr,
        shutdown: tx,
        metrics,
        dev_token,
        hmr,
        ppr_shells,
        swappable_tree,
    })
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
    // §7 / §9.1: build the framework stack (renderer + loader) for the
    // production server. Same wiring as `cmd_dev`.
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| std::path::PathBuf::from(root));
    let mut cfg = load_config_or_default(&rootp, "production");
    let config_path_opt = find_config_file(&rootp).ok();
    let transform_handler = framework_shim::build_transform_handler(&rootp, &cfg);
    // §15.2: `cacheHandler` — a plugin may select the cache backend. Dev and
    // build already consult it; `start` must too, or the same project uses a
    // different tier in production than it built against.
    if let Ok(Some(chosen)) = resolve_cache_handler_via_plugin(&rootp, &cfg, w).await {
        cfg.cache.handler = chosen;
    }
    // §11.2: `cache.handler` is honored inside the stack (see cmd_dev).
    let stack = framework_shim::build_framework_stack(
        config_path_opt.as_deref(),
        cfg.server.backend,
        transform_handler,
        Some(&cfg.cache.handler),
    )
    .await;
    if let Some(name) = stack.renderer_name() {
        outln!(w, "framework renderer: {}", name);
    }

    let started = start_server(root, port, stack).await?;
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
    // §10.1: same AST refinement `cmd_build` uses, so the table shows the
    // authoritative mode rather than the scanner's line-oriented guess.
    // (Without it, a loader with `'…cookies…'` in a string literal reads
    // as dynamic here while the build treats it as static.)
    let tree =
        refine_render_modes_from_ast(&rootp, &cfg, scan_routes_with_plugins(&rootp, &cfg).await?);
    let i18n_for_typegen: Option<&[String]> = match cfg.i18n.strategy.as_str() {
        "none" => None,
        _ if cfg.i18n.locales.is_empty() => None,
        _ => Some(&cfg.i18n.locales),
    };
    let default_for_typegen: Option<&str> = if i18n_for_typegen.is_some() {
        Some(cfg.i18n.default_locale.as_str())
    } else {
        None
    };
    tree.typegen_with_i18n(&rootp, i18n_for_typegen, default_for_typegen)
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
    outln!(w, "PATTERN | KIND | MODE | CACHE | TAGS");
    for (pattern, kind, tags) in tree.table() {
        let mode = modes.get(&pattern).map(String::as_str).unwrap_or("-");
        // SPEC-19C / §19.1: the CACHE column names the L0 eligibility a
        // request for this route will hit. Mirrors the runtime predicate
        // at `l0_cache::mode_is_cacheable` — a route whose mode is
        // `static` or `isr` will serve from L0 on the second request; a
        // `dynamic` / `spa` / `islands` route bypasses.
        let cache = if crate::l0_cache::mode_is_cacheable(Some(mode)) {
            "L0"
        } else {
            "none"
        };
        let tags = if tags.is_empty() {
            "-".to_string()
        } else {
            tags.join(",")
        };
        outln!(w, "{pattern} | {kind} | {mode} | {cache} | {tags}");
    }
    Ok(0)
}

/// `oxide analyze` (§21 budgets): per-route JS bytes vs `budgets.routeJs` (falling
/// back to `budgets.clientJs`), from dist/client/client.manifest.json when present,
/// zeros otherwise. Exit 1 when any route is over budget.
async fn cmd_analyze(root: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let cfg = load_config_or_default(&rootp, "production");
    let tree = scan_routes_with_plugins(&rootp, &cfg).await?;
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
    // SPEC-25: enforce BOTH budgets independently — `routeJs` per route and
    // `clientJs` on the total client payload (the sum across routes). A
    // single fallback chain treated them as one number.
    // Per-route budget: `routeJs` if set, else fall back to `clientJs` (a
    // project that names only a total budget still gets every route checked
    // against it). The total check below is separate and always uses
    // `clientJs`.
    let route_src = cfg
        .budgets
        .route_js
        .as_deref()
        .or(cfg.budgets.client_js.as_deref());
    let route_budget: Option<u64> = match route_src {
        Some(s) => Some(
            oxide_config::parse_size(s)
                .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?,
        ),
        None => None,
    };
    let total_budget: Option<u64> = match cfg.budgets.client_js.as_deref() {
        Some(s) => Some(
            oxide_config::parse_size(s)
                .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?,
        ),
        None => None,
    };
    // A route's own JS plus its shared preload is what it costs; the sum over
    // routes overcounts shared chunks, so the total is the union of every
    // emitted client .js file.
    let client_dir = rootp.join("dist/client");
    let mut total_bytes: u64 = 0;
    {
        let mut stack = vec![client_dir.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().and_then(|x| x.to_str()) == Some("js") {
                    total_bytes += std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                }
            }
        }
    }
    outln!(w, "ROUTE | JS BYTES | SHARE% | BUDGET | OK");
    let mut any_over = false;
    for (route, bytes) in &rows {
        let (budget_str, limit) = match route_budget {
            Some(b) => (b.to_string(), Some(b)),
            None => ("-".to_string(), None),
        };
        let ok = limit.is_none_or(|l| *bytes <= l);
        if !ok {
            any_over = true;
        }
        let status = if ok { "ok" } else { "OVER" };
        let share = if total_bytes == 0 {
            0.0
        } else {
            (*bytes as f64) * 100.0 / (total_bytes as f64)
        };
        outln!(
            w,
            "{route} | {bytes} | {share:.1} | {budget_str} | {status}"
        );
    }
    if let Some(limit) = total_budget {
        let ok = total_bytes <= limit;
        if !ok {
            any_over = true;
        }
        outln!(w, "--");
        outln!(
            w,
            "total client JS | {total_bytes} | {limit} | {}",
            if ok { "ok" } else { "OVER" }
        );
    }
    // §5-A: report the last build's graph recompute counts. A user editing
    // one component sees how many graph nodes that touch actually dirtied —
    // the concrete measurement a future incremental build is tuned against.
    let snap = crate::build_graph::load_snapshot(&rootp);
    if snap.node_count > 0 {
        outln!(w, "--");
        outln!(
            w,
            "graph (last build `{}`): {} nodes, {} recomputed ({} in the previous build)",
            snap.build_id,
            snap.node_count,
            snap.recomputed_count,
            snap.previous_node_count,
        );
    }
    // §19-C: the top modules by bytes. The manifest records each route's
    // reachable module list (PERF-09). A module shared by many routes is
    // counted once per route, so its number is "bytes paid by the routes
    // that include it" — the useful figure when deciding what to split out.
    if !manifest_routes.is_empty() {
        // The manifest records canonicalized module paths; `rootp` here is
        // whatever `--root` was (often relative or via a symlink), so
        // canonicalize before stripping or the prefix never matches.
        let root_canon = rootp.canonicalize().unwrap_or_else(|_| rootp.clone());
        let mut module_bytes: BTreeMap<String, u64> = BTreeMap::new();
        for row in manifest_routes.values() {
            let Some(mods) = row.get("modules").and_then(|m| m.as_array()) else {
                continue;
            };
            for m in mods {
                let Some(p) = m.as_str() else { continue };
                let display = std::path::Path::new(p)
                    .strip_prefix(&root_canon)
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| p.to_string());
                let size = std::fs::metadata(p).map(|x| x.len()).unwrap_or(0);
                *module_bytes.entry(display).or_insert(0) += size;
            }
        }
        let mut sorted: Vec<(String, u64)> = module_bytes.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let top_n = std::env::var("OXIDE_ANALYZE_TOP")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(20);
        // The share is over the sum of *module source* bytes, not the
        // emitted-chunk total — those are different denominators (a module
        // is counted once per route, and chunk bytes are post-minification),
        // and mixing them produced shares over 100%.
        let module_total: u64 = sorted.iter().map(|(_, b)| *b).sum();
        let denom = module_total.max(1);
        outln!(w, "--");
        outln!(w, "top {top_n} module(s) by reachable source bytes:");
        outln!(w, "MODULE | BYTES | SHARE%");
        for (module, bytes) in sorted.iter().take(top_n) {
            let share = (*bytes as f64) * 100.0 / (denom as f64);
            outln!(w, "{module} | {bytes} | {share:.1}");
        }
    }
    Ok(if any_over { 1 } else { 0 })
}

/// `oxide check` (§19.1): config validation + route conventions diagnostics.
async fn cmd_check(root: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let mut ok = true;
    let mut diags: Vec<oxide_diagnostics::Diagnostic> = Vec::new();

    // (1) config + routes — the two checks the tool can always run.
    match oxide_config::load(&rootp, "development", BTreeMap::new()) {
        Ok(lc) => outln!(w, "config: OK ({} env vars)", lc.env.len()),
        Err(e) => {
            outln!(w, "config: {} {}", e.0.code, e.0.message);
            diags.push(e.0.clone());
            ok = false;
        }
    }
    let cfg = load_config_or_default(&rootp, "development");
    match scan_routes_with_plugins(&rootp, &cfg).await {
        Ok(tree) => outln!(w, "routes: OK ({} entries)", tree.entries.len()),
        Err(e) => {
            outln!(w, "routes: {e:#}");
            ok = false;
        }
    }
    // SPEC-15.1 / §15.2: validate every `kind:"native"` entry in
    // `config.plugins`. An unknown name fails the check (fail-at-load).
    match crate::native_plugins::parse_native_plugins(&cfg) {
        Ok(plugins) => {
            if plugins.is_empty() {
                outln!(w, "native plugins: (none declared)");
            } else {
                let names: Vec<&str> = plugins.iter().map(|p| p.name()).collect();
                outln!(w, "native plugins: {}", names.join(", "));
                // §15.2 / WP-J2: surface what each native plugin *enables*.
                // The `enables()` wrapper is the activation table; a plugin
                // that declares no gate is inert (validation-only).
                let enabled: Vec<&str> = plugins
                    .iter()
                    .flat_map(|p| p.enables().iter().copied())
                    .collect();
                if enabled.is_empty() {
                    outln!(w, "native plugin gates: (none — all are validation-only)");
                } else {
                    outln!(w, "native plugin gates: {}", enabled.join(", "));
                }
            }
        }
        Err(e) => {
            outln!(w, "native plugins: {} {}", e.0.code, e.0.message);
            diags.push(e.0.clone());
            ok = false;
        }
    }

    // (2) §19.1 / D16: typecheck via `tsgo` and lint via `oxlint`. Both are
    // external binaries, resolved on `PATH`. When one is absent the check
    // says so LOUDLY (a silent pass would hide that the project was never
    // typechecked) and does not fail the command — a project that has not
    // installed the tool has not failed a check it never ran. `--strict`
    // turns a missing tool into a failure.
    let strict = std::env::var_os("OXIDE_CHECK_STRICT").is_some();
    for (label, bin, args) in [
        ("typecheck", "tsgo", vec!["--noEmit", "--pretty", "false"]),
        ("lint", "oxlint", vec!["--format", "json"]),
    ] {
        match which(bin) {
            None => {
                outln!(
                    w,
                    "{label}: SKIPPED (`{bin}` not on PATH; install it or set OXIDE_CHECK_STRICT=1 to fail)"
                );
                if strict {
                    ok = false;
                }
            }
            Some(_) => {
                let mut cmd = std::process::Command::new(bin);
                cmd.args(&args).current_dir(&rootp);
                match cmd.output() {
                    Ok(out) => {
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        if out.status.success() {
                            outln!(w, "{label}: OK");
                        } else {
                            outln!(w, "{label}: FAILED");
                            // Emit the tool's own lines verbatim (they are
                            // already human-formatted) and capture a single
                            // diagnostic so `.oxide/last-build-diagnostics.json`
                            // records the failure.
                            for line in stdout.lines().chain(stderr.lines()).take(50) {
                                outln!(w, "  {line}");
                            }
                            diags.push(oxide_diagnostics::Diagnostic::error(
                                if label == "typecheck" {
                                    "CHECK-TS"
                                } else {
                                    "CHECK-LINT"
                                },
                                format!("`{bin}` reported errors"),
                            ));
                            ok = false;
                        }
                    }
                    Err(e) => {
                        outln!(w, "{label}: could not run `{bin}`: {e}");
                        ok = false;
                    }
                }
            }
        }
    }

    // §15.2: `diagnostics` — a plugin adds to the check report. Each result
    // is `{ diagnostics: [{ code, message, severity? }] }`; an error-severity
    // entry fails the check. A plugin that cannot be reached (no pool) is
    // skipped rather than failing `check`.
    if plugin_declares_any(&rootp, &cfg, &["diagnostics"]) {
        use oxide_jshost::JsHost as _;
        let host = oxide_jshost::NodeHost::new(oxide_jshost::NodeHost::worker_script_default());
        if let Ok(pool) = host.tooling_pool(oxide_jshost::PoolConfig::default()) {
            let results = call_js_plugin_hook(
                &rootp,
                &cfg,
                &pool,
                "diagnostics",
                serde_json::json!({ "root": rootp.to_string_lossy() }),
            )
            .await;
            for r in results {
                let Some(list) = r.get("diagnostics").and_then(|d| d.as_array()) else {
                    continue;
                };
                for d in list {
                    let code = d.get("code").and_then(|c| c.as_str()).unwrap_or("PLUGIN");
                    let message = d.get("message").and_then(|m| m.as_str()).unwrap_or("");
                    let severity = d
                        .get("severity")
                        .and_then(|s| s.as_str())
                        .unwrap_or("error");
                    outln!(w, "plugin diagnostic {code}: {message}");
                    let diag = if severity == "warning" {
                        oxide_diagnostics::Diagnostic::warning(code, message)
                    } else {
                        oxide_diagnostics::Diagnostic::error(code, message)
                    };
                    diags.push(diag);
                    if severity != "warning" {
                        ok = false;
                    }
                }
            }
        }
    }
    // §19.4: every check result lands in the shared diagnostic file so the
    // editor/CI read one artifact.
    if !diags.is_empty() {
        persist_diagnostics(&rootp, &diags);
    }

    if ok {
        outln!(w, "OK");
        Ok(0)
    } else {
        Ok(1)
    }
}

/// Resolve a binary on `PATH` (no shell; avoids a shell-injection surface).
fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
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
            // PERF-09: prefer the recorded module list (precise); fall back to
            // a textual substring scan for a manifest built before the field
            // existed.
            let recorded = row
                .get("modules")
                .and_then(|m| m.as_array())
                .map(|arr| {
                    arr.iter()
                        .any(|v| v.as_str().map(|s| s.contains(module)).unwrap_or(false))
                })
                .unwrap_or(false);
            let references = recorded
                || std::fs::read_to_string(client_dir.join(chunk))
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
    // §9.4 / BUG-05: the build secret makes server-fn ids stable across
    // builds. A missing file is fine (the next build creates it); this is
    // an informational check, not a failure.
    let secret_file = rootp.join(".oxide/secret");
    let secret_state = match std::fs::read_to_string(&secret_file) {
        Ok(s) if s.trim().len() == 64 => "present (stable server-fn ids)".to_string(),
        Ok(_) => "malformed (delete it or run `oxide secrets rotate`)".to_string(),
        Err(_) => "absent (next build creates it)".to_string(),
    };
    outln!(w, "[--] build secret (.oxide/secret): {secret_state}");
    // SEC-10: cookie keys must be ≥ 32 bytes (the AEAD uses the first 32).
    let weak: Vec<usize> = cfg
        .security
        .cookie_keys
        .iter()
        .enumerate()
        .filter(|(_, k)| k.len() < 32)
        .map(|(i, k)| i * 0 + k.len())
        .collect();
    if cfg.security.cookie_keys.is_empty() {
        outln!(w, "[--] cookie keys: none set (derived from .oxide/secret)");
    } else if weak.is_empty() {
        outln!(
            w,
            "[ok] cookie keys: {} key(s), all ≥ 32 bytes",
            cfg.security.cookie_keys.len()
        );
    } else {
        outln!(
            w,
            "[fail] cookie keys: {} key(s) shorter than 32 bytes",
            weak.len()
        );
        all_ok = false;
    }
    Ok(if all_ok { 0 } else { 1 })
}

/// `oxide secrets rotate` (§9.4 / BUG-05): delete `.oxide/secret`.
async fn cmd_secrets(root: &str, op: SecretsOp, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    match op {
        SecretsOp::Rotate => {
            let f = rootp.join(".oxide/secret");
            match std::fs::remove_file(&f) {
                Ok(()) => outln!(
                    w,
                    "[ok] removed {} — next build mints a new secret",
                    f.display()
                ),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    outln!(w, "[--] no secret at {} (nothing to rotate)", f.display())
                }
                Err(e) => anyhow::bail!("cannot remove {}: {e}", f.display()),
            }
        }
    }
    Ok(0)
}

/// `oxide add <module>` (§15.4): records the module in `.oxide/modules.json` and
/// prints next steps. (TS config writes are fragile; the list file is the contract.)
async fn cmd_add(root: &str, module: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let modules_path = rootp.join(".oxide/modules.json");

    // SPEC-15.4 / §19-B: install the module unless the caller opted out
    // (`OXIDE_ADD_NO_INSTALL=1`) — tests use the opt-out with a
    // pre-populated `node_modules/<mod>/package.json` to exercise the
    // contract-discovery half without a network round-trip.
    let no_install = std::env::var_os("OXIDE_ADD_NO_INSTALL").is_some();
    if !no_install {
        // Detect the project's package manager from its lockfile.
        // The tuple is `(&str, Vec<&str>)`. Explicit types keep the
        // compiler from inferring among `AsRef<[u8]>`, `AsRef<Path>`, etc.
        let pm: (&str, Vec<&str>) = if rootp.join("pnpm-lock.yaml").is_file() {
            ("pnpm", vec!["add", module])
        } else if rootp.join("yarn.lock").is_file() {
            ("yarn", vec!["add", module])
        } else if rootp.join("bun.lockb").is_file() {
            ("bun", vec!["add", module])
        } else {
            // npm is the fallback (also covers a project with no lockfile).
            ("npm", vec!["install", module])
        };
        outln!(w, "installing {} via {}", module, pm.0);
        let status = tokio::process::Command::new(pm.0)
            .args(&pm.1)
            .current_dir(&rootp)
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("spawn `{} {}`: {e}", pm.0, module))?;
        if !status.success() {
            anyhow::bail!(
                "`{} {}` exited {}; not recording the module",
                pm.0,
                module,
                status.code().unwrap_or(-1),
            );
        }
    } else {
        outln!(w, "OXIDE_ADD_NO_INSTALL=1 — skipping the install step");
    }

    // SPEC-15.4: read the module's `oxide.module` contract. A package with
    // no such field is recorded (audit sees it) but contributes nothing.
    let node_mods = rootp.join("node_modules").join(module);
    let pkg_json = node_mods.join("package.json");
    if !pkg_json.is_file() && !no_install {
        outln!(
            w,
            "warn: node_modules/{module}/package.json missing after install —              recording anyway, `oxide build` will fail with a clear diagnostic"
        );
    }
    let resolved = crate::modules_loader::resolve_one(&node_mods, module.to_string());

    // Persist to `.oxide/modules.json` (what `audit` reads).
    let mut list: Vec<String> = read_json(&modules_path)
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
    std::fs::create_dir_all(modules_path.parent().unwrap_or(Path::new(".")))?;
    std::fs::write(&modules_path, serde_json::to_string_pretty(&list)? + "\n")?;
    outln!(w, "[ok] added {module} to .oxide/modules.json");

    // §15-E: report what the module actually contributes. Routes and the
    // transform hook ARE merged now; everything else is enumerated as
    // resolved-but-deferred so a user is never promised a merge that does
    // not happen yet.
    match &resolved {
        None => {
            outln!(
                w,
                "  note: `{module}` has no `oxide.module` export — it is an ordinary \
                 npm package and will not contribute hooks/routes"
            );
        }
        Some(m) => {
            outln!(w, "  oxide module detected — contributes:");
            match &m.routes_dir {
                Some(p) if p.is_dir() => {
                    let pages = oxide_router::RouteTree::scan(p)
                        .map(|t| {
                            t.entries
                                .iter()
                                .filter(|e| matches!(e.kind, oxide_router::RouteKind::Page))
                                .count()
                        })
                        .unwrap_or(0);
                    outln!(w, "    routes: {pages} page(s) under /modules/{module}/");
                }
                _ => outln!(w, "    routes: (none)"),
            }
            if m.hooks.is_empty() {
                outln!(w, "    hooks:  (none)");
            } else {
                // WP-J4: derive the merged/deferred split from the actual
                // pipeline (transform, resolveId, load all route through the
                // plugin chain today; everything else is deferred).
                let merged_set = ["transform", "resolveId", "load"];
                let merged: Vec<&str> = m
                    .hooks
                    .iter()
                    .map(|s| s.as_str())
                    .filter(|h| merged_set.contains(h))
                    .collect();
                let deferred: Vec<&str> = m
                    .hooks
                    .iter()
                    .map(|s| s.as_str())
                    .filter(|h| !merged_set.contains(h))
                    .collect();
                let mut parts = Vec::new();
                if !merged.is_empty() {
                    parts.push(format!("{} (merged)", merged.join(", ")));
                }
                if !deferred.is_empty() {
                    // §15 / §15-B: every hook here IS wired — via the
                    // general plugin path (`plugins: [...]` + the per-hook
                    // build/dev dispatch), not via the module loader that
                    // only consumes transform/resolveId/load. Say that,
                    // instead of the stale "deferred" wording that read as
                    // "not implemented".
                    parts.push(format!(
                        "{} (via the general plugin path — not module-loader hooks)",
                        deferred.join(", ")
                    ));
                }
                outln!(w, "    hooks:  {}", parts.join("; "));
            }
            if !m.adapters.is_empty() {
                outln!(w, "    adapters: {}", m.adapters.join(", "));
                let cfg = load_config_or_default(&rootp, "development");
                if !m.adapters.iter().any(|a| a == &cfg.adapter) {
                    outln!(
                        w,
                        "    warn MOD-ADAPTER: module supports [{}], project adapter is `{}`",
                        m.adapters.join(", "),
                        cfg.adapter,
                    );
                }
            }
            if m.typegen.is_some() {
                outln!(
                    w,
                    "    typegen: present (not merged — deferred to a follow-up)"
                );
            }
            // J2: a native plugin's `enables()` is consulted here so
            // `oxide add` reports which features the project's plugin set
            // activates, rather than leaving the list inert.
            {
                let cfg = load_config_or_default(&rootp, "development");
                let enabled: Vec<&str> = crate::native_plugins::parse_native_plugins(&cfg)
                    .unwrap_or_default()
                    .iter()
                    .map(|p| p.name())
                    .collect();
                if !enabled.is_empty() {
                    outln!(w, "    native plugins: {}", enabled.join(", "));
                }
            }
            if m.config_defaults.is_some() {
                outln!(
                    w,
                    "    configDefaults: present (not merged — deferred to a follow-up)"
                );
            }
        }
    }
    outln!(
        w,
        "next steps: `oxide build` / `oxide dev` will pick up `{module}` from \
         .oxide/modules.json automatically. To enable it explicitly in config, \
         add it to `modules: []` in oxide.config.ts"
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
        CacheOp::Gc {
            idle_secs,
            max_bytes,
        } => {
            // SPEC-5D: two-phase GC — first drop entries older than the idle
            // window, then if still over cap, drop the OLDEST remaining
            // entries until under `max_bytes`. The pipeline never reads a GC'd
            // entry back (they are content-addressed; a re-compute re-creates
            // the same key), so eviction is safe mid-build.
            let now = std::time::SystemTime::now();
            let idle = std::time::Duration::from_secs(idle_secs);
            let mut sizes: Vec<(std::path::PathBuf, u64, std::time::SystemTime)> = files
                .iter()
                .filter_map(|f| {
                    let meta = std::fs::metadata(f).ok()?;
                    Some((f.clone(), meta.len(), meta.modified().ok()?))
                })
                .collect();
            // Phase 1: idle eviction.
            let mut idle_removed = 0usize;
            let mut idle_bytes = 0u64;
            sizes.retain(|(path, bytes, mtime)| {
                if now
                    .duration_since(*mtime)
                    .map(|d| d > idle)
                    .unwrap_or(false)
                {
                    if std::fs::remove_file(path).is_ok() {
                        idle_removed += 1;
                        idle_bytes += *bytes;
                    }
                    false
                } else {
                    true
                }
            });
            // Phase 2: cap eviction (oldest first).
            let mut total: u64 = sizes.iter().map(|(_, b, _)| *b).sum();
            let mut cap_removed = 0usize;
            let mut cap_bytes = 0u64;
            if total > max_bytes {
                sizes.sort_by_key(|(_, _, m)| *m);
                for (path, bytes, _) in &sizes {
                    if total <= max_bytes {
                        break;
                    }
                    if std::fs::remove_file(path).is_ok() {
                        total = total.saturating_sub(*bytes);
                        cap_removed += 1;
                        cap_bytes += *bytes;
                    }
                }
            }
            outln!(
                w,
                "[ok] gc: idle_removed={idle_removed} ({idle_bytes} B), cap_removed={cap_removed} ({cap_bytes} B), remaining={} ({} B)",
                sizes.len().saturating_sub(cap_removed),
                total,
            );
        }
    }
    Ok(0)
}

/// `oxide audit` (§17.2): static advisory pass over `.oxide/modules.json` + config
/// plugins. The real registry feed plugs in behind the same output shape.
async fn cmd_audit(root: &str, json: bool, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let offline = std::env::var_os("OXIDE_AUDIT_OFFLINE").is_some();

    // SPEC-19.1 / §19-B: the real advisory scan reads the lockfile's
    // installed packages and queries OSV for each. `offline` (env var) is
    // the CI-safe mode — a fresh cache hit is served, a miss is
    // AUDIT-OFFLINE.
    let pkgs = match crate::audit::read_package_lock(&rootp) {
        Ok(p) => p,
        Err(e) => {
            if json {
                let err_json = serde_json::json!({
                    "root": rootp.to_string_lossy(),
                    "error": format!("{e:#}"),
                    "exit_code": 2,
                });
                outln!(w, "{}", serde_json::to_string_pretty(&err_json)?);
            } else {
                outln!(w, "{e:#}");
            }
            return Ok(2);
        }
    };

    let advisories = match crate::audit::query_osv(&rootp, &pkgs, offline).await {
        Ok(a) => a,
        Err(e) => {
            if json {
                let err_json = serde_json::json!({
                    "root": rootp.to_string_lossy(),
                    "package_count": pkgs.len(),
                    "error": format!("{e:#}"),
                    "exit_code": 2,
                });
                outln!(w, "{}", serde_json::to_string_pretty(&err_json)?);
            } else {
                outln!(w, "{e:#}");
            }
            return Ok(2);
        }
    };

    // Exit 1 when any HIGH/CRITICAL is present (CI gate convention); a
    // low/moderate advisory is a warning-not-a-failure so a transitive
    // unfixable LOW doesn't block every build.
    let fatal = advisories
        .iter()
        .any(|a| matches!(a.severity.as_str(), "HIGH" | "CRITICAL"));
    let exit_code = if fatal { 1 } else { 0 };

    if json {
        // Stable machine-readable schema. A CI consumer can parse this
        // without depending on the human message wording (which the plan
        // has re-versioned twice since r4).
        let adv_json: Vec<serde_json::Value> = advisories
            .iter()
            .map(|a| {
                serde_json::json!({
                    "id": a.id,
                    "package": a.package,
                    "version": a.version,
                    "severity": if a.severity.is_empty() { "?" } else { a.severity.as_str() },
                    "summary": a.summary,
                })
            })
            .collect();
        let report = serde_json::json!({
            "root": rootp.to_string_lossy(),
            "package_count": pkgs.len(),
            "advisories": adv_json,
            "fatal": fatal,
            "exit_code": exit_code,
        });
        outln!(w, "{}", serde_json::to_string_pretty(&report)?);
        return Ok(exit_code);
    }

    outln!(
        w,
        "auditing {} installed npm package(s) via OSV",
        pkgs.len()
    );
    if advisories.is_empty() {
        outln!(w, "[ok] no advisories");
        return Ok(exit_code);
    }
    outln!(
        w,
        "{} advisor{} found:",
        advisories.len(),
        if advisories.len() == 1 { "y" } else { "ies" }
    );
    for a in &advisories {
        let sev = if a.severity.is_empty() {
            "?"
        } else {
            a.severity.as_str()
        };
        outln!(
            w,
            "  {} {}@{}  [{}]  {}",
            a.id,
            a.package,
            a.version,
            sev,
            a.summary
        );
    }
    Ok(exit_code)
}

/// `oxide migrate <source>` (§19.2): dry-run plan; never writes.
async fn cmd_migrate(root: &str, source: &str, write: bool, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    // §19-B: plan via the codemod engine, report every file (converted or
    // skipped), then optionally apply. A dry-run never writes; `--write`
    // refuses to clobber an existing destination.
    let plan = match crate::migrate::plan(&rootp, source) {
        Ok(p) => p,
        Err(e) => {
            outln!(w, "{e}");
            return Ok(2);
        }
    };
    let converted = plan.converted_count();
    let skipped = plan.skipped_count();
    if write {
        outln!(
            w,
            "migration plan ({} → oxide) — writing {converted} file(s), {skipped} follow-up(s):",
            plan.label,
        );
    } else {
        outln!(
            w,
            "migration plan ({} → oxide) — dry run; {converted} file(s) would be written, {skipped} follow-up(s):",
            plan.label,
        );
    }
    for c in &plan.conversions {
        match &c.to {
            Some(dst) => {
                outln!(w, "  - {} → {}", c.from.display(), dst.display());
                if let Some(note) = &c.note {
                    outln!(w, "      note: {note}");
                }
            }
            None => {
                let note = c.note.as_deref().unwrap_or("(no reason given)");
                outln!(w, "  - {}  ✗ skipped: {note}", c.from.display());
            }
        }
    }
    if !write {
        outln!(
            w,
            "  no files written — pass --write to apply (a dry run never modifies disk)"
        );
        return Ok(0);
    }
    match crate::migrate::apply(&plan) {
        Ok(()) => {
            outln!(w, "  wrote {converted} file(s)");
            Ok(0)
        }
        Err(e) => {
            outln!(w, "  write errors:");
            for line in e.lines() {
                outln!(w, "    {line}");
            }
            Ok(1)
        }
    }
}

/// `oxide test` (§16.6): gates on build manifests; the in-process `app.fetch` helper
/// itself lives in packages/oxide (documented in the output).

/// `oxide mcp` (§19.3): run the MCP server on stdio. The root path is resolved
/// relative to cwd, same as every other command. The writer is ignored — the
/// MCP protocol owns stdout; only a startup line is emitted to stderr to keep
/// the JSON-RPC stream clean.

/// `oxide schema` (§19.3): print the JSON Schema for `oxide.config.{ts,json}`.
/// Editors pick this up via a `$schema` field in the config or a
/// `json.schemas` setting.
fn cmd_schema(w: Writer<'_>) -> anyhow::Result<i32> {
    let json = oxide_config::config_schema_pretty()
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    outln!(w, "{json}");
    Ok(0)
}

/// `oxide render` (§7): load the framework adapter and dispatch a page
/// render through it. Writes the resulting HTML to stdout. This is the
/// read-path proof: config → adapter → render → HTML.
async fn cmd_render(
    root: &str,
    route: &str,
    url: Option<&str>,
    params: &[String],
    data_json: &str,
    stream: bool,
    w: Writer<'_>,
) -> anyhow::Result<i32> {
    use oxide_jshost::{FrameworkHost, JsHost, LoaderHost, NodeHost, PoolConfig};

    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    // §16.5 / WP-r9-1: thread the project root to worker
    // subprocesses so `getCollection` resolves `dist/content`
    // against the project, not the shell's cwd. Edition 2024:
    // set_var is process-global; this runs before any worker
    // thread spawns.
    unsafe {
        std::env::set_var("OXIDE_ROOT", rootp.to_string_lossy().as_ref());
    }
    let config_path = find_config_file(&rootp)?;
    // Load the config to find `routes.dir` for the loader lookup. Non-strict:
    // a config that the Rust extractor cannot fully parse (e.g. a
    // `createRealAdapter(...)` factory call with a callback arg) still
    // renders fine because the worker re-loads the file fresh. Falling back
    // to defaults here just means the loader path is `<root>/src/routes`.
    let cfg = load_config_or_default(&rootp, "production");

    // Parse params (`k=v` pairs; comma splits produce arrays).
    let mut params_map: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    for raw in params {
        let Some((k, v)) = raw.split_once('=') else {
            anyhow::bail!("--param expects KEY=VALUE, got `{raw}`");
        };
        let entry = params_map
            .entry(k.to_string())
            .or_insert_with(|| serde_json::Value::Array(Vec::new()));
        match entry {
            serde_json::Value::Array(a) => a.push(serde_json::Value::String(v.to_string())),
            _ => unreachable!("entry initialized as array"),
        }
    }
    let params_json: serde_json::Map<String, serde_json::Value> = params_map
        .into_iter()
        .map(|(k, v)| {
            let scalar = match v {
                serde_json::Value::Array(ref a) if a.len() == 1 => a[0].clone(),
                other => other,
            };
            (k, scalar)
        })
        .collect();

    let data: serde_json::Value = serde_json::from_str(data_json)
        .map_err(|e| anyhow::anyhow!("--data is not valid JSON: {e}"))?;

    let url_str = url.map(String::from).unwrap_or_else(|| {
        let stripped = route.strip_prefix("routes/").unwrap_or(route);
        format!("/{stripped}")
    });

    // One worker pool backs both the loader and the renderer.
    let host = NodeHost::new(NodeHost::worker_script_default());
    let pool = host
        .tooling_pool(PoolConfig::default())
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;

    // §9.1: run the route's `page.server.ts` loader (if present) so the
    // CLI one-shot renders the same output as dev/serve. `--data` is the
    // fallback; a loader that returns non-200 short-circuits with a
    // diagnostic.
    let mut effective_data = data.clone();
    let route_dir = route.strip_prefix("routes/").unwrap_or(route);
    let page_server_path = rootp
        .join(&cfg.routes.dir)
        .join(route_dir)
        .join("page.server.ts");
    if page_server_path.is_file() {
        let loader_host = LoaderHost::new(std::sync::Arc::clone(&pool));
        let params_for_loader: serde_json::Value = serde_json::Value::Object(params_json.clone());
        match loader_host
            .run(
                &page_server_path.to_string_lossy(),
                serde_json::json!({
                    "route": route,
                    "params": params_for_loader,
                    "url": url_str,
                    "method": "GET",
                    "headers": [],
                }),
            )
            .await
        {
            Ok(out) => {
                let status = out.get("status").and_then(|v| v.as_u64()).unwrap_or(200) as u16;
                if status != 200 {
                    let loc = out.get("location").and_then(|v| v.as_str()).unwrap_or("");
                    anyhow::bail!(
                        "loader short-circuited with status {status}{}",
                        if loc.is_empty() {
                            String::new()
                        } else {
                            format!(" -> {loc}")
                        }
                    );
                }
                if let Some(d) = out.get("data") {
                    effective_data = d.clone();
                }
            }
            Err(e) => {
                anyhow::bail!("loader failed: {} {}", e.0.code, e.0.message);
            }
        }
    }

    let mut input = serde_json::json!({
        "route": route,
        "url": url_str,
        "params": params_json,
        "data": effective_data,
        "head": { "meta": [], "link": [], "style": [], "script": [] },
    });
    // §6.4 / P0-10: `oxide render` must attach the same ComponentSource
    // the serve path builds, or a real-SSR config renders through the
    // worker's fallback path (which has no config-side component
    // resolution anymore) and 500s on every page. The entry is resolved
    // from the manifest by the same helper the renderer uses; the
    // filesystem probe is the fallback for a project that has not built
    // yet (in which case the worker's own IR path takes over and no
    // component is needed).
    if let Some(entry) = crate::framework_shim::resolve_ssr_entry(&rootp.join("dist/server")) {
        input["components"] = serde_json::json!({ "entry": entry });
    }

    let fw = FrameworkHost::new(pool);
    let path_str = config_path.to_string_lossy().to_string();

    if stream {
        // Exercise the streaming trait: pull chunks as they arrive and
        // write each one to the writer immediately. Output bytes are
        // identical to the non-streaming path; the difference is that a
        // slow upstream would start producing output before the whole
        // payload is ready.
        //
        // FrameworkHost::render_streaming returns a CallStream — an enum
        // over either one Bytes (Complete) or an mpsc Receiver<Result<Bytes>>
        // (Chunked). Iterate the two variants directly rather than wrapping
        // it as a Stream.
        let mut s = fw
            .render_streaming(&path_str, input)
            .await
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        match &mut s {
            oxide_jshost::CallStream::Complete(bytes) => {
                w.write_all(bytes)
                    .map_err(|e| anyhow::anyhow!("write stdout: {e}"))?;
                w.flush()
                    .map_err(|e| anyhow::anyhow!("flush stdout: {e}"))?;
            }
            oxide_jshost::CallStream::Chunked(rx) => {
                while let Some(item) = rx.recv().await {
                    let bytes =
                        item.map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
                    w.write_all(&bytes)
                        .map_err(|e| anyhow::anyhow!("write stdout: {e}"))?;
                    w.flush()
                        .map_err(|e| anyhow::anyhow!("flush stdout: {e}"))?;
                }
            }
        }
        // Match the non-streaming path's trailing newline. `outln!` appends
        // `\n`; the stream path writes bytes directly, so add it here.
        w.write_all(b"\n")
            .map_err(|e| anyhow::anyhow!("write stdout: {e}"))?;
        w.flush()
            .map_err(|e| anyhow::anyhow!("flush stdout: {e}"))?;
    } else {
        let html = fw
            .render(&path_str, input)
            .await
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        outln!(w, "{html}");
    }
    Ok(0)
}

/// `oxide config` (§7, §14): load oxide.config.{ts,mjs,js} through the
/// framework-adapter host and print the descriptor. This exercises the
/// exact load path the build/dev pipeline uses: a Node worker evaluates
/// the config, extracts the `framework` adapter, and returns its shape.
async fn cmd_config(root: &str, json: bool, w: Writer<'_>) -> anyhow::Result<i32> {
    use oxide_jshost::{FrameworkHost, JsHost, NodeHost, PoolConfig};

    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let config_path = find_config_file(&rootp)?;

    // One Node worker per CLI invocation; dropped when we return.
    let host = NodeHost::new(NodeHost::worker_script_default());
    let pool = host
        .tooling_pool(PoolConfig::default())
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;

    let fw = FrameworkHost::new(pool);
    let path_str = config_path.to_string_lossy().to_string();
    let desc = fw
        .load_config(&path_str)
        .await
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;

    if json {
        let v = serde_json::json!({
            "config": path_str,
            "name": desc.name,
            "extensions": desc.extensions,
            "capabilities": desc.capabilities,
        });
        outln!(w, "{}", serde_json::to_string_pretty(&v)?);
    } else {
        outln!(w, "config:       {path_str}");
        outln!(w, "framework:    {}", desc.name);
        outln!(w, "extensions:   {}", desc.extensions.join(", "));
        let caps = serde_json::to_string(&desc.capabilities).unwrap_or_default();
        outln!(w, "capabilities: {caps}");
    }
    Ok(0)
}

/// Locate the project's config file. Precedence: .ts > .mjs > .js. The .ts
/// case relies on Node 22.18+ native type stripping; on older Node the
/// worker's dynamic import fails and the error surfaces to the caller.
fn find_config_file(root: &Path) -> anyhow::Result<PathBuf> {
    for name in ["oxide.config.ts", "oxide.config.mjs", "oxide.config.js"] {
        let p = root.join(name);
        if p.is_file() {
            return Ok(p);
        }
    }
    anyhow::bail!(
        "no oxide.config.{{ts,mjs,js}} found under {} (spec §14.1)",
        root.display()
    )
}

async fn cmd_mcp(root: &str, _w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    // Progress/log go to stderr so stdout stays pure JSON-RPC.
    eprintln!("oxide mcp: serving project at {}", rootp.display());
    crate::mcp::serve_stdio(rootp).await?;
    Ok(0)
}

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

// ─── §15-E: extended plugin hooks (resolveId + load) ─────────────────────────
//
// Additive: these use the same `plugin:load` / `plugin:call` worker ops that
// `build_js_plugin_hook` already uses for `transform`, but build a
// `ResolveHook` / `LoadHook` (types already in `oxide_bundler::BundleOptions`)
// so a `kind:js` plugin — or a `modules:[]` entry — can alias or virtualize
// modules. The bundler wiring (setting `BundleOptions.resolve_hook` etc.) is
// in `cmd_build`.
//
// Each collector honors the same `confine_plugin_path` rule the transform
// path applies, so a plugin path escape is a warning+skip, not an error.

/// Collect every candidate plugin entry that could export `hook_name`.
/// Reuses `confine_plugin_path` for the kind:js plugin paths (same rule the
/// transform path applies) so a path escape is caught at collect time.
fn collect_plugin_paths_for_hook(root: &Path, cfg: &OxideConfig, hook_name: &str) -> Vec<String> {
    let mut plugins: Vec<String> = Vec::new();
    let allow_abs = plugins_allow_absolute(cfg);
    for p in &cfg.plugins {
        if p.get("kind").and_then(|v| v.as_str()) != Some("js") {
            continue;
        }
        let Some(rel) = p.get("path").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(abs) = confine_plugin_path(root, rel, allow_abs) else {
            continue;
        };
        if !abs.is_file() {
            eprintln!("warn: js plugin {} not found; skipping", abs.display());
            continue;
        }
        plugins.push(abs.to_string_lossy().to_string());
    }
    for m in crate::modules_loader::load_quiet(root, cfg) {
        if !m.hooks.iter().any(|h| h == hook_name) {
            continue;
        }
        let Some(ref entry) = m.entry else {
            eprintln!(
                "warn: module `{}` declares `{hook_name}` but has no entry file; skipping",
                m.name,
            );
            continue;
        };
        if !entry.is_file() {
            eprintln!(
                "warn: module `{}` entry {} is missing; skipping {hook_name}",
                m.name,
                entry.display(),
            );
            continue;
        }
        plugins.push(entry.to_string_lossy().to_string());
    }
    plugins
}

/// SPEC-15.2: build the `resolveId` hook. Chaining is Vite-style: the FIRST
/// plugin that returns a non-null `id` wins; a `{id: null}` passes to the
/// next plugin and finally to the bundler's default resolver.
fn build_js_plugin_resolve_hook(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_jshost::Pool>>,
) -> Option<oxide_bundler::ResolveHook> {
    let pool = pool?;
    let plugins = collect_plugin_paths_for_hook(root, cfg, "resolveId");
    if plugins.is_empty() {
        return None;
    }
    let pool = std::sync::Arc::clone(pool);
    let loaded = std::sync::Arc::new(std::sync::Mutex::new(false));
    Some(oxide_bundler::ResolveHook::new(move |specifier, from| {
        let pool = std::sync::Arc::clone(&pool);
        let loaded = std::sync::Arc::clone(&loaded);
        let plugins = plugins.clone();
        let specifier = specifier.to_string();
        let from = from.to_string();
        std::thread::scope(|scope| {
            scope
                .spawn(|| -> oxide_diagnostics::Result<Option<PathBuf>> {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| {
                            oxide_diagnostics::OxideError(oxide_diagnostics::Diagnostic::error(
                                "PLG-BRIDGE",
                                format!("bridge runtime: {e}"),
                            ))
                        })?;
                    rt.block_on(async move {
                        let need_load = {
                            let mut g = loaded.lock().unwrap();
                            if *g {
                                false
                            } else {
                                *g = true;
                                true
                            }
                        };
                        for path in &plugins {
                            if need_load {
                                let load = oxide_jshost::CallInput {
                                    payload: serde_json::json!({ "path": path }),
                                    headers: Default::default(),
                                    has_body: false,
                                    request_id: format!("plugin:load:{path}"),
                                    route: None,
                                };
                                let entry = oxide_jshost::EntryRef {
                                    kind: "plugin:load".into(),
                                    module: path.clone(),
                                    export: "default".into(),
                                };
                                let _ = pool.call(entry, load).await;
                            }
                            let call = oxide_jshost::CallInput {
                                payload: serde_json::json!({
                                    "path": path,
                                    "hook": "resolveId",
                                    "args": { "specifier": specifier, "from": from },
                                }),
                                headers: Default::default(),
                                has_body: false,
                                request_id: format!("plugin:call:{path}:resolveId"),
                                route: None,
                            };
                            let entry = oxide_jshost::EntryRef {
                                kind: "plugin:call".into(),
                                module: path.clone(),
                                export: "resolveId".into(),
                            };
                            // A plugin that does not export resolveId (or one
                            // whose call errors) passes to the next plugin.
                            let Ok(mut stream) = pool.call(entry, call).await else {
                                continue;
                            };
                            let Ok(bytes) = stream.collect().await else {
                                continue;
                            };
                            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                                continue;
                            };
                            if let Some(id_str) = hook_result_str(&v, "id") {
                                return Ok(Some(PathBuf::from(id_str)));
                            }
                        }
                        Ok(None)
                    })
                })
                .join()
                .unwrap_or_else(|_| {
                    Err(oxide_diagnostics::OxideError(
                        oxide_diagnostics::Diagnostic::error(
                            "PLG-BRIDGE",
                            "plugin bridge thread panicked",
                        ),
                    ))
                })
        })
    }))
}

/// SPEC-15.2: build the `load` hook. Chaining: the FIRST plugin that
/// returns a non-null `code` overrides the disk read; `{code: null}` passes
/// to the next plugin and finally to disk.
fn build_js_plugin_load_hook(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_jshost::Pool>>,
) -> Option<oxide_bundler::LoadHook> {
    let pool = pool?;
    let plugins = collect_plugin_paths_for_hook(root, cfg, "load");
    if plugins.is_empty() {
        return None;
    }
    let pool = std::sync::Arc::clone(pool);
    let loaded = std::sync::Arc::new(std::sync::Mutex::new(false));
    Some(oxide_bundler::LoadHook::new(move |id| {
        let pool = std::sync::Arc::clone(&pool);
        let loaded = std::sync::Arc::clone(&loaded);
        let plugins = plugins.clone();
        let id = id.to_string();
        std::thread::scope(|scope| {
            scope
                .spawn(|| -> oxide_diagnostics::Result<Option<String>> {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| {
                            oxide_diagnostics::OxideError(oxide_diagnostics::Diagnostic::error(
                                "PLG-BRIDGE",
                                format!("bridge runtime: {e}"),
                            ))
                        })?;
                    rt.block_on(async move {
                        let need_load = {
                            let mut g = loaded.lock().unwrap();
                            if *g {
                                false
                            } else {
                                *g = true;
                                true
                            }
                        };
                        for path in &plugins {
                            if need_load {
                                let load = oxide_jshost::CallInput {
                                    payload: serde_json::json!({ "path": path }),
                                    headers: Default::default(),
                                    has_body: false,
                                    request_id: format!("plugin:load:{path}"),
                                    route: None,
                                };
                                let entry = oxide_jshost::EntryRef {
                                    kind: "plugin:load".into(),
                                    module: path.clone(),
                                    export: "default".into(),
                                };
                                let _ = pool.call(entry, load).await;
                            }
                            let call = oxide_jshost::CallInput {
                                payload: serde_json::json!({
                                    "path": path,
                                    "hook": "load",
                                    "args": { "id": id },
                                }),
                                headers: Default::default(),
                                has_body: false,
                                request_id: format!("plugin:call:{path}:load"),
                                route: None,
                            };
                            let entry = oxide_jshost::EntryRef {
                                kind: "plugin:call".into(),
                                module: path.clone(),
                                export: "load".into(),
                            };
                            let Ok(mut stream) = pool.call(entry, call).await else {
                                continue;
                            };
                            let Ok(bytes) = stream.collect().await else {
                                continue;
                            };
                            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                                continue;
                            };
                            if let Some(code) = hook_result_str(&v, "code") {
                                return Ok(Some(code));
                            }
                        }
                        Ok(None)
                    })
                })
                .join()
                .unwrap_or_else(|_| {
                    Err(oxide_diagnostics::OxideError(
                        oxide_diagnostics::Diagnostic::error(
                            "PLG-BRIDGE",
                            "plugin bridge thread panicked",
                        ),
                    ))
                })
        })
    }))
}

// ─── §5-A full incremental: per-route prerender reuse ──────────────────────
//
// A build re-runs the full pipeline; the win comes from **skipping the
// expensive per-route render** when the route's source files (and the
// config) are unchanged since the last build. The reuse granularity is
// per-URL, so a `generateParams` route that produced N pages keeps its
// rendered output while a page whose source changed gets a fresh render.
//
// The hash deliberately includes the serialized config: any config change
// (framework adapter, i18n locale list, `build.sourcemap`, etc.) flushes
// every route. That is coarser than a field-by-field dependency, but it is
// conservative in the safe direction — the worst case is a full rerender,
// never a stale render.

/// Hash every source that fed this route's render: the serialized config,
/// the page file, its `page.server.ts`, every layout in the chain (both
/// `layout.*` and `layout.server.*`), and the error boundary when present.
fn route_source_hash(
    rootp: &Path,
    cfg: &OxideConfig,
    tree: &oxide_router::RouteTree,
    entry: &oxide_router::RouteEntry,
    layout_ids: &[String],
    layout_modules: &[(String, std::path::PathBuf)],
    error_route_id: Option<&str>,
) -> String {
    const HASH_VERSION: &str = "route-source-hash-v1";
    let mut h = blake3::Hasher::new();
    h.update(HASH_VERSION.as_bytes());
    h.update(&[0]);
    // The config is hashed as a whole — a config change is a flush.
    if let Ok(cfg_bytes) = serde_json::to_vec(cfg) {
        h.update(&cfg_bytes);
    }
    h.update(&[0]);
    let routes_root = rootp.join(&cfg.routes.dir);
    // Page source + page server module.
    let page_path = routes_root.join(&entry.file);
    hash_file_into(&mut h, &page_path);
    let page_server_path = routes_root.join(entry.file.with_file_name("page.server.ts"));
    hash_file_into(&mut h, &page_server_path);
    // Layout chain (both the layout file and its server module).
    for lid in layout_ids {
        if let Some(le) = tree.entries.iter().find(|x| &x.id.0 == lid) {
            let lfile = routes_root.join(&le.file);
            hash_file_into(&mut h, &lfile);
            let lserver = routes_root.join(le.file.with_file_name("layout.server.ts"));
            hash_file_into(&mut h, &lserver);
        }
    }
    // `layout_modules` also has the on-disk `layout.server.ts` paths — include
    // them so a rename of a layout module (which the id lookup may miss) still
    // invalidates.
    for (_, p) in layout_modules {
        hash_file_into(&mut h, p);
    }
    // Error boundary (if any).
    if let Some(eid) = error_route_id {
        if let Some(ee) = tree.entries.iter().find(|x| x.id.0 == eid) {
            let efile = routes_root.join(&ee.file);
            hash_file_into(&mut h, &efile);
        }
    }
    h.finalize().to_hex().to_string()
}

/// Hash `path` and the file's content into `h`. A missing file is a
/// well-defined sentinel rather than an error, so the source hash stays
/// stable when a file legitimately doesn't exist (e.g., a page with no
/// `page.server.ts`).
fn hash_file_into(h: &mut blake3::Hasher, path: &Path) {
    h.update(path.to_string_lossy().as_bytes());
    h.update(&[0]);
    match std::fs::read(path) {
        Ok(bytes) => {
            h.update(blake3::hash(&bytes).as_bytes());
        }
        Err(_) => {
            h.update(b"<missing>");
        }
    }
    h.update(&[0]);
}

/// If a prior build's record for `(route_id, url)` has the same source
/// hash and its saved output file exists, return `(html, status, prev_nonce)`.
/// The caller is responsible for replacing `prev_nonce` with the current
/// build's nonce (and rewriting the disk file to match, so a static-file
/// serve sees a matching CSP).
fn try_reuse_prerender(
    disk_path: &Path,
    route_id: &str,
    url: &str,
    src_hash: &str,
    prev_map: &std::collections::BTreeMap<
        (String, String),
        &crate::build_graph::RoutePrerenderRecord,
    >,
) -> Option<(String, u16, String)> {
    let prev = prev_map.get(&(route_id.to_string(), url.to_string()))?;
    if prev.source_hash != src_hash {
        return None;
    }
    if !disk_path.is_file() {
        return None;
    }
    let html = std::fs::read_to_string(disk_path).ok()?;
    Some((html, prev.status, prev.nonce.clone()))
}

// ─── §5-B: plugin-options inventory ────────────────────────────────────────
//
// The graph's `Input::PluginOptions` variant lets a build node record "I was
// transformed by plugin X with these options." `file_compute_with_plugins`
// records that edge for every module source whose path matches a plugin
// entry. This helper is what produces the `(name, options_hash, path)`
// triples the build graph consumes.
//
// The hash covers the plugin's *whole* config entry (kind, path, plus every
// field an author put in the object). A change to any option the plugin was
// configured with changes the hash, so every module that plugin transforms
// invalidates on the next build — which is exactly the §5-B contract.

fn collect_plugin_options(root: &Path, cfg: &OxideConfig) -> Vec<(String, blake3::Hash, PathBuf)> {
    let mut out = Vec::new();
    for (idx, p) in cfg.plugins.iter().enumerate() {
        if p.get("kind").and_then(|v| v.as_str()) != Some("js") {
            continue;
        }
        let Some(rel) = p.get("path").and_then(|v| v.as_str()) else {
            continue;
        };
        let abs = if Path::new(rel).is_absolute() {
            PathBuf::from(rel)
        } else {
            root.join(rel)
        };
        // The name is what the plugin object calls itself, or an
        // index-based fallback. A name carries no semantics beyond making
        // the diagnostic readable.
        let name = p
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("plugin#{idx}"));
        // Hash the whole config entry — every field the author put in the
        // object is an option the plugin received.
        let canonical = serde_json::to_vec(p).unwrap_or_default();
        out.push((name, blake3::hash(&canonical), abs));
    }
    out
}

// ─── §15-B: renderChunk / generateBundle / transformHtml hook builders ─────
//
// Same worker-op contract the existing transform/resolveId/load builders use:
// `plugin:load` once per candidate, then `plugin:call {path, hook, args}`.
// Each builder is deliberately simple — the chaining rules differ per hook
// (renderChunk: chain the source; generateBundle: last non-null wins;
// transformHtml: chain the HTML), so each is its own function.

/// §15-B: build the `renderChunk` hook. Chaining: each plugin's
/// `renderChunk(file, source)` receives the previous plugin's output; the
/// final string is what the bundler writes.
fn build_js_plugin_render_chunk_hook(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_jshost::Pool>>,
) -> Option<oxide_bundler::RenderChunkHook> {
    let pool = pool?;
    let plugins = collect_plugin_paths_for_hook(root, cfg, "renderChunk");
    if plugins.is_empty() {
        return None;
    }
    let pool = std::sync::Arc::clone(pool);
    let loaded = std::sync::Arc::new(std::sync::Mutex::new(false));
    Some(oxide_bundler::RenderChunkHook::new(move |file, source| {
        let pool = std::sync::Arc::clone(&pool);
        let loaded = std::sync::Arc::clone(&loaded);
        let plugins = plugins.clone();
        let file = file.to_string();
        let source = source.to_string();
        std::thread::scope(|scope| {
            scope
                .spawn(|| -> oxide_diagnostics::Result<Option<String>> {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| {
                            oxide_diagnostics::OxideError(oxide_diagnostics::Diagnostic::error(
                                "PLG-BRIDGE",
                                format!("bridge runtime: {e}"),
                            ))
                        })?;
                    rt.block_on(async move {
                        let need_load = {
                            let mut g = loaded.lock().unwrap();
                            if *g {
                                false
                            } else {
                                *g = true;
                                true
                            }
                        };
                        let mut code = source;
                        let mut changed = false;
                        for path in &plugins {
                            if need_load {
                                let req = oxide_jshost::CallInput {
                                    payload: serde_json::json!({ "path": path }),
                                    headers: Default::default(),
                                    has_body: false,
                                    request_id: format!("plugin:load:{path}"),
                                    route: None,
                                };
                                let entry = oxide_jshost::EntryRef {
                                    kind: "plugin:load".into(),
                                    module: path.clone(),
                                    export: "default".into(),
                                };
                                let _ = pool.call(entry, req).await;
                            }
                            let req = oxide_jshost::CallInput {
                                payload: serde_json::json!({
                                    "path": path,
                                    "hook": "renderChunk",
                                    "args": { "file": file, "code": code },
                                }),
                                headers: Default::default(),
                                has_body: false,
                                request_id: format!("plugin:call:{path}:renderChunk"),
                                route: None,
                            };
                            let entry = oxide_jshost::EntryRef {
                                kind: "plugin:call".into(),
                                module: path.clone(),
                                export: "renderChunk".into(),
                            };
                            let Ok(mut stream) = pool.call(entry, req).await else {
                                continue;
                            };
                            let Ok(bytes) = stream.collect().await else {
                                continue;
                            };
                            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                                continue;
                            };
                            if let Some(s) = hook_result_str(&v, "code") {
                                code = s;
                                changed = true;
                            }
                        }
                        Ok(if changed { Some(code) } else { None })
                    })
                })
                .join()
                .unwrap_or_else(|_| {
                    Err(oxide_diagnostics::OxideError(
                        oxide_diagnostics::Diagnostic::error(
                            "PLG-BRIDGE",
                            "renderChunk bridge thread panicked",
                        ),
                    ))
                })
        })
    }))
}

/// §15-B: build the `generateBundle` hook. Chaining: the LAST plugin that
/// returns a non-null file list wins (Vite convention).
fn build_js_plugin_generate_bundle_hook(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_jshost::Pool>>,
) -> Option<oxide_bundler::GenerateBundleHook> {
    let pool = pool?;
    let plugins = collect_plugin_paths_for_hook(root, cfg, "generateBundle");
    if plugins.is_empty() {
        return None;
    }
    let pool = std::sync::Arc::clone(pool);
    let loaded = std::sync::Arc::new(std::sync::Mutex::new(false));
    Some(oxide_bundler::GenerateBundleHook::new(move |files| {
        let pool = std::sync::Arc::clone(&pool);
        let loaded = std::sync::Arc::clone(&loaded);
        let plugins = plugins.clone();
        let files: Vec<String> = files.to_vec();
        std::thread::scope(|scope| {
            scope
                .spawn(|| -> oxide_diagnostics::Result<Option<Vec<String>>> {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| {
                            oxide_diagnostics::OxideError(oxide_diagnostics::Diagnostic::error(
                                "PLG-BRIDGE",
                                format!("bridge runtime: {e}"),
                            ))
                        })?;
                    rt.block_on(async move {
                        let need_load = {
                            let mut g = loaded.lock().unwrap();
                            if *g {
                                false
                            } else {
                                *g = true;
                                true
                            }
                        };
                        let mut current: Option<Vec<String>> = None;
                        for path in &plugins {
                            if need_load {
                                let req = oxide_jshost::CallInput {
                                    payload: serde_json::json!({ "path": path }),
                                    headers: Default::default(),
                                    has_body: false,
                                    request_id: format!("plugin:load:{path}"),
                                    route: None,
                                };
                                let entry = oxide_jshost::EntryRef {
                                    kind: "plugin:load".into(),
                                    module: path.clone(),
                                    export: "default".into(),
                                };
                                let _ = pool.call(entry, req).await;
                            }
                            let list = current.as_deref().unwrap_or(&files);
                            let req = oxide_jshost::CallInput {
                                payload: serde_json::json!({
                                    "path": path,
                                    "hook": "generateBundle",
                                    "args": { "files": list },
                                }),
                                headers: Default::default(),
                                has_body: false,
                                request_id: format!("plugin:call:{path}:generateBundle"),
                                route: None,
                            };
                            let entry = oxide_jshost::EntryRef {
                                kind: "plugin:call".into(),
                                module: path.clone(),
                                export: "generateBundle".into(),
                            };
                            let Ok(mut stream) = pool.call(entry, req).await else {
                                continue;
                            };
                            let Ok(bytes) = stream.collect().await else {
                                continue;
                            };
                            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                                continue;
                            };
                            if let Some(files) = hook_result_str_array(&v, "files") {
                                current = Some(files);
                            }
                        }
                        Ok(current)
                    })
                })
                .join()
                .unwrap_or_else(|_| {
                    Err(oxide_diagnostics::OxideError(
                        oxide_diagnostics::Diagnostic::error(
                            "PLG-BRIDGE",
                            "generateBundle bridge thread panicked",
                        ),
                    ))
                })
        })
    }))
}

/// §15-B: build the `transformHtml` hook. Chaining: each plugin's
/// `transformHtml(route_id, html)` receives the previous plugin's output.
fn build_js_plugin_transform_html_hook(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_jshost::Pool>>,
) -> Option<oxide_bundler::TransformHtmlHook> {
    let pool = pool?;
    let plugins = collect_plugin_paths_for_hook(root, cfg, "transformHtml");
    if plugins.is_empty() {
        return None;
    }
    let pool = std::sync::Arc::clone(pool);
    let loaded = std::sync::Arc::new(std::sync::Mutex::new(false));
    Some(oxide_bundler::TransformHtmlHook::new(
        move |route_id, html| {
            let pool = std::sync::Arc::clone(&pool);
            let loaded = std::sync::Arc::clone(&loaded);
            let plugins = plugins.clone();
            let route_id = route_id.to_string();
            let html = html.to_string();
            std::thread::scope(|scope| {
                scope
                    .spawn(|| -> oxide_diagnostics::Result<Option<String>> {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|e| {
                                oxide_diagnostics::OxideError(oxide_diagnostics::Diagnostic::error(
                                    "PLG-BRIDGE",
                                    format!("bridge runtime: {e}"),
                                ))
                            })?;
                        rt.block_on(async move {
                            let need_load = {
                                let mut g = loaded.lock().unwrap();
                                if *g {
                                    false
                                } else {
                                    *g = true;
                                    true
                                }
                            };
                            let mut doc = html;
                            let mut changed = false;
                            for path in &plugins {
                                if need_load {
                                    let req = oxide_jshost::CallInput {
                                        payload: serde_json::json!({ "path": path }),
                                        headers: Default::default(),
                                        has_body: false,
                                        request_id: format!("plugin:load:{path}"),
                                        route: None,
                                    };
                                    let entry = oxide_jshost::EntryRef {
                                        kind: "plugin:load".into(),
                                        module: path.clone(),
                                        export: "default".into(),
                                    };
                                    let _ = pool.call(entry, req).await;
                                }
                                let req = oxide_jshost::CallInput {
                                    payload: serde_json::json!({
                                        "path": path,
                                        "hook": "transformHtml",
                                        "args": { "route": route_id, "html": doc },
                                    }),
                                    headers: Default::default(),
                                    has_body: false,
                                    request_id: format!("plugin:call:{path}:transformHtml"),
                                    route: None,
                                };
                                let entry = oxide_jshost::EntryRef {
                                    kind: "plugin:call".into(),
                                    module: path.clone(),
                                    export: "transformHtml".into(),
                                };
                                let Ok(mut stream) = pool.call(entry, req).await else {
                                    continue;
                                };
                                let Ok(bytes) = stream.collect().await else {
                                    continue;
                                };
                                let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes)
                                else {
                                    continue;
                                };
                                if let Some(s) = hook_result_str(&v, "html") {
                                    doc = s;
                                    changed = true;
                                }
                            }
                            Ok(if changed { Some(doc) } else { None })
                        })
                    })
                    .join()
                    .unwrap_or_else(|_| {
                        Err(oxide_diagnostics::OxideError(
                            oxide_diagnostics::Diagnostic::error(
                                "PLG-BRIDGE",
                                "transformHtml bridge thread panicked",
                            ),
                        ))
                    })
            })
        },
    ))
}

// ─── §15-B: hook-result shape helpers ──────────────────────────────────────
//
// A plugin author writes the shape every bundler documents: `transform`
// returns the rewritten code string, `resolveId` returns an id string (or
// null), `load` returns a source string (or null), `transformHtml` returns
// HTML, `generateBundle` returns a file array. The Rust readers used to
// require an object (`{code: …}`) — which no plugin author writes — so every
// hook's return value was silently dropped. These helpers accept both: the
// documented bare value, and an object with the named field (for a plugin
// that wants to carry extra metadata alongside).

/// Extract `field` from a hook result. Accepts a bare string or
/// `{ "<field>": "…" }`.
fn hook_result_str(v: &serde_json::Value, field: &str) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    v.get(field).and_then(|x| x.as_str()).map(str::to_string)
}

/// Extract `field` from a hook result as a string array. Accepts a bare
/// array of strings or `{ "<field>": [...] }`.
fn hook_result_str_array(v: &serde_json::Value, field: &str) -> Option<Vec<String>> {
    let arr = match v {
        serde_json::Value::Array(a) => a,
        _ => v.get(field)?.as_array()?,
    };
    Some(
        arr.iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
    )
}

// ─── §15: generic plugin hook dispatch ─────────────────────────────────────
//
// The six "structural" hooks (transform, resolveId, load, renderChunk,
// generateBundle, transformHtml) each have a bespoke builder because their
// chaining rules differ. The rest are notifications or single-shot
// contributions: load each candidate, call the hook, collect the results.
// `call_js_plugin_hook` is that shape.

/// Call `hook_name` across every plugin/module that declares it. Returns the
/// per-plugin results in declaration order. An empty vec means no plugin
/// declared it — the caller skips. A plugin whose call fails is skipped, not
/// fatal: a lifecycle notification must not fail a build.
///
/// Loads each candidate first (`plugin:load` is idempotent on the worker —
/// it replaces the cached module). A lifecycle hook is called once per build
/// per plugin, so the load cost is negligible.
async fn call_js_plugin_hook(
    root: &Path,
    cfg: &OxideConfig,
    pool: &std::sync::Arc<dyn oxide_jshost::Pool>,
    hook_name: &str,
    args: serde_json::Value,
) -> Vec<serde_json::Value> {
    let plugins = collect_plugin_paths_for_hook(root, cfg, hook_name);
    if plugins.is_empty() {
        return Vec::new();
    }
    let mut results = Vec::new();
    for path in &plugins {
        let load = oxide_jshost::CallInput {
            payload: serde_json::json!({ "path": path }),
            headers: Default::default(),
            has_body: false,
            request_id: format!("plugin:load:{path}"),
            route: None,
        };
        let load_entry = oxide_jshost::EntryRef {
            kind: "plugin:load".into(),
            module: path.clone(),
            export: "default".into(),
        };
        let _ = pool.call(load_entry, load).await;

        let req = oxide_jshost::CallInput {
            payload: serde_json::json!({
                "path": path,
                "hook": hook_name,
                "args": args,
            }),
            headers: Default::default(),
            has_body: false,
            request_id: format!("plugin:call:{path}:{hook_name}"),
            route: None,
        };
        let entry = oxide_jshost::EntryRef {
            kind: "plugin:call".into(),
            module: path.clone(),
            export: hook_name.to_string(),
        };
        let Ok(mut stream) = pool.call(entry, req).await else {
            continue;
        };
        let Ok(bytes) = stream.collect().await else {
            continue;
        };
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
            results.push(v);
        }
    }
    results
}

/// Fire a lifecycle hook from an async `cmd_*`. Async (not `block_on`):
/// every caller is already inside a runtime, and blocking on a future from
/// within a runtime panics.
async fn fire_lifecycle_hook(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_jshost::Pool>>,
    hook_name: &str,
    args: serde_json::Value,
) -> Vec<serde_json::Value> {
    let Some(pool) = pool else {
        return Vec::new();
    };
    call_js_plugin_hook(root, cfg, pool, hook_name, args).await
}

/// True when any configured `kind:js` plugin (or module entry) exports
/// `hook_name`. Used to skip building a tooling pool for a hook nothing
/// declares — the pool costs one Node child.
fn plugin_declares_any(root: &Path, cfg: &OxideConfig, hook_names: &[&str]) -> bool {
    hook_names
        .iter()
        .any(|h| !collect_plugin_paths_for_hook(root, cfg, h).is_empty())
}

/// Merge a `config` hook's returned patch into the live config.
///
/// The patch is a partial JSON object whose keys match the config's own.
/// Only the fields a plugin would realistically set are honored — a
/// wholesale re-serialize would lose every field this CLI does not model.
/// An unknown key is reported and skipped (a plugin targeting a newer config
/// field should not fail a build).
fn apply_config_patch(
    cfg: &mut OxideConfig,
    patch: &serde_json::Value,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    let Some(obj) = patch.as_object() else {
        return Ok(());
    };
    for (key, value) in obj {
        match key.as_str() {
            "adapter" => {
                if let Some(s) = value.as_str() {
                    cfg.adapter = s.to_string();
                }
            }
            "siteUrl" | "site_url" => {
                if let Some(s) = value.as_str() {
                    cfg.site_url = Some(s.to_string());
                }
            }
            "routes" => {
                if let Some(dir) = value.get("dir").and_then(|d| d.as_str()) {
                    cfg.routes.dir = dir.to_string();
                }
            }
            "build" => {
                if let Some(t) = value.get("target").and_then(|v| v.as_str()) {
                    cfg.build.target = t.to_string();
                }
            }
            other => {
                outln!(
                    w,
                    "warn: plugin config patch key `{other}` is not modeled by this CLI; ignored"
                );
            }
        }
    }
    Ok(())
}

/// §15.2: `renderHead` — a plugin contributes to the neutral head model.
/// Each result is `{ meta?: [...], link?: [...], style?: [...], script?: [...] }`,
/// merged into the model the adapter receives. Called once per prerendered
/// route (the hook sees the route id), not per request.
async fn plugin_render_head(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_jshost::Pool>>,
    route_id: &str,
) -> serde_json::Value {
    let mut merged = serde_json::json!({
        "meta": [], "link": [], "style": [], "script": []
    });
    let Some(pool) = pool else { return merged };
    if collect_plugin_paths_for_hook(root, cfg, "renderHead").is_empty() {
        return merged;
    }
    let results = call_js_plugin_hook(
        root,
        cfg,
        pool,
        "renderHead",
        serde_json::json!({ "route": route_id }),
    )
    .await;
    let Some(obj) = merged.as_object_mut() else {
        return merged;
    };
    for r in results {
        for field in ["meta", "link", "style", "script"] {
            if let Some(arr) = r.get(field).and_then(|v| v.as_array()) {
                if let Some(slot) = obj.get_mut(field).and_then(|v| v.as_array_mut()) {
                    slot.extend(arr.iter().cloned());
                }
            }
        }
    }
    merged
}

/// §15.2: `cacheHandler` — a plugin *selects* the cache backend.
///
/// A plugin cannot implement `oxide_cache::CacheHandler` itself: the trait is
/// async Rust over a Rust `Entry` (body bytes, policy, etag), and every
/// operation would cross an RPC boundary. The hook's real value is choosing
/// among the drivers the CLI already builds — a deploy plugin knows whether
/// its platform wants redis, s3, an HTTP cache, or memory.
///
/// Each plugin returns `{ kind: "memory" | "disk" | "redis" | "s3" | "http",
/// url?, bucket? }`. The last plugin to return a valid kind wins. An unknown
/// kind, or a missing url/bucket, is reported and the config's choice stands.
async fn resolve_cache_handler_via_plugin(
    root: &Path,
    cfg: &OxideConfig,
    w: Writer<'_>,
) -> anyhow::Result<Option<oxide_config::CacheHandlerConfig>> {
    if collect_plugin_paths_for_hook(root, cfg, "cacheHandler").is_empty() {
        return Ok(None);
    }
    // The hook runs on the tooling pool — which is built without a cache
    // handler, since choosing it is the whole point.
    use oxide_jshost::JsHost as _;
    let host = oxide_jshost::NodeHost::new(oxide_jshost::NodeHost::worker_script_default());
    let Some(pool) = host.tooling_pool(oxide_jshost::PoolConfig::default()).ok() else {
        return Ok(None);
    };
    let results = call_js_plugin_hook(
        root,
        cfg,
        &pool,
        "cacheHandler",
        serde_json::json!({ "current": format!("{:?}", cfg.cache.handler) }),
    )
    .await;
    let mut chosen: Option<oxide_config::CacheHandlerConfig> = None;
    for r in results {
        let Some(kind) = r.get("kind").and_then(|k| k.as_str()) else {
            continue;
        };
        let candidate = match kind {
            "memory" => Some(oxide_config::CacheHandlerConfig::Memory),
            "disk" => Some(oxide_config::CacheHandlerConfig::Disk),
            "redis" => r.get("url").and_then(|u| u.as_str()).map(|url| {
                oxide_config::CacheHandlerConfig::Redis {
                    url: url.to_string(),
                }
            }),
            "s3" => r.get("bucket").and_then(|b| b.as_str()).map(|bucket| {
                oxide_config::CacheHandlerConfig::S3 {
                    bucket: bucket.to_string(),
                }
            }),
            "http" => r.get("url").and_then(|u| u.as_str()).map(|url| {
                oxide_config::CacheHandlerConfig::Http {
                    url: url.to_string(),
                }
            }),
            other => {
                outln!(
                    w,
                    "warn: cacheHandler plugin returned unknown kind `{other}`; ignored"
                );
                None
            }
        };
        if let Some(c) = candidate {
            chosen = Some(c);
        } else if matches!(kind, "redis" | "s3" | "http") {
            outln!(
                w,
                "warn: cacheHandler kind `{kind}` is missing its url/bucket; ignored"
            );
        }
    }
    Ok(chosen)
}

/// Render a plugin's neutral head model (`renderHead`'s return value) into
/// `<head>` markup, for the placeholder path where no framework renderer
/// runs. The full model a real adapter merges is richer (title templates,
/// JSON-LD); this covers the four fields a plugin contributes — meta, link,
/// style, script — which is what the placeholder document can carry.
fn neutral_head_to_html(head: &serde_json::Value) -> String {
    let mut out = String::new();
    let Some(obj) = head.as_object() else {
        return out;
    };
    // `title` is a scalar, not an array — a route's `meta` sets it and it
    // belongs first in the head.
    if let Some(t) = obj.get("title").and_then(|v| v.as_str()) {
        out.push_str(&format!("<title>{}</title>", html_escape(t)));
    }
    for field in ["meta", "link", "style", "script"] {
        let Some(arr) = obj.get(field).and_then(|v| v.as_array()) else {
            continue;
        };
        for entry in arr {
            let Some(attrs) = entry.as_object() else {
                // A bare string: treat as the element's text content.
                if let Some(text) = entry.as_str() {
                    let tag = if field == "style" { "style" } else { "script" };
                    out.push_str(&format!("<{tag}>{}</{tag}>", html_escape(text)));
                }
                continue;
            };
            let tag = match field {
                "meta" => "meta",
                "link" => "link",
                "style" => "style",
                "script" => "script",
                _ => continue,
            };
            // A `<style>` / `<script>` entry may carry `content` as body
            // text. For any other tag (`meta`, `link`) `content` is a normal
            // attribute — `<meta name="description" content="...">` is the
            // common case, and skipping it there emitted an attribute-less
            // tag.
            let body = if matches!(tag, "style" | "script") {
                attrs.get("content").and_then(|c| c.as_str())
            } else {
                None
            };
            let mut tag_out = format!("<{tag}");
            let mut keys: Vec<&String> = attrs.keys().collect();
            keys.sort();
            for k in keys {
                if k == "content" && body.is_some() {
                    continue;
                }
                if let Some(v) = attrs.get(k).and_then(|v| v.as_str()) {
                    tag_out.push_str(&format!(" {}=\"{}\"", k, html_escape(v)));
                }
            }
            match body {
                Some(b) if matches!(tag, "style" | "script") => {
                    tag_out.push_str(&format!(">{}</{tag}>", html_escape(b)));
                }
                _ => tag_out.push('>'),
            }
            out.push_str(&tag_out);
        }
    }
    // §16.3: JSON-LD is a structured value, not a `script.content` string.
    // Emit it as `<script type="application/ld+json">` so a `defineMeta`
    // that returns structured data reaches the head.
    if let Some(arr) = obj.get("jsonLd").and_then(|v| v.as_array()) {
        for entry in arr {
            let body = entry
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| entry.to_string());
            out.push_str(&format!(
                "<script type=\"application/ld+json\">{}</script>",
                html_escape(&body)
            ));
        }
    }
    out
}

/// Minimal HTML-attribute/text escape for the placeholder path.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Normalize a route's flat `meta` export into the neutral head model.
///
/// `defineMeta` historically returns a flat object (`{ title, description,
/// openGraph, canonical, jsonLd }`). The neutral model is structured
/// (`{ title, meta: [...], link: [...], jsonLd: [...] }`). Projects that
/// already return the structured shape pass through unchanged; a flat object
/// is lifted field-by-field so richer fields are not silently dropped.
fn normalize_route_meta(meta: serde_json::Value) -> serde_json::Value {
    let Some(obj) = meta.as_object() else {
        return meta;
    };
    const NEUTRAL: [&str; 5] = ["meta", "link", "style", "script", "jsonLd"];
    if NEUTRAL.iter().any(|k| obj.contains_key(*k)) {
        return meta;
    }
    let mut out = serde_json::Map::new();
    let mut metas: Vec<serde_json::Value> = Vec::new();
    let mut links: Vec<serde_json::Value> = Vec::new();
    let mut json_ld: Vec<serde_json::Value> = Vec::new();
    let stringify = |v: &serde_json::Value| {
        v.as_str()
            .map(str::to_string)
            .unwrap_or_else(|| v.to_string())
    };
    for (k, v) in obj {
        match k.as_str() {
            "title" | "titleTemplate" | "base" => {
                out.insert(k.clone(), v.clone());
            }
            "canonical" | "canonicalUrl" => {
                links.push(serde_json::json!({"rel": "canonical", "href": stringify(v)}));
            }
            "openGraph" | "og" => {
                if let Some(o) = v.as_object() {
                    for (ok, ov) in o {
                        metas.push(serde_json::json!({
                            "property": format!("og:{ok}"),
                            "content": stringify(ov),
                        }));
                    }
                }
            }
            "twitter" => {
                if let Some(o) = v.as_object() {
                    for (tk, tv) in o {
                        metas.push(serde_json::json!({
                            "name": format!("twitter:{tk}"),
                            "content": stringify(tv),
                        }));
                    }
                }
            }
            "jsonLd" | "json_ld" => match v {
                serde_json::Value::Array(a) => json_ld.extend(a.iter().cloned()),
                _ => json_ld.push(v.clone()),
            },
            _ => {
                metas.push(serde_json::json!({"name": k, "content": stringify(v)}));
            }
        }
    }
    out.insert("meta".into(), serde_json::Value::Array(metas));
    if !links.is_empty() {
        out.insert("link".into(), serde_json::Value::Array(links));
    }
    if !json_ld.is_empty() {
        out.insert("jsonLd".into(), serde_json::Value::Array(json_ld));
    }
    serde_json::Value::Object(out)
}

/// Merge one neutral head model into another.
///
/// The shape is `{meta: [...], link: [...], style: [...], script: [...],
/// title?: "..."}`. Arrays concatenate with `inner` last, so a route's entry
/// lands after a plugin's and a consumer reading the last match sees the page
/// author's value. `title` is inner-wins.
fn merge_neutral_head(outer: serde_json::Value, inner: serde_json::Value) -> serde_json::Value {
    let mut out = match outer {
        serde_json::Value::Object(o) => o,
        _ => serde_json::Map::new(),
    };
    let Some(inner_obj) = inner.as_object() else {
        return serde_json::Value::Object(out);
    };
    for field in ["meta", "link", "style", "script", "jsonLd"] {
        match (out.get_mut(field), inner_obj.get(field)) {
            (Some(serde_json::Value::Array(a)), Some(serde_json::Value::Array(b))) => {
                a.extend(b.iter().cloned());
            }
            (None, Some(v)) => {
                out.insert(field.to_string(), v.clone());
            }
            _ => {}
        }
    }
    for scalar in ["title", "titleTemplate", "base"] {
        if let Some(t) = inner_obj.get(scalar) {
            out.insert(scalar.to_string(), t.clone());
        }
    }
    serde_json::Value::Object(out)
}

/// Extract the message keys a route's source names via `t('key')` or
/// `t("key")`. A conservative string scan: a route that builds a key
/// dynamically (`t(`nav.${x}`)`) names no static key and therefore ships no
/// subset — it falls back to the full catalog, which is correct.
fn scan_message_keys(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for quote in ['\'', '"'] {
        let pat = format!("t({quote}");
        let mut rest = src;
        while let Some(i) = rest.find(&pat) {
            let after = &rest[i + pat.len()..];
            if let Some(end) = after.find(quote) {
                let key = &after[..end];
                if !key.is_empty()
                    && key
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '-')
                {
                    out.push(key.to_string());
                }
                rest = &after[end + 1..];
            } else {
                break;
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Find the 1-based `(line, column)` of the first occurrence of `needle` in
/// `src`. Returns `None` when the literal is not found as-is (a multi-line
/// string, or an escaped form) — in which case the diagnostic carries no
/// frame rather than pointing at the wrong line.
fn find_literal_line(src: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let mut offset = 0usize;
    for (i, line) in src.lines().enumerate() {
        if let Some(byte_col) = line.find(needle) {
            // Column is a character count within the line.
            let col = line[..byte_col].chars().count() + 1;
            return Some((i + 1, col));
        }
        offset += line.len() + 1;
        let _ = offset;
    }
    None
}

/// Read each route's reachable chunk list from the client manifest and hash
/// every chunk file. The result is what a per-chunk incremental build would
/// diff against; recording it makes the question measurable even though
/// Rolldown 1.2.13's API does not expose a per-module rebuild.
fn collect_route_chunks(
    rootp: &Path,
    client_manifest: &serde_json::Value,
) -> Vec<crate::build_graph::RouteChunkRecord> {
    let client_dir = rootp.join("dist/client");
    let mut out = Vec::new();
    let Some(routes) = client_manifest.get("routes").and_then(|r| r.as_object()) else {
        return out;
    };
    for (route_id, row) in routes {
        let mut chunks: Vec<(String, String)> = Vec::new();
        if let Some(entry) = row.get("entry_chunk").and_then(|v| v.as_str()) {
            push_chunk_hash(&client_dir, entry, &mut chunks);
        }
        if let Some(pre) = row.get("preload").and_then(|v| v.as_array()) {
            for p in pre {
                if let Some(f) = p.as_str() {
                    push_chunk_hash(&client_dir, f, &mut chunks);
                }
            }
        }
        chunks.sort();
        chunks.dedup();
        out.push(crate::build_graph::RouteChunkRecord {
            route_id: route_id.clone(),
            chunks,
        });
    }
    out
}

fn push_chunk_hash(dir: &Path, file: &str, out: &mut Vec<(String, String)>) {
    let Ok(bytes) = std::fs::read(dir.join(file)) else {
        return;
    };
    out.push((file.to_string(), blake3::hash(&bytes).to_hex().to_string()));
}

/// B2: hold the document's closing tags until the deferred settles finish,
/// emitting one resolution script per settle as it arrives.
///
/// The render stream ends with `</body></html>`. This adapter withholds the
/// last `suffix_len` bytes, forwards everything before them immediately, then
/// drains the settle receiver (one `<script type="application/oxide-deferred"
/// data-id="…">` per settle, in settle order), and finally emits the
/// tail + the withheld closing bytes.
///
/// The point: the shell and the initial data reach the browser at once; each
/// deferred slot resolves when *its* promise settles, not when the slowest
/// one does. A no-defer route never enters this path (its receiver is empty
/// and the caller uses the buffered `deferred_tag`).
fn strip_close_and_append_settles(
    src: oxide_server::BodyStream,
    mut settles: tokio::sync::mpsc::Receiver<serde_json::Value>,
    nonce: String,
    // §20 / O3b: waterfall context. When present, the spawned drain task
    // records a `deferred-settles` stage on the request's row after the
    // last settle lands. The stage's value is the time from the start of
    // the settle drain to the last delivered settle — i.e. the extra
    // time the settles added *after* the shell finished, not the total
    // request time. That is what the inspector bar means.
    waterfall: Option<oxide_dev::WaterfallRecorder>,
    request_id: String,
    trace_id: Option<String>,
) -> oxide_server::BodyStream {
    use bytes::Bytes;
    use futures::StreamExt as _;

    const CLOSE: &[u8] = b"</body></html>";
    let (tx, mut rx) = tokio::sync::mpsc::channel::<std::result::Result<Bytes, std::io::Error>>(16);
    tokio::spawn(async move {
        let mut src = src;
        // Hold back at least CLOSE.len() bytes so a `</body></html>` split
        // across chunks is still stripped. Keep one extra byte of margin so
        // a chunk boundary cannot defeat the match.
        let hold = CLOSE.len() + 1;
        let mut buf: Vec<u8> = Vec::new();
        while let Some(item) = src.next().await {
            match item {
                Ok(chunk) => {
                    buf.extend_from_slice(&chunk);
                    if buf.len() > hold {
                        let emit_upto = buf.len() - hold;
                        let head = buf[..emit_upto].to_vec();
                        buf.drain(..emit_upto);
                        if tx.send(Ok(Bytes::from(head))).await.is_err() {
                            return; // consumer gone
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            }
        }
        // Strip the trailing close, if present (it always is for a full doc).
        let stripped = if buf.ends_with(CLOSE) {
            buf.truncate(buf.len() - CLOSE.len());
            std::mem::take(&mut buf)
        } else {
            std::mem::take(&mut buf)
        };
        if !stripped.is_empty() && tx.send(Ok(Bytes::from(stripped))).await.is_err() {
            return;
        }
        // §20 / O3b: mark the drain start so `deferred-settles` measures
        // the settle window only, not the whole request (which is already
        // recorded by the caller at dispatch time).
        let settles_started = std::time::Instant::now();
        // §20 / WP-r9 nit: count actual settles so we do not record a
        // ~0 ms "deferred-settles" row on a no-defer stream. An empty
        // record_staged is noise in the inspector.
        let mut settle_count: usize = 0;
        // One resolution script per settle, in settle order. A sender that
        // is never dropped would otherwise hold the document open forever;
        // 30s is far past any real defer and turns a leak into a closed page.
        let deadline = std::time::Duration::from_secs(30);
        loop {
            let v = match tokio::time::timeout(deadline, settles.recv()).await {
                Ok(Some(v)) => v,
                Ok(None) => break, // channel closed: all settles delivered
                Err(_) => {
                    // A settle hung: close the document rather than hold it.
                    tracing::warn!("deferred settle wait timed out; closing the document");
                    break;
                }
            };
            let Some(obj) = v.get("deferredSettle") else {
                continue;
            };
            settle_count += 1;
            let Some(id) = obj.get("id").and_then(|x| x.as_str()) else {
                continue;
            };
            let payload = match obj.get("value") {
                Some(value) => value.clone(),
                None => serde_json::json!({
                    "__oxide_deferred_error": obj
                        .get("error")
                        .and_then(|e| e.as_str())
                        .unwrap_or("deferred rejected")
                }),
            };
            let json = serde_json::to_string(&payload).unwrap_or_else(|_| "null".into());
            let safe = json.replace('<', "\\u003c");
            let tag = format!(
                r#"<script type="application/oxide-deferred" data-id="{id}" nonce="{nonce}">{safe}</script>"#
            );
            if tx.send(Ok(Bytes::from(tag))).await.is_err() {
                return;
            }
        }
        // §20 / O3b: record the settle window on the request's waterfall
        // row. Merges with the initial middleware/load/render record via
        // `record_staged` — the two calls produce one row per request.
        if settle_count > 0 {
            if let Some(rec) = waterfall {
                let settle_ms = settles_started.elapsed().as_secs_f64() * 1000.0;
                rec.record_staged(
                    &request_id,
                    vec![("deferred-settles".to_string(), settle_ms)],
                    trace_id,
                );
            }
        }
        // Close the document only after every settle has been delivered.
        let _ = tx.send(Ok(Bytes::from_static(CLOSE))).await;
        // `tx` drops, closing the body stream.
    });
    Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)))
}

// §10.8 / B-27: process-local PPR shell cache, keyed by (build id, route,
// params, vary). One store per server; only populated when `render.ppr` is
// enabled.
//
// Bounded by PPR_SHELLS_MAX with least-recently-inserted eviction. The pre-r9
// behavior cleared the whole map on the 256th insert, so every visitor paid
// the first-request tap again after the cap.
type PprShells = std::sync::Arc<std::sync::Mutex<PprShellStore>>;

const PPR_SHELLS_MAX: usize = 256;

/// Bounded LRU store for PPR shells. Insertion-order, not access-order: a
/// shell being served will be re-fetched if evicted, which re-inserts it.
/// A miss costs a tap, not a wrong answer.
pub(crate) struct PprShellEntry {
    html: String,
    /// Same tags the L0 entry carries (`route`, `path:<path>`), so a
    /// revalidate action or SWR refresh can drop the shell with it.
    tags: Vec<String>,
}

pub(crate) struct PprShellStore {
    map: std::collections::HashMap<String, PprShellEntry>,
    /// Insertion order for eviction. A re-insert appends a fresh occurrence;
    /// the older one is skipped on evict (the map lookup says which is live).
    order: std::collections::VecDeque<String>,
}

impl Default for PprShellStore {
    fn default() -> Self {
        Self {
            map: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
        }
    }
}

impl PprShellStore {
    pub(crate) fn get(&self, key: &str) -> Option<&String> {
        self.map.get(key).map(|e| &e.html)
    }

    /// Drop every shell carrying `tag` (hard invalidation).
    pub(crate) fn invalidate_tag(&mut self, tag: &str) -> usize {
        let doomed: Vec<String> = self
            .map
            .iter()
            .filter(|(_, e)| e.tags.iter().any(|t| t == tag))
            .map(|(k, _)| k.clone())
            .collect();
        for k in &doomed {
            self.map.remove(k);
            if let Some(pos) = self.order.iter().position(|o| o == k) {
                self.order.remove(pos);
            }
        }
        doomed.len()
    }

    /// Drop every shell carrying `path:<path>` (hard invalidation).
    pub(crate) fn invalidate_path(&mut self, path: &str) -> usize {
        self.invalidate_tag(&format!("path:{path}"))
    }

    pub(crate) fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    /// Insert, evicting the oldest live key when at capacity.
    ///
    /// Re-insert of an existing key removes its old order entry first, so a
    /// refreshed shell is at the *back* of the eviction queue. The earlier
    /// form appended a duplicate order entry and relied on the map lookup
    /// to skip stale occurrences — but a re-insert keeps the key in the
    /// map, so the lookup could not distinguish the stale occurrence from
    /// the live one and evicted the freshly-refreshed key.
    ///
    /// `VecDeque::remove(pos)` is O(256); taps run once per route per
    /// process, so the scan is invisible next to the render itself.
    pub(crate) fn insert(&mut self, key: String, shell: String) {
        self.insert_tagged(key, shell, Vec::new());
    }

    pub(crate) fn insert_tagged(&mut self, key: String, shell: String, tags: Vec<String>) {
        if self.map.contains_key(&key) {
            // Refresh: drop the stale order occurrence so age is reset.
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
            }
        } else if self.map.len() >= PPR_SHELLS_MAX {
            // New key at capacity: evict the oldest live entry.
            while let Some(oldest) = self.order.pop_front() {
                if self.map.remove(&oldest).is_some() {
                    break;
                }
            }
        }
        self.map
            .insert(key.clone(), PprShellEntry { html: shell, tags });
        self.order.push_back(key);
    }
}

/// First request for a PPR route: the render is streamed through unchanged,
/// while the completed document is split into a shell (holes for each
/// `data-ox-b` boundary) and stored with the nonce replaced by
/// [`l0_cache::NONCE_SENTINEL`] so another request can reuse it.
fn ppr_tap_stream(
    src: oxide_server::BodyStream,
    shells: PprShells,
    key: String,
    nonce: String,
    tags: Vec<String>,
) -> oxide_server::BodyStream {
    use futures::StreamExt as _;
    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<std::result::Result<bytes::Bytes, std::io::Error>>(8);
    tokio::spawn(async move {
        let mut src = src;
        let mut collected: Vec<u8> = Vec::new();
        while let Some(item) = src.next().await {
            match item {
                Ok(chunk) => {
                    collected.extend_from_slice(&chunk);
                    if tx.send(Ok(chunk)).await.is_err() {
                        return;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            }
        }
        // The client already saw EOF; storing after this point adds no latency.
        if let Ok(html) = String::from_utf8(collected) {
            if let Ok(shell) = oxide_render::ppr::split_shell(&html) {
                if !shell.hole_ids.is_empty() {
                    let stored = shell.html.replace(&nonce, l0_cache::NONCE_SENTINEL);
                    if let Ok(mut store) = shells.lock() {
                        store.insert_tagged(key, stored, tags);
                    }
                }
            }
        }
    });
    Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)))
}

/// Later requests: emit the cached shell immediately, then forward only the
/// resolved boundary fragments from the live render (`<template data-ox-c>`),
/// dropping the duplicated static document. The resolver in the shell swaps
/// each fragment into its hole as it arrives.
fn ppr_filter_stream(
    shell: String,
    src: oxide_server::BodyStream,
    nonce: String,
) -> oxide_server::BodyStream {
    use futures::StreamExt as _;
    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<std::result::Result<bytes::Bytes, std::io::Error>>(16);
    tokio::spawn(async move {
        let mut src = src;
        let prelude = format!("{shell}{}", oxide_render::ppr::client_resolver(&nonce));
        if tx.send(Ok(bytes::Bytes::from(prelude))).await.is_err() {
            return;
        }
        const OPEN: &[u8] = b"<template data-ox-c=";
        const CLOSE: &[u8] = b"</template>";
        let mut buf: Vec<u8> = Vec::new();
        while let Some(item) = src.next().await {
            match item {
                Ok(chunk) => {
                    buf.extend_from_slice(&chunk);
                    loop {
                        let Some(rel_open) = find_subslice(&buf, OPEN) else {
                            // No boundary yet. Keep only a tail long enough to
                            // contain a split opening tag.
                            if buf.len() > OPEN.len() + 8 {
                                let keep = OPEN.len() + 8;
                                buf.drain(..buf.len() - keep);
                            }
                            break;
                        };
                        let Some(rel_close) = find_subslice(&buf[rel_open..], CLOSE) else {
                            // A boundary may span chunks; drop the prefix and
                            // wait for the rest.
                            buf.drain(..rel_open);
                            break;
                        };
                        let end = rel_open + rel_close + CLOSE.len();
                        let frag = bytes::Bytes::copy_from_slice(&buf[rel_open..end]);
                        buf.drain(..end);
                        if tx.send(Ok(frag)).await.is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            }
        }
        // `tx` drops → EOF.
    });
    Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)))
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

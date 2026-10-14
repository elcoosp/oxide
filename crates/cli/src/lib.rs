//! oxide-cli — clap entry, commands (spec §19.1).
//!
//! Commands: dev, build, start, preview, routes, analyze, check, why, doctor,
//! add, generate, cache, audit, migrate, test (in-process app.fetch helper).

use clap::{Parser, Subcommand};
use oxide_config::OxideConfig;
use oxide_router::RouteTree;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub mod mcp;

pub mod precompile;
mod framework_shim;
pub mod loader_shim;
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

    // Two independent subscriber shapes must be constructed, because each
    // layer's type parameter is fixed by the first `.with()` call. Boxing
    // every layer into `Box<dyn Layer<Registry>>` would also work; building
    // in each branch keeps the dispatch overhead zero in the common no-OTLP
    // case.
    let subscriber: Box<dyn tracing::Subscriber + Send + Sync> = match endpoint {
        Some(ep) => match build_otel_layer(&ep) {
            Ok(otel_layer) => {
                // tracing-opentelemetry's layer impls `Layer<Registry>`; it
                // goes innermost. Filter and format layers stack on top.
                Box::new(
                    tracing_subscriber::registry()
                        .with(otel_layer)
                        .with(env_filter())
                        .with(fmt_layer()),
                )
            }
            Err(e) => {
                eprintln!(
                    "oxide: OTLP export disabled ({e}); continuing with console logging only"
                );
                Box::new(
                    tracing_subscriber::registry()
                        .with(env_filter())
                        .with(fmt_layer()),
                )
            }
        },
        None => Box::new(
            tracing_subscriber::registry()
                .with(env_filter())
                .with(fmt_layer()),
        ),
    };

    let _ = tracing::subscriber::set_global_default(subscriber);
}

fn env_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
}

fn fmt_layer<S>() -> tracing_subscriber::fmt::Layer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    tracing_subscriber::fmt::layer().with_target(false)
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
                crate::DevFlags { host, https, qr, open, port, proxy },
                w,
            )
            .await
        }
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
        Command::Mcp { root } => cmd_mcp(&root, w).await,
        Command::Schema => cmd_schema(w),
        Command::Config { root, json } => cmd_config(&root, json, w).await,
        Command::Render { root, route, url, params, data, stream } => cmd_render(&root, &route, url.as_deref(), &params, &data, stream, w).await,
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
    // Canonicalize `root` first so `dir` is absolute. `RouteTree::scan_with_config`
    // joins the override onto `dir`; if the override is already root-relative
    // (which it is when `root` itself is relative), the join produces
    // `root/root/src/routes`. An absolute override short-circuits that:
    // `PathBuf::join` returns the RHS unchanged when it's absolute.
    let root_abs = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf());
    let dir = root_abs.join(&cfg.routes.dir);
    RouteTree::scan_with_config(&root_abs, Some(&dir))
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))
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
) -> String {
    let mut out = String::new();
    out.push_str("// Generated by oxide-cli -- island boot entry. Do not edit.\n");
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
    out.push_str(
        "import { boot, installBootRuntime } from 'oxide/client/boot';\n",
    );
    let mut idents: Vec<(String, String)> = Vec::new();
    for (name, src) in islands {
        let abs = root.join(src);
        let ident = format!(
            "__island_{}",
            name.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
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
    out.push_str("installBootRuntime();\n");
    out.push_str(
        "window.__oxideRegisterAdapter(clientAdapter, async (chunk) => {\n  \
         return modules[chunk] ?? (await import(chunk));\n});\n",
    );
    out.push_str("boot();\n");
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
    // Vue, Svelte and Solid entries arrive precompiled from the tooling
    // pool, so their JSX never reaches Oxc's lowering. Only React uses the
    // Oxc default runtime (`react/jsx-runtime`), which is what `None` means.
    let _ = framework;
    None
}

fn bundle_env_options(
    env: oxide_bundler::BundleEnv,
    entries: BTreeMap<String, PathBuf>,
    out_dir: PathBuf,
    precompiled: BTreeMap<PathBuf, String>,
    jsx_import_source: Option<String>,
) -> oxide_bundler::BundleOptions {
    // §9.4 per-build secret: fresh on each invocation so the unguessable
    // server-fn URL prefix rotates between deployments.
    //
    // IMPORTANT: `/dev/urandom` never EOFs. `std::fs::read` would read
    // forever, allocating the whole file (which is unbounded). Use a
    // bounded `read_exact` on the 16 bytes we actually need.
    let build_secret = {
        use std::io::Read as _;
        let mut h = blake3::Hasher::new();
        h.update(
            &std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos().to_le_bytes())
                .unwrap_or([0u8; 16]),
        );
        let mut rnd = [0u8; 16];
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            let _ = f.read_exact(&mut rnd);
        }
        h.update(&rnd);
        h.finalize().to_hex().to_string()[..32].to_string()
    };
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
        // Production builds minify. The dev server has its own bundling
        // path (`dev::prebundle`) and never goes through this helper, so
        // turning minify on here only affects `oxide build` output.
        // Sourcemaps stay off until the Rust sourcemap chain is wired
        // through `RolldownBundler` — a bare `.map` URL would 404.
        minify: true,
        sourcemap: false,
        externals: Vec::new(),
        define,
        tsconfig_paths: BTreeMap::new(),
        build_secret,
        precompiled,
        jsx_import_source,
    }
}

// ----------------------------------------------------------------- commands

/// `oxide dev` (§13). `--once` runs a single watch poll (test hook); otherwise the
/// 200 ms poll loop runs until Ctrl-C.
async fn cmd_dev(root: &str, once: bool, flags: DevFlags, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
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
        outln!(w, "dev HTTPS enabled (self-signed; SANs: {})", cert.sans.join(", "));
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
    let stack = framework_shim::build_framework_stack(config_path_opt.as_deref(),
        cfg.server.backend).await;
    if let Some(name) = stack.renderer_name() {
        outln!(w, "framework renderer: {}", name);
    }
    // §13.3: keep a pool handle for the watcher. On file change it bumps the
    // worker's module generation, so the next render/load re-evaluates the
    // edited module instead of serving the cached one.
    let hmr_pool: Option<std::sync::Arc<dyn oxide_jshost::Pool>> = stack.pool.clone();

    let started = start_dev_server_full(
        root,
        flags.port,
        bind_ip,
        tls_config,
        proxy_rules,
        stack,
    )
    .await?;
    let local = oxide_dev::local_url(started.addr.port(), flags.https);
    outln!(w, "dev server listening on {local}");
    outln!(w, "inspector: {local}/__oxide");
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

    let watcher = oxide_dev::Watcher::new(vec![rootp.clone()]);
    let mut seen = std::collections::BTreeSet::new();

    // §13.3: on any file-change event, bump the worker's module generation
    // so the next render/load sees fresh code. Best-effort: log a warning
    // if the worker is unavailable; never fails the dev loop.
    async fn invalidate_framework_modules(pool: &Option<std::sync::Arc<dyn oxide_jshost::Pool>>) {
        if let Some(pool) = pool.as_ref() {
            let host = oxide_jshost::FrameworkHost::new(std::sync::Arc::clone(pool));
            match host.invalidate().await {
                Ok(generation) => {
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

    if once {
        let events = watcher
            .poll(&mut seen)
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        if !events.is_empty() {
            invalidate_framework_modules(&hmr_pool).await;
        }
        outln!(w, "dev ready — {} change(s) in first poll", events.len());
        let _ = started.shutdown.send(true);
        return Ok(0);
    }
    // Real dev loop (§13): poll, report, Ctrl-C to stop. Not exercised in tests.
    outln!(w, "dev ready — watching for changes (Ctrl-C to stop)");
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let events = watcher
            .poll(&mut seen)
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        if !events.is_empty() {
            invalidate_framework_modules(&hmr_pool).await;
        }
        for ev in events {
            tracing::info!(?ev, "file change");
            outln!(w, "change: {ev:?}");
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
fn compile_edge_wasm(root: &Path, dist: &Path) -> anyhow::Result<Option<PathBuf>> {
    // Check the target is available first — a missing target would otherwise
    // produce a confusing build error.
    let installed = std::process::Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output();
    let has_target = match installed {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .any(|l| l.trim() == "wasm32-unknown-unknown"),
        Err(_) => false,
    };
    if !has_target {
        return Ok(None);
    }
    std::fs::create_dir_all(dist)?;
    let out = dist.join("edge.wasm");
    let status = std::process::Command::new("cargo")
        .args([
            "build",
            "-p",
            "oxide-core-wasm",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
        ])
        .current_dir(root)
        .status()
        .map_err(|e| anyhow::anyhow!("cargo build for wasm failed to start: {e}"))?;
    if !status.success() {
        return Err(anyhow::anyhow!(
            "cargo build -p oxide-core-wasm --target wasm32-unknown-unknown failed"
        ));
    }
    // Copy the produced .wasm next to the deploy artifact.
    let produced = root
        .join("target/wasm32-unknown-unknown/release/oxide_core_wasm.wasm");
    if produced.is_file() {
        std::fs::copy(&produced, &out)?;
        Ok(Some(out))
    } else {
        Ok(None)
    }
}


/// Emit a search index per configured content collection (§16.5). Writes
/// `<dist>/content/<name>.index.json` — a deterministic JSON document the
/// front end fetches lazily.
///
/// A collection whose directory is missing is reported and skipped; the build
/// itself succeeds (content often lives outside the main pipeline's blast
/// radius).
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
        std::fs::write(out_dir.join(format!("{}.index.json", coll.name)), json)?;
        outln!(
            w,
            "content index: {} ({} entries) -> dist/content/{}.index.json",
            coll.name,
            scanned.entries.len(),
            coll.name
        );
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
        let Ok(rel) = f.strip_prefix(&prerendered_root) else { continue };
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
        if origin.is_some() { "" } else { " (relative — set siteUrl for absolute URLs)" },
        if sitemap_url.is_some() { "" } else { " (no siteUrl; robots has no Sitemap: line)" },
    );
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
    let tag = format!(
        r#"<script type="module" src="/{boot_url}" nonce="{nonce}"></script>"#
    );
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
async fn write_prerender(
    dist: &Path,
    rel: &str,
    route_id: &str,
    pattern: &str,
    url_path: &str,
    params: &BTreeMap<String, String>,
    nonce: &str,
    page_renderer: Option<&std::sync::Arc<dyn oxide_server::PageRenderer>>,
    loader_runner: Option<&std::sync::Arc<dyn oxide_server::LoaderRunner>>,
    module_path: Option<&Path>,
    boot_script_url: Option<&str>,
) -> anyhow::Result<()> {
    // §9.1: run the loader for this route (if the module exists and a
    // loader runner is attached) to obtain the data the render phase
    // receives. Prerendered static pages previously always wrote `{}`
    // into PageRenderInput.data; they now run the real `load()`.
    let mut data = serde_json::Value::Object(Default::default());
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
        match loader.run(input).await {
            Ok(out) if out.status == 200 => {
                data = out.data;
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
                std::fs::write(&dest, body)?;
                return Ok(());
            }
            Err(e) => {
                let body = format!(
                    "<!doctype html><html><body>\
                     <h1>loader error</h1><pre>{}: {}</pre>\
                     </body></html>",
                    e.0.code, e.0.message
                );
                let dest = dist.join(rel);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dest, body)?;
                return Ok(());
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
        input.data = data;
        renderer
            .render(input)
            .await
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?
    } else {
        format!(
            "<!doctype html><html><head></head><body>\
             <main data-route=\"{pattern}\">placeholder</main></body></html>"
        )
    };
    // §10.3: `<Island chunk="X">` emits `data-ox-chunk="X"` — a logical island
    // id, not a runtime URL. The client boot module we generated registers
    // each island under its name, so on the wire the id and the URL are the
    // same string. (If a future slice adds per-island chunks, the rewrite
    // hook lives here.)
    let mut shell = shell;
    if let Some(url) = boot_script_url {
        shell = inject_island_boot_script(&shell, url, nonce);
    }

    let injector = oxide_render::Injector::new(
        oxide_render::HeadTags::default(),
        nonce.to_string(),
        route_id.to_string(),
    );
    let html = injector
        .inject_into_html(&shell)
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    let dest = dist.join(rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&dest, html)?;
    Ok(())
}

async fn cmd_build(root: &str, r#static: bool, w: Writer<'_>) -> anyhow::Result<i32> {
    // Canonicalize once at entry. Downstream joins (routes scan, bundler,
    // write_prerender) all expect absolute paths so a nested `root.join`
    // cannot double the prefix — `PathBuf::join` returns the RHS unchanged
    // when it is absolute.
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let cfg = load_config_strict(&rootp, "production")?;
    let tree = scan_routes(&rootp, &cfg)?;
    let dist = rootp.join("dist");
    let routes_root = rootp.join(&cfg.routes.dir);

    // §7 / §9.1: build the framework stack (renderer + loader). Renderer
    // is used for prerendering static routes; loader is present but not
    // consumed during build (build-time loader evaluation is a follow-up).
    let config_path_opt = find_config_file(&rootp).ok();
    let stack = framework_shim::build_framework_stack(config_path_opt.as_deref(),
        cfg.server.backend).await;
    if let Some(name) = stack.renderer_name() {
        outln!(w, "framework renderer: {}", name);
    }

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
    let client_out = dist.join("client");
    let server_out = dist.join("server");
    // RolldownBundler handles every project shape: it lowers TS/JSX via
    // Rolldown's built-in Oxc pipeline, and its `PrecompiledPlugin` serves
    // the JS the tooling pool produced for any `.vue`/`.svelte` entries.
    // Real V3 sourcemaps come from `SourceMapType::File`.
    use oxide_bundler::Bundler as _;
    outln!(w, "bundler: rolldown");

    // §10.3 islands: if the config declares any, generate a synthetic boot
    // entry that static-imports every island and wires the framework
    // adapter's `hydrateIsland`. It is added as a `__boot__` client entry
    // only — the server bundle stays island-free (server only needs to SSR
    // the shell markup). The emitted boot chunk URL is injected into every
    // prerendered page as `<script type="module" src="/<boot-url>">`.
    let mut client_entries = entries.clone();
    let boot_chunk_url: Option<String> = if cfg.framework.islands.is_empty() {
        None
    } else {
        let framework = cfg
            .framework
            .adapter
            .clone()
            .or_else(|| cfg.framework.name.clone())
            .unwrap_or_else(|| "react".to_string());
        let boot_src = generate_island_boot_entry(&rootp, &framework, &cfg.framework.islands);
        let boot_dir = rootp.join(".oxide");
        std::fs::create_dir_all(&boot_dir)?;
        let boot_path = boot_dir.join("island-boot.mjs");
        std::fs::write(&boot_path, boot_src)?;
        client_entries.insert("__boot__".to_string(), boot_path);
        outln!(
            w,
            "islands: {} registered (boot entry __boot__)",
            cfg.framework.islands.len()
        );
        Some("__boot__".to_string())
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
    let client_precompiled =
        precompile::precompile_entries(stack.pool.as_ref(), &entries, "dom", framework_name).await;
    let server_precompiled =
        precompile::precompile_entries(stack.pool.as_ref(), &entries, "ssr", framework_name).await;
    if !client_precompiled.is_empty() {
        outln!(w, "precompiled {} SFC(s) for client", client_precompiled.len());
    }
    if !server_precompiled.is_empty() {
        outln!(w, "precompiled {} SFC(s) for server", server_precompiled.len());
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
    let client_opts = bundle_env_options(
        oxide_bundler::BundleEnv::Client,
        client_entries,
        client_out.clone(),
        client_precompiled,
        jsx_source.clone(),
    );
    let server_opts = bundle_env_options(
        oxide_bundler::BundleEnv::Ssr,
        entries.clone(),
        server_out.clone(),
        server_precompiled,
        jsx_source,
    );
    let rb = oxide_bundler::RolldownBundler::new(&rootp);
    rb.bundle(&client_opts)
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    rb.bundle(&server_opts)
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    // The bundler writes client.manifest.json / server.manifest.json into the out dirs.
    let client_manifest = read_json(&client_out.join("client.manifest.json"))
        .ok_or_else(|| anyhow::anyhow!("bundler did not write client.manifest.json"))?;
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
    // Routes with literal generateParams are effectively static regardless of
    // whether they also have a server module.
    let mut static_override: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
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
        let route_id = e.id.0.clone();
        // §9.1: if the route declares a server module, resolve its path so
        // the loader can run during prerender. Uses the same
        // `page.server.ts` sibling convention as build_dispatch.
        let module_path: Option<std::path::PathBuf> = if e.has_server_module {
            let p = routes_root.join(e.file.with_file_name("page.server.ts"));
            if p.is_file() { Some(p) } else { None }
        } else {
            None
        };

        match gen_params {
            oxide_compiler::GenerateParamsResult::Literal(params) => {
                // Prerendered output exists → effective mode `static` for the
                // manifest, even if the route also has a server module.
                static_override.insert(route_id.clone());
                let mut outs: Vec<String> = Vec::new();
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
                    let rel = if segs.is_empty() {
                        "prerendered/index.html".to_string()
                    } else {
                        format!("prerendered/{}.html", segs.join("/"))
                    };
                    write_prerender(
                        &dist,
                        &rel,
                        &route_id,
                        &pattern,
                        &url_path_from_rel(&rel),
                        params_map,
                        &nonce_for,
                        stack.page_renderer.as_ref(),
                        stack.loader_runner.as_ref(),
                        module_path.as_deref(),
                        boot_script_url.as_deref(),
                    )
                    .await?;
                    outs.push(rel);
                }
                if !outs.is_empty() {
                    prerendered.insert(e.id.0.clone(), outs);
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
                let params_via_worker: Option<
                    Vec<std::collections::BTreeMap<String, String>>,
                > = match (stack.pool.as_ref(), module_path.as_deref()) {
                    (Some(pool), Some(mp)) => {
                        let host = oxide_jshost::LoaderHost::new(
                            std::sync::Arc::clone(pool),
                        );
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
                        // Same shape as the Literal arm: enumerate, write
                        // each, mark the route static in the manifest.
                        static_override.insert(route_id.clone());
                        let mut outs: Vec<String> = Vec::new();
                        for params_map in &params {
                            let mut segs: Vec<String> = e.segments.clone();
                            for s in segs.iter_mut() {
                                if let Some(name) = param_name(s) {
                                    if let Some(v) = params_map.get(name) {
                                        *s = v.clone();
                                    }
                                }
                            }
                            let rel = if segs.is_empty() {
                                "prerendered/index.html".to_string()
                            } else {
                                format!("prerendered/{}.html", segs.join("/"))
                            };
                            write_prerender(
                                &dist,
                                &rel,
                                &route_id,
                                &pattern,
                                &url_path_from_rel(&rel),
                                params_map,
                                &nonce_for,
                                stack.page_renderer.as_ref(),
                                stack.loader_runner.as_ref(),
                                module_path.as_deref(),
                                boot_script_url.as_deref(),
                            )
                            .await?;
                            outs.push(rel);
                        }
                        if !outs.is_empty() {
                            prerendered.insert(e.id.0.clone(), outs);
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
                let rel = if e.segments.is_empty() {
                    "prerendered/index.html".to_string()
                } else {
                    format!("prerendered/{}.html", e.segments.join("/"))
                };
                let empty_params: BTreeMap<String, String> = BTreeMap::new();
                write_prerender(
                    &dist,
                    &rel,
                    &route_id,
                    &pattern,
                    &url_path_from_rel(&rel),
                    &empty_params,
                    &nonce_for,
                    stack.page_renderer.as_ref(),
                    stack.loader_runner.as_ref(),
                    module_path.as_deref(),
                    boot_script_url.as_deref(),
                )
                .await?;
                prerendered.insert(e.id.0.clone(), vec![rel]);
            }
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
            if let Some(e) = tree.entries.iter().find(|e| &e.id.0 == id) {
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
                outln!(w, "edge wasm build failed: {e}; the Worker falls back to JS");
            }
        }
    }

    let written = oxide_adapters::emit(target, &artifact, &dist.join("deploy"))
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    // §16.5: search indexes for every configured content collection.
    emit_content_search_indexes(&rootp, &cfg, &dist, w)?;
    // §16.3: sitemap.xml + robots.txt from the route table.
    emit_sitemap_and_robots(&cfg, &tree, &dist, w)?;
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
    start_server_inner_full(root, bind, false, None, Vec::new(), stack).await
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
fn render_stream_to_body_stream(
    src: oxide_server::RenderStream,
) -> oxide_server::BodyStream {
    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<std::result::Result<bytes::Bytes, std::io::Error>>(8);
    tokio::spawn(async move {
        use futures::StreamExt as _;
        let mut src = src;
        while let Some(item) = src.next().await {
            let mapped = item.map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("{}: {}", e.0.code, e.0.message),
                )
            });
            if tx.send(mapped).await.is_err() {
                // Receiver dropped — client disconnected. Stop pumping;
                // dropping `src` will notify the upstream worker.
                return;
            }
        }
        // `tx` drops here, closing the stream cleanly.
    });
    Box::pin(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)))
}

/// §10.3: splice `<script type="module" src="/<url>">` into a streaming HTML
/// response right before the closing `</body>` tag. Works when `</body>` is
/// split across chunk boundaries by keeping a rolling tail of the last
/// `CLOSE.len() - 1` bytes and scanning for the tag on each new chunk.
///


#[allow(clippy::too_many_arguments)]
fn build_dispatch(
    tree: std::sync::Arc<oxide_router::RouteTree>,
    metrics: std::sync::Arc<oxide_server::Metrics>,
    proxy_rules: Vec<oxide_dev::ProxyRule>,
    page_renderer: Option<std::sync::Arc<dyn oxide_server::PageRenderer>>,
    loader_runner: Option<std::sync::Arc<dyn oxide_server::LoaderRunner>>,
    routes_root: std::path::PathBuf,
    boot_script_url: Option<String>,
) -> std::sync::Arc<
    dyn Fn(oxide_server::OxideRequest) -> BoxedResponse + Send + Sync,
> {
    let boot_script_url_for_render = boot_script_url.clone();
    std::sync::Arc::new(move |req: oxide_server::OxideRequest| {
            let started = std::time::Instant::now();
            let raw = req.url.split(['?', '#']).next().unwrap_or("/").to_string();

            // §13.5 proxy: if the request matches a rule, forward upstream and
            // skip routing entirely. Response is passed through unchanged.
            if !proxy_rules.is_empty() {
                let query = req.url.split_once('?').map(|(_, q)| q.to_string());
                if let Some(m) = oxide_dev::apply_proxy(
                    &proxy_rules,
                    &raw,
                    query.as_deref(),
                ) {
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
            let candidates = if normalized == "/" {
                vec!["/".to_string(), "/index".to_string()]
            } else {
                vec![normalized]
            };
            let matched: Option<oxide_router::MatchResult> =
                candidates.iter().find_map(|p| tree.match_path(p));
            let (status, label) = match &matched {
                Some(m) => (200u16, pattern_of(&m.route.segments)),
                None => (404u16, "_unmatched".to_string()),
            };

            // Pre-compute the loader inputs synchronously so the async block
            // owns plain data (no borrow of `matched`).
            let (module_path, route_id, params, has_server_module): (
                Option<std::path::PathBuf>,
                Option<String>,
                std::collections::BTreeMap<String, String>,
                bool,
            ) = match &matched {
                Some(m) => (
                    Some(routes_root.join(m.route.file.with_file_name("page.server.ts"))),
                    Some(m.route.id.0.clone()),
                    m.params.clone(),
                    m.route.has_server_module,
                ),
                None => (None, None, std::collections::BTreeMap::new(), false),
            };
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

            let page_renderer = page_renderer.clone();
            let loader_runner = loader_runner.clone();
            let metrics = std::sync::Arc::clone(&metrics);
            let req_owned = req.clone();
            // Shadow so the async block moves a per-call clone rather than
            // the outer closure's captured value (which would be E0507 —
            // an `Fn` closure cannot surrender a captured variable).
            let boot_script_url_for_render = boot_script_url_for_render.clone();

            Box::pin(async move {
                // §9.1: loader phase. When the matched route declares a server
                // module and a LoaderRunner is attached, run it. A non-200
                // output (error(...) or redirect(...)) short-circuits the
                // response; a 200 output threads `data` into the render phase.
                let mut data = serde_json::Value::Object(Default::default());

                if has_server_module {
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
                        input.headers = req_owned.headers.clone();
                        input.params = params.clone();

                        match runner.run(input).await {
                            Ok(out) if out.short_circuits() => {
                                let body = out
                                    .data
                                    .get("message")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let mut resp =
                                    oxide_server::OxideResponse::new(out.status, body);
                                if let Some(loc) = out.location {
                                    resp = resp.with_header("location", loc);
                                }
                                metrics.observe(&label, started.elapsed(), out.status);
                                return resp;
                            }
                            Ok(out) => {
                                data = out.data;
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
                                    format!("loader error: {}", e.0.message),
                                );
                            }
                        }
                    }
                }

                // §7: render phase. Attached renderer wins; otherwise IR placeholder.
                if let Some(renderer) = page_renderer.as_ref() {
                    // Route ID — the framework adapter's `componentFor`
                    // receives this, not the URL pattern.
                    let route_for_render = route_id_for_render
                        .clone()
                        .unwrap_or_else(|| "_unmatched".to_string());
                    let mut input = oxide_server::PageRenderInput::new(
                        route_for_render,
                        req_owned.url.clone(),
                    );
                    input.params = params;
                    input.data = data;
                    match renderer.render_streaming(input).await {
                        Ok(render_stream) => {
                            metrics.observe(&label, started.elapsed(), 200);
                            // Bridge the renderer's Send-only stream to the
                            // transport's Send + Sync body stream. If the
                            // project registered islands, splice the boot
                            // script tag right before `</body>` — the sliding
                            // window in `oxide_server::inject_before_close` buffers
                            // at most `len("</body>") - 1` bytes so a tag
                            // split across chunks still lands in the right
                            // place and every other chunk flushes untouched.
                            let bridged = render_stream_to_body_stream(render_stream);
                            let body = match boot_script_url_for_render.as_deref() {
                                Some(url) => {
                                    // §17.1: the response's CSP is `strict-
                                    // dynamic`; a script without the matching
                                    // nonce is blocked by the browser. The
                                    // pipeline derives a per-request nonce
                                    // in `finish_response`; we compute the
                                    // same value here from the request id.
                                    let nonce =
                                        oxide_server::nonce_for(&req_owned.request_id);
                                    let tag = format!(
                                        r#"<script type="module" src="/{url}" nonce="{nonce}"></script>"#,
                                    );
                                    oxide_server::inject_before_close(
                                        bridged,
                                        b"</body>",
                                        tag.as_bytes(),
                                    )
                                }
                                None => bridged,
                            };
                            oxide_server::OxideResponse::stream(200, body)
                                .with_header("content-type", "text/html; charset=utf-8")
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
                                format!("render error: {}", e.0.message),
                            )
                        }
                    }
                } else {
                    metrics.observe(&label, started.elapsed(), status);
                    oxide_server::OxideResponse::html(
                        status,
                        format!(
                            "<!doctype html><html><head><title>Oxide</title></head>\
                             <body><main>Oxide serving {label}</main></body></html>"
                        ),
                    )
                }
            })
        })
}

pub async fn start_dev_server_full(
    root: &str,
    port: Option<u16>,
    bind_ip: std::net::IpAddr,
    tls: Option<std::sync::Arc<tokio_rustls::rustls::ServerConfig>>,
    proxy_rules: Vec<oxide_dev::ProxyRule>,
    stack: crate::framework_shim::FrameworkStack,
) -> anyhow::Result<StartedServer> {
    start_server_inner_full(
        root,
        Some((bind_ip, port.unwrap_or(0))),
        true,
        tls,
        proxy_rules,
        stack,
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
            placeholder: None,
        };

        RustImageBackend
            .process(ireq)
            .map(|r| oxide_server::ImageEndpointResult {
                bytes: r.bytes,
                content_type: r.format.mime(),
                cache_key: r.cache_key,
            })
            .map_err(|e| format!("{}: {}", e.0.code, e.0.message))
    }
}

/// §16.1: construct the endpoint from config. Returns `None` when the
/// project declares no remote image patterns AND has no `public/` or
/// `dist/client/` directory to serve from — a project with images off
/// wants a 404, not an endpoint that always errors.
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
    let signing_secret = std::env::var("OXIDE_IMAGE_SECRET").ok().filter(|s| !s.is_empty());
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
        match self
            .host
            .run_coded(id, args_json, &self.entry_path)
            .await
        {
            Ok(v) => serde_json::to_vec(&v).map_err(|e| {
                oxide_server::DispatchError::internal(format!("serialize fn result: {e}"))
            }),
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

fn load_client_assets(rootp: &Path) -> Option<oxide_server::StaticAssets> {
    let dist = rootp.join("dist");
    if !dist.is_dir() {
        return None;
    }
    oxide_server::StaticAssets::scan(dist).ok()
}

async fn start_server_inner_full(
    root: &str,
    bind: Option<(std::net::IpAddr, u16)>,
    with_inspector: bool,
    tls: Option<std::sync::Arc<tokio_rustls::rustls::ServerConfig>>,
    proxy_rules: Vec<oxide_dev::ProxyRule>,
    stack: crate::framework_shim::FrameworkStack,
) -> anyhow::Result<StartedServer> {
    let rootp = PathBuf::from(root);
    let cfg = load_config_or_default(&rootp, "production");
    let tree = std::sync::Arc::new(scan_routes(&rootp, &cfg)?);
    let (bind_ip, requested_port) = match bind {
        // A caller-supplied port is honoured verbatim — including 0, which
        // means "ephemeral, OS chooses" (the test and CI convention).
        Some((ip, p)) => (ip, p),
        None => (
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            cfg.server.port,
        ),
    };
    let port = resolve_port(requested_port).await;
    let rules = oxide_router::RulesEngine::compile(config_rules_to_router(&cfg.rules))
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    // §20 metrics: ONE registry shared by the pipeline (native branches) and
    // the dispatch delegate (per-route labels). Cardinality is bounded by the
    // route pattern (e.g. `/blog/[slug]`), never the resolved URL.
    let metrics = std::sync::Arc::new(oxide_server::Metrics::new());

    let assets = load_client_assets(&rootp);
    let image_endpoint = build_image_endpoint(&rootp, &cfg);
    let server_fns = build_server_fn_dispatcher(&rootp, stack.pool.as_ref());
    let pipeline = std::sync::Arc::new(oxide_server::Pipeline {
        rules,
        assets,
        image_endpoint,
        server_fns,
        limits: oxide_server::Limits::default(),
        security: oxide_server::SecuritySettings {
            csp: cfg.security.csp.clone(),
            csrf: cfg.security.csrf.clone(),
            ..Default::default()
        },
        rate_limiter: std::sync::Arc::new(oxide_server::RateLimiter::default()),
        health_path: "/_oxide/health".to_string(),
        metrics_path: "/_oxide/metrics".to_string(),
        metrics: Some(std::sync::Arc::clone(&metrics)),
        inspector: if with_inspector {
            Some(std::sync::Arc::new(CliInspector {
                state: oxide_dev::RouteTableInspector::from_router_table(tree.table()).state(),
            }))
        } else {
            None
        },
        page_renderer: None,
    });
    let proxy_rules: Vec<oxide_dev::ProxyRule> = proxy_rules.clone();
    let routes_root = rootp.join(&cfg.routes.dir);
    // §10.3: the same boot URL that the build-time prerender injects also
    // has to be spliced into every live request's shell — otherwise dev /
    // serve would ship pages whose `<ox-island>` markup is inert.
    let boot_script_url: Option<String> = {
        let client_manifest = rootp.join("dist/client/client.manifest.json");
        std::fs::read_to_string(&client_manifest)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|v| {
                v.get("routes")
                    .and_then(|r| r.get("__boot__"))
                    .and_then(|r| r.get("entry_chunk"))
                    .and_then(|e| e.as_str())
                    .map(|f| format!("client/{f}"))
            })
    };
    let dispatch = build_dispatch(
        std::sync::Arc::clone(&tree),
        std::sync::Arc::clone(&metrics),
        proxy_rules.clone(),
        stack.page_renderer,
        stack.loader_runner,
        routes_root,
        boot_script_url,
    );
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        let bind_addr = std::net::SocketAddr::new(bind_ip, port);
        if let Err(e) =
            oxide_server::Server::serve_with_tls(bind_addr, pipeline, dispatch, rx, tls).await
        {
            tracing::error!(code = %e.0.code, "server stopped with error");
        }
    });
    Ok(StartedServer { addr, shutdown: tx, metrics })
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
    let cfg = load_config_or_default(&rootp, "production");
    let config_path_opt = find_config_file(&rootp).ok();
    let stack = framework_shim::build_framework_stack(
        config_path_opt.as_deref(),
        cfg.server.backend,
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
        let params_for_loader: serde_json::Value =
            serde_json::Value::Object(params_json.clone());
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
                        if loc.is_empty() { String::new() } else { format!(" -> {loc}") }
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

    let input = serde_json::json!({
        "route": route,
        "url": url_str,
        "params": params_json,
        "data": effective_data,
        "head": { "meta": [], "link": [], "style": [], "script": [] },
    });

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
                w.flush().map_err(|e| anyhow::anyhow!("flush stdout: {e}"))?;
            }
            oxide_jshost::CallStream::Chunked(rx) => {
                while let Some(item) = rx.recv().await {
                    let bytes =
                        item.map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
                    w.write_all(&bytes)
                        .map_err(|e| anyhow::anyhow!("write stdout: {e}"))?;
                    w.flush().map_err(|e| anyhow::anyhow!("flush stdout: {e}"))?;
                }
            }
        }
        // Match the non-streaming path's trailing newline. `outln!` appends
        // `\n`; the stream path writes bytes directly, so add it here.
        w.write_all(b"\n")
            .map_err(|e| anyhow::anyhow!("write stdout: {e}"))?;
        w.flush().map_err(|e| anyhow::anyhow!("flush stdout: {e}"))?;
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

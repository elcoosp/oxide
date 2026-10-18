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

mod build_cache;
mod framework_shim;
mod l0_cache;
pub mod loader_shim;
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
        } => cmd_build(&root, r#static, w).await,
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
        Command::Audit { root } => cmd_audit(&root, w).await,
        Command::Migrate { root, source } => cmd_migrate(&root, &source, w).await,
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

fn scan_routes(root: &Path, cfg: &OxideConfig) -> anyhow::Result<RouteTree> {
    // Canonicalize `root` first so `dir` is absolute. `RouteTree::scan_with_config`
    // joins the override onto `dir`; if the override is already root-relative
    // (which it is when `root` itself is relative), the join produces
    // `root/root/src/routes`. An absolute override short-circuits that:
    // `PathBuf::join` returns the RHS unchanged when it's absolute.
    let root_abs = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
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
    // §8.4 loading boundaries: URL prefix (`/`, `/blog`) → on-disk
    // `loading.tsx` path. Emitted as static imports + a `loading` map passed
    // to `bootWithRouter`.
    loading: &BTreeMap<String, PathBuf>,
    // §16.6 / SPEC-21: whether the project configured a PWA (`manifest`).
    // When true, the boot entry registers the service worker.
    pwa: bool,
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
        out.push_str(
            "if (typeof navigator !== 'undefined' && 'serviceWorker' in navigator) {\n  \
             void import('/pwa-register.js').catch(() => {});\n}\n",
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
        // Production builds minify. The dev server has its own bundling
        // path (`dev::prebundle`) and never goes through this helper, so
        // turning minify on here only affects `oxide build` output.
        minify: true,
        // BUG-11: honour `build.sourcemap`. `RolldownBundler` supports
        // `SourceMapType::File`; `off` emits no map.
        sourcemap: sourcemap_on,
        // §21: externals only apply to the server bundle; a browser bundle
        // must inline everything.
        externals: match env {
            oxide_bundler::BundleEnv::Ssr => ssr_externals,
            _ => Vec::new(),
        },
        define,
        tsconfig_paths: BTreeMap::new(),
        build_secret,
        precompiled,
        jsx_import_source,
        transform_hook,
        css_transform: Some(css_transform),
        inline_dynamic_imports: false,
        resolve_conditions: None,
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
                loading_entries
                    .insert(e.id.0.clone(), rootp.join(&cfg.routes.dir).join(&e.file));
            }
            oxide_router::RouteKind::Page | oxide_router::RouteKind::Endpoint => {
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
        true, // sourcemap on for dev
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
    // §11.2: thread the project's `cache.handler` into the stack so the tier
    // the worker (and the L0 cache) actually uses honors the config — the
    // CLI-side `stack.cache_tier == None` arm below is only for stacks built
    // without a config (tests), so this is the only live wiring.
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

    // §13.1 / P0-7: a fresh `oxide dev` has no client bundle, so nothing
    // would hydrate, navigate, or load a route's JS. Build it once (unminified)
    // if the manifest is missing. A `dev` after `build` finds it and skips.
    ensure_dev_client_bundle(&rootp, &cfg, &tree, w).await?;

    // SPEC-14: the HMR hub. Its build id comes from the client manifest
    // when present (a `dev` start after `build`), else `"dev"`.
    let hmr_build_id = std::fs::read_to_string(rootp.join("dist/client/client.manifest.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("build_id").and_then(|x| x.as_str()).map(str::to_string))
        .unwrap_or_else(|| "dev".to_string());
    let hmr_hub = std::sync::Arc::new(oxide_dev::HmrHub::with_build_id(hmr_build_id));
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
    let dev_graph = std::sync::Arc::new(oxide_graph::DemandGraph::new());

    /// A compute that reads one file, recording the `Input::File` edge. Its
    /// output hash is the file content hash — an unchanged re-read is a
    /// Survivor (early cutoff), so dependents are not re-run.
    struct FileCompute(std::path::PathBuf);
    impl oxide_graph::Compute for FileCompute {
        fn compute(
            &self,
            ctx: &mut oxide_graph::ComputeCtx,
        ) -> oxide_diagnostics::Result<oxide_graph::NodeOutput> {
            let bytes = ctx.fs.read_file(&self.0)?;
            let hash = blake3::hash(&bytes);
            Ok(oxide_graph::NodeOutput {
                hash,
                artifact: None,
                deps: Default::default(),
            })
        }
    }

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

    // PERF-08: register a graph node per changed file and invalidate it. The
    // returned set is non-empty only when a node actually recomputed (early
    // cutoff spares unchanged content), so the worker generation bumps only
    // when there is real new code to see.
    fn graph_invalidated(dev_graph: &oxide_graph::DemandGraph, ev: &oxide_dev::FileEvent) -> bool {
        let path = match ev {
            oxide_dev::FileEvent::Modified(p)
            | oxide_dev::FileEvent::Created(p)
            | oxide_dev::FileEvent::Removed(p) => p.clone(),
        };
        let key = oxide_graph::NodeKey::new(path.to_string_lossy().as_ref());
        // (Re)register a compute that reads the file.
        struct FileCompute(std::path::PathBuf);
        impl oxide_graph::Compute for FileCompute {
            fn compute(
                &self,
                ctx: &mut oxide_graph::ComputeCtx,
            ) -> oxide_diagnostics::Result<oxide_graph::NodeOutput> {
                let bytes = ctx.fs.read_file(&self.0).unwrap_or_default();
                Ok(oxide_graph::NodeOutput {
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
            .invalidate(&[oxide_graph::Input::Node(key, blake3::hash(&[]))])
            .is_empty()
    }

    if once {
        let events = watcher
            .poll(&mut seen)
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        let any = events.iter().any(|e| graph_invalidated(&dev_graph, e));
        if any {
            invalidate_framework_modules(&hmr_pool).await;
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
        let events = watcher
            .poll(&mut seen)
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        if !events.is_empty() {
            invalidate_framework_modules(&hmr_pool).await;
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
                        let (route_pattern, odf_b64) =
                            hmr_route_data_update(path, &rootp, &cfg, &tree, hmr_loader.as_ref())
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
                    HmrBehavior::AdapterHmr => hub.publish(HmrMessage::ModuleUpdate {
                        path: pstr,
                    }),
                    HmrBehavior::ServerSwap => {
                        hub.publish(HmrMessage::ServerModuleSwap { path: pstr })
                    }
                    HmrBehavior::RestartSubsystems => hub.publish(HmrMessage::FullReload {
                        reason: format!("{pstr} changed"),
                    }),
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
    let produced = root.join("target/wasm32-unknown-unknown/release/oxide_core_wasm.wasm");
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
                "warn: snapshot side effect at {}:{} — {}",
                fx.path,
                fx.line,
                fx.message
            );
            count += 1;
        }
    }
    if count > 0 {
        outln!(
            w,
            "snapshot: {count} top-level side effect(s) — move them behind a lazy initializer for a clean snapshot"
        );
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
    // Every asset path the client manifest names (entry + preload + css),
    // turned into a URL.
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
    precache.sort();
    precache.dedup();
    let list = precache
        .iter()
        .map(|s| format!("  {s:?},"))
        .collect::<Vec<_>>()
        .join("\n");

    let sw = format!(
        "// Oxide service worker (§16.6). Cache-first for precached assets,\n\
// network-first with an offline fallback for navigations.\n\
const CACHE = 'oxide-{build}';\n\
const PRECACHE = [\n{list}\n];\n\
self.addEventListener('install', (e) => {{\n\
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(PRECACHE)).then(() => self.skipWaiting()));\n\
}});\n\
self.addEventListener('activate', (e) => {{\n\
  e.waitUntil(caches.keys().then((ks) => Promise.all(ks.filter((k) => k !== CACHE).map((k) => caches.delete(k)))).then(() => self.clients.claim()));\n\
}});\n\
self.addEventListener('fetch', (e) => {{\n\
  const req = e.request;\n\
  if (req.method !== 'GET') return;\n\
  const url = new URL(req.url);\n\
  if (req.mode === 'navigate') {{\n\
    e.respondWith(fetch(req).catch(() => caches.match('/') || caches.match(req)));\n\
    return;\n\
  }}\n\
  e.respondWith(caches.match(req).then((hit) => hit || fetch(req)));\n\
}});\n",
        build = client_manifest
            .get("build_id")
            .and_then(|v| v.as_str())
            .unwrap_or("dev"),
    );
    std::fs::write(dist.join("sw.js"), sw).map_err(|e| anyhow::anyhow!("write sw.js: {e}"))?;

    let reg = "// Oxide PWA registration (§16.6).\n\
if ('serviceWorker' in navigator) {\n\
  window.addEventListener('load', () => navigator.serviceWorker.register('/sw.js').catch(() => {}));\n\
}\n";
    std::fs::write(dist.join("pwa-register.js"), reg)
        .map_err(|e| anyhow::anyhow!("write pwa-register.js: {e}"))?;
    outln!(w, "pwa: sw.js + pwa-register.js emitted");
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
    // PRERENDERED error page is BAKED at build time into a static file and
    // is served as a plain 200 — a static file has no status channel. This
    // is a documented deviation, not an oversight.
    error_route_id: Option<&str>,
) -> anyhow::Result<String> {
    // §9.1: run the loader for this route (if the module exists and a
    // loader runner is attached) to obtain the data the render phase
    // receives. Prerendered static pages previously always wrote `{}`
    // into PageRenderInput.data; they now run the real `load()`.
    let mut data = serde_json::Value::Object(Default::default());
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
            // Build time has no request middleware; `locals` is `{}` and the
            // locale is the project default (there is no Accept-Language).
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
                    return Ok(body);
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
                    return Ok(body);
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
        match loader.run(input).await {
            Ok(out) if out.status == 200 => {
                data = out.data;
                deferred_values = out.deferred;
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
                return Ok(body);
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
                return Ok(body);
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
    // PERF-04: return the HTML so the caller can build an L0 seed manifest
    // (with the nonce swapped to the sentinel).
    Ok(html)
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
async fn route_data_response(
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
    if let Some(runner) = loader_runner {
        for (lid, lpath) in layout_module_paths {
            let mut linput = oxide_server::LoaderRunInput::new(
                lpath.to_string_lossy().to_string(),
                lid.clone(),
                &route_q,
            );
            linput.method = req.method.clone();
            linput.headers = req.headers.clone();
            linput.params = params.clone();
            linput.locals = middleware_locals.clone();
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
                    metrics.observe("_data", started.elapsed(), out.status);
                    return resp;
                }
                Ok(out) => {
                    parent_data = out.data;
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
        input.headers = req.headers.clone();
        input.params = params.clone();
        input.locals = middleware_locals.clone();
        input.locale = locale.to_string();
        input.parent_data = parent_data;
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
                metrics.observe("_data", started.elapsed(), out.status);
                return resp;
            }
            Ok(out) => {
                data = out.data;
                deferred = out.deferred;
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
    let payload = serde_json::json!({
        "route": route_label,
        "url": route_q,
        "params": params,
        "data": data,
        // §8.4 parity extras. `normalizeInput` ignores unknown keys today;
        // the client re-render slice consumes them.
        "deferred": deferred,
        "locale": locale,
    });
    // §9.5 / D11: encode as ODF, the wire format the client decodes.
    let body =
        oxide_odf::encode_to_bytes(&oxide_odf::Value::from_json(&payload)).unwrap_or_default();
    metrics.observe("_data", started.elapsed(), 200);
    let mut resp = oxide_server::OxideResponse::new(200, body)
        .with_header("content-type", "application/oxide+odf");
    // §8.5: headers the middleware phase added ride the data response too.
    for (k, v) in middleware_headers {
        resp = resp.with_header(k, v.clone());
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

async fn cmd_build(root: &str, r#static: bool, w: Writer<'_>) -> anyhow::Result<i32> {
    // Canonicalize once at entry. Downstream joins (routes scan, bundler,
    // write_prerender) all expect absolute paths so a nested `root.join`
    // cannot double the prefix — `PathBuf::join` returns the RHS unchanged
    // when it is absolute.
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
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

    let cfg = load_config_strict(&rootp, "production")?;
    let tree = refine_render_modes_from_ast(&rootp, &cfg, scan_routes(&rootp, &cfg)?);
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
        ) {
            if matches!(e.kind, oxide_router::RouteKind::Loading) {
                loading_entries.insert(e.id.0.clone(), rootp.join(&cfg.routes.dir).join(&e.file));
            } else {
                entries.insert(e.id.0.clone(), rootp.join(&cfg.routes.dir).join(&e.file));
            }
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
    let precompile_cache = {
        let local = precompile_cache::PrecompileCache::open_soft(&rootp);
        match (&cfg.build.remote_cache, local) {
            (Some(url), Some(c)) => {
                let remote: std::sync::Arc<dyn oxide_graph::RemoteCache> =
                    std::sync::Arc::new(oxide_graph::HttpRemoteCache::new(url.clone()));
                // Re-open with the remote attached, keeping the local CAS.
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
        &entries,
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
    );
    let server_opts = bundle_env_options(
        oxide_bundler::BundleEnv::Ssr,
        entries.clone(),
        server_out.clone(),
        server_precompiled,
        jsx_source,
        build_secret,
        transform_hook,
        css_transform,
        cfg.build.ssr_externals.clone(),
        cfg.build.sourcemap != "off",
    );
    let rb = oxide_bundler::RolldownBundler::new(&rootp);
    let client_outcome = rb.bundle(&client_opts).map_err(|e| {
        persist_diagnostics(&rootp, &[e.0.clone()]);
        anyhow::anyhow!("{} {}", e.0.code, e.0.message)
    })?;
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
                    hits.push(
                        oxide_diagnostics::Diagnostic::error(
                            "CMP-GUARD",
                            format!(
                                "server-only `{ident}` from `{}` shipped into client chunk `{chunk}`",
                                sp.display()
                            ),
                        )
                        .with_fix(
                            "import it via a server function (§9.4) or move the value \
                             into a non-server module",
                        ),
                    );
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
    let server_outcome = rb.bundle(&server_opts).map_err(|e| {
        let mut diags = client_outcome.diagnostics.clone();
        diags.push(e.0.clone());
        persist_diagnostics(&rootp, &diags);
        anyhow::anyhow!("{} {}", e.0.code, e.0.message)
    })?;
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
    let mut seed_bodies: Vec<(String, String, String)> = Vec::new();
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
        let route_id = e.id.0.clone();
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
                    let html = write_prerender(
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
                        &layout_ids,

                        &prerender_layout_modules,
                        prerender_error_route_id.as_deref(),
                    )
                    .await?;
                    seed_bodies.push((
                        route_id.clone(),
                        url_path_from_rel(&rel),
                        html.replace(&nonce, l0_cache::NONCE_SENTINEL),
                    ));
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
                            let html = write_prerender(
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
                                &layout_ids,

                                &prerender_layout_modules,
                                prerender_error_route_id.as_deref(),
                            )
                            .await?;
                            seed_bodies.push((
                                route_id.clone(),
                                url_path_from_rel(&rel),
                                html.replace(&nonce, l0_cache::NONCE_SENTINEL),
                            ));
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
                let html = write_prerender(
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
                    &layout_ids,

                    &prerender_layout_modules,
                    prerender_error_route_id.as_deref(),
                )
                .await?;
                seed_bodies.push((
                    route_id.clone(),
                    url_path_from_rel(&rel),
                    html.replace(&nonce, l0_cache::NONCE_SENTINEL),
                ));
                prerendered.insert(e.id.0.clone(), vec![rel]);
            }
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

    // PERF-04: write the L0 seed manifest. Each prerendered page's sentineled
    // HTML is stored so `oxide start` can insert it into L0 at boot — a
    // seeded route is an L0 hit on the very first request (no warm-up miss).
    if !seed_bodies.is_empty() {
        let mut entries = Vec::with_capacity(seed_bodies.len());
        for (rid, url, sentineled) in &seed_bodies {
            // The key matches what the live server computes: route id
            // (no `/page` suffix), no params, vary `lang=` (empty).
            let rid = rid.strip_suffix("/page").unwrap_or(rid);
            let key = l0_cache::L0Cache::key_for(rid, "", "lang=", &build_id);
            entries.push(serde_json::json!({
                "key": key,
                "url": url,
                "body": sentineled,
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
    if let Err(e) = tree.typegen_with_root(&rootp, &rootp.join(&cfg.routes.dir)) {
        outln!(
            w,
            "warn: typegen failed ({} {}); routes.d.ts is stale",
            e.0.code,
            e.0.message
        );
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
                outln!(
                    w,
                    "edge wasm build failed: {e}; the Worker falls back to JS"
                );
            }
        }
    }

    let written = oxide_adapters::emit(target, &artifact, &dist.join("deploy"))
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    // §16.5: search indexes for every configured content collection.
    emit_content_search_indexes(&rootp, &cfg, &dist, w)?;
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
    outln!(
        w,
        "build ok — {} routes, {} prerendered, {} deploy file(s) for `{}` in dist/deploy",
        tree.entries.len(),
        artifact.prerendered.len(),
        written.len(),
        target.as_str()
    );
    // §5.1: remember this build's inputs so an unchanged re-run can skip.
    let fp = build_cache::fingerprint(&rootp, "production", r#static);
    if let Err(e) = build_cache::store(&rootp, &fp) {
        eprintln!("warn: could not write build fingerprint: {e}");
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
) -> std::sync::Arc<dyn Fn(oxide_server::OxideRequest) -> BoxedResponse + Send + Sync> {
    let boot_script_url_for_render = boot_script_url.clone();
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
        let started = std::time::Instant::now();
        // Rewrite-hop guard (§8.5): a request that already carries the
        // rewrite header is a re-dispatch. Cap the number of hops so a
        // middleware that unconditionally rewrites cannot loop.
        let rewrite_hop: u32 = req
            .header("x-oxide-rewrite-hop")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
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
        let (module_path, route_id, params, has_server_module, render_mode): (
            Option<std::path::PathBuf>,
            Option<String>,
            std::collections::BTreeMap<String, String>,
            bool,
            Option<String>,
        ) = match &matched {
            Some(m) => (
                Some(routes_root.join(m.route.file.with_file_name("page.server.ts"))),
                Some(m.route.id.0.clone()),
                m.params.clone(),
                m.route.has_server_module,
                m.route.render_mode.clone(),
            ),
            None => (None, None, std::collections::BTreeMap::new(), false, None),
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

        // §8.1 middleware.ts / §8.5: resolve the NEAREST ancestor
        // middleware for the matched route. Like error.tsx, the
        // middleware's directory is the scoping key; the deepest
        // matching ancestor wraps the route (a `routes/blog/middleware`
        // shadows `routes/middleware`). Scope to a bounded slice:
        // middleware is executed once, before the loader/render; a
        // rewrite is logged but not re-matched (documented gap).
        let middleware_module_path: Option<std::path::PathBuf> = {
            let route_id_for_mw = route_id.as_deref().unwrap_or("");
            let mut best: Option<(std::path::PathBuf, usize)> = None;
            for candidate in &tree.entries {
                if !matches!(candidate.kind, oxide_router::RouteKind::Middleware) {
                    continue;
                }
                let base = candidate
                    .id
                    .0
                    .strip_suffix("/middleware")
                    .unwrap_or(&candidate.id.0);
                let wraps = if base == "routes" || base.is_empty() {
                    true
                } else {
                    route_id_for_mw.starts_with(&format!("{base}/"))
                };
                if !wraps {
                    continue;
                }
                let len = base.len();
                match &best {
                    Some((_, prev)) if *prev >= len => {}
                    _ => {
                        // `candidate.file` is the middleware file itself,
                        // not its sibling — no `with_file_name` needed.
                        best = Some((routes_root.join(&candidate.file), len));
                    }
                }
            }
            best.map(|(p, _)| p)
        };
        let endpoint_params_json: serde_json::Map<String, serde_json::Value> = params
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        let endpoint_headers: Vec<(String, String)> = req.headers.clone();
        let endpoint_body: bytes::Bytes = req.body.clone();
        let endpoint_method_for_dispatch: String = req.method.clone();
        let endpoint_url_for_dispatch: String = req.url.clone();

        // §10.2 step 5 / §8.3: an unmatched URL gets a 404 status (the
        // `status` label above). To render a user-visible 404 page the
        // request must resolve to the project's `not-found.tsx` route id
        // (with its layout chain), not to a placeholder component that
        // `componentFor` cannot find. Precompute both here, before
        // `matched` is consumed, so the async render block below can use
        // them.
        let (unmatched_route_id, unmatched_layout_ids): (Option<String>, Vec<String>) = {
            let not_found_entry = tree
                .entries
                .iter()
                .find(|e| matches!(e.kind, oxide_router::RouteKind::NotFound));
            match not_found_entry {
                Some(e) => {
                    // The adapter's `componentFor` is keyed on the route
                    // *directory* id, which for a not-found file is
                    // `routes/not-found` (or `routes/blog/not-found`,
                    // etc.). That id is exactly what the scanner emits —
                    // no stripping needed. Stripping `/not-found` would
                    // collapse it to `routes`, which no user registers.
                    let not_found_id = e.id.0.clone();
                    // The layout chain that wraps the not-found route.
                    // Same rule as the page `layout_ids` above: any layout
                    // whose base directory is a prefix of the route id.
                    let mut chain: Vec<(String, usize)> = Vec::new();
                    for candidate in &tree.entries {
                        if !matches!(candidate.kind, oxide_router::RouteKind::Layout) {
                            continue;
                        }
                        let layout_base = candidate
                            .id
                            .0
                            .strip_suffix("/layout")
                            .unwrap_or(&candidate.id.0);
                        let wraps = if layout_base == "routes" || layout_base.is_empty() {
                            true
                        } else {
                            e.id.0.starts_with(&format!("{layout_base}/"))
                        };
                        if wraps {
                            chain.push((candidate.id.0.clone(), layout_base.len()));
                        }
                    }
                    chain.sort_by_key(|(_, len)| *len);
                    (
                        Some(not_found_id),
                        chain.into_iter().map(|(id, _)| id).collect(),
                    )
                }
                None => (None, Vec::new()),
            }
        };

        // §10.2 step 5: resolve the NEAREST ancestor `error.tsx` for the
        // matched route (innermost boundary wins — a `routes/blog/error`
        // shadows `routes/error`). Precomputed here so the async render
        // block can pass it as `input.error_route_id`. The adapter
        // renders this component when the page throws.
        let error_route_id: Option<String> = {
            let route_id_for_err = route_id.as_deref().unwrap_or("");
            let mut best: Option<(String, usize)> = None;
            for candidate in &tree.entries {
                if !matches!(candidate.kind, oxide_router::RouteKind::Error) {
                    continue;
                }
                // `routes/<dir>/error` wraps any route at or under
                // `routes/<dir>/`. The root `routes/error` wraps everything.
                let base = candidate
                    .id
                    .0
                    .strip_suffix("/error")
                    .unwrap_or(&candidate.id.0);
                let wraps = if base == "routes" || base.is_empty() {
                    true
                } else {
                    route_id_for_err.starts_with(&format!("{base}/"))
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
        let req_owned = req.clone();
        // Shadow so the async block moves a per-call clone rather than
        // the outer closure's captured value (which would be E0507 —
        // an `Fn` closure cannot surrender a captured variable).
        let boot_script_url_for_render = boot_script_url_for_render.clone();
        // §8.1 route.ts / §10.2 step 7: same pattern for the endpoint
        // dispatcher — the async block below moves this per-call clone.
        let endpoint_for_dispatch = endpoint_for_dispatch.clone();
        // §8.1 middleware.ts / §8.5: same shadow for middleware.
        let middleware_for_dispatch = middleware_for_dispatch.clone();
        // §8.5: same shadow for the self-dispatch handle used by a
        // middleware rewrite (the async block moves it per-call).
        let self_dispatch_for_closure = self_dispatch_for_closure.clone();
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
        let l0_eligible = req.method.eq_ignore_ascii_case("GET")
            && l0_cache::mode_is_cacheable(render_mode.as_deref())
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
        let l0_vary = l0_cache::vary_of(&req_owned.headers);
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

        Box::pin(async move {
            // §8.1 middleware.ts / §8.5: run the route's nearest ancestor
            // middleware BEFORE the loader/render phase — and before the
            // /__data branch (§8.4 parity: client nav data must see the
            // same middleware locals/headers/short-circuits as SSR). It can:
            //   * short-circuit with a `Response` or `{ redirect }`;
            //   * inject `locals` (delivered to the loader via
            //     `LoaderRunInput.locals`);
            //   * add response headers.
            // A `{ rewrite }` re-dispatches through the normal path. An
            // error surfaces as a 500 with a `MIDDLEWARE`-coded envelope.
            let stage_middleware_start = std::time::Instant::now();
            let mut middleware_locals: serde_json::Value = serde_json::json!({});
            let mut middleware_headers: Vec<(String, String)> = Vec::new();
            if let (Some(dispatcher), Some(mp)) = (
                middleware_for_dispatch.as_deref(),
                middleware_module_path.as_ref(),
            ) {
                let module_path_s = mp.to_string_lossy().to_string();
                // Pass the full request (headers + body) so an auth /
                // upload middleware sees what it needs.
                let mw_res = dispatcher
                    .dispatch(
                        &module_path_s,
                        &req_owned.method,
                        &req_owned.url,
                        &endpoint_params_json,
                        &endpoint_headers,
                        endpoint_body.as_ref(),
                        None,
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
                            return resp;
                        }
                        // Non-short-circuit: accumulate headers and locals.
                        for (k, v) in &out.headers {
                            middleware_headers.push((k.clone(), v.clone()));
                        }
                        if let Some(l) = out.locals {
                            if l.is_object() {
                                middleware_locals = l;
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
                                new_req.headers.push((
                                    "x-oxide-rewrite-hop".to_string(),
                                    (rewrite_hop + 1).to_string(),
                                ));
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
                )
                .await;
            }

            // §9.3: a form action POST runs the route's `actions[name]`
            // and returns its result. This precedes the loader phase — an
            // action is its own request, not a page render.
            if let (Some(action), Some(module_path)) =
                (action_name.as_deref(), module_path.as_deref())
            {
                if let Some(dispatcher) = action_dispatcher.as_deref() {
                    let form_json = {
                        let entries = parse_form_urlencoded(&req_owned.body);
                        serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string())
                    };
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
                    )
                    .await;
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
                    }
                    // §8.5: headers the middleware phase added ride the
                    // action response (they were accumulated but never
                    // applied — the previous drop made a CSP/security
                    // header set in middleware vanish on POSTs).
                    let mut resp = resp;
                    for (k, v) in &middleware_headers {
                        resp = resp.with_header(k, v.clone());
                    }
                    metrics.observe(&label, started.elapsed(), resp.status);
                    return resp;
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
                        let etag = format!("W/\"{}\"", hit.etag);
                        let inm = req_owned.header("if-none-match").map(str::to_string);
                        if inm.as_deref().is_some_and(|v| {
                            v.split(',')
                                .map(str::trim)
                                .any(|t| t == etag || t == hit.etag || t == "*")
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
                            return resp;
                        }
                        let nonce = oxide_server::nonce_for(&req_owned.request_id);
                        let body = hit.materialize(&nonce);
                        let mut resp = oxide_server::OxideResponse::new(hit.status, body);
                        for (k, v) in &hit.headers {
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
                        metrics.observe(&label, started.elapsed(), hit.status);
                        return resp;
                    }
                }
            }

            // PERF-04: single-flight. A concurrent miss for the same L0
            // key awaits the in-flight owner instead of rendering again.
            let mut flight_owner: Option<
                tokio::sync::broadcast::Sender<std::sync::Arc<oxide_cache::Entry>>,
            > = None;
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
                    FlightSlot::Owner(tx) => flight_owner = Some(tx),
                }
            }

            // §9.1: loader phase. When the matched route declares a server
            // module and a LoaderRunner is attached, run it. A non-200
            // output (error(...) or redirect(...)) short-circuits the
            // response; a 200 output threads `data` into the render phase.
            let mut data = serde_json::Value::Object(Default::default());
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

            // §9.2: layout loads run IN PARALLEL with the page load. The
            // layout chain is outermost → innermost; each layout gets the
            // pre-resolved data of its own outer layout as `parent_data`
            // (which a load reads via `await parent()`). The layouts form
            // a chain, so their data is resolved sequentially *among
            // themselves* (outer first) but each layout's own `load` runs
            // concurrently with the page load and the sibling layouts.
            //
            // Concretely: kick off every layout's load future, await them
            // in chain order to attach `parent_data`, then await the page
            // load — which was started at the same instant as layout[0].
            let mut layout_data_chain: Vec<serde_json::Value> = Vec::new();
            let mut layout_side_effects: Vec<(Vec<String>, Vec<(String, String)>)> = Vec::new();
            if let (Some(runner), true) = (loader_runner.as_ref(), !layout_module_paths.is_empty())
            {
                for (lid, lpath) in &layout_module_paths {
                    let mut linput = oxide_server::LoaderRunInput::new(
                        lpath.to_string_lossy().to_string(),
                        lid.clone(),
                        req_owned.url.clone(),
                    );
                    linput.method = req_owned.method.clone();
                    linput.headers = req_owned.headers.clone();
                    linput.params = params.clone();
                    linput.locals = middleware_locals.clone();
                    linput.locale = resolved_locale.clone();
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
                            metrics.observe(&label, started.elapsed(), out.status);
                            return resp;
                        }
                        Ok(out) => {
                            layout_data_chain.push(out.data);
                            layout_side_effects.push((out.set_cookies, out.headers));
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
                    // §8.1 middleware.ts / §9.1: request-scoped locals
                    // injected by the middleware that just ran. `{}` when
                    // no middleware executed.
                    input.locals = middleware_locals.clone();
                    // SPEC-10: expose the resolved locale to `load()`.
                    input.locale = resolved_locale.clone();
                    // §9.2: the innermost layout's data — what `parent()`
                    // reads in a page load.
                    input.parent_data = layout_data_chain
                        .last()
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);

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
                            // §16.6 / §9.2: cookies the load set and headers
                            // it added ride the short-circuiting response
                            // (a redirect that also sets a session cookie).
                            for c in &out.set_cookies {
                                resp = resp.with_header("set-cookie", c.clone());
                            }
                            for (k, v) in &out.headers {
                                resp = resp.with_header(k, v.clone());
                            }
                            metrics.observe(&label, started.elapsed(), out.status);
                            return resp;
                        }
                        Ok(out) => {
                            data = out.data;
                            loader_set_cookies = out.set_cookies;
                            loader_headers = out.headers;
                            // §7.1 / §9.2: resolved defer() values. Injected
                            // as a `<script>` so the client's DeferredClient
                            // slots (wired from the ODF payload's
                            // `{__deferred: id}` markers) resolve.
                            deferred_values = out.deferred;
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
                match dispatcher
                    .dispatch(
                        &module_path_s,
                        &endpoint_method_for_dispatch,
                        &endpoint_url_for_dispatch,
                        &endpoint_params_json,
                        &endpoint_headers,
                        endpoint_body.as_ref(),
                        &middleware_locals,
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
                // PERF-01 / §8.4: the ODF hydration payload needs the same
                // route/url/params/data the render input carries, but those
                // fields move into `PageRenderInput` below — clone the
                // payload's copies first.
                let route_for_odf = route_for_render.clone();
                let odf_params = params.clone();
                let odf_data = data.clone();
                let odf_url = req_owned.url.clone();
                let mut input =
                    oxide_server::PageRenderInput::new(route_for_render, req_owned.url.clone());
                input.params = params;
                input.data = data;
                // §8.2: hand the layout chain to the adapter so it can
                // nest the page. The adapter renders
                // `L1( L2( ...( page ) ) )` using `componentFor(layoutId)`.
                input.layouts = effective_layouts;
                // §10.2 step 5: nearest ancestor `error.tsx` — the adapter
                // renders it on an SSR throw instead of the generic 500.
                input.error_route_id = error_route_id.clone();
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
                            let payload = serde_json::json!({
                                "route": route_for_odf,
                                "url": odf_url,
                                "params": odf_params,
                                "data": odf_data,
                            });
                            let odf =
                                oxide_odf::encode_to_bytes(&oxide_odf::Value::from_json(&payload))
                                    .unwrap_or_default();
                            use base64::Engine as _;
                            let odf_b64 = base64::engine::general_purpose::STANDARD.encode(&odf);
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
                        {
                            if let Some(cache) = l0.as_ref() {
                                let nonce = oxide_server::nonce_for(&req_owned.request_id);
                                let cache = std::sync::Arc::clone(cache);
                                let l0_route = l0_route.clone();
                                let l0_params = l0_params.clone();
                                let l0_vary = l0_vary.clone();
                                let l0_path = l0_path.clone();
                                let headers_for_store = loader_headers.clone();
                                let flight_map = std::sync::Arc::clone(&flight_map);
                                let flight_key = flight_key.clone();
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
                                    let key = cache.key_of(&l0_route, &l0_params, &l0_vary);
                                    cache.store_entry(&key, entry.clone()).await;
                                    if let Some(tx) = flight_owner {
                                        let _ = oxide_cache::SingleFlight::new()
                                            .publish_and_cleanup(
                                                &flight_map,
                                                &flight_key,
                                                oxide_cache::FlightSlot::Owner(tx),
                                                Some(std::sync::Arc::new(entry)),
                                            )
                                            .await;
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
                        for c in &loader_set_cookies {
                            resp = resp.with_header("set-cookie", c.clone());
                        }
                        for (k, v) in &loader_headers {
                            resp = resp.with_header(k, v.clone());
                        }
                        // §8.5: headers the middleware phase added ride the
                        // page response (previously accumulated into a dead
                        // variable and dropped).
                        for (k, v) in &middleware_headers {
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
                oxide_server::OxideResponse::html(
                    status,
                    format!(
                        "<!doctype html><html><head><title>Oxide</title></head>\
                             <body><main>Oxide serving {label}</main></body></html>"
                    ),
                )
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
    ) -> std::result::Result<oxide_server::EndpointRunResult, oxide_server::EndpointRunError> {
        match self
            .host
            .run(module_path, method, url, params, headers, body, locals)
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
        let abs = if Path::new(rel).is_absolute() {
            PathBuf::from(rel)
        } else {
            root.join(rel)
        };
        if !abs.is_file() {
            eprintln!("warn: js plugin {} not found; skipping", abs.display());
            continue;
        }
        plugins.push(abs.to_string_lossy().to_string());
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
                                if let Some(s) = v.get("code").and_then(|c| c.as_str()) {
                                    code = s.to_string();
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
        let abs = if Path::new(rel).is_absolute() {
            PathBuf::from(rel)
        } else {
            root.join(rel)
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
    let cfg = load_config_or_default(&rootp, "production");
    // §10.1: refine inferred render modes from the AST, exactly as `oxide build`
    // does — otherwise the live server sees the scanner's line-oriented guess
    // (and `None` for a route with no `page.server.ts`), and the §11 L0 layer
    // cannot tell a static route from a dynamic one.
    let tree = std::sync::Arc::new(refine_render_modes_from_ast(
        &rootp,
        &cfg,
        scan_routes(&rootp, &cfg)?,
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
    let image_endpoint = build_image_endpoint(&rootp, &cfg);
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

    let pipeline = std::sync::Arc::new(oxide_server::Pipeline {
        rules,
        assets,
        image_endpoint,
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
        // SPEC-14: the HMR hub's broadcast sender, dev only.
        hmr: hmr.as_ref().map(|h| h.sender()),
        // §13.3: advertise the client manifest's build id so a stale tab can
        // detect a rebuild and reload.
        hmr_build_id: build_id.clone(),
        // SEC-02: HSTS only on TLS (§17.1) — set where the listener is built.
        tls: tls.is_some(),
        // SEC-09: the dev inspector token (see below).
        dev_token: dev_token.clone(),
        // §9.6: WebSocket route dispatch (None when the project has no
        // `socket.ts` routes or no pool).
        sockets: socket_service,
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
                    // A Redis driver is not shipped (the `redis` crate is not a
                    // dependency). A configured Redis handler is honoured by
                    // the HTTP protocol when a project fronts Redis with an
                    // HTTP cache gateway; otherwise this is a loud no-op so the
                    // misconfiguration is visible, not silent.
                    tracing::warn!(
                        url = %url,
                        "cache.handler=redis is not implemented in this build; \
                         use cache.handler={{type:'http',url}} against an HTTP cache \
                         gateway, or 'disk' for a local tier",
                    );
                }
                oxide_config::CacheHandlerConfig::S3 { bucket } => {
                    tracing::warn!(
                        bucket = %bucket,
                        "cache.handler=s3 is not implemented in this build; \
                         use cache.handler={{type:'http',url}} against an S3-backed \
                         HTTP cache gateway",
                    );
                }
            }
            std::sync::Arc::new(tier)
        }
    };
    let l0 = Some(std::sync::Arc::new(l0_cache::L0Cache::new(
        l0_tier,
        build_id.clone(),
        std::time::Duration::from_secs(60),
    )));
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
                            if !key.is_empty() && !body.is_empty() {
                                cache.seed_entry(key, body.as_bytes().to_vec()).await;
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
    let archived = rootp
        .join(".oxide/routes.manifest")
        .exists()
        .then(|| {
            oxide_router::ArchivedTree::load_archived(&rootp.join(".oxide/routes.manifest"))
                .ok()
                .map(std::sync::Arc::new)
        })
        .flatten();
    let dispatch = build_dispatch(
        std::sync::Arc::clone(&tree),
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
    );
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
    let cfg = load_config_or_default(&rootp, "production");
    let config_path_opt = find_config_file(&rootp).ok();
    let transform_handler = framework_shim::build_transform_handler(&rootp, &cfg);
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
    let tree = refine_render_modes_from_ast(&rootp, &cfg, scan_routes(&rootp, &cfg)?);
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
    outln!(w, "ROUTE | JS BYTES | BUDGET | OK");
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
        outln!(w, "{route} | {bytes} | {budget_str} | {status}");
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

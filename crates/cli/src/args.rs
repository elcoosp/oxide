//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

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
        /// §2.9: use the bundler path instead of the native ESM dev server
        /// (equivalent to `dev: { esm: false }` in `oxide.config.ts`).
        /// Serves `/client/*.js` from `dist/client/` instead of
        /// `/src/**` transformed modules.
        #[arg(long = "no-esm")]
        no_esm: bool,
        /// V2-F15: when `record:` is on, keep the secret header trio
        /// (`authorization`, `cookie`, `set-cookie`) verbatim in the
        /// records instead of the `sha256:<12 hex>` redaction — a replay
        /// can then exercise authenticated paths. Dev-only; the production
        /// stack never reads it.
        #[arg(long)]
        include_secrets: bool,
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
        /// V2-R02 (spec §V2-R02): print the chosen target's §18 capability
        /// matrix row at the end of the build — which capabilities are
        /// Full, which Degraded (with the documented degradation string),
        /// which Unsupported.
        #[arg(long)]
        explain: bool,
        #[arg(long)]
        mode: Option<String>,
        /// §17.2: deny outbound network from plugin/tooling workers.
        #[arg(long)]
        offline_plugins: bool,
        /// §V2-F20: write `dist/build-id` and `dist/attest/` (the
        /// CycloneDX SBOM, the in-toto provenance and its ed25519
        /// signature) for the dist this build emitted.
        #[arg(long)]
        attest: bool,
        /// §V2-F20: the ed25519 signing key for `--attest` (PKCS#8 PEM or
        /// DER). Overrides `OXIDE_SIGNING_KEY`; the build fails without a
        /// key rather than attesting unsigned.
        #[arg(long)]
        key: Option<String>,
    },
    /// Start the production server (binary target, §12).
    Start {
        #[arg(long, default_value = ".")]
        root: String,
        #[arg(long)]
        port: Option<u16>,
        /// PERF-23 (readiness): skip the renderer warm-up. Warm-up costs ~1 s
        /// of startup (a synthetic render) so the first REAL request never
        /// pays the cold path; short-lived benchmark/CI runs and
        /// pre-scaled fleets use this to boot instantly.
        #[arg(long = "no-warmup")]
        no_warmup: bool,
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
    /// Format with `oxfmt`, falling back to `biome format` when absent
    /// (§V2-C11). No config key — the formatter is a CLI concern only.
    Fmt {
        #[arg(long, default_value = ".")]
        root: String,
        /// Check only: report files whose formatting differs and exit 1
        /// instead of writing.
        #[arg(long)]
        check: bool,
    },
    /// Explain importers and chunks of a module (§13.4), or — with
    /// `--route` — why a route is dynamic, with OX-DYN diagnostics (§V2-F16).
    Why {
        #[arg(long, default_value = ".")]
        root: String,
        /// Module path whose importers are reported (the §13.4 surface).
        module: Option<String>,
        /// Route id (e.g. `routes/dashboard`) whose dynamic reads are
        /// reported (the §V2-F16 surface). Mutually exclusive with the
        /// module argument.
        #[arg(long)]
        route: Option<String>,
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
    /// Migrate to oxide from a detected source framework (V2-E03): a dry
    /// run prints the plan; `--apply` performs the mechanical halves
    /// (renames + import rewrites) and requires a clean git tree unless
    /// `--force`. The framework is detected from the project's manifest
    /// and config signatures; an explicit `source` must match it.
    Migrate {
        #[arg(long, default_value = ".")]
        root: String,
        /// The expected source framework (`sveltekit|next|nuxt|vite`) —
        /// optional; given, it must match the detection.
        source: Option<String>,
        /// Apply the plan's mechanical halves (renames + import
        /// rewrites). A dry run never writes.
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Apply even when the git tree is dirty (the clean-tree gate
        /// exists so a half-migration can be reverted).
        #[arg(long, default_value_t = false)]
        force: bool,
        /// The pre-E03 spelling of `--apply` (kept as a hidden alias).
        #[arg(long = "write", hide = true, default_value_t = false)]
        write: bool,
    },
    /// V2-F15: re-execute a captured request deterministically and compare
    /// against its record (`.oxide/records/<id>.json`).
    Replay {
        #[arg(long, default_value = ".")]
        root: String,
        /// The request id the record is stored under (the file stem).
        id: String,
        /// Compare the replayed response against the recorded one
        /// (nonce-normalized).
        #[arg(long)]
        diff: bool,
        /// For agents: emit `{ ok, divergences: [..] }` and nothing else.
        #[arg(long)]
        json: bool,
    },
    /// In-process test helper: boot the app and expose `fetch` (§16.6 oxide/test).
    Test {
        #[arg(long, default_value = ".")]
        root: String,
    },
    /// MCP server over stdio (§19.3): exposes routes, cache, graph and
    /// diagnostics as MCP tools so coding agents see accurate project facts.
    /// Read-only by default; `--allow-write` enables the write tools
    /// (V2-E07, the SEC-09 CLI-flag shape).
    Mcp {
        #[arg(long, default_value = ".")]
        root: String,
        /// Enable the registry's write tools (`build.run`). Read-only by
        /// default — a write tool called without this flag is refused with
        /// a named refusal, not a protocol error.
        #[arg(long, default_value_t = false)]
        allow_write: bool,
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
    /// Performance tooling (§V2-F18): `oxide perf bisect` hunts the commit
    /// that regressed a metric between two revisions.
    Perf {
        /// Project directory (default: cwd).
        #[arg(long, default_value = ".")]
        root: String,
        #[command(subcommand)]
        op: PerfOp,
    },
    /// Generated reference docs (§V2-E11): `oxide docs generate` writes
    /// `docs/reference/` from the crates the binary links — the §18
    /// capability matrix, the config schema, the clap command tree —
    /// and `--check` verifies the on-disk pages (the CI `docs-check`
    /// gate).
    Docs {
        /// Project directory (default: cwd; the pages land under
        /// `<root>/docs/reference`).
        #[arg(long, default_value = ".")]
        root: String,
        #[command(subcommand)]
        op: DocsOp,
    },
    /// Verify an attested build (§V2-F20): the canonical form of the
    /// provenance, its ed25519 signature (with `--pubkey`) and every
    /// subject digest against the dist on disk. `--rebuild` re-runs the
    /// build in a scratch copy and compares the dist digests.
    Verify {
        /// Project directory (default: cwd).
        #[arg(long, default_value = ".")]
        root: String,
        /// The dist to verify (default: `<root>/dist`).
        #[arg(long)]
        dist: Option<String>,
        /// The ed25519 verifier key (SPKI PEM, 64-char hex or 32-byte
        /// base64). Without it the signature check is skipped loudly and
        /// the digests still verify.
        #[arg(long)]
        pubkey: Option<String>,
        /// Rebuild in a scratch copy of the project and compare the dist
        /// digests (the §V2-F20 reproducibility check).
        #[arg(long)]
        rebuild: bool,
    },
}

/// `oxide cache` operations (§19.1).
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

/// `oxide perf` subcommands (§V2-F18).
#[derive(Debug, Subcommand)]
pub enum PerfOp {
    /// Bisect a performance regression: build each commit in `good..bad`
    /// (first-parent) in a git worktree under `.oxide/bisect/` and report
    /// the first one whose metric exceeds the good baseline by more than
    /// the threshold. All candidates share the project's cache via
    /// `OXIDE_CACHE_DIR` so rebuilds are incremental.
    Bisect {
        /// Known-good revision (an ancestor of `bad`).
        good: String,
        /// Known-bad revision (the descendant that shows the regression).
        bad: String,
        /// What to measure: build-ms, client-js-bytes or ttfb-ms.
        #[arg(long, value_enum, default_value = "build-ms")]
        metric: perf_bisect::Metric,
        /// Regression threshold: a non-negative percentage (`10%` or `10`).
        #[arg(long, default_value = "10%")]
        threshold: String,
        /// Measurements per candidate; the median is what compares.
        #[arg(long, default_value_t = 3)]
        runs: usize,
    },
}

/// `oxide docs` subcommands (§V2-E11).
#[derive(Debug, Subcommand)]
pub enum DocsOp {
    /// Write `docs/reference/` from the generators in [`docs_gen`] — the
    /// §18 capability matrix, the config schema, the clap command tree
    /// (deterministic bytes; the committed pages are the gate). With
    /// `--check` nothing is written: the on-disk pages are verified and
    /// drift exits 1 — the gate the CI `docs-check` job runs.
    Generate {
        /// Output directory (default: `<root>/docs/reference`).
        #[arg(long)]
        out: Option<String>,
        /// Verify the on-disk pages and exit 1 on drift, writing nothing.
        #[arg(long, default_value_t = false)]
        check: bool,
    },
}

impl Command {
    /// P1 (readiness): the `--root` argument, when this command takes one.
    /// Read by [`apply_startup_env`] so env writes happen pre-runtime.
    pub fn root_arg(&self) -> Option<&str> {
        Some(match self {
            Command::Dev { root, .. }
            | Command::Build { root, .. }
            | Command::Start { root, .. }
            | Command::Preview { root, .. }
            | Command::Routes { root, .. }
            | Command::Analyze { root, .. }
            | Command::Check { root, .. }
            | Command::Fmt { root, .. }
            | Command::Why { root, .. }
            | Command::Doctor { root, .. }
            | Command::Secrets { root, .. }
            | Command::Add { root, .. }
            | Command::Generate { root, .. }
            | Command::Cache { root, .. }
            | Command::Audit { root, .. }
            | Command::Migrate { root, .. }
            | Command::Replay { root, .. }
            | Command::Test { root, .. }
            | Command::Mcp { root, .. }
            | Command::Config { root, .. }
            | Command::Render { root, .. }
            | Command::Perf { root, .. }
            | Command::Docs { root, .. }
            | Command::Verify { root, .. } => root.as_str(),
            Command::Schema => return None,
        })
    }

    /// P1 (readiness): true for `oxide dev` — the command whose runtime
    /// behaviour dev-mode gates.
    pub fn is_dev_command(&self) -> bool {
        matches!(self, Command::Dev { .. })
    }
}

/// P1 (readiness): ALL startup-time environment writes happen HERE, in the
/// synchronous binary entry point BEFORE the tokio runtime (and its worker
/// threads) exist. Previously each `cmd_*` wrote `OXIDE_DEV` / `OXIDE_ROOT` /
/// `OXIDE_DEV_MODE` mid-async — `set_var` concurrent with a `get_var` on a
/// request thread is a data race (why edition 2024 marks `set_var` unsafe),
/// and production behaviour could flip because someone exported
/// `OXIDE_DEV_MODE` in a shell. Child worker processes still inherit the
/// variables from this parent env at spawn time.
///
/// In-process dev-mode probes answer from the atomic
/// (`oxide_fw_server::set_dev_mode`) instead of re-reading the env per request.
pub fn apply_startup_env(cli: &Cli) {
    // Dev mode is a command-level fact, decided once. `set_dev_mode` makes
    // the atomic authoritative for the process; the env var remains only for
    // child-process inheritance (loader hooks, workers).
    oxide_fw_server::set_dev_mode(cli.command.is_dev_command());
    if cli.command.is_dev_command() {
        // SAFETY: pre-runtime — see the function doc.
        unsafe { std::env::set_var("OXIDE_DEV", "1") };
        if let Command::Dev { no_esm, .. } = &cli.command
            && *no_esm
        {
            // SAFETY: pre-runtime — see the function doc.
            unsafe { std::env::set_var("OXIDE_DEV_NO_ESM", "1") };
        }
    }
    // PERF-23: `oxide start --no-warmup` skips the renderer warm-up. Read
    // once by the warm-up block in `start_server_inner_full`.
    if let Command::Start {
        no_warmup: true, ..
    } = &cli.command
    {
        // SAFETY: pre-runtime — see the function doc.
        unsafe { std::env::set_var("OXIDE_NO_WARMUP", "1") };
    }
    // §17.2: deny outbound network from plugin/tooling workers (`--offline-plugins`).
    // Read by `PoolConfig::default` for every worker pool; process-global is
    // correct — the build is one process and the flag applies to all workers.
    if let Command::Build {
        offline_plugins: true,
        ..
    } = &cli.command
    {
        // SAFETY: pre-runtime — `main` calls this before `Runtime::new`, so
        // no other thread exists to race an env read (P1 readiness).
        unsafe { std::env::set_var("OXIDE_OFFLINE_PLUGINS", "1") };
    }
    // Thread the project root to worker subprocesses so `getCollection`
    // resolves `dist/content` against the project, not the shell's cwd.
    if let Some(root) = cli.command.root_arg() {
        let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| std::path::PathBuf::from(root));
        // SAFETY: pre-runtime — see above.
        unsafe { std::env::set_var("OXIDE_ROOT", rootp.to_string_lossy().as_ref()) };
    }
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
pub async fn run_with_writer(cli: Cli, w: &mut (dyn std::io::Write + Send)) -> anyhow::Result<i32> {
    match cli.command {
        Command::Dev {
            root,
            port,
            host,
            https,
            qr,
            open,
            proxy,
            no_esm,
            include_secrets,
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
                    no_esm,
                    include_secrets,
                },
                w,
            )
            .await
        }
        Command::Build {
            root,
            r#static,
            explain,
            mode: _,
            offline_plugins: _,
            attest,
            key,
        } => {
            // P1 (readiness): `OXIDE_OFFLINE_PLUGINS` is written pre-runtime
            // by `apply_startup_env` — see `apply_startup_env` and `main.rs`.
            cmd_build(&root, r#static, explain, attest, key.as_deref(), w).await
        }
        Command::Start {
            root,
            port,
            no_warmup: _,
        } => cmd_serve(&root, port, "oxide start", w).await,
        Command::Preview { root } => cmd_serve(&root, None, "oxide preview", w).await,
        Command::Routes { root } => cmd_routes(&root, w).await,
        Command::Analyze { root } => cmd_analyze(&root, w).await,
        Command::Check { root } => cmd_check(&root, w).await,
        Command::Fmt { root, check } => cmd_fmt(&root, check, w).await,
        Command::Why {
            root,
            module,
            route,
        } => cmd_why(&root, module.as_deref(), route.as_deref(), w).await,
        Command::Doctor { root } => cmd_doctor(&root, w).await,
        Command::Secrets { root, op } => cmd_secrets(&root, op, w).await,
        Command::Add { root, module } => cmd_add(&root, &module, w).await,
        Command::Generate { root, kind, path } => cmd_generate(&root, &kind, &path, w).await,
        Command::Cache { root, op } => cmd_cache(&root, op, w).await,
        Command::Audit { root, json } => cmd_audit(&root, json, w).await,
        Command::Migrate {
            root,
            source,
            apply,
            force,
            write,
        } => cmd_migrate(&root, source.as_deref(), apply || write, force, w).await,
        Command::Replay {
            root,
            id,
            diff,
            json,
        } => cmd_replay(&root, &id, diff, json, w).await,
        Command::Test { root } => cmd_test(&root, w).await,
        Command::Mcp {
            root, allow_write, ..
        } => cmd_mcp(&root, allow_write, w).await,
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
        Command::Perf { root, op } => match op {
            PerfOp::Bisect {
                good,
                bad,
                metric,
                threshold,
                runs,
            } => cmd_perf_bisect(&root, &good, &bad, metric, &threshold, runs, w).await,
        },
        Command::Docs { root, op } => match op {
            DocsOp::Generate { out, check } => cmd_docs(&root, out.as_deref(), check, w),
        },
        Command::Verify {
            root,
            dist,
            pubkey,
            rebuild,
        } => cmd_verify(&root, dist.as_deref(), pubkey.as_deref(), rebuild, w).await,
    }
    .map_err(|e| {
        tracing::error!(error = %e, "command failed");
        e
    })
}

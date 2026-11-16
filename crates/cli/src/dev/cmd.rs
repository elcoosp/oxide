//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// [wr208] Rebuild local workspace packages whose `dist/` is older than `src/`.
/// Without this, edits to `packages/core/src/*.ts` never reach the client
/// bundle — `oxide dev` reads from `node_modules/@oxide-fw/core/dist/`,
/// not from source. Diagnostics that blamed the site's own code wasted
/// hours chasing changes that were already correct.
pub(crate) fn rebuild_stale_workspace_packages(rootp: &std::path::Path) {
    let packages_dir = rootp
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("packages"));
    let Some(packages_dir) = packages_dir else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&packages_dir) else {
        return;
    };
    let mut to_rebuild = Vec::new();
    for entry in entries.flatten() {
        let pkg = entry.path();
        if !pkg.is_dir() {
            continue;
        }
        let dist = pkg.join("dist");
        let src = pkg.join("src");
        if !src.is_dir() {
            continue;
        }
        let dist_time = dist.metadata().and_then(|m| m.modified()).ok();
        let src_time = src.metadata().and_then(|m| m.modified()).ok();
        // Three-way match: a package whose `dist/` doesn't exist at all
        // (fresh clone, `.gitignore`d build artifacts) but whose `src/` does
        // is a workspace package that has never been built. The previous
        // two-way match skipped it silently, so `oxide dev` served a stale
        // (or missing) `dist/client-hot.js` and hot.ts imports failed with
        // ERR_MODULE_NOT_FOUND.
        let needs = match (dist_time, src_time) {
            (None, Some(_)) => true,
            (Some(dt), Some(st)) => st > dt,
            _ => false,
        };
        if needs {
            to_rebuild.push(pkg);
        }
    }
    if to_rebuild.is_empty() {
        return;
    }
    eprintln!(
        "oxide: rebuilding {} stale workspace package(s)...",
        to_rebuild.len()
    );
    // Package build scripts call `oxide` (`packages/oxide-website`'s build is
    // `oxide build`). On a machine without a global `oxide` install — a fresh
    // clone, CI, this repo's own dev container — that resolution failed with
    // ENOENT, the rebuild silently no-oped, and the package stayed stale
    // forever: every subsequent boot retried the rebuild, and the retry's
    // node_modules churn flooded any connected HMR client with full-reload
    // frames. Prepend the running binary's directory so `oxide` resolves to
    // the exact binary that is serving right now.
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let mut paths: Vec<std::path::PathBuf> = exe_dir.into_iter().collect();
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path_env = std::env::join_paths(&paths)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| std::env::var("PATH").unwrap_or_default());
    for pkg in &to_rebuild {
        let name = pkg.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        eprintln!("  · packages/{}", name);
        let _ = std::process::Command::new("pnpm")
            .args(["build"])
            .current_dir(pkg)
            .env("PATH", &path_env)
            .status();
    }
}

/// §1.2 (diagnostics): when `OXIDE_DEV_LOG_FILE` is set, tee everything the
/// dev server would print — tracing output AND Rust panics — into that file.
/// `%p` in the value expands to the process id, so a test harness that spawns
/// several `oxide dev` children gets one log per server
/// (`OXIDE_DEV_LOG_FILE=/tmp/logs/dev-%p.log`). Panic capture is the point:
/// a panic in a watcher/worker thread kills the process without anything
/// reading the stdio pipes being able to attribute the death — the pipe
/// buffers belong to the test process and its in-memory tail is dropped when
/// the test itself fails. A per-server file survives.
pub(crate) fn install_dev_log_file() {
    use std::io::Write as _;
    use std::sync::{Arc, Mutex};

    let raw = match std::env::var("OXIDE_DEV_LOG_FILE") {
        Ok(v) if !v.is_empty() => v,
        _ => return,
    };
    let path = raw.replace("%p", &std::process::id().to_string());
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        eprintln!("warn: OXIDE_DEV_LOG_FILE={path} is not writable; continuing without it");
        return;
    };
    let sink = Arc::new(Mutex::new(file));

    // Panic hook: write the panic message + location to the file, then chain
    // to the default hook so stderr still shows it.
    let default_hook = std::panic::take_hook();
    let panic_sink = Arc::clone(&sink);
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut f) = panic_sink.lock() {
            let loc = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "<unknown>".to_string());
            let msg = info.payload();
            let msg = msg
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| msg.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic payload>".to_string());
            let backtrace = std::backtrace::Backtrace::force_capture();
            let _ = writeln!(
                f,
                "\n===== PANIC at {loc} =====\n{msg}\nbacktrace:\n{backtrace}\n====="
            );
            let _ = f.flush();
        }
        default_hook(info);
    }));

    // Tracing layer: every tracing event also lands in the file.
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(move || {
            struct SinkWriter(Arc<Mutex<std::fs::File>>);
            impl std::io::Write for SinkWriter {
                fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                    let mut f = self.0.lock().map_err(|_| std::io::ErrorKind::Other)?;
                    f.write(buf)
                }
                fn flush(&mut self) -> std::io::Result<()> {
                    let mut f = self.0.lock().map_err(|_| std::io::ErrorKind::Other)?;
                    f.flush()
                }
            }
            SinkWriter(Arc::clone(&sink))
        });
    // The subscriber is initialized later (see `init_tracing`); stash the
    // layer via a global so the real init can pick it up. To keep this
    // self-contained, initialize a dedicated global default here — calls
    // that initialize their own subscriber later will override it, which is
    // acceptable: the panic hook above is the critical capture path.
    use tracing_subscriber::layer::SubscriberExt as _;
    let _ =
        tracing::subscriber::set_global_default(tracing_subscriber::registry().with(file_layer));
    eprintln!("oxide dev: logging to {path} (OXIDE_DEV_LOG_FILE)");
}

pub(crate) async fn cmd_dev(
    root: &str,
    once: bool,
    flags: DevFlags,
    w: Writer<'_>,
) -> anyhow::Result<i32> {
    install_dev_log_file();
    // V2-F15: the `--include-secrets` toggle rides the env the way the
    // other dev toggles do (`OXIDE_DEV_NO_ESM`) — the stack builder reads
    // it where the recorder capture is built. Only `dev` sets it; the
    // production stack never does, so a record made in prod keeps the
    // secret trio redacted no matter what a dev flag said.
    if flags.include_secrets {
        // SAFETY: single-threaded command startup, before any worker spawn
        // reads the value (the same window every dev toggle here uses).
        unsafe { std::env::set_var("OXIDE_INCLUDE_SECRETS", "1") };
    }
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let mut cfg = load_config_or_default(&rootp, "development");
    // §2.9: `--no-esm` overrides the config after load. The dev server
    // also re-loads cfg inside `start_server_inner_full`, so the env var
    // is set here and re-read after the reload (below).
    if flags.no_esm {
        cfg.dev.esm = false;
        // P1 (readiness): `OXIDE_DEV_NO_ESM` is written pre-runtime by
        // `apply_startup_env` (the config reload below re-reads it).
        tracing::info!("oxide dev: --no-esm — using bundler path");
    }
    // SPEC-15.1 / §15.2: fail at load for an unknown `kind:"native"` plugin.
    if let Err(e) = crate::native_plugins::parse_native_plugins(&cfg) {
        anyhow::bail!("{} {}", e.0.code, e.0.message);
    }
    let tree = scan_routes_with_plugins(&rootp, &cfg).await?;
    // §16.5: emit content entries before the server starts so a dev loader's
    // first `getCollection` call works without a prior `oxide build`.
    emit_content_search_indexes(&rootp, &cfg, &rootp.join("dist"), w)?;
    // V2-F09: emit the built-in search index at boot too, so
    // `/_oxide/search` answers without a prior `oxide build` — the same
    // posture the projection's boot emission documents (the CAS marker
    // keeps a warm restart a no-op).
    #[cfg(feature = "search")]
    emit_search_engine_index(&rootp, &cfg, &rootp.join("dist"), w)?;
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
    let _lock_hash = match &lockfile {
        Some(p) => {
            oxide_fw_bundler::ChunkBundler::content_hash(&[&std::fs::read(p).unwrap_or_default()])
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
        let cert = oxide_fw_dev::generate_dev_cert()
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        let cfg = oxide_fw_server::Server::tls_config_from_pem(
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
    // subprocess at spawn — both are written PRE-RUNTIME by
    // `apply_startup_env` (P1 readiness env-race fix), so nothing to do here.
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
    // Extracted into a helper so `start_server_inner_full` (which the smoke
    // test calls directly, bypassing this function) can populate the same
    // map in ESM mode.
    populate_dev_component_source(&rootp, &cfg, &tree, &stack).await;
    // §1.4: pre-compute the CSS-module JS maps for SSR. The Node loader
    // cannot itself run lightningcss (no Rust callback); the client
    // `/@css/` path transforms on demand via `serve_css_module`. Both
    // MUST produce identical mangled names, so the same Rust backend
    // computes once and writes to `.oxide/css-modules/`.
    if let Err(e) = crate::esm_dev::precompute_css_modules(&rootp) {
        tracing::warn!(error = %e, "§1.4: precompute CSS modules failed");
    }
    if let Some(name) = stack.renderer_name() {
        outln!(w, "framework renderer: {}", name);
    }
    // §13.3: keep a pool handle for the watcher. On file change it bumps the
    // worker's module generation, so the next render/load re-evaluates the
    // edited module instead of serving the cached one.
    let hmr_pool: Option<std::sync::Arc<dyn oxide_fw_jshost::Pool>> = stack.pool.clone();
    // §13.3: keep a loader handle for the watcher too — a `page.server.ts`
    // change re-runs the route's loader so the published HMR
    // `route-data-update` message carries fresh ODF bytes (not an empty
    // payload the client has to fetch itself).
    let hmr_loader: Option<std::sync::Arc<dyn oxide_fw_server::LoaderRunner>> =
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
    // Path A: one RolldownBundler for the whole dev session, so the
    // incremental worker (and Rolldown's internal module snapshot) survives
    // across rebuilds. Creating a fresh bundler per call (the previous
    // behavior) forces a cold rebuild every save — 1.5s regardless of how
    // few modules changed.
    let client_bundler = oxide_fw_bundler::RolldownBundler::new(&rootp);
    if cfg.dev.esm {
        // Phase 5: native ESM mode has no client bundle to build — the boot
        // entry imports source modules directly. CSS is unaffected: it flows
        // through two paths that run below regardless of this branch:
        //
        // 1. `tailwind::precompute` writes `dist/client/tailwind-<hash>.css`
        //    (runs later in this function, independent of the bundle).
        // 2. `AdapterPageRenderer::inject_route_css`'s disk-scan fallback
        //    (wr101) emits `<link rel="stylesheet">` for any `tailwind-*.css`
        //    it finds — `with_manifest_path` is set unconditionally in
        //    `build_framework_stack`, so the disk scan fires even when no
        //    `client.manifest.json` exists.
        //
        // A project with raw CSS imports inside a route component would
        // 404 on `/src/**/*.css` — that was already true in ESM mode and is
        // tracked as Phase 5b (deferred).
        tracing::info!("dev: ESM mode — skipping cold-start client bundle");
    } else {
        let t0 = std::time::Instant::now();
        ensure_dev_client_bundle(
            &rootp,
            &cfg,
            &tree,
            stack.tooling_pool.as_ref(),
            &client_bundler,
            w,
        )
        .await?;
        tracing::info!(
            elapsed_ms = t0.elapsed().as_millis() as u64,
            "dev: initial client bundle (cold worker)"
        );
    }

    // [wr208] Rebuild stale workspace packages before bundling
    rebuild_stale_workspace_packages(&rootp);

    // [wr205] Enrich manifest with layout chains — was only done in cmd_build.
    // Without layout_chain, client-side navigation can't compose the layout tree
    // (e.g. docs-shell wrapper for /docs/* routes), and falls to the IR fallback
    // which returns NotFoundPage for any route not in PAGES.
    {
        let manifest_path = rootp.join("dist/client/client.manifest.json");
        if manifest_path.is_file() {
            let _ = enrich_manifest_layout_chains(&manifest_path, &tree);
        }
    }

    // [wr200] Consolidate tailwind CSS files + sync manifest.
    // Prevents stale-hash manifest entries from breaking CSR navigation
    // after a source change produces a new CSS hash.
    consolidate_tailwind_css_and_manifest(&rootp);

    // [wr101] Force dev mode for React dev build.
    // P1 (readiness): `OXIDE_DEV_MODE` is written pre-runtime by
    // `apply_startup_env` (dev command); in-process probes read the atomic
    // via `oxide_fw_server::dev_mode()`.

    // [wr106] Call precompute UNCONDITIONALLY. It self-checks:
    //   `!has_tailwind(&cfg) && !css_has_tailwind_import(&rootp)` -> Ok(None)
    // The old `if has_tailwind(&cfg) { ... }` guard was a bug: config
    // extraction fails on `plugins: [tailwind()]` (extract_minimal_framework
    // doesn't collect plugins), so has_tailwind returned false and precompute
    // never ran — hence 19KB CSS without utilities.
    if let Some(pool) = stack.tooling_pool.clone() {
        match crate::tailwind::precompute(pool, &cfg, &rootp).await {
            Ok(Some(out)) => {
                let client_dir = rootp.join("dist/client");
                let _ = std::fs::create_dir_all(&client_dir);
                let hash = blake3::hash(out.css.as_bytes()).to_hex().to_string();
                let filename = format!("tailwind-{}.css", &hash[..12]);
                let _ = std::fs::write(client_dir.join(&filename), &out.css);
                outln!(w, "tailwind: {} ({} bytes)", filename, out.css.len());
            }
            Ok(None) => {}
            Err(e) => eprintln!(
                "warn: tailwind precompute failed: {} {}",
                e.0.code, e.0.message
            ),
        }
    }
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
    let hmr_hub = std::sync::Arc::new(oxide_fw_dev::HmrHub::with_build_id(hmr_build_id.clone()));
    // §15.2: `configureServer` — a plugin gets the dev server's handles and
    // may contribute HMR behaviour. Each result may carry
    // `{ reload: bool }` to force a full reload on a config-shaped change;
    // the hub already reloads on module updates, so the hook's value today
    // is the notification plus a place for a plugin to install its own
    // listeners against the hub.
    if !collect_plugin_paths_for_hook(&rootp, &cfg, "configureServer").is_empty()
        && let Some(pool) = hmr_pool.as_ref()
    {
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
    let local = oxide_fw_dev::local_url(started.addr.port(), flags.https);
    outln!(w, "dev server listening on {local}");
    match &started.dev_token {
        Some(t) => outln!(w, "inspector: {local}/__oxide?token={t}"),
        None => outln!(w, "inspector: {local}/__oxide"),
    }
    if let Some(net) = oxide_fw_dev::network_url(started.addr.port(), flags.https) {
        if flags.host {
            outln!(w, "network: {net}");
        }
        if flags.qr {
            match oxide_fw_dev::render_qr(&net) {
                Ok(qr) => {
                    outln!(w, "{qr}");
                }
                Err(e) => outln!(w, "QR render failed: {}", e.0.message),
            }
        }
        if flags.open {
            let opened = oxide_fw_dev::open_browser(&local);
            outln!(w, "browser open: {opened}");
        }
    }

    // SPEC-16: watch the app AND any linked workspace packages, so an edit in
    // `packages/ui` (a `file:` dependency) invalidates the app.
    let mut watch_roots: Vec<std::path::PathBuf> = vec![rootp.clone()];
    if let Some(ws) = oxide_fw_config::detect_workspace_root(&rootp) {
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
                if (p.join("package.json").is_file() || p.join("Cargo.toml").is_file())
                    && p != rootp
                    && !watch_roots.contains(&p)
                {
                    watch_roots.push(p);
                }
            }
        }
    }
    let watcher = oxide_fw_dev::Watcher::with_notify(watch_roots.clone());
    let mut seen = std::collections::BTreeSet::new();
    // §1.3 (boot baseline): the on-disk state at boot is NOT a change. Seed
    // the hash set so the first poll tick diffs boot-time file touches
    // (workspace rebuild, prebundle write-out, editor/sync-daemon churn)
    // against reality instead of fanning them out as Created events.
    watcher.seed_seen(&mut seen);

    // PERF-08: a `DemandGraph` over the watched modules. A node per source
    // file whose compute reads that file; a change invalidates only the nodes
    // that observed the changed input (early cutoff spares the rest). The dev
    // loop bumps the worker generation only when a node actually recomputed,
    // so a no-op touch (or a change to an unrelated file) does not churn the
    // framework module graph.
    let dev_graph = std::sync::Arc::new(oxide_fw_graph::DemandGraph::new());

    // §13.3: on any file-change event, bump the worker's module generation
    // so the next render/load sees fresh code. Best-effort: log a warning
    // if the worker is unavailable; never fails the dev loop.
    async fn invalidate_framework_modules(
        pool: &Option<std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
        root: &std::path::Path,
    ) {
        if let Some(pool) = pool.as_ref() {
            let host = oxide_fw_jshost::FrameworkHost::new(std::sync::Arc::clone(pool));
            match host.invalidate_all().await {
                Ok(generation) => {
                    let gen_path = root.join(".oxide").join("hmr-generation");
                    if let Some(parent) = gen_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(&gen_path, generation.to_string());
                    tracing::debug!(generation, "framework modules invalidated on ALL workers");
                }
                Err(e) => tracing::warn!(
                    code = %e.0.code,
                    error = %e.0.message,
                    "framework invalidate_all failed",
                ),
            }
        }
    }

    // PERF-08: register a graph node per changed file and invalidate it. The
    // returned set is non-empty only when a node actually recomputed (early
    // cutoff spares unchanged content), so the worker generation bumps only
    // when there is real new code to see.
    fn graph_invalidated(
        dev_graph: &oxide_fw_graph::DemandGraph,
        ev: &oxide_fw_dev::FileEvent,
    ) -> bool {
        let path = match ev {
            oxide_fw_dev::FileEvent::Modified(p)
            | oxide_fw_dev::FileEvent::Created(p)
            | oxide_fw_dev::FileEvent::Removed(p) => p.clone(),
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
                ) -> oxide_fw_diagnostics::Result<oxide_fw_graph::NodeOutput> {
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
            ) -> oxide_fw_diagnostics::Result<oxide_fw_graph::NodeOutput> {
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
            #[cfg(feature = "search")]
            refresh_search_index(&rootp, &cfg, started.hot_search.as_ref(), w)?;
        }
        // §15.2: `handleHotUpdate` gets every changed file and may return
        // `{reload: bool}`. A plugin that wants a full reload (a config
        // change, a new dependency) forces one; the default is the graph
        // invalidation below.
        let mut force_reload = false;
        if let Some(pool) = hmr_pool.as_ref() {
            for ev in &events {
                let p = match ev {
                    oxide_fw_dev::FileEvent::Modified(p)
                    | oxide_fw_dev::FileEvent::Created(p)
                    | oxide_fw_dev::FileEvent::Removed(p) => p.clone(),
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
        .filter(|e| matches!(e.kind, oxide_fw_router::RouteKind::Page))
        .filter(|e| {
            e.segments
                .iter()
                .all(|s| !s.contains(':') && !s.contains('*'))
        })
        .map(|e| {
            if e.segments.is_empty() {
                "/".to_string()
            } else {
                format!("/{}", e.segments.join("/"))
            }
        })
        .collect();
    let mut idle_since = std::time::Instant::now();
    let mut warmed = true; // nothing to do until the first quiet period
    loop {
        // §3.7 latency fix: 50ms (was 200ms). The outer poll interval sets
        // the FLOOR on write→detection latency: a write at T=0.001ms is not
        // seen until the next tick. 200ms meant an average 100ms delay
        // before the debounce even started. 50ms brings the average to
        // ~25ms. Combined with the §3.7 fix in poll_notify (debounce only
        // when events are queued), the whole write→frame path drops from
        // ~142ms to ~40ms on the smoke app.
        //
        // CPU impact is nil on an idle filesystem: poll_notify now drains
        // `try_recv` and returns immediately when nothing is queued — no
        // blocking sleep, no hashing, no event construction. 20 polls/sec
        // of a cheap try_recv is microseconds.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let events_raw = watcher
            .poll(&mut seen)
            .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
        // SPEC-14: writing `.oxide/hmr-generation` on every invalidate
        // triggers another watcher event — a pathological loop. The notify
        // backend sees every path under the root (SKIP_DIRS only applies
        // to the walk_root path, not the notify callback). Filter BEFORE
        // `invalidate_framework_modules` — filtering after it left the
        // invalidate→write→notify→invalidate loop running.
        //
        // §1.3 (boot-flood fix): events under `node_modules/` are also
        // HMR-insignificant — EXCEPT `package.json`, which §3.3 uses as the
        // prebundle-invalidation trigger. Without this filter, the boot-time
        // workspace-package rebuild (which relinks `node_modules/@oxide-fw/*/
        // dist` symlinks) publishes ~30 `full-reload` frames ("rebuild
        // skipped for non-src file .../node_modules/...") into the socket of
        // any client that connects during the burst — the page reload-spams
        // right after boot and every early test assertion races a navigation.
        // Framework dist churn is picked up by `ensure_dev_client_bundle`'s
        // staleness scan (§3.11) at request time, not by HMR.
        let events: Vec<_> = events_raw
            .into_iter()
            .filter(|ev| {
                let p = match ev {
                    oxide_fw_dev::FileEvent::Modified(p)
                    | oxide_fw_dev::FileEvent::Created(p)
                    | oxide_fw_dev::FileEvent::Removed(p) => p,
                };
                !p.components().any(|c| c.as_os_str() == ".oxide")
                    // The app's own dist AND every workspace package's dist
                    // (the packages/* dirs are watch roots of their own):
                    // framework dist churn is picked up by
                    // `ensure_dev_client_bundle`'s staleness scan (§3.11) at
                    // request time, not by HMR.
                    && !watch_roots.iter().any(|r| p.starts_with(r.join("dist")))
                    && !(p.starts_with(rootp.join("node_modules"))
                        && p.file_name().and_then(|n| n.to_str()) != Some("package.json"))
            })
            .collect();
        let mut client_rebuilt = false;
        let mut client_rebuild_error: Option<String> = None;
        if !events.is_empty() {
            // §10.8: PPR shells are keyed by build id; dev's id is stable, so
            // a source edit must clear the store explicitly or the next
            // visitor is served a shell from the pre-edit render.
            if let Some(shells) = started.ppr_shells.as_ref()
                && let Ok(mut store) = shells.lock()
            {
                store.clear();
            }
            // §16.5: rescan collections before the loaders for this batch run.
            // The write is atomic (`write_content_atomic`) and lands under
            // `dist`, which the filter above ignores to avoid a notify loop.
            if content_source_changed(&rootp, &cfg, &events) {
                emit_content_search_indexes(&rootp, &cfg, &rootp.join("dist"), w)?;
                // V2-F09: the search index rides the same content-change
                // branch — re-emit (CAS skip when unchanged) and swap the
                // serve side onto the rebuild without a restart.
                #[cfg(feature = "search")]
                refresh_search_index(&rootp, &cfg, started.hot_search.as_ref(), w)?;
                if let Some(hub) = started.hmr.as_ref() {
                    hub.publish(oxide_fw_dev::HmrMessage::FullReload {
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
                    oxide_fw_dev::FileEvent::Modified(p)
                    | oxide_fw_dev::FileEvent::Created(p)
                    | oxide_fw_dev::FileEvent::Removed(p) => p,
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
            // §3.3: a lockfile change OR a `node_modules/<pkg>/package.json`
            // change invalidates the ESM prebundle. The prebundle (and the
            // `rewrite_map` derived from it) is built once at boot; without
            // this branch, installing a new package — or `pnpm update`ing a
            // bundled dep — leaves `/@prebundle/<name>.js` serving stale
            // CJS-to-ESM conversions forever. `pnpm add foo` is the canonical
            // trigger: the lockfile changes AND node_modules/foo/package.json
            // is created, both in the same watcher batch.
            //
            // Invalidation = re-prebundle to disk + FullReload. The rewrite
            // map would need Arc<RwLock<...>> to be live-updated (its
            // consumers cache it at request-entry), so the simpler correct
            // behaviour is: rebuild the on-disk artifacts, tell the client
            // to reload. The client's next request builds a fresh EsmDevState
            // with the new rewrite_map. Any in-flight HMR is superseded by
            // the reload, which is correct — a package change is not an
            // edit the React Refresh path can meaningfully preserve.
            let has_prebundle_invalidation = events.iter().any(|ev| {
                let p = match ev {
                    oxide_fw_dev::FileEvent::Modified(p)
                    | oxide_fw_dev::FileEvent::Created(p)
                    | oxide_fw_dev::FileEvent::Removed(p) => p,
                };
                let pstr = p.to_string_lossy();
                pstr.contains("/node_modules/") && pstr.ends_with("/package.json")
            });
            if has_prebundle_invalidation && cfg.dev.esm {
                // §1.3 note: this branch fires only for package.json events
                // whose content actually changed (the boot baseline seeding
                // in `Watcher::seed_seen` swallows relink churn for
                // unchanged files) and the prebundle is regenerated BEFORE
                // the FullReload publishes, so a reloading client re-fetches
                // against the fresh artifacts. The per-event node_modules
                // skip below keeps every OTHER relink event silent, so this
                // is at most ONE reload per real dependency change.
                tracing::info!("§3.3: node_modules/*/package.json change — re-prebundling");
                if let Err(e) = crate::esm_dev::prebundle_cjs_deps(&rootp, &cfg.dev.prebundle) {
                    tracing::warn!(error = %e, "§3.3: prebundle invalidation failed");
                } else {
                    // Rebuild the rewrite map even though consumers cache it
                    // at boot — the reload below triggers a fresh
                    // EsmDevState build on next request. (No in-memory swap
                    // needed: FullReload is the signal.)
                    let _ = crate::esm_dev::build_rewrite_map(&rootp, &cfg.dev.prebundle);
                    if let Some(hub) = started.hmr.as_ref() {
                        hub.publish(oxide_fw_dev::HmrMessage::FullReload {
                            reason: format!(
                                "node_modules change — prebundle regenerated ({} package.json event(s))",
                                events.iter().filter(|ev| {
                                    let p = match ev {
                                        oxide_fw_dev::FileEvent::Modified(p)
                                            | oxide_fw_dev::FileEvent::Created(p)
                                            | oxide_fw_dev::FileEvent::Removed(p) => p,
                                    };
                                    let pstr = p.to_string_lossy();
                                    pstr.contains("/node_modules/")
                                        && pstr.ends_with("/package.json")
                                })
                                .count()
                            ),
                        });
                    }
                }
            }
            if has_lockfile {
                tracing::info!("lockfile change — resolver epoch bump");
                // §3.3: also re-prebundle on lockfile change. The lockfile
                // mirrors `node_modules` state, but a package can be
                // hoisted/re-linked without touching its own package.json.
                // Treat a lockfile touch as a prebundle-invalidation trigger
                // independent of the per-package check above.
                if cfg.dev.esm
                    && let Err(e) = crate::esm_dev::prebundle_cjs_deps(&rootp, &cfg.dev.prebundle)
                {
                    tracing::warn!(error = %e, "§3.3: lockfile prebundle re-run failed");
                }
            }

            // §3.4: a `.env*` change invalidates `import.meta.env` values.
            // Vite parity: full reload, NOT HMR. Env values are read once
            // per module evaluation; letting a hot update through would
            // leave modules that already ran with STALE values — silently
            // divergent from a fresh page load, which is the worst kind of
            // bug. Reload is the honest signal.
            //
            // Match the exact files config::layer knows about (§14.2):
            // `.env`, `.env.<mode>`, `.env.local`, `.env.<mode>.local`.
            // Filename must START with `.env` (a file literally named
            // `.env` counts) and the parent directory must be the project
            // root — a `.env` inside `src/` is a source file, not config.
            let env_file_changed = events.iter().any(|ev| {
                let p = match ev {
                    oxide_fw_dev::FileEvent::Modified(p)
                    | oxide_fw_dev::FileEvent::Created(p)
                    | oxide_fw_dev::FileEvent::Removed(p) => p,
                };
                let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                    return false;
                };
                name.starts_with(".env")
                    && p.parent()
                        .map(|par| par == rootp.as_path())
                        .unwrap_or(false)
            });
            if env_file_changed {
                // §3.4: full reload. `serve_source_file` reads the env fresh
                // on each request, so the post-reload page load sees the
                // new values — no in-memory update needed here.
                tracing::info!("§3.4: .env* change — full reload");
                if let Some(hub) = started.hmr.as_ref() {
                    hub.publish(oxide_fw_dev::HmrMessage::FullReload {
                        reason: ".env* change — import.meta.env values refreshed".to_string(),
                    });
                }
            }
            invalidate_framework_modules(&hmr_pool, &rootp).await;
            let needs_client_rebuild = !cfg.dev.esm
                && events.iter().any(|e| {
                    let p = match e {
                        oxide_fw_dev::FileEvent::Modified(p)
                        | oxide_fw_dev::FileEvent::Created(p)
                        | oxide_fw_dev::FileEvent::Removed(p) => p,
                    };
                    is_client_source(p, &rootp)
                        && !matches!(
                            oxide_fw_dev::hmr_behavior(oxide_fw_dev::classify_event(e)),
                            oxide_fw_dev::HmrBehavior::RestartSubsystems
                        )
                });
            tracing::info!(
                n_events = events.len(),
                needs_client_rebuild = needs_client_rebuild,
                "DIAG watcher decision"
            );
            if needs_client_rebuild {
                let live_tree = started.swappable_tree.load();
                let changed_paths: Vec<std::path::PathBuf> = events
                    .iter()
                    .filter_map(|e| match e {
                        oxide_fw_dev::FileEvent::Modified(p)
                        | oxide_fw_dev::FileEvent::Created(p)
                        | oxide_fw_dev::FileEvent::Removed(p) => {
                            if is_client_source(p, &rootp) {
                                Some(p.clone())
                            } else {
                                None
                            }
                        }
                    })
                    .collect();
                let t0 = std::time::Instant::now();
                match build_client_bundle(
                    &rootp,
                    &cfg,
                    &live_tree,
                    hmr_tooling_pool.as_ref(),
                    &client_bundler,
                    Some(&changed_paths),
                    w,
                )
                .await
                {
                    Ok(()) => {
                        tracing::info!(
                            changed = changed_paths.len(),
                            elapsed_ms = t0.elapsed().as_millis() as u64,
                            "watcher: incremental client bundle OK"
                        );
                        let manifest_path = rootp.join("dist/client/client.manifest.json");
                        let _ = enrich_manifest_layout_chains(&manifest_path, &live_tree);
                        consolidate_tailwind_css_and_manifest(&rootp);
                        client_rebuilt = true;
                        // A successful rebuild supersedes any previous
                        // failure. Without this reset, once a rebuild failed,
                        // `client_rebuild_error` stayed `Some` for the rest of
                        // the dev session and every subsequent AdapterHmr
                        // event re-published `HmrMessage::Error` — the
                        // overlay could never clear.
                        client_rebuild_error = None;
                    }
                    Err(e) => {
                        client_rebuild_error = Some(format!("{e}"));
                        tracing::error!(error = %e, "watcher: rebuild FAILED");
                    }
                }
            }
            // §5.6: bump the stale generation BEFORE releasing the loop,
            // so any in-flight dev dispatch's `select!` (see
            // `wrap_dispatch_with_stale_guard` below) aborts and returns
            // a retryable 503 rather than a result computed from the
            // pre-change source. The bump is also what distinguishes a
            // "file changed" from "in-flight" — the dispatch reads the
            // generation at entry and the exit check is only needed if
            // the bump lands while the dispatch was already running.
            //
            // §1.3: the bump is gated on the batch containing at least one
            // app-source event. A pure `node_modules` batch (a relinked
            // dependency's package.json) produces no client message and no
            // source change — bumping the generation on it aborted every
            // in-flight module fetch with a retryable 503 that nothing
            // retries (a browser `import()` fails hard), which surfaced as
            // a page whose hydration never completed when a package relink
            // landed mid-boot.
            let has_source_event = events.iter().any(|e| {
                let p = match e {
                    oxide_fw_dev::FileEvent::Modified(p)
                    | oxide_fw_dev::FileEvent::Created(p)
                    | oxide_fw_dev::FileEvent::Removed(p) => p,
                };
                !p.to_string_lossy().contains("/node_modules/")
            });
            if has_source_event && let Some(hub) = started.hmr.as_ref() {
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
                        oxide_fw_dev::FileEvent::Modified(p)
                        | oxide_fw_dev::FileEvent::Created(p)
                        | oxide_fw_dev::FileEvent::Removed(p) => p.clone(),
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
            if force_reload && let Some(hub) = started.hmr.as_ref() {
                hub.publish(oxide_fw_dev::HmrMessage::FullReload {
                    reason: "plugin handleHotUpdate requested a reload".to_string(),
                });
            }
            idle_since = std::time::Instant::now();
            warmed = false;
        }
        // §3.9: FSEvents on macOS (and any watcher that reports directory-level
        // diffs) can deliver Created + Modified for the same path when a
        // writeFileSync changes an already-existing file. The Created would route
        // to ConfigOrRoutes -> RestartSubsystems -> FullReload, racing the
        // Modified's AdapterHmr -> a v1 js entry. Drop a Created event when a
        // Modified for the same path is in the same batch: the file existed
        // before the batch, so it was not actually created.
        let events = {
            use std::collections::HashSet;
            let mut evs = events;
            let modded: HashSet<std::path::PathBuf> = evs
                .iter()
                .filter_map(|e| match e {
                    oxide_fw_dev::FileEvent::Modified(p) => Some(p.clone()),
                    _ => None,
                })
                .collect();
            let before = evs.len();
            evs.retain(|e| !matches!(e, oxide_fw_dev::FileEvent::Created(p) if modded.contains(p)));
            if evs.len() != before {
                tracing::info!(
                    dropped = before - evs.len(),
                    "§3.9: dropped Created events shadowed by Modified in same batch"
                );
            }
            evs
        };
        // §3.2: accumulate AdapterHmr events across this batch. Publishing
        // one v1 Update frame at the end lets the client fan out the
        // re-imports and run a single performReactRefresh() — necessary for
        // a `git checkout` burst (~50 distinct files, each currently a
        // separate refresh). Non-AdapterHmr events (Css/ServerLoader/
        // ServerModule/ConfigOrRoutes) still publish per-event below, since
        // they carry different semantics and shouldn't be batched.
        let mut adapter_hmr_batch: Vec<(String, String)> = Vec::new();
        // V2-E01 phase 4: the state-preservation policy (the injectable
        // `StatePolicy` seam — the default maps a boundary update to `keep`
        // and a full reload to `clear` with the walk's reason) and the
        // tick-scoped reload tracker: when ANY event in this batch planned
        // a full reload, the accumulated update frame must not claim
        // `keep` — the client would stash state for a page that is about
        // to go away. The verdict for that case is `clear` with the SAME
        // reason the `full-reload` frame carries, so the two frames tell
        // one story.
        let state_policy = oxide_fw_dev::state::DefaultStatePolicy;
        let mut tick_reload_reason: Option<String> = None;
        for ev in events {
            tracing::info!(?ev, "file change");
            outln!(w, "change: {ev:?}");
            // SPEC-14: emit the HMR message for this change.
            if let Some(hub) = started.hmr.as_ref() {
                use oxide_fw_dev::{HmrBehavior, HmrMessage};
                let path = match &ev {
                    oxide_fw_dev::FileEvent::Modified(p)
                    | oxide_fw_dev::FileEvent::Created(p)
                    | oxide_fw_dev::FileEvent::Removed(p) => p,
                };
                let pstr = path.to_string_lossy().to_string();
                // §1.3: node_modules paths never dispatch per-event HMR. A
                // `package.json` under node_modules was already consumed by
                // the §3.3 prebundle-invalidation branch above (ONE
                // consolidated FullReload per batch); any other node_modules
                // event is library churn with no client-visible meaning —
                // publishing it per-event produced a `full-reload` frame per
                // relinked file during the boot-time workspace rebuild, and a
                // client that connected during that burst reload-spammed
                // itself into a broken boot.
                if pstr.contains("/node_modules/") {
                    continue;
                }
                // §1.4: capture BEFORE the match moves `pstr` into
                // HmrMessage variants — the post-module-update branch below
                // needs to know whether this was a .module.css edit.
                let is_module_css = pstr.ends_with(".module.css");
                // Phase 4: tailwind source change -> rebuild CSS to disk.
                // `is_tailwind_relevant` (not `is_source_file`) so an edit to
                // a `src/styles/*.css` entry — new utilities in @theme, a new
                // @apply — also rebuilds the compiled tailwind-*.css. The
                // rebuild is a content-diff no-op when nothing changed, so
                // the extra CSS events cost one scan, not a rebuild.
                if tailwind::has_tailwind(&cfg)
                    && tailwind::is_tailwind_relevant(&pstr)
                    && let Some(pool) = hmr_tooling_pool.clone()
                {
                    match tailwind::rebuild_for_dev(pool, &cfg, &rootp).await {
                        Ok(Some((url, css))) => {
                            tracing::info!(
                                path = %pstr,
                                url = %url,
                                bytes = css.len(),
                                "tailwind HMR: rebuilt CSS"
                            );
                            hub.publish(oxide_fw_dev::state::update_frame(
                                // Tailwind rebuild: the entry IS the whole
                                // surface (a rebuilt sheet at its cache-busted
                                // URL) — boundary-scoped by construction, so
                                // the synthetic plan the policy reads says so.
                                &oxide_fw_dev::boundary::ClassifiedPlan {
                                    kind: oxide_fw_dev::UpdateKind::Css,
                                    plan: oxide_fw_dev::boundary::Plan::Update(vec![
                                        path.to_path_buf(),
                                    ]),
                                },
                                &state_policy,
                                // The rebuilt sheet is served at `url` — the
                                // v1 entry is not file-backed (the swap
                                // refetches through the cache-busted URL;
                                // the v0 inline `code` payload had no
                                // client reader).
                                vec![oxide_fw_dev::UpdateEntry::stamped(
                                    oxide_fw_dev::UpdateKind::Css,
                                    url.clone(),
                                    url,
                                    None,
                                )],
                            ));
                        }
                        Ok(None) => {
                            tracing::debug!(
                                path = %pstr,
                                "tailwind HMR: no CSS change to publish"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                path = %pstr,
                                err = %e.0.message,
                                "tailwind HMR: rebuild failed"
                            );
                        }
                    }
                }
                let kind = oxide_fw_dev::classify_event(&ev);
                match oxide_fw_dev::hmr_behavior(kind) {
                    HmrBehavior::StyleSwap => {
                        // ESM mode: a CSS file in `src/` is imported through a
                        // `/@css/<rel>` module (see `serve_css_module`) that
                        // self-accepts and replaces its `<style>` in place.
                        // Publishing an update entry with the module URL lets
                        // the client's `reloadModule` re-import it, which
                        // re-executes the module body and swaps the style tag.
                        //
                        // Non-`src/` CSS (or bundler mode) keeps the legacy
                        // a css entry — a `<link>` href swap for the bundler-
                        // emitted stylesheet.
                        let src_dir = rootp.join("src");
                        let in_src = path.starts_with(&src_dir);
                        let is_esm = cfg.dev.esm;
                        if in_src && is_esm {
                            // V2-E01 phase 2: the CSS decision is the
                            // planner's (`plan_classified`), not a
                            // side-channel. The §13.3 terminal makes a
                            // stylesheet swap NEVER a reload — the walk's
                            // verdict collapses to the in-place swap of the
                            // changed sheet — and the emitted frames below
                            // stay byte-identical to the pre-substrate wire.
                            let changed_pb =
                                std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
                            let graph = build_hmr_graph(&rootp, &rootp.join(&cfg.routes.dir));
                            let classified = {
                                let importers: std::collections::HashMap<
                                    std::path::PathBuf,
                                    Vec<std::path::PathBuf>,
                                > = graph
                                    .importers
                                    .iter()
                                    .map(|(module, imps)| {
                                        (module.clone(), imps.iter().cloned().collect())
                                    })
                                    .collect();
                                oxide_fw_dev::boundary::plan_classified(
                                    oxide_fw_dev::ChangeKind::Css,
                                    &changed_pb,
                                    &importers,
                                    &graph.accepting,
                                    &graph.entries,
                                )
                            };
                            if classified.is_full_reload() {
                                // Unreachable for stylesheets (the §13.3
                                // terminal collapses the walk) — logged
                                // loudly if a future ChangeKind ever routes
                                // here with a reload verdict.
                                if let oxide_fw_dev::Plan::FullReload(reason) = &classified.plan {
                                    tracing::warn!(
                                        reason = %reason,
                                        "DEV-HMR: a style swap planned a full reload"
                                    );
                                }
                            } else if !graph
                                .importers
                                .get(&changed_pb)
                                .is_some_and(|imps| !imps.is_empty())
                            {
                                // DEV-HMR-002: the graph has no edge for this
                                // stylesheet — the swap still runs (a static
                                // `<link>` may carry it), but nothing on the
                                // module side references it, so the boundary
                                // fan-out will never include it.
                                let diag =
                                    oxide_fw_dev::boundary::css_orphan_diagnostic(&changed_pb);
                                tracing::warn!(
                                    code = diag.code,
                                    message = %diag.message,
                                    "DEV-HMR-002: orphan stylesheet"
                                );
                                hub.publish(HmrMessage::Error { diagnostic: diag });
                            }
                            let root_str = rootp.to_string_lossy();
                            let rel = pstr.strip_prefix(root_str.as_ref()).unwrap_or(&pstr);
                            let rel_slash = rel.replace('\\', "/");
                            let trimmed = rel_slash.trim_start_matches('/');
                            if let Some(inner) = trimmed.strip_prefix("src/") {
                                // A src/ stylesheet swaps through its /@css/
                                // module: the entry's acceptedPath is the
                                // module URL the client re-imports (style
                                // injection, never a reload — §13.3).
                                hub.publish(oxide_fw_dev::state::update_frame(
                                    &classified,
                                    &state_policy,
                                    vec![oxide_fw_dev::UpdateEntry::stamped(
                                        oxide_fw_dev::UpdateKind::Css,
                                        pstr.clone(),
                                        format!("/@css/{inner}"),
                                        Some(std::path::Path::new(&pstr)),
                                    )],
                                ));
                            } else {
                                hub.publish(oxide_fw_dev::state::update_frame(
                                    &classified,
                                    &state_policy,
                                    vec![oxide_fw_dev::UpdateEntry::stamped(
                                        oxide_fw_dev::UpdateKind::Css,
                                        pstr.clone(),
                                        pstr.clone(),
                                        Some(std::path::Path::new(&pstr)),
                                    )],
                                ));
                            }
                        } else {
                            hub.publish(oxide_fw_dev::state::update_frame(
                                // Non-ESM / non-src CSS: a `<link>` href swap
                                // for the changed sheet — the single entry is
                                // the whole surface (boundary-scoped by
                                // construction).
                                &oxide_fw_dev::boundary::ClassifiedPlan {
                                    kind: oxide_fw_dev::UpdateKind::Css,
                                    plan: oxide_fw_dev::boundary::Plan::Update(vec![
                                        path.to_path_buf(),
                                    ]),
                                },
                                &state_policy,
                                vec![oxide_fw_dev::UpdateEntry::stamped(
                                    oxide_fw_dev::UpdateKind::Css,
                                    pstr.clone(),
                                    pstr.clone(),
                                    Some(std::path::Path::new(&pstr)),
                                )],
                            ));
                        }
                        // §1.4: a `.module.css` change also regenerates its
                        // pre-computed SSR map. The client's own update comes
                        // from the css entry published above (the /@css/
                        // reload re-injects the style; React Refresh is not
                        // needed for CSS). The SSR side reads the fresh
                        // `.oxide/css-modules/<rel>.mjs` on the next render.
                        if is_module_css
                            && let Err(e) = crate::esm_dev::precompute_css_modules(&rootp)
                        {
                            tracing::warn!(error = %e, "§1.4: precompute after .module.css change failed");
                        }
                    }
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
                            // No route pattern resolved: the v0 wire passed
                            // the FILE path as the route (the client's §1.3
                            // pattern match then dropped it). The v1 entry
                            // keeps the same conservative shape — a data
                            // entry whose path the client pattern-matches.
                            hub.publish(oxide_fw_dev::state::update_frame(
                                // A data entry re-fetches `/__data` for one
                                // route — the single entry is the whole
                                // surface (boundary-scoped by construction).
                                &oxide_fw_dev::boundary::ClassifiedPlan {
                                    kind: oxide_fw_dev::UpdateKind::Data,
                                    plan: oxide_fw_dev::boundary::Plan::Update(vec![
                                        path.to_path_buf(),
                                    ]),
                                },
                                &state_policy,
                                vec![oxide_fw_dev::UpdateEntry::stamped(
                                    oxide_fw_dev::UpdateKind::Data,
                                    pstr.clone(),
                                    String::new(),
                                    Some(std::path::Path::new(&pstr)),
                                )],
                            ));
                        } else {
                            hub.publish(HmrMessage::OxData {
                                route: route_pattern,
                                data: odf_b64,
                            });
                        }
                    }
                    HmrBehavior::AdapterHmr => {
                        if let Some(err_msg) = client_rebuild_error.as_ref() {
                            let diag = oxide_fw_diagnostics::Diagnostic::error(
                                "DEV-BUNDLE",
                                format!("rebuild failed for {pstr}: {err_msg}"),
                            );
                            hub.publish(HmrMessage::Error { diagnostic: diag });
                        } else if cfg.dev.esm {
                            tracing::info!("HMR: publishing update entries for ESM file");
                            let root_str = rootp.to_string_lossy();
                            let rel = pstr.strip_prefix(root_str.as_ref()).unwrap_or(&pstr);
                            let rel_slash = rel.replace('\\', "/");
                            let trimmed = rel_slash.trim_start_matches('/');
                            if trimmed.starts_with("src/") {
                                // §3.2: accumulate; the post-loop block
                                // publishes a single singular message if
                                // only one item was collected, or a batch
                                // if multiple. This preserves the singular
                                // shape for the common 1-file save.
                                //
                                // §3.2: accumulate; post-loop publishes
                                // single (1) or batch (>1).
                                adapter_hmr_batch.push((pstr.clone(), format!("/{trimmed}")));
                                //
                                // §1.3: also fan out to every importer of
                                // this file. TWO cases need it:
                                //
                                // 1. NON-COMPONENT edits (`.ts`/`.js`/`.mjs`
                                //    utils): no component registration in
                                //    compiled form, so nothing else would
                                //    update the importer's stale binding.
                                //
                                // 2. Component edits on the component-
                                //    capability frameworks (Vue, Solid,
                                //    Svelte): there is no React-Refresh-style
                                //    in-place patch — the PAGE module must
                                //    re-import so its import binding picks
                                //    up the fresh child module, and the
                                //    boot's re-render (componentFor →
                                //    loadChunk) mounts the new tree. React
                                //    keeps the skip: `performReactRefresh`
                                //    patches the registered family directly,
                                //    and fanning out would turn a singular
                                //    the update frame into a redundant batch.
                                let changed_is_component = {
                                    let lower = pstr.to_ascii_lowercase();
                                    lower.ends_with(".tsx")
                                        || lower.ends_with(".jsx")
                                        || lower.ends_with(".vue")
                                        || lower.ends_with(".svelte")
                                };
                                let esm_framework = cfg
                                    .framework
                                    .adapter
                                    .as_deref()
                                    .or(cfg.framework.name.as_deref())
                                    .unwrap_or("react");
                                let react_refresh_owns_update =
                                    esm_framework == "react" && changed_is_component;
                                if !react_refresh_owns_update {
                                    // V2-E01: the fan-out is the boundary
                                    // planner's decision (§V2-E01), not a
                                    // one-hop walk. The changed file itself
                                    // always re-imports (pushed to
                                    // `adapter_hmr_batch` above); the planner
                                    // supplies the boundary set to fan out
                                    // to — walking PAST non-boundary
                                    // importers, which the one-hop block this
                                    // replaced left stale (a
                                    // `util.ts <- util2.ts <- page.tsx`
                                    // chain updated nothing but `util.ts`,
                                    // so the page's import binding kept the
                                    // old module bytes).
                                    let changed_pb = std::fs::canonicalize(path)
                                        .unwrap_or_else(|_| path.to_path_buf());
                                    let graph =
                                        build_hmr_graph(&rootp, &rootp.join(&cfg.routes.dir));
                                    // An orphan (nothing imports the changed
                                    // file) cannot reach any client — a
                                    // reload would be user-visible noise for
                                    // a no-op (the spec's `_ =>` arm is
                                    // right for the pure function, the dev
                                    // loop pre-filters it; see
                                    // docs/deviations.md). The quiet
                                    // self-update already in the batch is
                                    // the whole plan.
                                    let has_importers = graph
                                        .importers
                                        .get(&changed_pb)
                                        .is_some_and(|imps| !imps.is_empty());
                                    if has_importers {
                                        let importers: std::collections::HashMap<
                                            std::path::PathBuf,
                                            Vec<std::path::PathBuf>,
                                        > = graph
                                            .importers
                                            .into_iter()
                                            .map(|(module, imps)| {
                                                (module, imps.into_iter().collect())
                                            })
                                            .collect();
                                        match oxide_fw_dev::plan(
                                            &changed_pb,
                                            &importers,
                                            &graph.accepting,
                                            &graph.entries,
                                        ) {
                                            oxide_fw_dev::Plan::Update(boundaries) => {
                                                tracing::debug!(
                                                    changed = %pstr,
                                                    boundaries = boundaries.len(),
                                                    "V2-E01: boundary plan computed"
                                                );
                                                let root_str = rootp.to_string_lossy();
                                                let mut in_batch: std::collections::HashSet<
                                                    String,
                                                > = std::collections::HashSet::new();
                                                in_batch.insert(pstr.clone());
                                                for b in boundaries {
                                                    if b == changed_pb {
                                                        continue;
                                                    }
                                                    let b_str = b.to_string_lossy().to_string();
                                                    if !in_batch.insert(b_str.clone()) {
                                                        continue;
                                                    }
                                                    let rel = b_str
                                                        .strip_prefix(root_str.as_ref())
                                                        .unwrap_or(&b_str);
                                                    let rel_slash = rel.replace('\\', "/");
                                                    let trimmed_imp =
                                                        rel_slash.trim_start_matches('/');
                                                    if trimmed_imp.starts_with("src/") {
                                                        adapter_hmr_batch.push((
                                                            b_str,
                                                            format!("/{trimmed_imp}"),
                                                        ));
                                                    }
                                                }
                                            }
                                            oxide_fw_dev::Plan::FullReload(reason) => {
                                                // §13.4: the reload reason is
                                                // actionable — publish the
                                                // DEV-HMR-001 diagnostic beside
                                                // the frame so the overlay
                                                // carries the fix line.
                                                tracing::info!(
                                                    changed = %pstr,
                                                    %reason,
                                                    "V2-E01: boundary walk fell back to a reload"
                                                );
                                                let diag =
                                                    oxide_fw_dev::full_reload_diagnostic(&reason);
                                                hub.publish(HmrMessage::Error { diagnostic: diag });
                                                tick_reload_reason = Some(reason.clone());
                                                hub.publish(HmrMessage::FullReload { reason });
                                            }
                                        }
                                    }
                                }
                            } else {
                                tick_reload_reason =
                                    Some(format!("rebuild skipped for non-src file {pstr}"));
                                hub.publish(HmrMessage::FullReload {
                                    reason: format!("rebuild skipped for non-src file {pstr}"),
                                });
                            }
                        } else if client_rebuilt {
                            tick_reload_reason =
                                Some(format!("rebuilt client bundle after {pstr}"));
                            hub.publish(HmrMessage::FullReload {
                                reason: format!("rebuilt client bundle after {pstr}"),
                            });
                        }
                    }
                    HmrBehavior::ServerSwap => {
                        // The v1 wire retired `server-module-swap`: the frame
                        // was a client no-op (the DOM stays put; the watcher's
                        // `invalidate_framework_modules` bump already owns the
                        // server-side swap), so the server stops publishing it
                        // (docs/deviations.md, the phase-3 record).
                        tracing::debug!(
                            path = %pstr,
                            "HMR: server module swap invalidated (no client frame)"
                        );
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
                                        use oxide_fw_router::RouteKind;
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
                                    let live_tree = started.swappable_tree.load();
                                    if cfg.dev.esm {
                                        // Native ESM mode: no client bundle to
                                        // rebuild. The boot entry imports each
                                        // route's source module directly, so
                                        // regenerate `.oxide/esm-manifest.json`
                                        // and `.oxide/esm-boot.mjs` from the
                                        // fresh tree. `EsmDevState` itself is
                                        // immutable across a route add — the
                                        // rewrite map only depends on the
                                        // framework exports + the prebundle
                                        // list, both fixed at dev-start.
                                        let mut components: std::collections::BTreeMap<
                                            String,
                                            String,
                                        > = std::collections::BTreeMap::new();
                                        let routes_root = rootp.join(&cfg.routes.dir);
                                        for entry in &live_tree.entries {
                                            let rel_from_routes =
                                                entry.file.to_string_lossy().to_string();
                                            let id_str = entry.id.0.as_str().to_string();
                                            components.insert(
                                                id_str,
                                                format!(
                                                    "/src/{}/{rel_from_routes}",
                                                    cfg.routes
                                                        .dir
                                                        .trim_start_matches("src/")
                                                        .trim_end_matches('/')
                                                ),
                                            );
                                        }
                                        if let Err(e) =
                                            crate::esm_dev::build_esm_manifest(&rootp, &components)
                                        {
                                            tracing::warn!(error = %e, "esm dev: manifest regen failed");
                                        }
                                        // Rebuild the island map from config
                                        // (identical to the boot-time path
                                        // above). Cheap — it's a BTreeMap
                                        // walk over the config's already-
                                        // parsed registry. The boot entry's
                                        // import specifier is a ROOT-ABSOLUTE
                                        // URL (the browser resolves a bare
                                        // relative path against the current
                                        // page URL, which breaks on nested
                                        // routes), so the source is wrapped
                                        // in its URL spelling.
                                        let mut islands: std::collections::BTreeMap<
                                            String,
                                            oxide_fw_config::IslandSpec,
                                        > = std::collections::BTreeMap::new();
                                        for (name, spec) in &cfg.framework.islands {
                                            let rel_path = spec.src();
                                            let full_path = rootp.join(rel_path);
                                            if full_path.is_file() {
                                                islands.insert(
                                                    name.clone(),
                                                    oxide_fw_config::IslandSpec::Plain(format!(
                                                        "/{rel_path}"
                                                    )),
                                                );
                                            }
                                        }
                                        if let Err(e) = crate::esm_dev::generate_esm_boot_entry(
                                            &rootp,
                                            &components,
                                            &islands,
                                            cfg.framework
                                                .adapter
                                                .as_deref()
                                                .or(cfg.framework.name.as_deref())
                                                .unwrap_or("react"),
                                        ) {
                                            tracing::warn!(error = %e, "esm dev: boot entry regen failed");
                                        }
                                        let _ = &routes_root;
                                    } else if build_client_bundle(
                                        &rootp,
                                        &cfg,
                                        &live_tree,
                                        hmr_tooling_pool.as_ref(),
                                        &client_bundler,
                                        None,
                                        w,
                                    )
                                    .await
                                    .is_ok()
                                    {
                                        let manifest_path =
                                            rootp.join("dist/client/client.manifest.json");
                                        let _ = enrich_manifest_layout_chains(
                                            &manifest_path,
                                            &live_tree,
                                        );
                                        consolidate_tailwind_css_and_manifest(&rootp);
                                    }
                                    tick_reload_reason = Some("route tree rebuilt".to_string());
                                    hub.publish(HmrMessage::FullReload {
                                        reason: "route tree rebuilt".to_string(),
                                    });
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        "dev: route rescan failed; keeping current tree"
                                    );
                                    tick_reload_reason =
                                        Some(format!("{pstr} changed (route rescan failed)"));
                                    hub.publish(HmrMessage::FullReload {
                                        reason: format!("{pstr} changed (route rescan failed)"),
                                    });
                                }
                            }
                        } else {
                            tick_reload_reason = Some(format!("{pstr} changed"));
                            hub.publish(HmrMessage::FullReload {
                                reason: format!("{pstr} changed"),
                            });
                        }
                    }
                }
            }
        }
        // §3.2: publish the accumulated batch — one v1 Update frame whether
        // the tick saw one AdapterHmr event (the common 1-file save) or a
        // burst of several.
        if let Some(hub) = started.hmr.as_ref()
            && !adapter_hmr_batch.is_empty()
        {
            // §3.2: the v1 `updates` array is natively a batch — the
            // singular/batch wire distinction dissolves into N entries:
            // one frame, one serialization, one dispatch tick.
            let updates: Vec<oxide_fw_dev::UpdateEntry> = adapter_hmr_batch
                .into_iter()
                .map(|(path, url)| {
                    oxide_fw_dev::UpdateEntry::stamped(
                        oxide_fw_dev::UpdateKind::Js,
                        path.clone(),
                        url,
                        Some(std::path::Path::new(&path)),
                    )
                })
                .collect();
            // V2-E01 phase 4: the frame's verdict is the policy's call on
            // the batch plan — unless this same tick planned a full reload
            // (any branch above), in which case the verdict is `clear` with
            // the SAME reason the `full-reload` frame carries.
            let batch_plan = oxide_fw_dev::boundary::ClassifiedPlan {
                kind: oxide_fw_dev::UpdateKind::Js,
                plan: oxide_fw_dev::boundary::Plan::Update(
                    // The accepted dev URLs (`/src/...`) are the boundaries
                    // the client re-imports; the batch is already
                    // deduplicated by the fan-out's `in_batch` set.
                    updates
                        .iter()
                        .filter(|e| !e.accepted_path.is_empty())
                        .map(|e| std::path::PathBuf::from(e.accepted_path.trim_start_matches('/')))
                        .collect(),
                ),
            };
            let preserve = match &tick_reload_reason {
                Some(reason) => oxide_fw_dev::Preservation::Clear {
                    reason: reason.clone(),
                },
                None => oxide_fw_dev::state::StatePolicy::preservation(&state_policy, &batch_plan),
            };
            hub.publish(oxide_fw_dev::HmrMessage::Update { updates, preserve });
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

pub async fn start_dev_server_full(
    root: &str,
    port: Option<u16>,
    bind_ip: std::net::IpAddr,
    tls: Option<std::sync::Arc<tokio_rustls::rustls::ServerConfig>>,
    proxy_rules: Vec<oxide_fw_dev::ProxyRule>,
    stack: crate::framework_shim::FrameworkStack,
    hmr: Option<std::sync::Arc<oxide_fw_dev::HmrHub>>,
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

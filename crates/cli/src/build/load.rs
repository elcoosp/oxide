//! Moved verbatim from build/cmd.rs by scripts/build_phase_split.py (V2-A01 phase 2).
//!
//! phase_load — entry, early cutoff, config + route scan, framework stack
//! (spec WP-A01 phase 2, the sketch's `phase_load`). Canonicalizes the root
//! once, short-circuits an up-to-date build (returns `None` -> exit 0), loads
//! the production config, fires the `config`/`configResolved` plugin hooks,
//! fails on unknown native plugins, scans + refines the route tree and builds
//! the framework stack. Returns the fully-seeded [`BuildCtx`].

use crate::*;

// >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
pub(crate) async fn phase_load<'w>(
    root: &str,
    r#static: bool,
    explain: bool,
    attest: bool,
    key_path: Option<&'w str>,
    w: Writer<'w>,
) -> anyhow::Result<Option<BuildCtx<'w>>> {
    // <<< phase-wrapper
    // Canonicalize once at entry. Downstream joins (routes scan, bundler,
    // write_prerender) all expect absolute paths so a nested `root.join`
    // cannot double the prefix — `PathBuf::join` returns the RHS unchanged
    // when it is absolute.
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    // §16.5 / WP-r9-1: OXIDE_ROOT is written PRE-RUNTIME by
    // `apply_startup_env` (P1 readiness env-race fix); nothing to do here.
    let dist = rootp.join("dist");

    // §5.1 early cutoff. The fingerprint walk is only paid when the previous
    // output is present — a clean tree short-circuits before any I/O.
    // `OXIDE_FORCE_BUILD=1` bypasses the skip for a build that must run.
    if dist.join("client/client.manifest.json").is_file()
        && dist.join("server/server.manifest.json").is_file()
        && std::env::var_os("OXIDE_FORCE_BUILD").is_none()
    {
        let fp = build_cache::fingerprint(
            &rootp,
            "production",
            r#static,
            &build_cache::compiler_backend_hint(),
        );
        if build_cache::is_up_to_date(&rootp, &fp) {
            outln!(w, "build up to date (no tracked input changed) — skipping");
            return Ok(None);
        }
    }

    let mut cfg = load_config_strict(&rootp, "production")?;

    // §16.5 / WP-r9-1: emit content entries BEFORE the
    // prerender phase. A prerendered loader that calls
    // `getCollection` must see the current build's data — not
    // the previous build's (or nothing, on a first build).
    emit_content_search_indexes(&rootp, &cfg, &dist, w)?;
    // V2-F09: the built-in search engine index + static fallback (the
    // `search.collections` corpus; a no-op when unconfigured or when the
    // `search` feature is off — the pre-F09 shape).
    #[cfg(feature = "search")]
    emit_search_engine_index(&rootp, &cfg, &dist, w)?;
    // §15.2: `config` runs before the build uses the config; a plugin may
    // return a patch object. `configResolved` runs after, read-only. Both
    // need a pool, which the build has not built yet — so they fire through
    // a throwaway tooling pool created here, before the framework stack.
    // The pool is cheap (one Node child) and only built when a plugin
    // declares either hook.
    if plugin_declares_any(&rootp, &cfg, &["config", "configResolved"]) {
        // `tooling_pool` is a method on the `JsHost` trait, not an inherent
        // method — the trait must be in scope.
        use oxide_fw_jshost::JsHost as _;
        let host =
            oxide_fw_jshost::NodeHost::new(oxide_fw_jshost::NodeHost::worker_script_default());
        if let Ok(pool) = host.tooling_pool(oxide_fw_jshost::PoolConfig::default()) {
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
    let scanned = scan_routes_with_plugins(&rootp, &mut cfg).await?;
    let tree = refine_render_modes_from_ast(&rootp, &cfg, scanned);
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
    // §V2-A09: the build-time prerender imports route/layout modules from
    // the plugin-transformed SERVER BUNDLE (the pass runs after
    // phase_bundle_server), so its renderer is constructed with
    // `Bundle(dist/server)` — the SPEC-17 remainder. Dev, serve, replay and
    // every other caller keep `ModuleSource::Source`.
    let stack = framework_shim::build_framework_stack(
        config_path_opt.as_deref(),
        cfg.server.backend,
        transform_handler,
        Some(&cfg.cache.handler),
        oxide_fw_jshost::ModuleSource::Bundle(dist.join("server")),
    )
    .await;
    if let Some(name) = stack.renderer_name() {
        outln!(w, "framework renderer: {}", name);
    }
    // >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
    Ok(Some(BuildCtx {
        rootp,
        dist,
        w,
        r#static,
        explain,
        attest,
        key_path,
        cfg,
        tree,
        routes_root,
        stack,
        entries: None,
        client_out: None,
        server_out: None,
        boot_chunk_url: None,
        server_precompiled: None,
        build_secret_for_fp: None,
        css_transform: None,
        client_opts: None,
        server_opts: None,
        rb: None,
        transform_html_hook: None,
        client_outcome: None,
        client_manifest: None,
        client_manifest_snapshot: None,
        boot_script_url: None,
        boot_script_integrity: None,
        manifest_integrity: None,
        head_producer: None,
        server_manifest: None,
        build_id: None,
        prerendered: None,
        seed_bodies: None,
        static_override: None,
        new_prerender_records: None,
    }))
    // <<< phase-wrapper
}

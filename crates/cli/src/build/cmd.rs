//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

pub(crate) async fn cmd_build(
    root: &str,
    r#static: bool,
    explain: bool,
    attest: bool,
    key_path: Option<&str>,
    w: Writer<'_>,
) -> anyhow::Result<i32> {
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
            return Ok(0);
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
            oxide_fw_router::RouteKind::Page
                | oxide_fw_router::RouteKind::Endpoint
                | oxide_fw_router::RouteKind::Loading
                // §8.2: layouts are client entries too — the client hydrate
                // rebuilds the server's `<RootLayout><Page/></RootLayout>`
                // tree via `componentFor`, which needs the layout chunk in
                // `client.manifest.json`. Without this the client can only
                // hydrate a bare page and React reports #418.
                | oxide_fw_router::RouteKind::Layout
                // §6.4 / WP-A1: the generated SSR entry is the component
                // surface; it must include the error / not-found / template
                // modules the renderer resolves on failures and navigation.
                | oxide_fw_router::RouteKind::Error
                | oxide_fw_router::RouteKind::NotFound
                | oxide_fw_router::RouteKind::Template
        ) {
            if matches!(e.kind, oxide_fw_router::RouteKind::Loading) {
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
    use oxide_fw_bundler::Bundler as _;
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
            .any(|e| matches!(e.kind, oxide_fw_router::RouteKind::Page));
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
                cfg.offline.enabled && cfg.sw.enabled,
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
    // §V2-C01 tail: the registry of compiled-in native backends (empty in
    // the default build — C02–C04's features add theirs) and the resolved
    // `framework.compilerBackend`. `compile_one` consults `pick` ahead of
    // the pool branch; the registry rides in an `Arc` so the blocking
    // compile closure can re-pick without a `'static` borrow (no `unsafe`).
    let native_registry =
        std::sync::Arc::new(oxide_fw_compiler::native::NativeRegistry::from_enabled_features());
    let compiler_backend = match cfg.framework.compiler_backend {
        oxide_fw_config::CompilerBackend::Auto => oxide_fw_compiler::native::Backend::Auto,
        oxide_fw_config::CompilerBackend::Native => oxide_fw_compiler::native::Backend::Native,
        oxide_fw_config::CompilerBackend::Node => oxide_fw_compiler::native::Backend::Node,
    };
    let client_precompiled = precompile::precompile_entries(precompile::PrecompilePlan {
        pool: stack.tooling_pool.as_ref(),
        entries: &entries,
        svelte_generate: "dom",
        framework: framework_name,
        cache: precompile_cache.as_ref(),
        tool_version,
        native: Some(std::sync::Arc::clone(&native_registry)),
        backend: compiler_backend,
    })
    .await?;
    let server_precompiled = precompile::precompile_entries(precompile::PrecompilePlan {
        pool: stack.tooling_pool.as_ref(),
        entries: &server_entries,
        svelte_generate: "ssr",
        framework: framework_name,
        cache: precompile_cache.as_ref(),
        tool_version,
        native: Some(std::sync::Arc::clone(&native_registry)),
        backend: compiler_backend,
    })
    .await?;
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
    let transform_hook: Option<oxide_fw_bundler::TransformHook> = match (js_hook, wasm_hook) {
        (Some(js), Some(wasm)) => Some(oxide_fw_bundler::TransformHook::new(move |id, src| {
            let js_out = js.call(id, src)?;
            let wasm_out = wasm.call(id, &js_out.code)?;
            Ok(compose_transform_outputs(js_out, wasm_out))
        })),
        (Some(js), None) => Some(js),
        (None, Some(wasm)) => Some(wasm),
        (None, None) => None,
    };
    // §6.6: Lightning CSS for every emitted stylesheet.
    let tw_output = match stack.tooling_pool.clone() {
        Some(pool) => tailwind::precompute(pool, &cfg, &rootp).await?,
        None => None,
    };

    // [Phase 2 I4] Copy public/ → dist/ so `oxide build` is standalone deployable.
    let public_dir = rootp.join("public");
    if public_dir.is_dir() {
        // SEC-STATIC: propagate instead of swallowing — a build that could not
        // copy public/ must say so (missing favicon/robots is invisible otherwise).
        copy_tree(&public_dir, &rootp.join("dist"))
            .map_err(|e| anyhow::anyhow!("copying public/ into dist/: {e}"))?;
    }

    let css_transform = build_css_transform_hook(&cfg, tw_output.map(std::sync::Arc::new));

    let client_opts = bundle_env_options(BundleEnvArgs {
        env: oxide_fw_bundler::BundleEnv::Client,
        entries: client_entries,
        out_dir: client_out.clone(),
        precompiled: client_precompiled,
        jsx_import_source: jsx_source.clone(),
        build_secret: build_secret.clone(),
        transform_hook: transform_hook.clone(),
        css_transform: css_transform.clone(),
        ssr_externals: Vec::new(),
        sourcemap_on: cfg.build.sourcemap != "off",
        minify: true,
    });
    let server_opts = bundle_env_options(BundleEnvArgs {
        env: oxide_fw_bundler::BundleEnv::Ssr,
        entries: server_entries.clone(),
        out_dir: server_out.clone(),
        precompiled: server_precompiled.clone(),
        jsx_import_source: jsx_source,
        build_secret,
        transform_hook,
        css_transform: css_transform.clone(),
        ssr_externals: cfg.build.ssr_externals.clone(),
        sourcemap_on: cfg.build.sourcemap != "off",
        minify: true,
    });
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
    let rb = oxide_fw_bundler::RolldownBundler::new(&rootp);
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
                    // V2-C07: honor `svg.optimize` at the one import hook
                    // the tree ships today (`.svg?react`). The knob-off
                    // path is byte-identical; every refusal (the feature
                    // missing, a preservation rule, a backend failure)
                    // warns with its diagnostic code and keeps the source.
                    let raw = oxide_fw_assets::svg::optimize_for_build(&raw, cfg.svg.optimize);
                    let display = svg_path.to_string_lossy().to_string();
                    match oxide_fw_bundler::svg_react::svg_to_react_component(&raw, &display) {
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
                    let url_base = format!("/_oxide/image?url=/{rel_url}");
                    let srcset = oxide_fw_assets::srcset(&widths, &url_base);
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
        oxide_fw_bundler::BundleOutput {
            modules: Vec::new(),
            chunks: Vec::new(),
            client_manifest: None,
            server_manifest: None,
            diagnostics: Vec::new(),
        }
    } else {
        rb.bundle(&client_opts).map_err(|e| {
            persist_diagnostics(&rootp, std::slice::from_ref(&e.0));
            // The Diagnostic message flattens the error chain; print the full
            // chain on stderr so a bundler failure is debuggable without
            // re-running under a debugger.
            eprintln!("bundler (client) failed — full chain: {e:#}");
            anyhow::anyhow!("{} {}", e.0.code, e.0.message)
        })?
    };
    // SPEC-02 / §4.6.3: a V8 build whose server modules need Node (a `.node`
    // addon or an unshimmed `node:` builtin) must fail loudly. The bundler
    // externalizes `node:` specifiers, so scanning the emitted JS misses them;
    // scan the source server modules instead.
    if matches!(cfg.server.backend, oxide_fw_config::JsBackend::V8) {
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
                        if let Some(reason) = oxide_fw_jshost::v8_loader::requires_node_backend(
                            &format!("node:{bname}"),
                        ) {
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
            let d = oxide_fw_diagnostics::Diagnostic::error("JSH-V8KIND", msg);
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
                            let rhs = after.split_once('=').map(|x| x.1).unwrap_or("");
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
        let mut hits: Vec<oxide_fw_diagnostics::Diagnostic> = Vec::new();
        for (sp, ident) in &server_exports {
            for (chunk, code) in &client_code {
                if code.contains(ident.as_str()) {
                    // 19-E: locate the literal in its source file so the
                    // diagnostic can show the offending line. `ident` is the
                    // string literal the guard found (>= 8 chars), so a
                    // substring search for it in the quoted source is exact
                    // enough to name the line.
                    let mut diag = oxide_fw_diagnostics::Diagnostic::error(
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
                    if let Ok(src) = std::fs::read_to_string(sp)
                        && let Some((line_no, col)) = find_literal_line(&src, ident)
                    {
                        diag = diag.at(oxide_fw_diagnostics::Location::file(
                            sp.to_string_lossy(),
                            line_no as u32,
                            col as u32,
                        ));
                        let frame =
                            oxide_fw_diagnostics::render_code_frame(&src, line_no, Some(col));
                        if !frame.is_empty() {
                            diag = diag.with_code_frame(frame);
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
    let secret_hits = oxide_fw_bundler::scan_client_secrets(&client_outcome.chunks);
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
    if !skip_client && let Err(e) = build_cache::env_store(&rootp, "client", &client_fp) {
        eprintln!("warn: could not store client env fingerprint: {e}");
    }
    let server_outcome = rb.bundle(&server_opts).map_err(|e| {
        let mut diags = client_outcome.diagnostics.clone();
        diags.push((*e.0).clone());
        persist_diagnostics(&rootp, &diags);
        eprintln!("bundler (server) failed — full chain: {e:#}");
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
    // V2-E04: the manifest's integrity digest for the boot chunk, stamped
    // onto the boot `<script type=module>` tag when `security.sri` is on
    // (the default). A manifest written by a pre-E04 bundler carries no
    // `integrity` map — the lookup returns `None` and the tag stays
    // byte-identical, so an incremental rebuild cannot emit a digest that
    // does not match the chunk it names.
    let boot_script_integrity: Option<String> = if cfg.security.sri {
        boot_script_url
            .as_deref()
            .and_then(|url| manifest_integrity_for(client_manifest.get("integrity"), url))
    } else {
        None
    };
    // V2-E04: the manifest's whole integrity map for the Injector (the
    // prerendered shells attach it; a modulepreload/stylesheet link whose
    // URL resolves to a digest emits `integrity` + `crossorigin`). Gated on
    // `security.sri` like the boot tag — off keeps every link byte-identical.
    let manifest_integrity: std::collections::BTreeMap<String, String> = if cfg.security.sri {
        client_manifest
            .get("integrity")
            .and_then(|v| v.as_object())
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        std::collections::BTreeMap::new()
    };
    // V2-E04 tail: the adapter-level head producer — the manifest-driven
    // projection of each route's modulepreload/stylesheet URLs (the seam
    // the E04 record left wired-and-ready). The typed parse runs beside
    // the JSON `Value` above (which stays authoritative for the locale
    // stamp and the boot lookups); a manifest that fails the typed parse
    // degrades to no producer — no links, the pre-milestone bytes — with
    // a loud warning, never a build failure.
    let head_producer: Option<oxide_fw_bundler::head::ManifestHeadProducer> =
        match serde_json::from_value::<oxide_fw_bundler::ClientManifest>(client_manifest.clone()) {
            Ok(typed) => Some(oxide_fw_bundler::head::ManifestHeadProducer::new(typed)),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "client.manifest.json does not type-parse; prerender shells carry no manifest head links"
                );
                None
            }
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
    let nonce = oxide_fw_bundler::ChunkBundler::content_hash(&[build_id.as_bytes()]);
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
        if !matches!(e.kind, oxide_fw_router::RouteKind::Page) {
            continue;
        }

        // Load the accompanying `page.server.ts` (if any) for generateParams.
        let server_mod = rootp
            .join(&cfg.routes.dir)
            .join(e.file.with_file_name("page.server.ts"));
        let gen_params = if server_mod.is_file() {
            let src = std::fs::read_to_string(&server_mod).unwrap_or_default();
            oxide_fw_compiler::extract_generate_params(&src)
        } else {
            oxide_fw_compiler::GenerateParamsResult::Absent
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
                if !matches!(candidate.kind, oxide_fw_router::RouteKind::Layout) {
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
                        le.id.0 == *lid && matches!(le.kind, oxide_fw_router::RouteKind::Layout)
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
                if !matches!(candidate.kind, oxide_fw_router::RouteKind::Error) {
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
        // V2-E04 tail: the route's manifest-derived head links, resolved
        // once per entry (the head is route-shaped, not URL-shaped — the
        // same reading the plugin head's comment carries). Both prerender
        // arms below hand the same links to every target of the route.
        use oxide_fw_bundler::head::HeadProducer as _;
        let head_links = head_producer.as_ref().and_then(|p| p.head_for(&route_id));
        match gen_params {
            oxide_fw_compiler::GenerateParamsResult::Literal(params) => {
                // Prerendered output exists → effective mode `static` for the
                // manifest, even if the route also has a server module.
                static_override.insert(route_id.clone());
                let locales = prerender_locales(&cfg);
                let mut targets = Vec::with_capacity(params.len() * locales.len());
                for params_map in &params {
                    // Substitute params into the segment list to build a path.
                    let mut segs: Vec<String> = e.segments.clone();
                    for s in segs.iter_mut() {
                        if let Some(name) = param_name(s)
                            && let Some(v) = params_map.get(name)
                        {
                            *s = v.clone();
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
                let (rows, reused) = prerender_targets(PrerenderTargetsArgs {
                    dist: &dist,
                    route_id: &route_id,
                    pattern: &pattern,
                    targets,
                    nonce: &nonce,
                    nonce_for: &nonce_for,
                    page_renderer: stack.page_renderer.as_ref(),
                    loader_runner: stack.loader_runner.as_ref(),
                    module_path: module_path.as_deref(),
                    boot_script_url: boot_script_url.as_deref(),
                    boot_script_integrity: boot_script_integrity.as_deref(),
                    manifest_integrity: &manifest_integrity,
                    head_links: head_links.as_ref(),
                    layout_ids: &layout_ids,
                    layout_modules: &prerender_layout_modules,
                    error_route: prerender_error_route_id.as_deref(),
                    plugin_head: &plugin_head,
                    prev_prerenders: &prev_prerenders,
                    route_src_hash: &route_src_hash,
                    prefetch: cfg.prefetch.learned,
                })
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
            oxide_fw_compiler::GenerateParamsResult::Runtime { .. } => {
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
                            let host =
                                oxide_fw_jshost::LoaderHost::new(std::sync::Arc::clone(pool));
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
                                if let Some(name) = param_name(s)
                                    && let Some(v) = params_map.get(name)
                                {
                                    *s = v.clone();
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
                        let (rows, reused) = prerender_targets(PrerenderTargetsArgs {
                            dist: &dist,
                            route_id: &route_id,
                            pattern: &pattern,
                            targets,
                            nonce: &nonce,
                            nonce_for: &nonce_for,
                            page_renderer: stack.page_renderer.as_ref(),
                            loader_runner: stack.loader_runner.as_ref(),
                            module_path: module_path.as_deref(),
                            boot_script_url: boot_script_url.as_deref(),
                            boot_script_integrity: boot_script_integrity.as_deref(),
                            manifest_integrity: &manifest_integrity,
                            head_links: head_links.as_ref(),
                            layout_ids: &layout_ids,
                            layout_modules: &prerender_layout_modules,
                            error_route: prerender_error_route_id.as_deref(),
                            plugin_head: &plugin_head,
                            prev_prerenders: &prev_prerenders,
                            route_src_hash: &route_src_hash,
                            prefetch: cfg.prefetch.learned,
                        })
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
            oxide_fw_compiler::GenerateParamsResult::Absent => {
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
                                // V2-A07: the reused page keeps its sidecar
                                // current (write-or-clear by the record's
                                // status) — same contract as the batch path.
                                if let Err(e) =
                                    oxide_fw_server::write_status_sidecar(&dist.join(&rel), status)
                                {
                                    tracing::warn!(
                                        rel = %rel,
                                        error = %e,
                                        "prerender reuse: status sidecar persist failed"
                                    );
                                }
                                (fresh, status)
                            } else {
                                write_prerender(PrerenderArgs {
                                    dist: &dist,
                                    rel: &rel,
                                    route_id: &route_id,
                                    pattern: &pattern,
                                    url_path: &url_path,
                                    params: &empty_params,
                                    locale,
                                    nonce: &nonce_for,
                                    page_renderer: stack.page_renderer.as_ref(),
                                    loader_runner: stack.loader_runner.as_ref(),
                                    module_path: module_path.as_deref(),
                                    boot_script_url: boot_script_url.as_deref(),
                                    boot_script_integrity: boot_script_integrity.as_deref(),
                                    manifest_integrity: &manifest_integrity,
                                    head_links: head_links.as_ref(),
                                    layouts: &layout_ids,
                                    layout_module_paths: &prerender_layout_modules,
                                    error_route_id: prerender_error_route_id.as_deref(),
                                    plugin_head: &plugin_head,
                                    prefetch: cfg.prefetch.learned,
                                })
                                .await?
                            }
                        }
                        None => {
                            write_prerender(PrerenderArgs {
                                dist: &dist,
                                rel: &rel,
                                route_id: &route_id,
                                pattern: &pattern,
                                url_path: &url_path,
                                params: &empty_params,
                                locale,
                                nonce: &nonce_for,
                                page_renderer: stack.page_renderer.as_ref(),
                                loader_runner: stack.loader_runner.as_ref(),
                                module_path: module_path.as_deref(),
                                boot_script_url: boot_script_url.as_deref(),
                                boot_script_integrity: boot_script_integrity.as_deref(),
                                manifest_integrity: &manifest_integrity,
                                head_links: head_links.as_ref(),
                                layouts: &layout_ids,
                                layout_module_paths: &prerender_layout_modules,
                                error_route_id: prerender_error_route_id.as_deref(),
                                plugin_head: &plugin_head,
                                prefetch: cfg.prefetch.learned,
                            })
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
                    Ok(Some(new_html))
                        if new_html != html && std::fs::write(&path, &new_html).is_ok() =>
                    {
                        touched += 1;
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
            oxide_fw_assets::route_search::build_route_search_index(&prerendered, &dist);
        if let Ok(json) = oxide_fw_assets::route_search::route_search_index_to_json(&search_index) {
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
            // Micro-debt retirement (the precompile/Watcher class): a seed
            // write failure used to vanish (`let _ =`), so `oxide start`
            // booted without L0 seeding and nobody could tell why the first
            // request was a miss. Warn with the path and the error.
            if let Err(e) = std::fs::write(&path, bytes) {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "l0 seed manifest write failed; first requests will be L0 misses"
                );
            }
        } else {
            tracing::warn!(
                path = %path.display(),
                "l0 seed manifest serialize failed; first requests will be L0 misses"
            );
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
        match oxide_fw_assets::i18n::load_catalog_from_dir(&locales_root) {
            Ok(cat) if !cat.entries.is_empty() => {
                let out_path = rootp.join(".oxide/types/messages.d.ts");
                if let Err(e) = oxide_fw_assets::i18n::write_messages_dts(&cat, &out_path) {
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
                    if !matches!(e.kind, oxide_fw_router::RouteKind::Page) {
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
                    oxide_fw_router::RouteKind::Error
                        | oxide_fw_router::RouteKind::NotFound
                        | oxide_fw_router::RouteKind::Template
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
        oxide_fw_adapters::Target::Static
    } else {
        oxide_fw_adapters::Target::parse(effective_adapter).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown adapter `{}`; expected one of binary|node|static|docker|cloudflare|lambda|vercel|netlify|deno|bun (§18)",
                cfg.adapter
            )
        })?
    };

    // V2-R02's config half (§18 refusal rule, spec §V2-R02): refuse the
    // config keys / project surfaces the chosen target cannot honor BEFORE
    // any deploy-artifact work — an Unsupported cell fails the build (the
    // registry code, e.g. `AST-LIV-001`), a Degraded one proceeds with the
    // documented degradation string as a warning.
    {
        let _span = tracing::debug_span!("cli_check_config_refusal").entered();
        let mut facts = project_facts_from_tree(&tree, &static_override);
        facts.rust_fns = has_rust_fn_sources(routes_root.parent().unwrap_or(routes_root.as_path()));
        let findings = oxide_fw_adapters::check_config(&cfg.feature_report(facts), target);
        for d in findings
            .iter()
            .filter(|d| d.severity == oxide_fw_diagnostics::Severity::Warning)
        {
            tracing::warn!(code = %d.code, target = %target.as_str(), "{}", d.message);
            outln!(w, "warn: [{}] {}", d.code, d.message);
        }
        if let Some(d) = findings
            .iter()
            .find(|d| d.severity == oxide_fw_diagnostics::Severity::Error)
        {
            anyhow::bail!("{} {}", d.code, d.message);
        }
    }

    let static_fallback = cfg.static_export.fallback.clone();
    let artifact = oxide_fw_adapters::BuildArtifact {
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
    if target == oxide_fw_adapters::Target::Cloudflare {
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
        oxide_fw_adapters::Target::Cloudflare | oxide_fw_adapters::Target::CloudflarePages
    ) && let Some(edge_entry) = find_edge_entry(&rootp)
    {
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

    let written = oxide_fw_adapters::emit(target, &artifact, &dist.join("deploy"))
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
    // V2-E07: dist/llms.txt + dist/llms-full.txt, when `llms.enabled` —
    // the LLM-facing route index next to the sitemap.
    llms::emit_llms(&cfg, &dist, w)?;
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
    // V2-A04 step 1 (the write half): stamp the canonical build id over the
    // final dist. Every covered write (dist/client/**, dist/server/**,
    // .oxide/routes.manifest) is done by now — the i18n manifest stamp just
    // above is the LAST writer into dist/client — so the id covers the
    // final bytes (the spec's "computed at the end of phase_emit"). The
    // id lands in `dist/build-id` + `.oxide/build-id`; `oxide start` /
    // `oxide preview` read it back through `oxide_fw_cache::read_build_id`
    // and the F10 pools key their generations off it. A build whose
    // identity cannot be stamped fails here — downstream consumers
    // (the data-plane check, attest) must never fall back to a guess.
    let stamped_build_id = oxide_fw_cache::stamp_build_id(&dist)?;
    // V2-A04 delta closure (the deviation in docs/deviations.md): bake the
    // CURRENT id into the prerendered shells. The prerender pass writes the
    // legacy three-field bootstrap because every covered-set tail writer
    // runs after it — an id baked there would be stale by serve time and
    // 409-loop the page. NOW the id exists and no covered writer will run
    // again (the shells live at dist/<page>.html, outside the covered
    // client/server/routes-manifest set, so the baked bytes never re-stamp
    // the identity) — each shell echoes an ACCEPTED id on the data plane
    // instead of riding the headerless allowance.
    let mut baked_shells = 0usize;
    for rels in artifact.prerendered.values() {
        for rel in rels {
            let path = dist.join(rel);
            let Ok(html) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(baked) = oxide_fw_render::bake_build_id(&html, &stamped_build_id)
                && std::fs::write(&path, baked).is_ok()
            {
                baked_shells += 1;
            }
        }
    }
    if baked_shells > 0 {
        outln!(w, "build-id baked into {baked_shells} prerendered shell(s)");
    }
    outln!(
        w,
        "build ok — build-id {} — {} routes, {} prerendered, {} deploy file(s) for `{}` in dist/deploy",
        stamped_build_id,
        tree.entries.len(),
        artifact.prerendered.len(),
        written.len(),
        target.as_str()
    );
    // §5-A: record a DemandGraph over this build's inputs and persist it so
    // `oxide analyze` and the MCP `oxide_fw_graph` tool can report real
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
    let fp = build_cache::fingerprint(
        &rootp,
        "production",
        r#static,
        &build_cache::compiler_backend_hint(),
    );
    if let Err(e) = build_cache::store(&rootp, &fp) {
        eprintln!("warn: could not write build fingerprint: {e}");
    }
    // §V2-F20: `--attest` writes dist/build-id + dist/attest/ (the SBOM,
    // the in-toto provenance and its ed25519 signature) for the dist this
    // build just emitted. A failed attestation fails the build — an
    // attestation that silently did not happen is worse than none.
    if attest {
        cmd_attest_write(&rootp, &dist, key_path, w)?;
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
        let cache_dir = perf_bisect::cache_dir_for(&rootp);
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

    // V2-R02's presentation half (spec §V2-R02): `--explain` prints the
    // chosen target's matrix row at the end of the build — the same lines
    // `oxide doctor` renders, against the target the build actually used.
    if explain {
        outln!(w, "capability matrix for `{}` (§18):", target.as_str());
        for line in capability_row_lines(target) {
            outln!(w, "{line}");
        }
    }
    Ok(0)
}

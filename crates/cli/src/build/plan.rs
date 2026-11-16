//! Moved verbatim from build/cmd.rs by scripts/build_phase_split.py (V2-A01 phase 2).
//!
//! phase_plan — entry sets, boot entry, precompile pass, bundle options
//! (spec WP-A01 phase 2, the front half of `phase_bundle_client`). Builds the
//! client/server entry maps, the synthetic `__boot__` entry, the Tier-B-backed
//! precompile pass for both envs, the config bundle, the transform hooks, the
//! two `BundleOptions` (svg?react + img?w virtual modules included) and the
//! `RolldownBundler`. Produces everything the two bundle phases read.

use crate::*;

// >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
pub(crate) async fn phase_plan(cx: &mut BuildCtx<'_>) -> anyhow::Result<()> {
    let rootp = &cx.rootp;
    let dist = &cx.dist;
    let cfg = &cx.cfg;
    let tree = &cx.tree;
    let stack = &cx.stack;
    let w = &mut *cx.w;
    // <<< phase-wrapper

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
                rootp,
                &framework,
                &islands_for_boot(cfg, tree, rootp, &mut *w)?,
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
        let local = precompile_cache::PrecompileCache::open_soft(rootp);
        if trusted_branch {
            match (&cfg.build.remote_cache, local) {
                (Some(url), Some(c)) => {
                    let remote: std::sync::Arc<dyn oxide_fw_graph::RemoteCache> =
                        std::sync::Arc::new(oxide_fw_graph::HttpRemoteCache::new(url.clone()));
                    match precompile_cache::PrecompileCache::open_with_remote(rootp, remote) {
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
        emit_config_bundle(cfg, rootp, merged, w)?;
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
    let build_secret = build_secret(rootp)?;
    // The env fingerprint needs the secret too, but `build_secret` is
    // moved into `server_opts` below. Clone once at creation.
    let build_secret_for_fp = build_secret.clone();

    // §15.1: a WASM component plugin's transform hook, if the config
    // declares one. Built once, shared by both envs.
    // SPEC-17: the JS plugin bridge (kind: js) composes with the WASM hook
    // (kind: wasm). JS runs first (it sees original source), then WASM.
    let wasm_hook = build_transform_hook(rootp, cfg);
    let js_hook = build_js_plugin_hook(rootp, cfg, stack.tooling_pool.as_ref());
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
        Some(pool) => tailwind::precompute(pool, cfg, rootp).await?,
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

    let css_transform = build_css_transform_hook(cfg, tw_output.map(std::sync::Arc::new));

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
    if plugin_declares_any(rootp, cfg, &["buildStart"]) {
        let _ = fire_lifecycle_hook(
            rootp,
            cfg,
            stack.tooling_pool.as_ref(),
            "buildStart",
            serde_json::json!({ "root": rootp.to_string_lossy() }),
        )
        .await;
    }
    let rb = oxide_fw_bundler::RolldownBundler::new(rootp);
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
    let js_resolve_hook = build_js_plugin_resolve_hook(rootp, cfg, stack.tooling_pool.as_ref());
    let js_load_hook = build_js_plugin_load_hook(rootp, cfg, stack.tooling_pool.as_ref());
    client_opts.resolve_hook = js_resolve_hook.clone();
    client_opts.load_hook = js_load_hook.clone();
    let mut server_opts = server_opts;
    server_opts.resolve_hook = js_resolve_hook;
    server_opts.load_hook = js_load_hook;
    // §15-B: the three post-emit / HTML hooks. renderChunk and generateBundle
    // run inside the bundler for both envs; transformHtml runs on each
    // prerendered document.
    let js_render_chunk_hook =
        build_js_plugin_render_chunk_hook(rootp, cfg, stack.tooling_pool.as_ref());
    let js_generate_bundle_hook =
        build_js_plugin_generate_bundle_hook(rootp, cfg, stack.tooling_pool.as_ref());
    let js_transform_html_hook =
        build_js_plugin_transform_html_hook(rootp, cfg, stack.tooling_pool.as_ref());
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
    // >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
    cx.entries = Some(entries);
    cx.client_out = Some(client_out);
    cx.server_out = Some(server_out);
    cx.boot_chunk_url = boot_chunk_url;
    cx.server_precompiled = Some(server_precompiled);
    cx.build_secret_for_fp = Some(build_secret_for_fp);
    cx.css_transform = Some(css_transform);
    cx.client_opts = Some(client_opts);
    cx.server_opts = Some(server_opts);
    cx.rb = Some(rb);
    cx.transform_html_hook = transform_html_hook;
    Ok(())
    // <<< phase-wrapper
}

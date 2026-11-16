//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// §V2-F01: the `.oxide/islands.json` tooling payload — every registry
/// entry resolved through the config crate's inference order (explicit
/// `framework` → extension → first-line pragma → project framework) and
/// serialized by the compiler-side boot planner. The file read is
/// best-effort: an unreadable island file contributes an empty pragma
/// line, so the inference still answers (the project framework).
pub(crate) fn islands_json_source(
    root: &Path,
    framework: &str,
    islands: &BTreeMap<String, oxide_fw_config::IslandSpec>,
) -> String {
    let mut resolved = BTreeMap::new();
    for (name, spec) in islands {
        let src = spec.src();
        let first_line = std::fs::read(root.join(src))
            .map(|b| {
                let text = String::from_utf8_lossy(&b);
                text.lines().next().unwrap_or("").to_string()
            })
            .unwrap_or_default();
        resolved.insert(
            name.clone(),
            oxide_fw_compiler::islands::ResolvedIsland {
                src: src.to_string(),
                framework: oxide_fw_config::island_framework(
                    src,
                    spec.framework(),
                    framework,
                    &first_line,
                ),
            },
        );
    }
    oxide_fw_compiler::islands::plan_island_boot(framework, &resolved).islands_json()
}

pub(crate) fn canonical_route_id(id: &str) -> String {
    id.strip_suffix("/page").unwrap_or(id).to_string()
}

pub(crate) async fn ensure_dev_client_bundle(
    rootp: &Path,
    cfg: &oxide_fw_config::OxideConfig,
    tree: &oxide_fw_router::RouteTree,
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
    bundler: &oxide_fw_bundler::RolldownBundler,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    let dist = rootp.join("dist");
    let client_out = dist.join("client");
    let manifest_path = client_out.join("client.manifest.json");

    let mut needs_build = !manifest_path.is_file();
    if !needs_build {
        let manifest_mtime = manifest_path
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or_else(|_| std::time::SystemTime::now());

        // §2.9 / BUG 3 follow-up: staleness must consider not just the app's
        // own source tree, but ALSO the framework packages' built dist. A
        // `packages/core/dist/client-hot.js` edit (e.g. a client-runtime bug
        // fix) invalidates the bundled client chunk that inlined the old
        // version — the browser would keep loading the STALE copy from
        // `dist/client/client-<hash>.js` forever, because the app's own src
        // has not changed since the last build.
        //
        // Scanned roots:
        //   1. `<root>/<cfg.routes.dir>` — the app's own source (previous
        //      behavior; only route-tree changes forced rebuild).
        //   2. `<root>/src` (whole) — catches component / island / util
        //      edits that aren't under routes/.
        //   3. `<root>/packages/*/dist` — workspace framework packages
        //      (`pnpm`-linked into the smoke app).
        //   4. `<root>/node_modules/@oxide-fw/*/dist` — published packages
        //      (non-workspace setup).
        //
        // A `packages/core/dist/client-hot.js` newer than the manifest
        // makes needs_build true. This is what made my §3.11 BUG 3 fix
        // invisible to the running dev server.
        let mut scan_roots: Vec<std::path::PathBuf> = vec![rootp.join(&cfg.routes.dir)];
        let src_all = rootp.join("src");
        if src_all.is_dir() {
            scan_roots.push(src_all);
        }
        if let Ok(entries) = std::fs::read_dir(rootp.join("packages")) {
            for entry in entries.flatten() {
                let dist = entry.path().join("dist");
                if dist.is_dir() {
                    scan_roots.push(dist);
                }
            }
        }
        let oxide_fw_nm = rootp.join("node_modules").join("@oxide-fw");
        if let Ok(entries) = std::fs::read_dir(&oxide_fw_nm) {
            for entry in entries.flatten() {
                let dist = entry.path().join("dist");
                if dist.is_dir() {
                    scan_roots.push(dist);
                }
            }
        }

        'outer: for root in scan_roots {
            let mut stack = vec![root];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let Ok(ft) = entry.file_type() else { continue };
                    if ft.is_dir() {
                        stack.push(entry.path());
                    } else if ft.is_file()
                        && let Ok(t) = entry.metadata().and_then(|m| m.modified())
                        && t > manifest_mtime
                    {
                        needs_build = true;
                        break 'outer;
                    }
                }
            }
        }
    }

    if needs_build {
        tracing::info!(
            "ensure_dev_client_bundle: source is newer than manifest or missing — rebuilding"
        );
        build_client_bundle(rootp, cfg, tree, pool, bundler, None, w).await
    } else {
        Ok(())
    }
}

pub(crate) async fn build_client_bundle(
    rootp: &Path,
    cfg: &oxide_fw_config::OxideConfig,
    tree: &oxide_fw_router::RouteTree,
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
    bundler: &oxide_fw_bundler::RolldownBundler,
    changed: Option<&[std::path::PathBuf]>,
    w: Writer<'_>,
) -> anyhow::Result<()> {
    tracing::info!(
        changed = changed.map(|c| c.len()).unwrap_or(0),
        "build_client_bundle ENTRY"
    );
    let dist = rootp.join("dist");
    let client_out = dist.join("client");

    // Entries: pages + endpoints + loading boundaries (mirrors cmd_build).
    let mut entries: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut loading_entries: BTreeMap<String, PathBuf> = BTreeMap::new();
    for e in &tree.entries {
        match e.kind {
            oxide_fw_router::RouteKind::Loading => {
                loading_entries.insert(e.id.0.clone(), rootp.join(&cfg.routes.dir).join(&e.file));
            }
            // §8.2: layouts must be client entries too — the client hydrate
            // rebuilds the server's `<RootLayout><Page/></RootLayout>` tree
            // via `componentFor`, which needs the layout chunk in
            // `client.manifest.json`. Without this the client can only
            // hydrate the bare page and React reports #418.
            oxide_fw_router::RouteKind::Layout => {
                entries.insert(e.id.0.clone(), rootp.join(&cfg.routes.dir).join(&e.file));
            }
            oxide_fw_router::RouteKind::Page
            | oxide_fw_router::RouteKind::Endpoint
            // §6.4 / WP-A1: error / not-found / template components are part
            // of the generated component surface too — the resolver must be
            // able to find them without a hand-written config map.
            | oxide_fw_router::RouteKind::Error
            | oxide_fw_router::RouteKind::NotFound
            | oxide_fw_router::RouteKind::Template => {
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
        .any(|e| matches!(e.kind, oxide_fw_router::RouteKind::Page));
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
            cfg.offline.enabled && cfg.sw.enabled,
        );
        let boot_dir = rootp.join(".oxide");
        std::fs::create_dir_all(&boot_dir)?;
        let boot_path = boot_dir.join("island-boot.mjs");
        std::fs::write(&boot_path, boot_src)?;
        // §V2-F01: the tooling payload rides the boot entry (the same
        // resolution, so the overlay's facts match what the client hydrates).
        std::fs::write(
            boot_dir.join("islands.json"),
            islands_json_source(rootp, &framework, &cfg.framework.islands),
        )?;
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
    let wasm_hook = build_transform_hook(rootp, cfg);
    let js_hook = build_js_plugin_hook(rootp, cfg, pool);
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
    let css_transform = build_css_transform_hook(cfg, None);

    // §6.5 / §6.1: unminified, sourcemaps on (dev debugging). No precompile
    // pass here — the Oxc path handles TS/JSX; SFC precompile is a build-time
    // optimization, and a dev start should not block on the tooling pool.
    let mut client_opts = bundle_env_options(BundleEnvArgs {
        env: oxide_fw_bundler::BundleEnv::Client,
        entries: client_entries,
        out_dir: client_out,
        precompiled: BTreeMap::new(),
        jsx_import_source: jsx_source,
        build_secret,
        transform_hook,
        css_transform,
        ssr_externals: Vec::new(),
        sourcemap_on: true,
        minify: false,
    });
    client_opts.resolve_hook = build_js_plugin_resolve_hook(rootp, cfg, pool);
    client_opts.load_hook = build_js_plugin_load_hook(rootp, cfg, pool);
    client_opts.render_chunk_hook = build_js_plugin_render_chunk_hook(rootp, cfg, pool);
    client_opts.generate_bundle_hook = build_js_plugin_generate_bundle_hook(rootp, cfg, pool);
    outln!(w, "dev: rebuilding client bundle...");
    use oxide_fw_bundler::Bundler as _;
    match bundler.bundle_incremental(&client_opts, changed) {
        Ok(outcome) => {
            // [wr115] Write compiled Tailwind CSS directly + patch manifest.
            if let Some(p) = pool
                && let Ok(Some(tw)) = crate::tailwind::precompute(p.clone(), cfg, rootp).await
            {
                let client_dir = rootp.join("dist/client");
                let _ = std::fs::create_dir_all(&client_dir);
                let hash = oxide_fw_bundler::ChunkBundler::content_hash(&[tw.css.as_bytes()]);
                let filename = format!("tailwind-{hash}.css");
                let _ = std::fs::write(client_dir.join(&filename), &tw.css);
                outln!(
                    w,
                    "tailwind: {} ({} bytes, {} utilities)",
                    filename,
                    tw.css.len(),
                    tw.css.matches('.').count()
                );

                // Update the manifest: point every route's css at the
                // compiled file, delete any other tailwind-*.css.
                let manifest_path = client_dir.join("client.manifest.json");
                if let Ok(raw) = std::fs::read_to_string(&manifest_path)
                    && let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&raw)
                {
                    if let Some(routes) = v.get_mut("routes").and_then(|r| r.as_object_mut()) {
                        for (_k, route) in routes.iter_mut() {
                            if let Some(css) = route.get_mut("css").and_then(|c| c.as_array_mut()) {
                                *css = vec![serde_json::Value::String(filename.clone())];
                            }
                        }
                    }
                    if let Some(assets) = v.get_mut("assets").and_then(|a| a.as_object_mut()) {
                        for (_k, val) in assets.iter_mut() {
                            *val = serde_json::Value::String(filename.clone());
                        }
                    }
                    let _ = std::fs::write(
                        &manifest_path,
                        serde_json::to_string_pretty(&v).unwrap_or_default(),
                    );
                }
                // Delete superseded CSS so exactly one exists
                if let Ok(entries) = std::fs::read_dir(&client_dir) {
                    for e in entries.flatten() {
                        let n = e.file_name().to_string_lossy().to_string();
                        if n.starts_with("tailwind-") && n.ends_with(".css") && n != filename {
                            let _ = std::fs::remove_file(e.path());
                        }
                    }
                }
            }

            outln!(
                w,
                "dev: client bundle rebuilt ({} chunks)",
                outcome.chunks.len()
            );
            Ok(())
        }
        Err(e) => {
            // A failed dev bundle is reported but not fatal: the SSR-only
            // page still serves (no client hydration). But the caller MUST
            // know it failed — publishing `FullReload` after a failed
            // rebuild is what causes "flash of change then revert": the
            // browser reloads, gets fresh SSR HTML, then hydrates with the
            // OLD client bundle that was never overwritten.
            outln!(
                w,
                "dev: client bundle failed ({} {}); NOT publishing FullReload",
                e.0.code,
                e.0.message
            );
            Err(anyhow::anyhow!(
                "dev client bundle failed: {} {}",
                e.0.code,
                e.0.message
            ))
        }
    }
}

/// True when any event touches a configured content-collection directory.
/// The dev loop rewrites `dist/content` when this is true; `getCollection`
/// then re-reads by content hash, so no worker restart or cache clear is
/// needed.
pub(crate) fn is_client_source(path: &Path, rootp: &Path) -> bool {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    let result;
    if lower.ends_with(".server.ts") || lower.ends_with(".server.tsx") {
        result = false;
        tracing::info!(path = %path.display(), reason = "server-ext", "DIAG is_client_source");
        return result;
    }
    if !matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("ts")
            | Some("tsx")
            | Some("js")
            | Some("jsx")
            | Some("vue")
            | Some("svelte")
            | Some("mjs")
            | Some("cjs")
    ) {
        result = false;
        tracing::info!(path = %path.display(), ext = ?path.extension(), reason = "bad-ext", "DIAG is_client_source");
        return result;
    }
    let canon_root = std::fs::canonicalize(rootp).unwrap_or_else(|_| rootp.to_path_buf());
    let canon_src = canon_root.join("src");
    let canon_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    result = canon_path.starts_with(&canon_src);
    tracing::info!(
        path = %canon_path.display(),
        src = %canon_src.display(),
        result = result,
        "DIAG is_client_source"
    );
    result
}

pub(crate) fn content_source_changed(
    root: &Path,
    cfg: &oxide_fw_config::OxideConfig,
    events: &[oxide_fw_dev::FileEvent],
) -> bool {
    events.iter().any(|ev| {
        let p = match ev {
            oxide_fw_dev::FileEvent::Modified(p)
            | oxide_fw_dev::FileEvent::Created(p)
            | oxide_fw_dev::FileEvent::Removed(p) => p,
        };
        cfg.content
            .collections
            .iter()
            .any(|c| !c.dir.is_empty() && p.starts_with(root.join(&c.dir)))
    })
}

/// [wr200] Dev-mode fix (plan T5): keep exactly ONE `tailwind-*.css` in
/// `dist/client/` and rewrite `client.manifest.json` to point every route's
/// `css` array at it. Without this, HMR rebuilds leave the manifest pointing
/// at a stale hash, so client-side navigation swaps to the old CSS and loses
/// new styles (e.g. docs-shell after a wr198-style source change).
pub(crate) fn consolidate_tailwind_css_and_manifest(rootp: &std::path::Path) {
    let client_dir = rootp.join("dist/client");
    let Ok(entries) = std::fs::read_dir(&client_dir) else {
        return;
    };

    // Find all tailwind-*.css files, newest last
    let mut css_files: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("tailwind-") && n.ends_with(".css"))
                .unwrap_or(false)
        })
        .collect();

    if css_files.is_empty() {
        return;
    }

    // Sort by mtime ascending (oldest first, newest last)
    css_files.sort_by_key(|p| {
        p.metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
    });

    let newest = match css_files.pop() {
        Some(p) => p,
        None => return,
    };
    let newest_name = newest
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_string())
        .unwrap_or_default();

    // Delete all older tailwind CSS files
    for old in &css_files {
        let _ = std::fs::remove_file(old);
    }

    // Rewrite the manifest's css arrays to point at the newest file
    let manifest_path = client_dir.join("client.manifest.json");
    let Ok(raw) = std::fs::read_to_string(&manifest_path) else {
        return;
    };
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return;
    };

    let mut changed = false;
    if let Some(routes) = v.get_mut("routes").and_then(|r| r.as_object_mut()) {
        for (_route_id, route) in routes.iter_mut() {
            if let Some(css) = route.get_mut("css").and_then(|c| c.as_array_mut()) {
                // Replace any tailwind-*.css entries with the newest
                let mut new_css: Vec<serde_json::Value> = Vec::new();
                let mut had_tailwind = false;
                for entry in css.iter() {
                    if let Some(name) = entry.as_str() {
                        if name.starts_with("tailwind-") && name.ends_with(".css") {
                            had_tailwind = true;
                            new_css.push(serde_json::Value::String(newest_name.clone()));
                        } else {
                            new_css.push(entry.clone());
                        }
                    }
                }
                if had_tailwind
                    && new_css
                        .iter()
                        .any(|e| e.as_str() == Some(newest_name.as_str()))
                    && (css.len() != new_css.len() || css.first() != new_css.first())
                {
                    *css = new_css;
                    changed = true;
                }
            }
        }
    }

    if changed && let Ok(pretty) = serde_json::to_string_pretty(&v) {
        let _ = std::fs::write(&manifest_path, pretty);
    }
}

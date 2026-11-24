//! Moved verbatim from build/cmd.rs by scripts/build_phase_split.py (V2-A01 phase 2).
//!
//! phase_emit — manifest enrichment, adapter artifacts, stamps, summary
//! (spec WP-A01 phase 2). Infers render modes into the server manifest, fires
//! the adapter hook, runs the §18 config refusal, compiles the edge artifacts,
//! emits the deploy target, the typegen/sitemap/llms/PWA/fonts/snapshot tail,
//! stamps the build id (A04) into dist + the shells, records the demand graph
//! + fingerprint, attests, GCs the artifact cache and prints the summary.

use crate::*;

// >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
pub(crate) async fn phase_emit(cx: &mut BuildCtx<'_>) -> anyhow::Result<()> {
    let rootp = &cx.rootp;
    let dist = &cx.dist;
    let cfg = &cx.cfg;
    let tree = &cx.tree;
    let routes_root = &cx.routes_root;
    let w = &mut *cx.w;
    let r#static = cx.r#static;
    let explain = cx.explain;
    let attest = cx.attest;
    let key_path = cx.key_path;
    let entries = cx.entries.as_ref().expect("phase_plan sets entries");
    let client_out = cx.client_out.as_ref().expect("phase_plan sets client_out");
    let server_out = cx.server_out.as_ref().expect("phase_plan sets server_out");
    let server_precompiled = cx
        .server_precompiled
        .as_ref()
        .expect("phase_plan sets server_precompiled");
    let css_transform = cx
        .css_transform
        .clone()
        .expect("phase_plan sets css_transform");
    let stack = &cx.stack;
    let static_override = cx
        .static_override
        .as_ref()
        .expect("phase_prerender sets static_override");
    let client_manifest = cx
        .client_manifest
        .take()
        .expect("phase_bundle_server sets client_manifest");
    let client_manifest_snapshot = cx
        .client_manifest_snapshot
        .as_ref()
        .expect("phase_bundle_server sets client_manifest_snapshot");
    let mut server_manifest = cx
        .server_manifest
        .take()
        .expect("phase_bundle_server sets server_manifest");
    let build_id = cx
        .build_id
        .as_ref()
        .expect("phase_bundle_server sets build_id");
    let prerendered = cx
        .prerendered
        .take()
        .expect("phase_prerender sets prerendered");
    let mut new_prerender_records = cx
        .new_prerender_records
        .take()
        .expect("phase_prerender sets new_prerender_records");
    // <<< phase-wrapper
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
    if plugin_declares_any(rootp, cfg, &["adapter"]) {
        let results = fire_lifecycle_hook(
            rootp,
            cfg,
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
        let mut facts = project_facts_from_tree(tree, static_override);
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
        match compile_edge_wasm(rootp, dist) {
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
    ) && let Some(edge_entry) = find_edge_entry(rootp)
    {
        match bundle_edge_entry(
            rootp,
            dist,
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
    if plugin_declares_any(rootp, cfg, &["typegen"]) {
        let results = fire_lifecycle_hook(
            rootp,
            cfg,
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
    emit_sitemap_and_robots(cfg, tree, dist, w)?;
    // V2-E07: dist/llms.txt + dist/llms-full.txt, when `llms.enabled` —
    // the LLM-facing route index next to the sitemap.
    llms::emit_llms(cfg, dist, w)?;
    // §16.3: manifest.webmanifest, when a `manifest` block is configured.
    emit_web_manifest(cfg, dist, w)?;
    // SPEC-21 / §16.6: the PWA service worker, when a manifest is configured.
    emit_pwa_worker(cfg, dist, client_manifest_snapshot, w)?;
    // §16.2: bundle configured font families into dist/fonts + fonts.css.
    emit_fonts(cfg, rootp, dist, w)?;
    // §4.4: build a V8 startup snapshot from the server bundle.
    emit_snapshot_blob(rootp, server_out, w)?;
    // §15.2: `buildEnd` — the build finished successfully; a plugin gets the
    // summary. Fired before the "build ok" line so a plugin's own output
    // appears above the summary.
    if plugin_declares_any(rootp, cfg, &["buildEnd"]) {
        let _ = fire_lifecycle_hook(
            rootp,
            cfg,
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
    stamp_i18n_locales(rootp, cfg);
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
    let stamped_build_id = oxide_fw_cache::stamp_build_id(dist)?;
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
    // §V2-F10 step 4: retain the finished dist as a generation copy
    // (`.oxide/generations/<id>/dist`) the rollback path serves — the
    // spec's "written at the end of `phase_emit`". Degrades to a warn:
    // the retention is a rollback convenience, never a build requirement
    // (the same posture the serve-side snapshot takes).
    if let Err(e) = crate::serve::generations::retain_dist_generation(
        rootp,
        &stamped_build_id,
        cfg.deploy.generations,
    ) {
        tracing::warn!(
            code = "SKW-RETAIN",
            msg = %e,
            "skew: the finished dist could not be retained as a rollback generation"
        );
    }
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
            rootp,
            entries,
            client_chunks: &client_chunks,
            server_chunks: &server_chunks,
            client_manifest_path: &cm_path,
            server_manifest_path: &sm_path,
            build_id,
            route_prerenders: std::mem::take(&mut new_prerender_records),
            plugin_options: collect_plugin_options(rootp, cfg),
            // `client_manifest` was moved into the build artifact above;
            // the snapshot clone taken right after it was read is the same
            // value.
            route_chunks: collect_route_chunks(rootp, client_manifest_snapshot),
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
        rootp,
        "production",
        r#static,
        &build_cache::compiler_backend_hint(),
    );
    if let Err(e) = build_cache::store(rootp, &fp) {
        eprintln!("warn: could not write build fingerprint: {e}");
    }
    // §V2-F20: `--attest` writes dist/build-id + dist/attest/ (the SBOM,
    // the in-toto provenance and its ed25519 signature) for the dist this
    // build just emitted. A failed attestation fails the build — an
    // attestation that silently did not happen is worse than none.
    if attest {
        cmd_attest_write(rootp, dist, key_path, w)?;
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
        let cache_dir = perf_bisect::cache_dir_for(rootp);
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
    // >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
    Ok(())
    // <<< phase-wrapper
}

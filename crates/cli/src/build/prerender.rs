//! Moved verbatim from build/cmd.rs by scripts/build_phase_split.py (V2-A01 phase 2).
//!
//! phase_prerender — the §10.5 static-route prerender walk
//! (spec WP-A01 phase 2). Computes the nonce, walks every page route (the
//! literal / runtime / absent `generateParams` arms), reuses unchanged
//! outputs by source hash, runs `transformHtml`, archives the route tree
//! and collects the L0 seed bodies + prerender records.

use crate::*;

// >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
pub(crate) async fn phase_prerender(cx: &mut BuildCtx<'_>) -> anyhow::Result<()> {
    let rootp = &cx.rootp;
    let dist = &cx.dist;
    let cfg = &cx.cfg;
    let tree = &cx.tree;
    let routes_root = &cx.routes_root;
    let stack = &cx.stack;
    let w = &mut *cx.w;
    let boot_script_url = &cx.boot_script_url;
    let boot_script_integrity = &cx.boot_script_integrity;
    let manifest_integrity = cx
        .manifest_integrity
        .as_ref()
        .expect("phase_bundle_server sets manifest_integrity");
    let head_producer = &cx.head_producer;
    let build_id = cx
        .build_id
        .as_ref()
        .expect("phase_bundle_server sets build_id");
    let transform_html_hook = &cx.transform_html_hook;
    // <<< phase-wrapper
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
    let prev_prerender_snapshot = crate::build_graph::load_snapshot(rootp);
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
            plugin_render_head(rootp, cfg, stack.tooling_pool.as_ref(), &route_id).await;
        let route_src_hash = route_source_hash(
            rootp,
            cfg,
            tree,
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
                let locales = prerender_locales(cfg);
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
                        let (rel, url_path) = prefixed_prerender_path(&base_rel, locale, cfg);
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
                    dist,
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
                    manifest_integrity,
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
                        splice_nonce(&html, &nonce, l0_cache::NONCE_SENTINEL),
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
                        let locales = prerender_locales(cfg);
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
                                    prefixed_prerender_path(&base_rel, locale, cfg);
                                targets.push(PrerenderTarget {
                                    rel,
                                    url_path,
                                    params: params_map.clone(),
                                    locale: locale.clone(),
                                });
                            }
                        }
                        let (rows, reused) = prerender_targets(PrerenderTargetsArgs {
                            dist,
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
                            manifest_integrity,
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
                                splice_nonce(&html, &nonce, l0_cache::NONCE_SENTINEL),
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
                let locales = prerender_locales(cfg);
                let mut outs: Vec<String> = Vec::with_capacity(locales.len());
                for locale in &locales {
                    let (rel, url_path) = prefixed_prerender_path(&base_rel, locale, cfg);
                    let (html, prerender_status) = match try_reuse_prerender(
                        &dist.join(&rel),
                        &route_id,
                        &url_path,
                        &route_src_hash,
                        &prev_prerenders,
                    ) {
                        Some((saved, status, old_nonce)) => {
                            let fresh = splice_nonce(&saved, &old_nonce, &nonce);
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
                                    dist,
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
                                    manifest_integrity,
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
                                dist,
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
                                manifest_integrity,
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
                        splice_nonce(&html, &nonce, l0_cache::NONCE_SENTINEL),
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

    // >>> phase-wrapper (not part of the moved body; split_selfcheck --phases strips this block)
    cx.prerendered = Some(prerendered);
    cx.seed_bodies = Some(seed_bodies);
    cx.static_override = Some(static_override);
    cx.new_prerender_records = Some(new_prerender_records);
    Ok(())
    // <<< phase-wrapper
}

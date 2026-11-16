//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

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
///     (the same `oxide_fw_odf::encode_to_bytes` shape the live hydration
///     payload and the L0 cache entries use), carrying the loader's FRESH
///     data. Params are empty: a file change has no request context, and
///     the inline HMR client re-fetches `/__data` for the concrete URL
///     anyway — `odf_b64` is the offline fallback.
///
/// Any miss (not a page's server module, no loader host, loader error,
/// encode error) degrades to `(file_path, "")` at the call site — the old
/// shape — so a misfire is never worse than the previous behavior.
pub(crate) async fn hmr_route_data_update(
    changed: &Path,
    rootp: &Path,
    cfg: &OxideConfig,
    tree: &oxide_fw_router::RouteTree,
    loader: Option<&std::sync::Arc<dyn oxide_fw_server::LoaderRunner>>,
) -> (String, String) {
    // Only a PAGE route's `page.server.ts` triggers a data update (a
    // `layout.server.ts` change maps to no single URL; the watcher falls
    // back to the file-path message for those).
    let routes_root = rootp.join(&cfg.routes.dir);
    let owned = match tree.entries.iter().find(|e| {
        matches!(e.kind, oxide_fw_router::RouteKind::Page)
            && routes_root.join(e.file.with_file_name("page.server.ts")) == changed
    }) {
        Some(e) => (
            // Route id → URL path pattern (`routes/blog/[slug]/page` →
            // `/blog/[slug]`). The `index` segment is the ROOT marker (the
            // scanner ids the root page `routes/index/page`), not a literal
            // URL segment: `routes/index/page` → `/`. Deriving `/index`
            // instead produced a pattern the client's
            // `routePatternMatchesPath` could never match against the
            // pathname `/` — the route-data-update frame arrived and was
            // silently dropped, so a loader edit never re-rendered.
            {
                let dir =
                    e.id.0
                        .strip_suffix("/page")
                        .unwrap_or(&e.id.0)
                        .strip_prefix("routes")
                        .unwrap_or("");
                match dir {
                    "" | "/" | "/index" | "index" => "/".to_string(),
                    d => d.to_string(),
                }
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
    let input =
        oxide_fw_server::LoaderRunInput::new(module_path, route_for_loader, pattern.clone());
    match loader.run(input).await {
        Ok(out) if out.status == 200 => {
            let payload = serde_json::json!({
                "route": pattern,
                "url": pattern,
                "params": {},
                "data": out.data,
            });
            match oxide_fw_odf::encode_to_bytes(&oxide_fw_odf::Value::from_json(&payload)) {
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
/// §6.4 / WP-A2: the canonical route id used by every emitted manifest and
/// by the runtime component resolver. It is the route *directory* form: the
/// scanner's `routes/<...>/page` loses its `/page` suffix; layouts, error,
/// not-found and template ids are already canonical.
/// §8.2 / §16.3 / O1: compute the layout chain for a route id from the
/// router tree. Route ids are the *canonical* form (`/page` stripped), and
/// a `layout.tsx` at each prefix of the id is checked. A page at
/// `routes/nested/about` sees `routes/layout` then `routes/nested/layout`
/// in that order (outermost first) — the same order the router's
/// `layout_chain` produces for a live match.
///
/// Pure and self-contained: walks `tree.entries` looking for `Layout` kind
/// entries whose canonical id matches `<prefix>/layout` at each depth of
/// the route id. No private router state, no lazy-init to trigger — the
/// CLI already has `tree.entries` in scope at every build call site.
/// Rolldown sanitizes `[slug]` → `_slug_` in chunk names. Manifest route
/// keys therefore use the sanitized form while the router tree uses the
/// source form. Map a manifest key back to the canonical route id so
/// layout-chain lookup can find the tree entry.
pub(crate) fn canonicalize_manifest_key(tree: &oxide_fw_router::RouteTree, key: &str) -> String {
    // Direct match: the manifest already carries the source form.
    if tree
        .entries
        .iter()
        .any(|e| e.id.0.strip_suffix("/page").unwrap_or(&e.id.0) == key)
    {
        return key.to_string();
    }
    // Try each tree route's sanitized form against the manifest key.
    for e in &tree.entries {
        let canon = e.id.0.strip_suffix("/page").unwrap_or(&e.id.0);
        let sanitized = canon
            .split('/')
            .map(|seg| {
                if seg.starts_with('[') && seg.ends_with(']') && seg.len() > 2 {
                    format!("_{}_", &seg[1..seg.len() - 1])
                } else {
                    seg.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        if sanitized == key {
            return canon.to_string();
        }
    }
    // Fallback: return as-is. compute_layout_chain will find nothing.
    key.to_string()
}

pub(crate) fn compute_layout_chain(
    tree: &oxide_fw_router::RouteTree,
    route_id: &str,
) -> Vec<String> {
    let segments: Vec<&str> = route_id.split('/').collect();
    let mut chain = Vec::new();
    for i in 0..segments.len() {
        let prefix = segments[..=i].join("/");
        let layout_id = format!("{prefix}/layout");
        if tree
            .entries
            .iter()
            .any(|e| matches!(e.kind, oxide_fw_router::RouteKind::Layout) && e.id.0 == layout_id)
        {
            chain.push(layout_id);
        }
    }
    chain
}

/// §8.2 / §16.3 / O1: enrich a written manifest with layout chains from the
/// router tree. The bundler writes an empty `layout_chain` on every route
/// because it doesn't know the tree; the CLI reads the JSON back, fills in
/// each route's chain, and writes it. Absent/None for older manifests is
/// safe — the field is `#[serde(default)]`.
pub(crate) fn enrich_manifest_layout_chains(
    manifest_path: &std::path::Path,
    tree: &oxide_fw_router::RouteTree,
) -> anyhow::Result<()> {
    let bytes = match std::fs::read(manifest_path) {
        Ok(b) => b,
        Err(_) => return Ok(()), // no manifest yet — nothing to enrich
    };
    let mut doc: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("manifest {} invalid JSON: {e}", manifest_path.display()))?;
    let routes = match doc.get_mut("routes").and_then(|v| v.as_object_mut()) {
        Some(r) => r,
        None => return Ok(()),
    };
    let mut enriched = 0usize;
    for (route_id, entry) in routes.iter_mut() {
        // §8.2 / O1: the manifest key may be in rolldown's sanitized form
        // (`routes/blog/_slug_`), while the router tree uses the source form
        // (`routes/blog/[slug]`). Canonicalize before the layout-chain walk
        // so a param route finds its layouts.
        let canonical = canonicalize_manifest_key(tree, route_id);
        let chain = compute_layout_chain(tree, &canonical);
        if let Some(obj) = entry.as_object_mut() {
            obj.insert(
                "layout_chain".to_string(),
                serde_json::to_value(&chain).unwrap_or(serde_json::Value::Array(Vec::new())),
            );
            if !chain.is_empty() {
                enriched += 1;
            }
        }
    }
    std::fs::write(manifest_path, serde_json::to_string_pretty(&doc)? + "\n")
        .map_err(|e| anyhow::anyhow!("write {}: {e}", manifest_path.display()))?;
    if enriched > 0 {
        tracing::debug!(count = enriched, "enriched manifest layout chains");
    }
    Ok(())
}

/// Populate `stack.component_source` with route → source-file mappings so the
/// dev render path can resolve components without a build-emitted
/// `dist/server/ssr/index.js`. Called from:
///
/// - `cmd_dev` (all modes) — one-time at boot.
/// - `start_server_inner_full` (ESM mode only) — covers the smoke test and
///   any other direct caller of `start_dev_server_full` that bypasses
///   `cmd_dev`. Without this, the ESM path (which skips
///   `ensure_dev_client_bundle` and therefore never writes an SSR entry)
///   has an empty `component_source` and every render 500s with
///   "no component resolved".
///
/// Idempotent: each call writes the same `sources` map.
pub(crate) async fn populate_dev_component_source(
    rootp: &Path,
    cfg: &oxide_fw_config::OxideConfig,
    tree: &oxide_fw_router::RouteTree,
    stack: &crate::framework_shim::FrameworkStack,
) {
    use oxide_fw_router::RouteKind;
    let routes_root = rootp.join(&cfg.routes.dir);
    let entries: std::collections::BTreeMap<String, std::path::PathBuf> = tree
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
        .map(|e| (canonical_route_id(&e.id.0), routes_root.join(&e.file)))
        .collect();
    if entries.is_empty() {
        return;
    }
    let framework = cfg
        .framework
        .adapter
        .as_deref()
        .or(cfg.framework.name.as_deref());
    let sources = crate::framework_shim::dev_component_sources(
        rootp,
        framework,
        entries,
        stack.tooling_pool.as_ref(),
    )
    .await;
    if let Ok(mut src) = stack.component_source.write() {
        src.sources = sources;
    }
}

/// V2-E01: the module-graph inputs of the HMR boundary planner, harvested
/// in one walk over `src/**/*.{ts,tsx,js,jsx,mjs,vue,svelte}` — and, since
/// the phase-2 CSS boundary substrate, `src/**/*.{css,scss,sass}` too:
/// stylesheets join the graph so a stylesheet change plans through
/// [`oxide_fw_dev::boundary::plan_classified`] (the §13.3 terminal makes the
/// swap never a reload). Stylesheets only RECEIVE importer edges
/// (JS→CSS side-effect imports, whose specifiers always carry the
/// extension, so exact-path resolution hits); a CSS `@import` chain needs
/// no edges — the changed child sheet self-accepts and swaps on its own.
pub(crate) struct HmrModuleGraph {
    /// Reverse deps: `module -> { importers }` over canonicalized paths.
    /// Rebuilt per watcher tick — cheap for a normal dev tree (~30 files),
    /// correct without needing to invalidate a cache on Created/Removed
    /// events.
    pub(crate) importers: std::collections::BTreeMap<
        std::path::PathBuf,
        std::collections::BTreeSet<std::path::PathBuf>,
    >,
    /// Self-accepting modules (§V2-E01): component files (the framework
    /// plugin marks component modules as boundaries), sources registering
    /// `import.meta.hot.accept` ([`oxide_fw_dev::source_self_accepts`]) and
    /// the server modules the §13.3 behavior table swaps in place.
    pub(crate) accepting: std::collections::HashSet<std::path::PathBuf>,
    /// Route-structural modules: reaching one without a boundary is the
    /// planner's full-reload arm ("... has no HMR boundary") — their
    /// identity, not just their module body, is load-bearing (the same
    /// prefix list `classify_event` treats as structural).
    pub(crate) entries: std::collections::HashSet<std::path::PathBuf>,
}

/// V2-E01: build the HMR module graph (§V2-E01): the reverse-dependency map
/// the old `build_importers_map` produced, plus the planner's
/// `accepting`/`entries` sets harvested in the SAME walk (one read per
/// file per tick — no second pass). Parses each file with the compiler's
/// `scan_import_spans` and resolves relative imports against the
/// filesystem.
pub(crate) fn build_hmr_graph(
    rootp: &std::path::Path,
    routes_dir: &std::path::Path,
) -> HmrModuleGraph {
    use std::collections::{BTreeMap, HashSet};
    use std::path::PathBuf;
    let canonical_routes =
        std::fs::canonicalize(routes_dir).unwrap_or_else(|_| routes_dir.to_path_buf());
    let mut graph = HmrModuleGraph {
        importers: BTreeMap::new(),
        accepting: HashSet::new(),
        entries: HashSet::new(),
    };
    let src = rootp.join("src");
    let Ok(_) = std::fs::canonicalize(&src) else {
        return graph;
    };

    fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.is_file() {
                let ext = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if matches!(
                    ext.as_str(),
                    "ts" | "tsx"
                        | "js"
                        | "jsx"
                        | "mjs"
                        | "vue"
                        | "svelte"
                        | "css"
                        | "scss"
                        | "sass"
                ) {
                    out.push(p);
                }
            }
        }
    }
    let mut files: Vec<PathBuf> = Vec::new();
    walk(&src, &mut files);
    for file in files {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(canon_file) = std::fs::canonicalize(&file) else {
            continue;
        };
        // Boundary marks, harvested in the same pass that already reads the
        // source: component files are boundaries by framework-plugin
        // convention, `.server.*` files by the §13.3 behavior table,
        // stylesheets by the phase-2 CSS terminal probe
        // (`css_self_accepts` — style injection swaps in place), and a
        // hot-accept registration self-accepts by the spec's definition.
        let lower_name = canon_file.to_string_lossy().to_ascii_lowercase();
        let is_component = lower_name.ends_with(".tsx")
            || lower_name.ends_with(".jsx")
            || lower_name.ends_with(".vue")
            || lower_name.ends_with(".svelte");
        let is_server = lower_name.ends_with(".server.ts") || lower_name.ends_with(".server.tsx");
        let is_stylesheet = oxide_fw_dev::boundary::css_self_accepts(&canon_file);
        if is_component || is_server || is_stylesheet || oxide_fw_dev::source_self_accepts(&content)
        {
            graph.accepting.insert(canon_file.clone());
        }
        // Route-structural files are the planner's entries.
        if canon_file.starts_with(&canonical_routes) {
            let fname = lower_name.rsplit('/').next().unwrap_or("");
            let structural = [
                "page.",
                "layout.",
                "error.",
                "not-found.",
                "loading.",
                "template.",
                "route.",
                "socket.",
            ]
            .iter()
            .any(|prefix| fname.starts_with(prefix));
            if structural {
                graph.entries.insert(canon_file.clone());
            }
        }
        for span in oxide_fw_compiler::scan_import_spans(&content) {
            let spec = &span.specifier;
            if !(spec.starts_with("./") || spec.starts_with("../")) {
                continue;
            }
            let Some(parent) = canon_file.parent() else {
                continue;
            };
            let Ok(target) = std::fs::canonicalize(parent.join(spec)) else {
                continue;
            };
            graph
                .importers
                .entry(target)
                .or_default()
                .insert(canon_file.clone());
        }
    }
    graph
}

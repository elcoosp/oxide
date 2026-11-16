//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// Tolerant config load for informational commands (§14): a missing/invalid config
/// falls back to defaults so `routes`/`doctor` still work on fresh projects.
pub(crate) fn load_config_or_default(root: &Path, mode: &str) -> OxideConfig {
    match oxide_fw_config::load(root, mode, BTreeMap::new()) {
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
/// The structured `OxideError` is preserved, not flattened into a string —
/// the `[CFG-*]` code, the message and the `fix:` help line all survive
/// into the anyhow chain the command boundary prints.
pub(crate) fn load_config_strict(root: &Path, mode: &str) -> anyhow::Result<OxideConfig> {
    oxide_fw_config::load(root, mode, BTreeMap::new())
        .map(|l| l.config)
        .map_err(anyhow::Error::new)
}

/// §10.1: refine each route's render mode with the AST-based inference.
///
/// The scanner sets `render_mode` from a line-oriented regex (fast, and
/// good enough for `oxide routes` without parsing every loader). The CLI
/// has `oxide-fw-compiler`, so before the build consumes the mode it re-runs
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
/// `oxide-fw-router`'s `read_page_config`). Keeps the AST refinement from
/// clobbering an explicit `render: 'dynamic'` with an inferred `static`.
pub(crate) fn has_explicit_render_config(source: &str) -> bool {
    let mut start = 0;
    while let Some(i) = source[start..].find("render") {
        let abs = start + i;
        // `render`, `renderMode` and `render_mode` all declare the mode.
        let rest = &source[abs + "render".len()..];
        let rest = rest
            .strip_prefix("Mode")
            .or_else(|| rest.strip_prefix("_mode"))
            .unwrap_or(rest);
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

pub(crate) fn refine_render_modes_from_ast(
    root: &Path,
    cfg: &OxideConfig,
    mut tree: RouteTree,
) -> RouteTree {
    let _span = tracing::debug_span!("cli_refine_render_modes").entered();
    let routes_root = root.join(&cfg.routes.dir);
    // V2-F16: reasons are collected while the loader source is already in
    // hand, then written to the `.oxide/routes.reasons.json` side-car (never
    // the rkyv archive the request hot path reads).
    let mut reasons: BTreeMap<String, Vec<oxide_fw_compiler::RouteReasonRow>> = BTreeMap::new();
    for e in &mut tree.entries {
        if !matches!(e.kind, oxide_fw_router::RouteKind::Page) {
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
        let route_id = e.id.0.strip_suffix("/page").unwrap_or(&e.id.0).to_string();
        let rows: Vec<oxide_fw_compiler::RouteReasonRow> =
            oxide_fw_compiler::infer_reasons(&source, &server.to_string_lossy())
                .iter()
                .map(|r| r.row())
                .collect();
        if !rows.is_empty() {
            reasons.insert(route_id, rows);
        }
        let m = oxide_fw_compiler::infer_render_mode(&source, &server.to_string_lossy());
        e.render_mode = Some(m.mode.to_string());
    }
    write_routes_reasons(root, &reasons);
    tree
}

pub(crate) async fn scan_routes(
    root: &Path,
    cfg: &OxideConfig,
    pool: Option<&std::sync::Arc<dyn oxide_fw_jshost::Pool>>,
) -> anyhow::Result<RouteTree> {
    // Canonicalize `root` first so `dir` is absolute. `RouteTree::scan_with_config`
    // joins the override onto `dir`; if the override is already root-relative
    // (which it is when `root` itself is relative), the join produces
    // `root/root/src/routes`. An absolute override short-circuits that:
    // `PathBuf::join` returns the RHS unchanged when it's absolute.
    let root_abs = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let dir = root_abs.join(&cfg.routes.dir);
    let mut tree = RouteTree::scan_with_config(&root_abs, Some(&dir))
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    // §15.2: `serverMiddleware` — a plugin contributes middleware source.
    // Each result is `{ name, source }`. The source is written under
    // `.oxide/plugins/` and registered as a synthetic root middleware
    // entry, so the existing chain (which joins a route's ancestor
    // middleware files) runs it for every route.
    if let Some(pool) = pool
        && !collect_plugin_paths_for_hook(&root_abs, cfg, "serverMiddleware").is_empty()
    {
        let results = call_js_plugin_hook(
            &root_abs,
            cfg,
            pool,
            "serverMiddleware",
            serde_json::json!({ "root": root_abs.to_string_lossy() }),
        )
        .await;
        let plugins_dir = root_abs.join(".oxide/plugins");
        let _ = std::fs::create_dir_all(&plugins_dir);
        for (i, r) in results.iter().enumerate() {
            let Some(src) = r.get("source").and_then(|s| s.as_str()) else {
                continue;
            };
            let name = r
                .get("name")
                .and_then(|n| n.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| format!("plugin-{i}"));
            let safe: String = name
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let file = plugins_dir.join(format!("{safe}.middleware.mjs"));
            if std::fs::write(&file, src).is_err() {
                continue;
            }
            tree.insert(oxide_fw_router::RouteEntry {
                live: None,
                priority: None,
                // A root middleware: base dir `routes`, so it wraps every
                // route (the chain rule is a prefix match).
                id: oxide_fw_router::RouteId(format!("routes/{safe}.plugin-middleware")),
                kind: oxide_fw_router::RouteKind::Middleware,
                segments: Vec::new(),
                file,
                render_mode: None,
                tags: Vec::new(),
                has_server_module: false,
                needs_parent: false,
                slot: None,
            });
        }
    }
    // §15-E: merge every `oxide.module` package's routes into the tree. This
    // is the single seam every command funnels through (build, dev, routes,
    // analyze, check) — patching it here makes a module's routes visible
    // everywhere without further plumbing. A module's route `file` is
    // rewritten to an absolute path under `node_modules/<name>/`, and its
    // `id` is namespaced `modules/<name>/…` so a project route cannot shadow
    // it. A broken module is reported on stderr and skipped — a module the
    // user did not write must never fail their build (a future tightening
    // pass can make `cfg.modules` strict).
    let mods = crate::modules_loader::load(&root_abs, cfg);
    let merged = crate::modules_loader::inject_routes(&mut tree, &mods);
    if merged > 0 {
        eprintln!(
            "modules: merged {merged} route entr(y/ies) from {} module(s)",
            mods.len()
        );
    }
    for m in crate::modules_loader::has_unmerged_hooks(&mods) {
        let unmerged: Vec<&str> = m
            .hooks
            .iter()
            // §15 / WP-r9: keep in sync with the message — transform,
            // resolveId and load all route through the pipeline.
            .filter(|h| !matches!(h.as_str(), "transform" | "resolveId" | "load"))
            .map(|s| s.as_str())
            .collect();
        eprintln!(
            "modules: `{}` declares hook(s) [{}] — transform, resolveId and load are merged; the rest are deferred",
            m.name,
            unmerged.join(", "),
        );
    }
    // §15.2: `routes` hook — a plugin contributes route entries. Each
    // result is `{ entries: [{ id, file, kind?, segments? }] }`. The file
    // must exist inside the project; the id is used as given (a plugin is
    // expected to namespace it). A malformed entry is reported and skipped.
    if let Some(pool) = pool
        && !collect_plugin_paths_for_hook(&root_abs, cfg, "routes").is_empty()
    {
        let results = call_js_plugin_hook(
            &root_abs,
            cfg,
            pool,
            "routes",
            serde_json::json!({ "root": root_abs.to_string_lossy() }),
        )
        .await;
        for r in results {
            let Some(list) = r.get("entries").and_then(|e| e.as_array()) else {
                continue;
            };
            for e in list {
                let Some(id) = e.get("id").and_then(|v| v.as_str()) else {
                    continue;
                };
                let Some(rel) = e.get("file").and_then(|v| v.as_str()) else {
                    continue;
                };
                let file = root_abs.join(rel);
                if !file.starts_with(&root_abs) || !file.is_file() {
                    eprintln!("warn: routes hook entry `{id}` has no readable file; skipped");
                    continue;
                }
                let kind = match e.get("kind").and_then(|v| v.as_str()).unwrap_or("page") {
                    "layout" => oxide_fw_router::RouteKind::Layout,
                    "endpoint" | "route" => oxide_fw_router::RouteKind::Endpoint,
                    "loading" => oxide_fw_router::RouteKind::Loading,
                    "error" => oxide_fw_router::RouteKind::Error,
                    _ => oxide_fw_router::RouteKind::Page,
                };
                let segments: Vec<String> = e
                    .get("segments")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                tree.insert(oxide_fw_router::RouteEntry {
                    live: None,
                    priority: None,
                    id: oxide_fw_router::RouteId(id.to_string()),
                    kind,
                    segments,
                    file,
                    render_mode: None,
                    tags: Vec::new(),
                    has_server_module: false,
                    needs_parent: false,
                    slot: None,
                });
            }
        }
    }

    Ok(tree)
}

/// URL pattern for display (§19.1 table): `/` for the root, `/a/b` otherwise.///
/// Extract a param name from a route segment: `:slug` → `slug`, `*rest` →
/// `rest`, `*?opt` → `opt`. Returns `None` for static segments.
pub(crate) fn param_name(segment: &str) -> Option<&str> {
    let s = segment.trim_start_matches(':');
    let s = s.trim_start_matches('*').trim_start_matches('?');
    if s.len() == segment.len() {
        return None; // no prefix consumed
    }
    // Strip regex constraint (`:slug{[a-z]+}` → `slug`).
    let s = s.split('{').next().unwrap_or(s);
    Some(s)
}

pub(crate) fn pattern_of(segments: &[String]) -> String {
    if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    }
}

pub(crate) fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
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

pub(crate) fn read_json(path: &Path) -> Option<serde_json::Value> {
    let s = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

pub(crate) fn config_rules_to_router(
    rules: &[oxide_fw_config::native_rule::NativeRule],
) -> Vec<oxide_fw_router::NativeRule> {
    rules
        .iter()
        .map(|r| oxide_fw_router::NativeRule {
            source: r.source.clone(),
            destination: r.destination.clone(),
            status: r.status,
            rule_type: r.rule_type.clone(),
            headers: r.headers.clone(),
            basic_auth: r.basic_auth.clone(),
            rate_limit: r.rate_limit.as_ref().map(|rl| oxide_fw_router::RateLimit {
                key: rl.key.clone(),
                max: rl.max,
                window_secs: rl.window_secs,
            }),
            // V2-F12: the rule's overload priority travels to the route
            // manifest (the enforcement stage consults it at brownout).
            priority: r.priority.clone(),
        })
        .collect()
}

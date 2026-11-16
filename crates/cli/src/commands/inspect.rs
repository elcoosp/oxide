//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// The §18 project facts the config-key refusal consults (V2-R02's config
/// half, spec §V2-R02), read from the scanned route tree:
///
/// - `live_routes` — any `RouteEntry` carrying the V2-F05 `live` opt-in;
/// - `non_static_routes` — any routable page whose EFFECTIVE render mode
///   (explicit `config.render` wins, then the `static_override` seeds,
///   then the `has_server_module` fallback) is not `static`. Infrastructure
///   rows (error / not-found / template) have no render mode of their own
///   and never count — the same rule the enriched server manifest applies.
///
/// `rust_fns` is not tree-visible (it needs a source walk) and is filled
/// by [`has_rust_fn_sources`] at the call site.
pub(crate) fn project_facts_from_tree(
    tree: &oxide_fw_router::RouteTree,
    static_override: &std::collections::BTreeSet<String>,
) -> oxide_fw_config::ProjectFacts {
    let mut facts = oxide_fw_config::ProjectFacts::default();
    let mut non_static = false;
    for e in &tree.entries {
        if e.live.is_some() {
            facts.live_routes = true;
        }
        // WP-A1: error / not-found / template rows are rendering
        // infrastructure, not routable pages — the §10.1 mode question is
        // about pages (the manifest enrichment loop skips them too).
        if matches!(
            e.kind,
            oxide_fw_router::RouteKind::Error
                | oxide_fw_router::RouteKind::NotFound
                | oxide_fw_router::RouteKind::Template
        ) {
            continue;
        }
        let mode = e.render_mode.as_deref().unwrap_or_else(|| {
            if static_override.contains(&e.id.0) {
                "static"
            } else if e.has_server_module {
                "dynamic"
            } else {
                "static"
            }
        });
        if mode != "static" {
            non_static = true;
        }
    }
    facts.non_static_routes = non_static;
    facts
}

/// V2-F08's project surface for the config-key refusal: does the project
/// carry native Rust server function sources (`**/*.server.rs`)?
///
/// Walks the project's source root (the routes dir's parent — the same
/// source root the route scanner treats as the project tree), skipping
/// dependency and build-output directories. F08's loader is still todo,
/// but the files' PRESENCE is decidable today, and RFN-002 is the
/// enforcement arm the registry already names: a project carrying the
/// files cannot deploy them to a target that cannot load a native binary.
pub(crate) fn has_rust_fn_sources(source_root: &Path) -> bool {
    let _span = tracing::debug_span!("cli_has_rust_fn_sources").entered();
    let mut stack = vec![source_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                // Dependency and build-output trees never carry first-party
                // server functions.
                if matches!(
                    name.as_ref(),
                    "node_modules" | "dist" | "target" | ".git" | ".oxide"
                ) {
                    continue;
                }
                stack.push(path);
            } else if name.ends_with(".server.rs") {
                return true;
            }
        }
    }
    false
}

/// The §18 capability-matrix row for `target` as the printable lines the
/// doctor's capability section and `build --explain` render (V2-R02's
/// deferred presentation half, spec §V2-R02: "oxide doctor prints a column
/// per capability").
///
/// One line per capability in [`Capability::ALL`] declaration order — the
/// order the matrix contract fixes (D39): `[full]`, `[degraded]` (with the
/// documented degradation string) and `[unsupported]`. Pure formatting
/// over [`oxide_fw_adapters::capability_matrix`] — the print can never
/// disagree with the matrix.
pub(crate) fn capability_row_lines(target: oxide_fw_adapters::Target) -> Vec<String> {
    let matrix = oxide_fw_adapters::capability_matrix(target);
    oxide_fw_adapters::Capability::ALL
        .iter()
        .map(|c| {
            let id = c.as_str();
            match matrix.get(c) {
                Some(oxide_fw_adapters::FeatureSupport::Full) => format!("  [full] {id}"),
                Some(oxide_fw_adapters::FeatureSupport::Degraded) => {
                    let reason = oxide_fw_adapters::degradation(target, *c)
                        .unwrap_or("no documented degradation reason");
                    format!("  [degraded] {id}: {reason}")
                }
                _ => format!("  [unsupported] {id}"),
            }
        })
        .collect()
}

/// V2-F16: write the per-route why-dynamic reasons side-car
/// (`.oxide/routes.reasons.json`), the JSON map inspectors and the MCP
/// `routes.list` tool consume instead of re-running the AST pass. A failure
/// to write is a loud warning, not a build failure — the side-car is
/// advisory, the request path never reads it.
pub(crate) fn write_routes_reasons(
    root: &Path,
    reasons: &BTreeMap<String, Vec<oxide_fw_compiler::RouteReasonRow>>,
) {
    let dir = root.join(".oxide");
    let body = serde_json::to_string_pretty(reasons).unwrap_or_else(|_| "{}".to_string());
    match std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(dir.join("routes.reasons.json"), body))
    {
        Ok(()) => {
            tracing::debug!(routes = reasons.len(), "wrote .oxide/routes.reasons.json");
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                "could not write .oxide/routes.reasons.json (advisory side-car; build continues)"
            );
        }
    }
}

/// Scan routes with a tooling pool when a plugin declares `routes` or
/// `serverMiddleware`. Those two hooks contribute entries during the scan, so
/// every command that builds a route table must pass a pool — not just serve.
/// The pool is created lazily: a project with no such plugin pays nothing.
pub(crate) async fn scan_routes_with_plugins(
    root: &Path,
    cfg: &OxideConfig,
) -> anyhow::Result<RouteTree> {
    if !plugin_declares_any(root, cfg, &["routes", "serverMiddleware"]) {
        return scan_routes(root, cfg, None).await;
    }
    use oxide_fw_jshost::JsHost as _;
    let host = oxide_fw_jshost::NodeHost::new(oxide_fw_jshost::NodeHost::worker_script_default());
    let pool = host
        .tooling_pool(oxide_fw_jshost::PoolConfig::default())
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    scan_routes(root, cfg, Some(&pool)).await
}

/// `oxide routes` (§10.1, §19.1): table with inferred modes + typegen into
/// `.oxide/types/` (routes.d.ts + routes.json, §8.3).
pub(crate) async fn cmd_routes(root: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let cfg = load_config_or_default(&rootp, "development");
    // §10.1: same AST refinement `cmd_build` uses, so the table shows the
    // authoritative mode rather than the scanner's line-oriented guess.
    // (Without it, a loader with `'…cookies…'` in a string literal reads
    // as dynamic here while the build treats it as static.)
    let tree =
        refine_render_modes_from_ast(&rootp, &cfg, scan_routes_with_plugins(&rootp, &cfg).await?);
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
    tree.typegen_with_i18n(&rootp, i18n_for_typegen, default_for_typegen)
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    // Join render modes from the machine-readable manifest typegen just wrote.
    let mut modes: BTreeMap<String, String> = BTreeMap::new();
    if let Some(rows) =
        read_json(&rootp.join(".oxide/types/routes.json")).and_then(|v| v.as_array().cloned())
    {
        for row in rows {
            if let (Some(pattern), Some(mode)) = (
                row.get("pattern").and_then(|p| p.as_str()),
                row.get("renderMode").and_then(|m| m.as_str()),
            ) {
                modes.insert(pattern.to_string(), mode.to_string());
            }
        }
    }
    outln!(w, "PATTERN | KIND | MODE | CACHE | TAGS");
    for (pattern, kind, tags) in tree.table() {
        let mode = modes.get(&pattern).map(String::as_str).unwrap_or("-");
        // SPEC-19C / §19.1: the CACHE column names the L0 eligibility a
        // request for this route will hit. Mirrors the runtime predicate
        // at `l0_cache::mode_is_cacheable` — a route whose mode is
        // `static` or `isr` will serve from L0 on the second request; a
        // `dynamic` / `spa` / `islands` route bypasses.
        let cache = if crate::l0_cache::mode_is_cacheable(Some(mode)) {
            "L0"
        } else {
            "none"
        };
        let tags = if tags.is_empty() {
            "-".to_string()
        } else {
            tags.join(",")
        };
        outln!(w, "{pattern} | {kind} | {mode} | {cache} | {tags}");
    }
    Ok(0)
}

/// `oxide analyze` (§21 budgets): per-route JS bytes vs `budgets.routeJs` (falling
/// back to `budgets.clientJs`), from dist/client/client.manifest.json when present,
/// zeros otherwise. Exit 1 when any route is over budget.
pub(crate) async fn cmd_analyze(root: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let cfg = load_config_or_default(&rootp, "production");
    let tree = scan_routes_with_plugins(&rootp, &cfg).await?;
    let manifest =
        read_json(&rootp.join("dist/client/client.manifest.json")).unwrap_or(serde_json::json!({}));
    let manifest_routes = manifest
        .get("routes")
        .and_then(|r| r.as_object())
        .cloned()
        .unwrap_or_default();
    let rows: Vec<(String, u64)> = if manifest_routes.is_empty() {
        tree.entries
            .iter()
            .filter(|e| matches!(e.kind, oxide_fw_router::RouteKind::Page))
            .map(|e| (e.id.0.clone(), 0u64))
            .collect()
    } else {
        manifest_routes
            .iter()
            .map(|(id, row)| {
                (
                    id.clone(),
                    row.get("js_bytes").and_then(|v| v.as_u64()).unwrap_or(0),
                )
            })
            .collect()
    };
    // SPEC-25: enforce BOTH budgets independently — `routeJs` per route and
    // `clientJs` on the total client payload (the sum across routes). A
    // single fallback chain treated them as one number.
    // Per-route budget: `routeJs` if set, else fall back to `clientJs` (a
    // project that names only a total budget still gets every route checked
    // against it). The total check below is separate and always uses
    // `clientJs`.
    let route_src = cfg
        .budgets
        .route_js
        .as_deref()
        .or(cfg.budgets.client_js.as_deref());
    let route_budget: Option<u64> = match route_src {
        Some(s) => Some(
            oxide_fw_config::parse_size(s)
                .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?,
        ),
        None => None,
    };
    let total_budget: Option<u64> = match cfg.budgets.client_js.as_deref() {
        Some(s) => Some(
            oxide_fw_config::parse_size(s)
                .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?,
        ),
        None => None,
    };
    // A route's own JS plus its shared preload is what it costs; the sum over
    // routes overcounts shared chunks, so the total is the union of every
    // emitted client .js file.
    let client_dir = rootp.join("dist/client");
    let mut total_bytes: u64 = 0;
    {
        let mut stack = vec![client_dir.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().and_then(|x| x.to_str()) == Some("js") {
                    total_bytes += std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                }
            }
        }
    }
    outln!(w, "ROUTE | JS BYTES | SHARE% | BUDGET | OK");
    let mut any_over = false;
    for (route, bytes) in &rows {
        let (budget_str, limit) = match route_budget {
            Some(b) => (b.to_string(), Some(b)),
            None => ("-".to_string(), None),
        };
        let ok = limit.is_none_or(|l| *bytes <= l);
        if !ok {
            any_over = true;
        }
        let status = if ok { "ok" } else { "OVER" };
        let share = if total_bytes == 0 {
            0.0
        } else {
            (*bytes as f64) * 100.0 / (total_bytes as f64)
        };
        outln!(
            w,
            "{route} | {bytes} | {share:.1} | {budget_str} | {status}"
        );
    }
    if let Some(limit) = total_budget {
        let ok = total_bytes <= limit;
        if !ok {
            any_over = true;
        }
        outln!(w, "--");
        outln!(
            w,
            "total client JS | {total_bytes} | {limit} | {}",
            if ok { "ok" } else { "OVER" }
        );
    }
    // §5-A: report the last build's graph recompute counts. A user editing
    // one component sees how many graph nodes that touch actually dirtied —
    // the concrete measurement a future incremental build is tuned against.
    let snap = crate::build_graph::load_snapshot(&rootp);
    if snap.node_count > 0 {
        outln!(w, "--");
        outln!(
            w,
            "graph (last build `{}`): {} nodes, {} recomputed ({} in the previous build)",
            snap.build_id,
            snap.node_count,
            snap.recomputed_count,
            snap.previous_node_count,
        );
    }
    // §19-C: the top modules by bytes. The manifest records each route's
    // reachable module list (PERF-09). A module shared by many routes is
    // counted once per route, so its number is "bytes paid by the routes
    // that include it" — the useful figure when deciding what to split out.
    if !manifest_routes.is_empty() {
        // The manifest records canonicalized module paths; `rootp` here is
        // whatever `--root` was (often relative or via a symlink), so
        // canonicalize before stripping or the prefix never matches.
        let root_canon = rootp.canonicalize().unwrap_or_else(|_| rootp.clone());
        let mut module_bytes: BTreeMap<String, u64> = BTreeMap::new();
        for row in manifest_routes.values() {
            let Some(mods) = row.get("modules").and_then(|m| m.as_array()) else {
                continue;
            };
            for m in mods {
                let Some(p) = m.as_str() else { continue };
                let display = std::path::Path::new(p)
                    .strip_prefix(&root_canon)
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| p.to_string());
                let size = std::fs::metadata(p).map(|x| x.len()).unwrap_or(0);
                *module_bytes.entry(display).or_insert(0) += size;
            }
        }
        let mut sorted: Vec<(String, u64)> = module_bytes.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let top_n = std::env::var("OXIDE_ANALYZE_TOP")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(20);
        // The share is over the sum of *module source* bytes, not the
        // emitted-chunk total — those are different denominators (a module
        // is counted once per route, and chunk bytes are post-minification),
        // and mixing them produced shares over 100%.
        let module_total: u64 = sorted.iter().map(|(_, b)| *b).sum();
        let denom = module_total.max(1);
        outln!(w, "--");
        outln!(w, "top {top_n} module(s) by reachable source bytes:");
        outln!(w, "MODULE | BYTES | SHARE%");
        for (module, bytes) in sorted.iter().take(top_n) {
            let share = (*bytes as f64) * 100.0 / (denom as f64);
            outln!(w, "{module} | {bytes} | {share:.1}");
        }
    }
    Ok(if any_over { 1 } else { 0 })
}

pub(crate) async fn cmd_check(root: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let mut ok = true;
    let mut diags: Vec<oxide_fw_diagnostics::Diagnostic> = Vec::new();
    // V2-C11 — resolved below: the typecheck backend from `check.typecheck`
    // and the P1-17 lint arm, both through [`resolve_tool`] + [`run_tool`].
    // (1) config + routes — the two checks the tool can always run.
    match oxide_fw_config::load(&rootp, "development", BTreeMap::new()) {
        Ok(lc) => outln!(w, "config: OK ({} env vars)", lc.env.len()),
        Err(e) => {
            outln!(w, "config: {} {}", e.0.code, e.0.message);
            diags.push((*e.0).clone());
            ok = false;
        }
    }
    let cfg = load_config_or_default(&rootp, "development");
    match scan_routes_with_plugins(&rootp, &cfg).await {
        Ok(tree) => outln!(w, "routes: OK ({} entries)", tree.entries.len()),
        Err(e) => {
            outln!(w, "routes: {e:#}");
            ok = false;
        }
    }
    // SPEC-15.1 / §15.2: validate every `kind:"native"` entry in
    // `config.plugins`. An unknown name fails the check (fail-at-load).
    match crate::native_plugins::parse_native_plugins(&cfg) {
        Ok(plugins) => {
            if plugins.is_empty() {
                outln!(w, "native plugins: (none declared)");
            } else {
                let names: Vec<&str> = plugins.iter().map(|p| p.name()).collect();
                outln!(w, "native plugins: {}", names.join(", "));
                // §15.2 / WP-J2: surface what each native plugin *enables*.
                // The `enables()` wrapper is the activation table; a plugin
                // that declares no gate is inert (validation-only).
                let enabled: Vec<&str> = plugins
                    .iter()
                    .flat_map(|p| p.enables().iter().copied())
                    .collect();
                if enabled.is_empty() {
                    outln!(w, "native plugin gates: (none — all are validation-only)");
                } else {
                    outln!(w, "native plugin gates: {}", enabled.join(", "));
                }
            }
        }
        Err(e) => {
            outln!(w, "native plugins: {} {}", e.0.code, e.0.message);
            diags.push((*e.0).clone());
            ok = false;
        }
    }

    // (2) §19.1 / D16: typecheck via `tsgo` and lint via `oxlint`. Both are
    // external binaries, resolved on `PATH`. When one is absent the check
    // says so LOUDLY (a silent pass would hide that the project was never
    // typechecked) and does not fail the command — a project that has not
    // installed the tool has not failed a check it never ran. `--strict`
    // turns a missing tool into a failure.
    //
    // V2-C11: the typecheck arm is now CONFIGURABLE (`check.typecheck`,
    // default `tsgo`): `oxlint` mode runs `oxlint --type-aware
    // --type-check` (it orchestrates the Go-based `tsgolint` — `oxide
    // doctor` says so), `tsrs` is opt-in and labelled experimental, plain
    // `tsc` stays available, and `off` skips the arm (the lint arm keeps
    // running — it has no backend switch). Resolution goes through
    // [`resolve_tool`]: the project-local `node_modules/.bin` install
    // wins over `PATH`, so a workspace-pinned toolchain is authoritative.
    let strict = std::env::var_os("OXIDE_CHECK_STRICT").is_some();
    let mut arms: Vec<(&str, &str, Vec<&str>, &'static str)> = Vec::new();
    match cfg.check.typecheck {
        oxide_fw_config::TypecheckBackend::Off => {
            outln!(w, "typecheck: off (config check.typecheck) — skipped");
        }
        backend => {
            if backend == oxide_fw_config::TypecheckBackend::Tsrs {
                outln!(
                    w,
                    "typecheck: tsrs (experimental — opt-in via check.typecheck)"
                );
            } else if backend == oxide_fw_config::TypecheckBackend::Oxlint {
                outln!(
                    w,
                    "typecheck: oxlint --type-aware --type-check (orchestrates the Go-based tsgolint)"
                );
            }
            let (bin, args) = typecheck_backend_plan(backend).expect("non-Off backend has a plan");
            arms.push(("typecheck", bin, args, "CHECK-TS"));
        }
    }
    arms.push(("lint", "oxlint", vec!["--format", "json"], "CHECK-LINT"));
    for (label, bin, args, code) in arms {
        match resolve_tool(bin, &rootp) {
            None => {
                outln!(
                    w,
                    "{label}: SKIPPED (`{bin}` not on PATH or in node_modules/.bin; install it or set OXIDE_CHECK_STRICT=1 to fail)"
                );
                if strict {
                    ok = false;
                }
            }
            Some(tool) => {
                if run_tool(label, &tool, &args, &rootp, w)? {
                    // OK — nothing to record.
                } else {
                    diags.push(oxide_fw_diagnostics::Diagnostic::error(
                        code,
                        format!("`{bin}` reported errors"),
                    ));
                    ok = false;
                }
            }
        }
    }

    // §15.2: `diagnostics` — a plugin adds to the check report. Each result
    // is `{ diagnostics: [{ code, message, severity? }] }`; an error-severity
    // entry fails the check. A plugin that cannot be reached (no pool) is
    // skipped rather than failing `check`.
    if plugin_declares_any(&rootp, &cfg, &["diagnostics"]) {
        use oxide_fw_jshost::JsHost as _;
        let host =
            oxide_fw_jshost::NodeHost::new(oxide_fw_jshost::NodeHost::worker_script_default());
        if let Ok(pool) = host.tooling_pool(oxide_fw_jshost::PoolConfig::default()) {
            let results = call_js_plugin_hook(
                &rootp,
                &cfg,
                &pool,
                "diagnostics",
                serde_json::json!({ "root": rootp.to_string_lossy() }),
            )
            .await;
            for r in results {
                let Some(list) = r.get("diagnostics").and_then(|d| d.as_array()) else {
                    continue;
                };
                for d in list {
                    let code = d.get("code").and_then(|c| c.as_str()).unwrap_or("PLUGIN");
                    let message = d.get("message").and_then(|m| m.as_str()).unwrap_or("");
                    let severity = d
                        .get("severity")
                        .and_then(|s| s.as_str())
                        .unwrap_or("error");
                    outln!(w, "plugin diagnostic {code}: {message}");
                    let diag = if severity == "warning" {
                        oxide_fw_diagnostics::Diagnostic::warning(code, message)
                    } else {
                        oxide_fw_diagnostics::Diagnostic::error(code, message)
                    };
                    diags.push(diag);
                    if severity != "warning" {
                        ok = false;
                    }
                }
            }
        }
    }
    // §19.4: every check result lands in the shared diagnostic file so the
    // editor/CI read one artifact.
    if !diags.is_empty() {
        persist_diagnostics(&rootp, &diags);
    }

    if ok {
        outln!(w, "OK");
        Ok(0)
    } else {
        Ok(1)
    }
}

/// Resolve a binary on `PATH` (no shell; avoids a shell-injection surface).
pub(crate) fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// `oxide why` dispatcher (§13.4 module surface, §V2-F16 route surface): the
/// two are mutually exclusive — exactly one of module argument / `--route`.
pub(crate) async fn cmd_why(
    root: &str,
    module: Option<&str>,
    route: Option<&str>,
    w: Writer<'_>,
) -> anyhow::Result<i32> {
    match (module, route) {
        (Some(_), Some(_)) | (None, None) => Err(anyhow::anyhow!(
            "specify either a module argument (importers of a module, §13.4) \
             or --route <id> (why the route is dynamic, §V2-F16) — exactly one"
        )),
        (Some(m), None) => cmd_why_module(root, m, w).await,
        (None, Some(r)) => cmd_why_route(root, r, w).await,
    }
}

/// `oxide why --route <id>` (§V2-F16): print the route's render mode and
/// every request-scoped read (or uncached fetch) as an `OX-DYN` diagnostic
/// with a code frame and the fix help text. Works without a build — the
/// walk runs on the current `page.server.ts` source.
pub(crate) async fn cmd_why_route(
    root: &str,
    route_id: &str,
    w: Writer<'_>,
) -> anyhow::Result<i32> {
    let _span = tracing::info_span!("cli_why_route", route = %route_id).entered();
    let rootp = PathBuf::from(root);
    let cfg = load_config_or_default(&rootp, "development");
    let tree = scan_routes_with_plugins(&rootp, &cfg).await?;
    let entry = find_route_entry(&tree, route_id).ok_or_else(|| {
        anyhow::anyhow!("no route matches `{route_id}` (run `oxide routes` to list the route ids)")
    })?;
    let routes_root = rootp.join(&cfg.routes.dir);
    let server = routes_root.join(entry.file.with_file_name("page.server.ts"));
    let display_path = server
        .strip_prefix(&rootp)
        .unwrap_or(&server)
        .to_string_lossy()
        .into_owned();
    let Ok(source) = std::fs::read_to_string(&server) else {
        // §10.1: a page route with no `page.server.ts` has no loader and
        // therefore reads no request state — it is `static`.
        outln!(w, "route {route_id} — mode: static");
        outln!(
            w,
            "no page.server.ts loader — the route reads no request state"
        );
        return Ok(0);
    };
    let mode = oxide_fw_compiler::infer_render_mode(&source, &display_path);
    outln!(w, "route {route_id} — mode: {}", mode.mode);
    // V2-F04 (§15.4): the L0-key experiment dimension. An experiment
    // evaluated natively becomes a `|exp:` dimension on the L0 key, so a
    // personalized route stays cacheable per cohort. The assignments are
    // per identity, so the printed dimension is the shape the key carries
    // when a request to this route lands in a cohort.
    let route_url = format!(
        "/{}",
        route_id
            .strip_prefix("routes/")
            .unwrap_or(route_id)
            .strip_suffix("/page")
            .unwrap_or(route_id.strip_prefix("routes/").unwrap_or(route_id))
    );
    let eligible: Vec<&oxide_fw_config::ExperimentConfig> = cfg
        .experiments
        .iter()
        .filter(|e| e.routes.is_empty() || e.routes.iter().any(|r| route_url.starts_with(r)))
        .collect();
    if !eligible.is_empty() {
        outln!(
            w,
            "l0 key experiment dimension: |exp:<id>=<variant> — cohorts get separate entries"
        );
        for e in eligible {
            let sticky = match &e.sticky {
                oxide_fw_config::StickyConfig::Cookie { cookie_name } => {
                    format!("cookie `{cookie_name}` (minted when absent)")
                }
                oxide_fw_config::StickyConfig::Header { header_name } => {
                    format!("header `{header_name}`")
                }
                oxide_fw_config::StickyConfig::IpHash => "client ip (hashed bucket)".to_string(),
            };
            outln!(
                w,
                "  experiment {} — traffic {}%, sticky: {}, variants: [{}]",
                e.id,
                e.traffic,
                sticky,
                e.variants.join(", ")
            );
        }
    }
    let reasons = oxide_fw_compiler::infer_reasons(&source, &display_path);
    tracing::debug!(
        route = %route_id,
        reasons = reasons.len(),
        "why-route reason walk complete"
    );
    if reasons.is_empty() {
        outln!(w, "no dynamic reads found in {display_path}");
        return Ok(0);
    }
    for reason in &reasons {
        outln!(w, "{}", reason.diagnostic(&display_path, &source));
    }
    Ok(0)
}

/// Resolve a `--route` id against the scanned tree. Accepts the canonical
/// id (`routes/dashboard`) or the directory form without the `routes/`
/// prefix (`dashboard`).
pub(crate) fn find_route_entry<'a>(
    tree: &'a RouteTree,
    route_id: &str,
) -> Option<&'a oxide_fw_router::RouteEntry> {
    // Accept the scanner id (`routes/dash/page`), the canonical directory
    // form (`routes/dash` — the form the architecture map and the reasons
    // side-car use) and the bare directory (`dash`).
    let dir_id = route_id
        .strip_prefix("routes/")
        .unwrap_or(route_id)
        .strip_suffix("/page")
        .unwrap_or(route_id.strip_prefix("routes/").unwrap_or(route_id));
    tree.entries.iter().find(|e| {
        let dir_form = e.id.0.strip_suffix("/page").unwrap_or(&e.id.0);
        let dir_bare = dir_form.strip_prefix("routes/").unwrap_or(dir_form);
        e.id.0 == route_id || dir_bare == dir_id
    })
}

/// `oxide why <module>` (§13.4): importers and chunks of a module.
pub(crate) async fn cmd_why_module(root: &str, module: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let client_dir = rootp.join("dist/client");
    // A missing manifest means "no build yet" and keeps the friendly hint
    // below; a manifest that does not parse is a real failure and must not
    // be silently swallowed into the same message (micro-debt: the old
    // `read_json(..).unwrap_or(json!({}))` conflated the two).
    let manifest_path = client_dir.join("client.manifest.json");
    let manifest = match std::fs::read_to_string(&manifest_path) {
        Ok(body) => serde_json::from_str::<serde_json::Value>(&body).map_err(|e| {
            anyhow::anyhow!(
                "cannot parse {} ({e}) — delete dist/ and re-run `oxide build`",
                manifest_path.display()
            )
        })?,
        Err(_) => serde_json::json!({}),
    };
    let mut hits: Vec<(String, String)> = Vec::new();
    if let Some(routes) = manifest.get("routes").and_then(|r| r.as_object()) {
        for (route, row) in routes {
            let chunk = row
                .get("entry_chunk")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if chunk.is_empty() {
                continue;
            }
            // PERF-09: prefer the recorded module list (precise); fall back to
            // a textual substring scan for a manifest built before the field
            // existed.
            let recorded = row
                .get("modules")
                .and_then(|m| m.as_array())
                .map(|arr| {
                    arr.iter()
                        .any(|v| v.as_str().map(|s| s.contains(module)).unwrap_or(false))
                })
                .unwrap_or(false);
            let references = recorded
                || std::fs::read_to_string(client_dir.join(chunk))
                    .map(|body| body.contains(module))
                    .unwrap_or(false);
            if references {
                hits.push((route.clone(), chunk.to_string()));
            }
        }
    }
    if hits.is_empty() {
        outln!(w, "not found: {module} (run `oxide build` first)");
        return Ok(0);
    }
    outln!(w, "module {module}");
    let mut chunks: Vec<&str> = hits.iter().map(|(_, c)| c.as_str()).collect();
    chunks.sort();
    chunks.dedup();
    outln!(w, "  chunk: {}", chunks.join(", "));
    for (route, _) in &hits {
        outln!(w, "  importer: {route}");
    }
    Ok(0)
}

/// `oxide doctor` (§19.1): environment and config health, ASCII `[ok]/[fail]` lines.
pub(crate) async fn cmd_doctor(root: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = PathBuf::from(root);
    let mut all_ok = true;
    let mut check = |w: &mut dyn std::io::Write, ok: bool, what: &str| -> anyhow::Result<()> {
        if !ok {
            all_ok = false;
        }
        outln!(w, "[{}] {what}", if ok { "ok" } else { "fail" });
        Ok(())
    };
    let node_ok = tokio::process::Command::new("node")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    check(w, node_ok, "node runtime (node --version)")?;
    let cargo_ok = tokio::process::Command::new("cargo")
        .arg("--version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    check(w, cargo_ok, "cargo (cargo --version)")?;
    check(
        w,
        oxide_fw_config::load(&rootp, "development", BTreeMap::new()).is_ok(),
        "config loadable",
    )?;
    let cfg = load_config_or_default(&rootp, "development");
    let routes_ok = rootp.join(&cfg.routes.dir).is_dir() || rootp.join("app").is_dir();
    check(w, routes_ok, "routes directory (src/routes or app/)")?;
    let cache_dir = perf_bisect::cache_dir_for(&rootp);
    let cache_ok = std::fs::create_dir_all(&cache_dir).is_ok();
    let cache_ok = cache_ok && {
        let probe = cache_dir.join(".oxide-doctor-probe");

        std::fs::write(&probe, b"probe").is_ok() && std::fs::remove_file(&probe).is_ok()
    };
    check(
        w,
        cache_ok,
        &format!("cache directory writable ({})", cache_dir.display()),
    )?;
    // §9.4 / BUG-05: the build secret makes server-fn ids stable across
    // builds. A missing file is fine (the next build creates it); this is
    // an informational check, not a failure.
    let secret_file = rootp.join(".oxide/secret");
    let secret_state = match std::fs::read_to_string(&secret_file) {
        Ok(s) if s.trim().len() == 64 => "present (stable server-fn ids)".to_string(),
        Ok(_) => "malformed (delete it or run `oxide secrets rotate`)".to_string(),
        Err(_) => "absent (next build creates it)".to_string(),
    };
    outln!(w, "[--] build secret (.oxide/secret): {secret_state}");
    // SEC-10: cookie keys must be ≥ 32 bytes (the AEAD uses the first 32).
    let weak: Vec<usize> = cfg
        .security
        .cookie_keys
        .iter()
        .enumerate()
        .filter(|(_, k)| k.len() < 32)
        .map(|(_, k)| k.len())
        .collect();
    if cfg.security.cookie_keys.is_empty() {
        outln!(w, "[--] cookie keys: none set (derived from .oxide/secret)");
    } else if weak.is_empty() {
        outln!(
            w,
            "[ok] cookie keys: {} key(s), all ≥ 32 bytes",
            cfg.security.cookie_keys.len()
        );
    } else {
        outln!(
            w,
            "[fail] cookie keys: {} key(s) shorter than 32 bytes",
            weak.len()
        );
        all_ok = false;
    }
    // P1 hardening (readiness): an empty `security.hostAllowList` accepts ANY
    // Host header — a DNS-rebinding / host-header-injection vector in
    // production. Doctor surfaces it as a warning (not a failure: dev servers
    // legitimately run without one).
    if cfg.security.host_allow_list.is_empty() {
        outln!(
            w,
            "[--] host allow list: empty — ANY Host header is accepted; set security.hostAllowList before exposing this server publicly"
        );
    } else {
        outln!(
            w,
            "[ok] host allow list: {} host pattern(s)",
            cfg.security.host_allow_list.len()
        );
    }
    // V2-R02's presentation half (spec §V2-R02): the per-capability column
    // for the project's configured adapter target — informational (`[--]`,
    // never a doctor failure), one line per capability in declaration
    // order, degradation strings included.
    let target =
        oxide_fw_adapters::Target::parse(&cfg.adapter).unwrap_or(oxide_fw_adapters::Target::Binary);
    outln!(w, "[--] capability matrix for `{}` (§18):", target.as_str());
    for line in capability_row_lines(target) {
        outln!(w, "{line}");
    }
    // §V2-C01 tail: one line per compiled-in native compiler —
    // `native compiler <id> <version> ok|unhealthy` — over the same
    // cached health verdicts `pick` reuses. The registry is empty until a
    // backend feature is compiled in; that is reported, not failed.
    // `compilerBackend: 'native'` with nothing compiled in IS a failure —
    // the config author's hard request cannot be honored (the same
    // refusal the precompile pass surfaces as `CMP-NAT-001`).
    let native_registry = oxide_fw_compiler::native::NativeRegistry::from_enabled_features();
    if native_registry.is_empty() {
        outln!(
            w,
            "[--] native compilers: none compiled in (build with --features native-<framework> to add backends)"
        );
    }
    for row in native_registry.report() {
        if row.healthy {
            outln!(w, "[ok] native compiler {} {} ok", row.id, row.version);
        } else {
            outln!(
                w,
                "[fail] native compiler {} {} unhealthy (its health fixture failed to compile)",
                row.id,
                row.version
            );
            all_ok = false;
        }
    }
    if cfg.framework.compiler_backend == oxide_fw_config::CompilerBackend::Native
        && native_registry.is_empty()
    {
        outln!(
            w,
            "[fail] framework.compilerBackend is 'native' but no native compiler is compiled in — CMP-NAT-001 refuses every framework compile"
        );
        all_ok = false;
    }
    // §V2-C11: the external tool inventory — one line per tool
    // `oxide check` / `oxide fmt` can shell out to, resolved through
    // [`resolve_tool`] (project-local `node_modules/.bin` first, then
    // `PATH`) with its own `--version`. Informational: a missing tool is
    // the SKIPPED convention's business, not a doctor failure.
    for (name, note) in [
        ("tsgo", "typecheck (the check.typecheck default)"),
        (
            "oxlint",
            "lint; `--type-aware --type-check` orchestrates the Go-based tsgolint",
        ),
        ("tsrs", "typecheck backend (experimental, opt-in)"),
        ("tsc", "typecheck backend"),
        ("oxfmt", "formatter (the `oxide fmt` default)"),
        ("biome", "formatter (`oxide fmt`'s fallback)"),
    ] {
        match resolve_tool(name, &rootp) {
            None => outln!(
                w,
                "[--] tool {name}: not found ({} — SKIPPED when needed)",
                note
            ),
            Some(tool) => {
                let version = std::process::Command::new(&tool)
                    .arg("--version")
                    .current_dir(&rootp)
                    .output()
                    .ok()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "(no --version)".to_string());
                outln!(w, "[ok] tool {name}: {version} ({note})");
            }
        }
    }
    Ok(if all_ok { 0 } else { 1 })
}

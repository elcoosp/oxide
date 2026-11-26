//! Moved verbatim from lib.rs by scripts/split_cli.py (V2-A01).

use crate::*;

/// `oxide add <module>` (§15.4): records the module in `.oxide/modules.json` and
/// prints next steps. (TS config writes are fragile; the list file is the contract.)
pub(crate) async fn cmd_add(root: &str, module: &str, w: Writer<'_>) -> anyhow::Result<i32> {
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let modules_path = rootp.join(".oxide/modules.json");

    // §V2-E06: detect the project's package manager from its lockfile
    // (the `module_config_edit::detect_pm_from_root` table — one source of
    // truth, replacing the inline if-chain).
    let no_install = std::env::var_os("OXIDE_ADD_NO_INSTALL").is_some();
    if !no_install {
        let pm = crate::module_config_edit::detect_pm_from_root(&rootp);
        let (cmd, args) = crate::module_config_edit::pm_install_command(pm, module);
        outln!(w, "installing {} via {}", module, cmd);
        let status = tokio::process::Command::new(cmd)
            .args(&args)
            .current_dir(&rootp)
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("spawn `{cmd} {}`: {e}", module))?;
        if !status.success() {
            anyhow::bail!(
                "`{cmd} {}` exited {}; not recording the module",
                module,
                status.code().unwrap_or(-1),
            );
        }
    } else {
        outln!(w, "OXIDE_ADD_NO_INSTALL=1 — skipping the install step");
    }

    // SPEC-15.4: read the module's `oxide.module` contract. A package with
    // no such field is recorded (audit sees it) but contributes nothing.
    let node_mods = rootp.join("node_modules").join(module);
    let pkg_json = node_mods.join("package.json");
    if !pkg_json.is_file() && !no_install {
        outln!(
            w,
            "warn: node_modules/{module}/package.json missing after install —              recording anyway, `oxide build` will fail with a clear diagnostic"
        );
    }
    let resolved = crate::modules_loader::resolve_one(&node_mods, module.to_string());

    // §V2-E06: enforce the adapters declaration LOUDLY at `add` time so
    // a mismatch is reported before the install round-trip finishes. The
    // build re-checks the same way (`enforce_adapters`); the error code
    // `MOD-ADAPTER-002` is registered in `docs/errors.toml`.
    if let Some(m) = &resolved
        && !m.adapters.is_empty()
    {
        let cfg = load_config_or_default(&rootp, "development");
        let adapter = cfg.adapter.as_str();
        if !m.adapters.iter().any(|a| a == adapter) {
            outln!(
                w,
                "err MOD-ADAPTER-002: module `{}` declares adapters [{}] but the project adapter is `{}`",
                module,
                m.adapters.join(", "),
                adapter,
            );
            outln!(
                w,
                "  fix: add `{adapter}` to the module's `adapters` declaration, or"
            );
            outln!(
                w,
                "       remove the module from `modules: []` in oxide.config.ts"
            );
        }
    }

    // Persist to `.oxide/modules.json` (what `audit` reads).
    let mut list: Vec<String> = read_json(&modules_path)
        .and_then(|v| v.as_array().cloned())
        .map(|a| {
            a.into_iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if !list.iter().any(|m| m == module) {
        list.push(module.to_string());
    }
    list.sort();
    list.dedup();
    std::fs::create_dir_all(modules_path.parent().unwrap_or(Path::new(".")))?;
    std::fs::write(&modules_path, serde_json::to_string_pretty(&list)? + "\n")?;
    outln!(w, "[ok] added {module} to .oxide/modules.json");

    // §V2-E06: edit `oxide.config.ts` to add the import + the `modules: [...]`
    // array element. Idempotent, comment-preserving, refuses a dynamic
    // config with the two-line fix. The edit is best-effort: a missing
    // config file is not an error (the modules.json record is enough for
    // the build to pick the module up); a dynamic config is reported with
    // the fix, not a hard failure.
    let config_ts = rootp.join("oxide.config.ts");
    if config_ts.is_file()
        && let Ok(src) = std::fs::read_to_string(&config_ts)
    {
        let alias = crate::module_config_edit::default_alias_for(module);
        match crate::module_config_edit::add_module_to_config(&src, module, Some(alias)) {
            Ok(edit) => {
                if edit.import_added || edit.module_added {
                    if let Err(e) = std::fs::write(&config_ts, &edit.bytes) {
                        outln!(w, "warn: could not edit oxide.config.ts: {e}");
                    } else {
                        outln!(
                            w,
                            "[ok] added `{module}` to oxide.config.ts (import + modules[])"
                        );
                    }
                }
            }
            Err(e) => {
                outln!(w, "{} {}", e.0.code, e.0.message);
                if let Some(fix) = &e.0.fix {
                    outln!(w, "  fix: {fix}");
                }
            }
        }
    }

    // §15-E: report what the module actually contributes. Every v1 field
    // is WIRED now — types merge into `.oxide/types/`, configDefaults merge
    // under `modules.<name>`, middlewares feed the chain, serverFns feed
    // server-fn discovery, adapters are enforced. The report names the
    // wiring honestly (no "deferred" wording for v1 contributions).
    match &resolved {
        None => {
            outln!(
                w,
                "  note: `{module}` has no `oxide.module` export — it is an ordinary \
                 npm package and will not contribute hooks/routes"
            );
        }
        Some(m) => {
            outln!(w, "  oxide module detected — contributes:");
            match &m.routes_dir {
                Some(p) if p.is_dir() => {
                    let pages = oxide_fw_router::RouteTree::scan(p)
                        .map(|t| {
                            t.entries
                                .iter()
                                .filter(|e| matches!(e.kind, oxide_fw_router::RouteKind::Page))
                                .count()
                        })
                        .unwrap_or(0);
                    outln!(w, "    routes: {pages} page(s) under /modules/{module}/");
                }
                _ => outln!(w, "    routes: (none)"),
            }
            if m.hooks.is_empty() {
                outln!(w, "    hooks:  (none)");
            } else {
                // WP-J4: derive the merged/deferred split from the actual
                // pipeline (transform, resolveId, load all route through the
                // plugin chain today; everything else is deferred).
                let merged_set = ["transform", "resolveId", "load"];
                let merged: Vec<&str> = m
                    .hooks
                    .iter()
                    .map(|s| s.as_str())
                    .filter(|h| merged_set.contains(h))
                    .collect();
                let deferred: Vec<&str> = m
                    .hooks
                    .iter()
                    .map(|s| s.as_str())
                    .filter(|h| !merged_set.contains(h))
                    .collect();
                let mut parts = Vec::new();
                if !merged.is_empty() {
                    parts.push(format!("{} (merged)", merged.join(", ")));
                }
                if !deferred.is_empty() {
                    // §15 / §15-B: every hook here IS wired — via the
                    // general plugin path (`plugins: [...]` + the per-hook
                    // build/dev dispatch), not via the module loader that
                    // only consumes transform/resolveId/load. Say that,
                    // instead of the stale "deferred" wording that read as
                    // "not implemented".
                    parts.push(format!(
                        "{} (via the general plugin path — not module-loader hooks)",
                        deferred.join(", ")
                    ));
                }
                outln!(w, "    hooks:  {}", parts.join("; "));
            }
            if !m.adapters.is_empty() {
                outln!(w, "    adapters: {}", m.adapters.join(", "));
                let cfg = load_config_or_default(&rootp, "development");
                let adapter = cfg.adapter.as_str();
                if !m.adapters.iter().any(|a| a == adapter) {
                    outln!(
                        w,
                        "    warn MOD-ADAPTER: module supports [{}], project adapter is `{}`",
                        m.adapters.join(", "),
                        adapter,
                    );
                }
            }
            if !m.types.is_empty() {
                outln!(
                    w,
                    "    types: {} (merged into .oxide/types/)",
                    m.types.len()
                );
            } else if m.typegen.is_some() {
                outln!(w, "    typegen: present (merged into .oxide/types/)");
            }
            // J2: a native plugin's `enables()` is consulted here so
            // `oxide add` reports which features the project's plugin set
            // activates, rather than leaving the list inert.
            {
                let cfg = load_config_or_default(&rootp, "development");
                let enabled: Vec<&str> = crate::native_plugins::parse_native_plugins(&cfg)
                    .unwrap_or_default()
                    .iter()
                    .map(|p| p.name())
                    .collect();
                if !enabled.is_empty() {
                    outln!(w, "    native plugins: {}", enabled.join(", "));
                }
            }
            if !m.middlewares.is_empty() {
                outln!(
                    w,
                    "    middlewares: {} (merged into the route tree's middleware index)",
                    m.middlewares.len()
                );
            }
            if !m.server_fns.is_empty() {
                outln!(
                    w,
                    "    serverFns: {} (fed to server-fn discovery)",
                    m.server_fns.len()
                );
            }
            if m.config_defaults.is_some() {
                let key = crate::module_config_edit::default_alias_for(module);
                outln!(
                    w,
                    "    configDefaults: present (merged under modules.{key})"
                );
            }
            // §V2-E06: resolve the module into the unified `ModuleManifest`
            // the plugin registry and loader agree on — one contract, not
            // two. The manifest's hook count is the loader's view of which
            // hooks the module's plugins contribute.
            let manifest = crate::modules_loader::to_manifest(m);
            if !manifest.hooks.is_empty() {
                outln!(w, "    manifest hooks: {}", manifest.hooks.join(", "));
            }
        }
    }
    outln!(
        w,
        "next steps: `oxide build` / `oxide dev` will pick up `{module}` from \
         .oxide/modules.json automatically"
    );
    Ok(0)
}

/// `oxide generate` (§19.1): route/layout/action scaffolds under the routes dir,
/// extension picked from the configured framework adapter (default `tsx`).
pub(crate) async fn cmd_generate(
    root: &str,
    kind: &str,
    path: &str,
    w: Writer<'_>,
) -> anyhow::Result<i32> {
    if !matches!(kind, "route" | "layout" | "action") {
        outln!(w, "unknown kind `{kind}`; expected route|layout|action");
        return Ok(2);
    }
    let rootp = PathBuf::from(root);
    let cfg = load_config_or_default(&rootp, "development");
    let ext = match cfg.framework.adapter.as_deref() {
        Some("vue") => "vue",
        Some("svelte") => "svelte",
        _ => "tsx",
    };
    let dir = rootp.join(&cfg.routes.dir).join(path);
    std::fs::create_dir_all(&dir)?;
    let mut created: Vec<PathBuf> = Vec::new();
    let write = |name: &str, body: String| -> anyhow::Result<PathBuf> {
        let p = dir.join(name);
        std::fs::write(&p, body)?;
        Ok(p)
    };
    let server_stub = |actions: bool| {
        let mut s = format!(
            "// Oxide server module for {path} (D10).\n\
             export const config = {{ render: 'static' }};\n\n\
             export async function load() {{\n  return {{ props: {{}} }};\n}}\n"
        );
        if actions {
            s.push_str("\nexport const actions = {\n  async submit(_data) {\n    // TODO: validate + persist (§9.3)\n    return { ok: true };\n  },\n};\n");
        }
        s
    };
    match kind {
        "route" => {
            created.push(write(
                &format!("page.{ext}"),
                format!(
                    "// Oxide route page ({path}).\n\
                     export default function Page() {{\n\
                       return <main data-route=\"{path}\">{path}</main>;\n\
                     }}\n"
                ),
            )?);
            created.push(write("page.server.ts", server_stub(false))?);
        }
        "layout" => {
            created.push(write(
                &format!("layout.{ext}"),
                format!(
                    "// Oxide layout ({path}).\n\
                     export default function Layout({{ children }}) {{\n\
                       return <section>{{children}}</section>;\n\
                     }}\n"
                ),
            )?);
        }
        "action" => {
            created.push(write("page.server.ts", server_stub(true))?);
        }
        _ => unreachable!("kind validated above"),
    }
    for p in &created {
        outln!(w, "created {}", p.display());
    }
    Ok(0)
}

/// `oxide migrate [source] [--apply]` (V2-E03): the plan-is-data engine
/// in `oxide-fw-compiler` detects the source framework, prints the plan
/// (renames, import rewrites, config keys, manual notes), and with
/// `--apply` performs the mechanical halves — the renames, the import
/// rewrites and the config keys (the engine writes `tsconfig.json` /
/// `oxide.config.ts`). `--apply` requires a clean
/// git tree unless `--force` — a half-migration must be revertible.
/// Exit codes: 0 plan/apply ok, 1 apply failure, 2 refusal (unknown
/// framework, source mismatch, dirty tree).
pub(crate) async fn cmd_migrate(
    root: &str,
    source: Option<&str>,
    apply: bool,
    force: bool,
    w: Writer<'_>,
) -> anyhow::Result<i32> {
    use oxide_fw_compiler::migrate::{Framework, apply_plan, detect_framework, plan_for};
    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));

    // Detect (the engine never guesses; `None` is the MIG-DETECT refusal
    // naming the four spellings). An explicit `source` must match it.
    let detected = detect_framework(&rootp);
    if let Some(want) = source {
        let want = match Framework::from_source_name(want) {
            Some(f) => f,
            None => {
                outln!(
                    w,
                    "unknown source `{want}` — oxide migrate plans for sveltekit, next, nuxt or vite"
                );
                return Ok(2);
            }
        };
        if detected != Some(want) {
            match detected {
                Some(actual) => {
                    outln!(
                        w,
                        "source `{}` does not match the detected framework `{}`",
                        want.as_str(),
                        actual.as_str()
                    );
                }
                None => {
                    outln!(
                        w,
                        "source `{}` was named but no source framework could be detected in {}",
                        want.as_str(),
                        rootp.display()
                    );
                }
            }
            return Ok(2);
        }
    }
    let plan = match plan_for(&rootp) {
        Ok(p) => p,
        Err(e) => {
            outln!(w, "{} {}", e.0.code, e.0.message);
            if let Some(fix) = &e.0.fix {
                outln!(w, "  fix: {fix}");
            }
            return Ok(2);
        }
    };

    // The plan is data: print every row, then (optionally) apply.
    outln!(
        w,
        "migration plan ({} → oxide): {} rename(s), {} import rewrite(s), {} config key(s), {} manual note(s)",
        detected.map(Framework::as_str).unwrap_or("?"),
        plan.renames.len(),
        plan.rewrites.len(),
        plan.config.len(),
        plan.manual.len(),
    );
    for (from, to) in &plan.renames {
        outln!(w, "  - {} → {}", from.display(), to.display());
    }
    for r in &plan.rewrites {
        outln!(w, "  - {} : {} → {}", r.file.display(), r.from, r.to);
    }
    for c in &plan.config {
        outln!(w, "  - config: {} = {}", c.key, c.value);
    }
    for n in &plan.manual {
        outln!(w, "  - manual: {}:{} {}", n.file.display(), n.line, n.text);
    }
    if !apply {
        outln!(
            w,
            "  no files written — pass --apply to perform the renames, rewrites and config keys (a dry run never modifies disk)"
        );
        return Ok(0);
    }

    // The §V2-E03 git gate: a dirty tree refuses unless --force. The gate
    // is advisory when the project is not a git repo (no VCS to consult).
    if !force && git_tree_dirty(&rootp) {
        outln!(
            w,
            "refusing --apply: the git tree is dirty (commit or stash first, or pass --force)"
        );
        return Ok(2);
    }
    match apply_plan(&rootp, &plan) {
        Ok(report) => {
            outln!(
                w,
                "  applied {} rename(s), {} import rewrite(s), {} config key(s); {} manual note(s) left pending",
                report.renamed,
                report.rewrote,
                report.config_applied.len(),
                report.manual,
            );
            for key in &report.config_applied {
                outln!(w, "  config applied: {key}");
            }
            for key in &report.config_pending {
                outln!(w, "  config pending: {key}");
            }
            Ok(0)
        }
        Err(e) => {
            outln!(w, "{} {}", e.0.code, e.0.message);
            if let Some(fix) = &e.0.fix {
                outln!(w, "  fix: {fix}");
            }
            Ok(1)
        }
    }
}

/// `oxide test` (§16.6): gates on build manifests; the in-process `app.fetch` helper
/// itself lives in packages/oxide (documented in the output).///
/// `oxide mcp` (§19.3): run the MCP server on stdio. The root path is resolved
/// relative to cwd, same as every other command. The writer is ignored — the
/// MCP protocol owns stdout; only a startup line is emitted to stderr to keep
/// the JSON-RPC stream clean.///
/// `oxide schema` (§19.3): print the JSON Schema for `oxide.config.{ts,json}`.
/// Editors pick this up via a `$schema` field in the config or a
/// `json.schemas` setting.
pub(crate) fn cmd_schema(w: Writer<'_>) -> anyhow::Result<i32> {
    let json = oxide_fw_config::config_schema_pretty()
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;
    outln!(w, "{json}");
    Ok(0)
}

/// `oxide config` (§7, §14): load oxide.config.{ts,mjs,js} through the
/// framework-adapter host and print the descriptor. This exercises the
/// exact load path the build/dev pipeline uses: a Node worker evaluates
/// the config, extracts the `framework` adapter, and returns its shape.
pub(crate) async fn cmd_config(root: &str, json: bool, w: Writer<'_>) -> anyhow::Result<i32> {
    use oxide_fw_jshost::{FrameworkHost, JsHost, NodeHost, PoolConfig};

    let rootp = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let config_path = find_config_file(&rootp)?;

    // One Node worker per CLI invocation; dropped when we return.
    let host = NodeHost::new(NodeHost::worker_script_default());
    let pool = host
        .tooling_pool(PoolConfig::default())
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;

    let fw = FrameworkHost::new(pool);
    let path_str = config_path.to_string_lossy().to_string();
    let desc = fw
        .load_config(&path_str)
        .await
        .map_err(|e| anyhow::anyhow!("{} {}", e.0.code, e.0.message))?;

    if json {
        let v = serde_json::json!({
            "config": path_str,
            "name": desc.name,
            "extensions": desc.extensions,
            "capabilities": desc.capabilities,
        });
        outln!(w, "{}", serde_json::to_string_pretty(&v)?);
    } else {
        outln!(w, "config:       {path_str}");
        outln!(w, "framework:    {}", desc.name);
        outln!(w, "extensions:   {}", desc.extensions.join(", "));
        // The loud convention: a serialization failure renders the
        // reason, never a silently empty capabilities line.
        let caps = serde_json::to_string(&desc.capabilities)
            .unwrap_or_else(|e| format!("<unavailable: {e}>"));
        outln!(w, "capabilities: {caps}");
    }
    Ok(0)
}

/// Locate the project's config file. Precedence: .ts > .mjs > .js. The .ts
/// case relies on Node 22.18+ native type stripping; on older Node the
/// worker's dynamic import fails and the error surfaces to the caller.
pub(crate) fn find_config_file(root: &Path) -> anyhow::Result<PathBuf> {
    for name in ["oxide.config.ts", "oxide.config.mjs", "oxide.config.js"] {
        let p = root.join(name);
        if p.is_file() {
            return Ok(p);
        }
    }
    anyhow::bail!(
        "no oxide.config.{{ts,mjs,js}} found under {} (spec §14.1)",
        root.display()
    )
}

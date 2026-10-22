# Oxide handoff — Session 2026-10-19 (post-§5-A + §15-E)

> [!WARNING]
> **SUPERSEDED — do not treat anything below as current.**
>
> This file is a snapshot from `4e38dec` (2026-10-19) and is retained only
> as a historical record (it is cited as an input to `docs/spec-v0.2-draft.md`
> and by the audit post). Every number and every "what remains" line in it
> is stale by hundreds of commits.
>
> **Current state lives in:**
> - `docs/deviations.md` — the numbered, TDD-locked spec deviations
> - `README.md` — the "Built, wiring in progress" list
> - `WORKLOG.md` — the session log
>
> If a statement here contradicts the tree, trust the tree.

**Date:** 2026-10-19
**HEAD:** `4e38dec` (`feat(mcp): §19-D oxide_graph reads the last build's DemandGraph snapshot`)
**Baseline:** `origin/main` = `4e38dec` — **fully pushed, nothing ahead.**
**Working tree:** clean
**Gates:** Rust workspace **1007/1007** (40 skipped) · node packages **305/305** ·
Playwright **17/17** · conformance **24×4 ALL GREEN**

> If any claim in this doc contradicts the tree, trust the tree.

---

## 0. Read this first

**The build is green. Nothing is broken.** The prior handoff opened with
"the build is currently broken — three bundler test files need
`resolve_hook: None, load_hook: None,` re-applied." That was fixed first
this session (`b79550f`) and the CLI add/audit test rewrites followed
(`ac87c29`). Both were pushed before any new work started.

Two plan items landed this session:

- **§5-A** — conservative landing: a `DemandGraph` over a build's inputs,
  persisted to `.oxide/graph.json`, surfaced in `oxide analyze`. The full
  incremental-build wiring (drive the bundler node-by-node) is the
  documented follow-up.
- **§15-E** — the module load side. `oxide add` records; the build
  now **merges** the module's routes and `transform` hook.

---

## 1. Commits landed this session (oldest → newest, all pushed)

| Commit | Section | What |
|---|---|---|
| `b79550f` | §2a fix | Re-applied `resolve_hook`/`load_hook` to 3 bundler test `BundleOptions` literals. Bundler 54/54. |
| `ac87c29` | §2b fix | Updated `add_appends_module_and_dedupes` (ENV_LOCK + `OXIDE_ADD_NO_INSTALL`); deleted obsolete `audit_prints_no_advisories…`. CLI lib 140/140. |
| `ff4028c` | §5-A | New `crates/cli/src/build_graph.rs` — `DemandGraph` over module/chunk/manifest/typegen nodes; manifests `ctx.dep` on chunks (real edges, not a flat list). |
| `c6337b6` | §5-A | Wire `record_and_persist` into `cmd_build`; `oxide analyze` reports node/recompute counts. |
| `3b271f6` | §5-A | Test: build writes snapshot; `OXIDE_FORCE_BUILD=1` rebuild with no source change → 0 recomputed. |
| `c34289b` | §15-E | New `crates/cli/src/modules_loader.rs` — resolve `oxide.module` packages, scan their routes, inject into the tree. |
| `b22094f` | §15-E | Wire `inject_routes` into `scan_routes` — the single seam every command (build/dev/routes/analyze/check) funnels through. |
| `ef2e574` | §15-E | Acceptance: module route prerendered under `/modules/<name>/…`; segments prefixed so it can't shadow a project route. |
| `24bfb17` | §15-E | Wire module `transform` into `build_js_plugin_hook` — a module declaring `hooks:["transform"]` runs its entry through the same `plugin:call` machinery a `kind:js` plugin uses. |
| `ad7b1e4` | §15-E | Loader unit tests: contract resolution, routes-dir fallback, `has_unmerged_hooks` semantics. |
| `98f02d4` | §15-E | `oxide add` reports merged vs deferred contributions + `MOD-ADAPTER` warning when the module's declared adapters don't include the project's. |
| `3141c8c` | docs | Refreshed HANDOFF.md to reflect session state at 98f02d4. |
| `4e38dec` | §19-D | MCP `oxide_graph` reads `.oxide/graph.json` and returns a `graph` summary (version, counts, per-kind breakdown). `GraphSummary::from_snapshot` keeps the response compact. |

---

## 2. What each landed item actually does

### §5-A — conservative landing (not full incremental)

`crates/cli/src/build_graph.rs` records a `DemandGraph` **after** every build,
then persists a snapshot:

```
.oxide/graph.json
{
  "version": "v1", "build_id": "<id>", "generated_at_unix_ms": …,
  "node_count": N, "recomputed_count": M, "previous_node_count": K,
  "nodes": [{ "key": "module:<abs>", "kind": "module", "hash": "<blake3>", "dep_count": 0 }, …]
}
```

Node kinds:
- `module:<abs-path>` — one per route source file + the boot entry.
- `chunk:<env>/<abs-path>` — one per emitted `.js` (client + SSR).
- `manifest:<env>` — one per manifest JSON. Its hash transitively covers every
  chunk node (recorded via `ctx.dep`), so the graph has real edges.
- `typegen:<name>` — routes.d.ts, routes.json, messages.d.ts.

`oxide build` prints `graph: N nodes, M recomputed (previous: K nodes)`.
`oxide analyze` re-prints the same line at the bottom of its table.

**What this enables today:** measure how many graph nodes a source change
dirties. A forced rebuild with no source change reports **0 recomputed** — the
acceptance test asserts this.

**What this does NOT do (the documented follow-up):** drive the bundler
node-by-node so that touching one component re-bundles exactly one chunk.
The pipeline still runs to completion; the graph is the measurement substrate
that a future incremental build diffs against.

### §15-E — module load side

The module contract (`oxide.module` in `package.json`):

```json
{
  "name": "…", "main": "./index.mjs",
  "oxide": { "module": {
    "routes": "./routes",
    "hooks": ["transform"],
    "adapters": ["node"],
    "typegen": "./types.d.ts",
    "configDefaults": { … }
  } }
}
```

Resolution path:
1. Names come from `cfg.modules` ∪ `.oxide/modules.json` (so `oxide add`
   alone is enough — no config edit required).
2. `modules_loader::load` resolves each against `<root>/node_modules/<name>/`.
3. `inject_routes` scans each module's routes dir, prefixes `segments` with
   `["modules", "<name>"]` (so the URL is `/modules/<name>/…`) and namespaces
   the id. `file` becomes absolute so downstream bundler/prerender join it
   directly.
4. `build_js_plugin_hook` picks up every module declaring `hooks:["transform"]`
   and appends its `entry` to the same plugin chain a `kind:js` plugin uses.

`oxide add <module>` now prints a structured report:

```
  oxide module detected — contributes:
    routes: 1 page(s) under /modules/full-mod/
    hooks:  transform (merged); resolveId (not yet merged)
    adapters: node
    configDefaults: present (not merged — deferred to a follow-up)
```

**Honest limits:** only `transform` is merged. `resolveId`, `load`,
`renderChunk`, `generateBundle`, `transformHtml`, and every other hook name
print as "not yet merged" in the `add` report and in a one-line build notice.
`typegen` and `configDefaults` are resolved-and-reported, not merged. None of
those are silent misses — they're honest deferrals.

---

## 3. What remains in the plan (all "L", all deferrable)

From `oxide-v0.1-plan-graph-plugins-platform-dx.md`:

### §5-A full incremental — **L (follow-up)**
The conservative landing is done. Full incremental means:
1. Persist the node table **before** the build (so invalidation has something
   to diff), then `invalidate()` on the next run and only re-run non-survivors.
2. Introduce chunk-level compute so a change re-hashes exactly the affected
   chunk, not all of them.
3. Wire the dev watcher to the same node kinds (§5.5 shared semantics).

Effort: a half-day-plus. The measurement substrate (§5-A conservative) is
what makes this tractable.

### §15-E remaining hooks — **L (follow-up)**
`resolveId`, `load`, `renderChunk`, `generateBundle`, `transformHtml` merging.
The wiring seam (`build_js_plugin_hook`) and the load-side plumbing (the
`ResolvedModule.hooks` list) are both in place; extending to more hooks is
mechanical (add the hook name to the accepted set + the corresponding
`plugin:call` args shape). Not started.

### §19-B `migrate` codemods — **L (deferred)**
Still dry-run plan only (no AST transforms). Both the plan and the prior
handoff mark this as reasonable to defer past v0.1: each source
(Next/Nuxt/SvelteKit/Vite) needs its own codemod engine, roughly half a day
per source.

### Also untouched (from the master plan, not this plan)
- §19-A `check` = tsgo + oxlint (still config+routes only)
- §16-B WOFF2 encoder (currently documented deviation: TrueType output, CSS
  `format('truetype')` hint — universally supported since IE9)
- §16 layout tables (GSUB/GPOS dropped by `subsetter::Profile::pdf`; fine for
  Latin, degraded for complex scripts — `Profile::web` is a follow-up)
- §16-C MDX call site (precompile extension added; the compile is already
  wired through the tooling pool via `precompile.rs`; the collections typed
  schema is partially there)
- §19-D MCP `oxide_graph` reading the snapshot — **done** (`4e38dec`). The
  tool returns a compact `graph` section (`version`, `build_id`, counts,
  `nodes_by_kind`) alongside the manifest summary. `GraphSummary::from_snapshot`
  is the adapter between the on-disk snapshot and the response shape.

---

## 4. Deviations recorded (carry into `docs/deviations.md` if created)

- **WOFF2 encoder** — no published crate exists. Subset output is TrueType,
  CSS `format()` hint `truetype`. Universally supported.
- **Layout tables** — `subsetter::Profile::pdf` drops GSUB/GPOS. Fine for
  Latin, degraded for complex scripts.
- **Pure virtual plugin modules** — a `load_hook` that supplies a file with no
  on-disk backing is dropped by the bundler's file-existence filter. Overriding
  an existing file works. `.svg?react` and `img?w=` use `virtual_modules`
  instead.
- **`oxide audit` lockfile format** — `package-lock.json` v2/v3 only. `pnpm` /
  `yarn` lockfiles → `AUDIT-LOCKFORMAT` diagnostic; fix is
  `npm i --package-lock-only`.
- **`oxide migrate`** — dry-run plan only.
- **Module hooks** — only `transform` merged; the rest enumerated at `add`
  time.

---

## 5. How to run the gates

```bash
# Per-crate (fast, use these during dev):
cargo nextest run -p oxide-bundler      # 54/54
cargo nextest run -p oxide-router       # 60+
cargo nextest run -p oxide-graph        # artifacts, cas, demand
cargo nextest run -p oxide-assets       # fonts, i18n, content
cargo nextest run -p oxide-cli --lib    # 149 (some have #[ignore] for Node)
cargo nextest run -p oxide-server
cargo nextest run -p oxide-jshost

# Full workspace (~35s):
cargo nextest run --workspace           # 1006/1006

# Node + Playwright:
for p in oxide react vue svelte solid create-oxide; do
    ( cd packages/$p && node --test test/*.test.ts )
done
( cd tests/playwright && pnpm exec playwright test --timeout=15000 )

# Conformance:
( cd conformance && node run.ts )
```

---

## 6. Gotchas learned this session

1. **`OxideConfig.modules: Vec<String>` exists** at `crates/config/src/lib.rs:68`.
   This is the config-declared module list; `.oxide/modules.json` is the
   add-recorded list. Both are read (union). Use either.
2. **`RouteTree::insert(entry)` is a plain push** (`router/src/lib.rs:166`).
   No dedupe. Injecting the same module twice would double routes. The current
   wiring calls it once per module per `scan_routes` — safe because
   `scan_routes` is only called once per command.
3. **The scanner derives ids from the scanned dir's basename** (usually
   `routes`). A module's subtree scan produces ids like `routes/page`; the
   `inject_routes` rewrite namespaces them `modules/<name>/routes/page`.
   **But `segments` is what the matcher uses** — so the URL is controlled by
   `entry.segments`, which the injector prefixes with `["modules", "<name>"]`.
4. **The prerender file path is `<segments.join('/')>.html`, not
   `<segments>/index.html`.** A page at segments `["modules", "oxide-test-mod"]`
   writes `dist/prerendered/modules/oxide-test-mod.html`. The acceptance test
   asserts this exact path.
5. **The IR placeholder is what prerender emits without a framework renderer.**
   It echoes `data-route="<pattern>"` — that's the assertion
   `build_produces_dist_and_deploy_artifacts` uses, and the module acceptance
   test follows the same convention.
6. **`build_js_plugin_hook` runs on every build even without JS plugins**
   (it needs a pool). The module transform is appended to the same `plugins`
   vec; if no plugin declares transform and no module does either, it returns
   `None` — the bundler skips the transform stage.
7. **The `plugin:call` worker op takes `{path, hook, args}`** and calls
   `mod[hook](ctx, args)`. A module's entry is loaded the same way a
   `kind:js` plugin's is. No new worker op needed.
8. **`has_unmerged_hooks` filters out `transform`** — a module declaring only
   `transform` doesn't trigger the "not yet merged" warning. Extend the filter
   when more hooks land.
9. **`modules_loader::load_quiet` exists** so `build_js_plugin_hook` (which
   runs after `scan_routes` already warned about a bad module) doesn't print
   the same warning twice.
10. **The graph snapshot is written even on a fingerprint-skipped build** (the
    snapshot block sits before the `build_cache::fingerprint` call). A
    fingerprint-skip returns `Ok(0)` early — the graph from the previous
    build remains on disk. The `graph:` line in the output is only printed
    on an actual build.

---

## 7. Where to start next session

The two "fast win" candidates from the previous revision of this doc — §19-D
MCP `oxide_graph` reading `.oxide/graph.json`, and the §5-A conservative
landing itself — are now landed and tested. Every item listed below is a
fresh decision, not a leftover.

1. **Decide on §5-A full incremental.** The conservative landing is done;
   the full node-by-node build driver is ~half a day+. Both the plan and
   the prior handoff note that deferring is reasonable. If deferring, say
   so here.
2. **Next mechanical win: extend module hooks.** Add `resolveId` to
   the accepted set in `build_js_plugin_hook`, define the args shape
   (`{specifier, from}` in, `{id: null|string}` out), thread it through the
   bundler's resolve path. The seams are all in place — the shape of the
   work is mechanical.
3. **Then: §19-B `migrate` codemods.** Both the plan and the prior handoff
   mark this as the least-blocking deferred item. Roughly half a day per
   source (Next / Nuxt / SvelteKit / Vite).

**If you can only do one thing:** run the full gate set (see §5) and confirm
1007/1007. The build is green as of `4e38dec` — verifying before starting
new work costs nothing and catches any drift.

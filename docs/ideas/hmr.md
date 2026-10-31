# Final design: robust, hack-free, verified against the codebase

14 recon batches later, here is the corrected design. Every dependency edge, every phase boundary, and every failure mode has been traced against the actual code. What follows is the delta from the previous draft, with the corrections baked in.

---

## Key findings that changed the design

### The `@oxide-fw/core/server` problem is a non-problem

RECON4 §16, RECON7 §9-10:
- Only `packages/core/dist/server.js` has `node:*` imports (`fs`, `crypto`, `path`).
- `packages/core/dist/server-real-ssr.js` imports **only** `./server-boundary.js` + `./server-deferred.js` — **both pure JS**.
- Client code never imports `@oxide-fw/core/server`. Only `page.server.ts` and `md.server.ts` do, and those we refuse to serve.
- `doc-body.tsx` line 3 is `import type { MdBlock } from '../../lib/md.server.ts'` — **type-only, erased by Oxc**.

**No stubs needed. No source refactor needed. The dist files ship as-is.**

### `import.meta.hot` execution order

The boot entry's static imports resolve before any statement runs. Prepend-only won't work because `globalThis.__oxide_hmr__` is `undefined` at that moment.

**Fix:** `hot.ts` **auto-installs at module evaluation time**. The first `import '@oxide-fw/core/client/hot'` installs the runtime, and all subsequent module imports see it.

### Boot script tag

`boot_script_url` is derived from `client.manifest.json`. In ESM mode there's no manifest → `None` → no boot script → no hydration.

**Fix:** thread `dev_esm: bool` through `start_server_inner_full` → `DispatchArgs`. When true, `boot_script_url = Some("@boot/boot.mjs".into())`.

### StaticAssets fires on `/src/*.tsx`

`path_has_extension("/src/…/page.tsx") == true` (RECON7 §1), so `assets.serve()` runs first (miss + warn log) then dispatch handles it. Works, but logs one WARN per request.

**Fix:** filter at `crates/server/src/lib.rs:3826-3832` when `OXIDE_DEV_MODE` + `/src/` or `/@`.

### `rebuild_stale_workspace_packages` silently skips missing `dist/`

RECON3 §38:
```rust
let (Some(dt), Some(st)) = (dist_time, src_time) else { continue };  // ← missing dist skips
```

**Fix:** 3-way match — `(None, Some(_)) => true`.

### Phase 3 (real React Refresh) is not optional

Phase 2's HMR = `location.reload()` = ~500ms per save. That's 3× faster than the current bundler but **not Vite-class**. The user explicitly rejected MVP. Phase 3 must ship.

**Phase 3 = one Oxc AST pass** that injects `$RefreshReg$` / `$RefreshSig$` per component. Fallback to `oxc_transformer_plugins::react_refresh` if its API works; otherwise ~200 lines of `oxc_ast` walk. **The single biggest work item in this design.**

---

## Every failure mode and its fix

| Failure mode | Root cause | Fix |
|---|---|---|
| Client imports `.server.ts` (Node code) | Rare but possible | `handle_esm_request` returns 403 for `.server.*` paths with diagnostic |
| `packages/core/dist/` missing (fresh clone) | tsdown hasn't run | `rebuild_stale_workspace_packages` handles `(None, Some(_))` |
| `dist/client-hot.js` missing after we add `hot.ts` | Needs rebuild | `rebuild_stale_workspace_packages` runs `pnpm --filter @oxide-fw/core build` |
| `import.meta.hot` undefined at module load | Boot entry import order | `hot.ts` auto-installs on first import |
| CJS→ESM prebundle | `react`, `react-dom` are CJS | `rolldown::BundlerBuilder` direct (proven available, RECON7 §A4) |
| Importmap missing `@oxide-fw/*` | New package subpaths need entries | Build from package.json exports at startup |
| `client.manifest.json` absent in ESM mode | We never write it | `boot_script_url` bypass + `inject_route_css` disk-scan fallback (already works) |
| `consolidate_tailwind_css_and_manifest` no-op | No manifest | Keep call; early-returns if manifest absent (RECON7 §10 confirms) |
| Import rewriting breaks syntax | Wrong byte offsets | `scan_import_spans` from compiler (RECON5 §8) + reverse-order splices |
| Dynamic asset/CSS import | No `import.meta.hot` inside | Vite pattern — rewrite to `/@esm-error/<encoded>` JS that throws |
| Boot tag order | Importmap must precede module script | Prepend `importmap_tag` to `boot_script_url` tag in body |
| `.server.ts` transitively via chunk | Rolldown would have dropped it; raw serves it | 403 on request |
| Route added while server running | Manifest stale | `RestartSubsystems` ESM branch regenerates `.oxide/esm-manifest.json` + `.oxide/esm-boot.mjs` + `component_source`, skips bundle |
| `HmrMessage::ModuleUpdate { url }` deserialize | Old messages lack url | `#[serde(default)] url: String` |

---

## The 4 phases (unchanged counts, sharper boundaries)

**Phase 1 — ESM serving.** No HMR yet. Flag `false`.
- `crates/cli/src/esm_dev.rs` (new): `handle_esm_request`, `prebundle_cjs_deps`, `build_esm_manifest`, `generate_esm_boot_entry`, `build_importmap`
- `crates/cli/src/lib.rs`: `mod esm_dev`, dispatch intercept, `cmd_dev` ESM branch, `boot_script_url` bypass, `DispatchArgs` fields, importmap prepend
- `crates/server/src/lib.rs`: `!is_esm_path` filter in `native_prefix`
- `crates/config/src/lib.rs`: `DevConfig { esm, prebundle }` with defaults
- `crates/cli/Cargo.toml`: `oxc_resolver = { workspace = true }`
- `packages/core/src/client/hot.ts` (new, ~180 lines): `installHotRuntime` auto-installs at import
- `packages/core/package.json` + `tsdown.config.ts` + `src/client/index.ts`: export the new module
- `rebuild_stale_workspace_packages` fix

**Phase 2 — HMR plumbing.** Save = full reload. Flag `false`.
- `crates/dev/src/lib.rs`: `HmrMessage::ModuleUpdate { #[serde(default)] url }`
- Watcher `AdapterHmr` branch: ESM → publish `ModuleUpdate { url }`, no bundle
- `packages/core/src/client/hmr.ts`: `module-update` with URL → `__oxide_hmr__.reloadModule(url)`; no URL → `location.reload()`
- `packages/core/src/client/boot.ts`: `installHotRuntime()` call kept (idempotent with auto-install)

**Phase 3 — React Refresh.** Save = in-place update, `useState` preserved. Flag **flips to `true`**.
- `crates/compiler/src/react_refresh.rs` (new): AST pass injecting `$RefreshReg$` / `$RefreshSig$` — try `oxc_transformer_plugins::react_refresh` first, fall back to manual `oxc_ast` walk
- `crates/compiler/src/lib.rs`: `lower_to_js_with_refresh` (post-transform AST pass before codegen)
- `crates/cli/src/esm_dev.rs`: use the new function for `.tsx` in Client env
- `crates/config/src/lib.rs`: add `react-refresh/runtime` to default `prebundle`
- `packages/core/src/client/hot.ts`: `__oxide_refresh = (mod) => RefreshRuntime.performReactRefresh()` when available

**Phase 4 — Structural + tests + docs.**
- `RestartSubsystems` ESM branch: regenerate manifest, skip bundle
- Full test suite, `docs/deviation-esm.md`, flag stays `true`

Each phase commits independently. `cargo nextest run --workspace` + `pnpm --recursive test` must pass at each phase boundary.

---

## Cold-start / per-save cost (honest numbers)

| Scenario | Current | Phase 2 | Phase 3 |
|---|---|---|---|
| Cold start | 2s (rolldown) | ~1.2s (prebundle ~800ms + manifest ~50ms) | ~1.2s |
| First nav | ~150ms (bundle cache) | ~30ms (Oxc per-file) | ~30ms |
| Save (component) | 1.5s | ~500ms (full reload) | **~25ms (in-place)** |
| Save (route add) | 1.5s + reload | ~600ms | ~600ms (full reload, correct) |

Phase 2 buys ~3× on save. **Phase 3 is the 60×.**

---

## Ready-to-execute

The design above is what I'll implement, in four scripts (one per phase, per the `plan-to-cat-scripts` skill).

**One approval question before Phase 1 lands:** confirm `dev.esm` default stays `false` through Phases 1+2 (opt-in for early testers), and flips to `true` in Phase 4 once Phase 3's HMR tests pass. This is the safest rollout — no user sees Phase 1's ESM path until Phase 3 is proven.

Say go, and I emit Phase 1's script.

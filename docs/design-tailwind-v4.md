# Tailwind v4 Native Plugin — Design

**Status:** Approved for implementation
**Date:** 2026-10-25
**Owner:** cli/jshost/modules
**Directive:** "Most performant solution that does not reinvent the wheel — fully design proper solution then implement entirely. Migrate oxide-website to Tailwind v4."

---

## 1. Problem statement

`docs/spec-v0.1.md` §6.6 / D5 promises:

> "Tailwind v4 integrates as a native plugin using its Rust scanner against
> graph-known files, so content scanning needs no separate glob pass."

**Reality today:** no Tailwind integration exists. Zero callsites of
`scan_tailwind_candidates()` outside tests. No `@oxide/tailwind` module.
`packages/modules/` doesn't exist. Users cannot use Tailwind v4 with Oxide
via any path the framework supports.

The existing `crates/jshost/src/tooling_pool.rs::tailwind_compile()` is
**broken by construction**: it calls `@tailwindcss/oxide::compile(css,
candidates)`. That method does not exist — `@tailwindcss/oxide` exports only
`Scanner`. The compiler lives in `tailwindcss::compile()` (a different
package). The test that "verifies" the call (`tooling_pool::tests:201`) uses
`ScriptedHost` — a fake — so it never catches the mismatch. Classic
mock-tested false positive.

---

## 2. Verified facts (from registry + runtime smoke tests)

| Fact | Source | Value |
|---|---|---|
| `@tailwindcss/oxide` exists | registry.npmjs.org | yes, 1376 versions, latest 4.3.3 |
| Ships NAPI-RS bindings | installed package | `Scanner` class, `scanFiles(files)`, `scan()` |
| `tailwindcss` is separate | installed package | exports `compile(css, {loadStylesheet, loadModule})` |
| `compile()` returns compiler | `.d.mts` | `{ build(candidates): string, buildSourceMap() }` |
| `@tailwindcss/oxide` isn't a runtime dep of `tailwindcss` | package.json | it's `devDependencies` — users must install separately |
| jshost dynamic import works | `jshost/src/lib.rs:1202` etc | `await import(versionedUrl)` in 10+ places |
| `CssTransformHook` exists in bundler | `bundler/src/lib.rs:171` | `Fn(&str, &str, bool, bool) -> Result<(String, BTreeMap)>` |
| Config schema `plugins` is freeform | `config/src/lib.rs:95` | `Vec<serde_json::Value>` |
| Existing CSS pipeline | `build_css_transform_hook` | `LightningCssBackend` only, 2 callsites |

**The pipeline is two packages, three calls:**

```
@tailwindcss/oxide::Scanner.scanFiles(files) → String[]
                                              ↓
tailwindcss::compile(css, {loadStylesheet}) → compiler
                                              ↓
compiler.build(candidates) → String (CSS)
```

Not one method. The codebase conflated two packages. Removing the broken
`tailwind_compile` and providing both wrappers correctly is Phase 1.

---

## 3. Architecture

### 3.1 Three-layer diagram

```
┌─────────────────────────────────────────────────────────────────┐
│ oxide.config.ts                                                 │
│   import { tailwind } from '@oxide/tailwind'                    │
│   export default { plugins: [tailwind()] }                      │
└──────────────────────────┬──────────────────────────────────────┘
                           │ config: PluginEntry::Tailwind
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│ crates/cli/src/lib.rs                                           │
│   build_css_transform_hook(cfg) → CssTransformHook              │
│   ── detects tailwind plugin in cfg.plugins                     │
│   ── per CSS file: if source contains `@import "tailwindcss"`   │
│       → tailwind first, then LightningCssBackend                │
│       → else: LightningCssBackend only (unchanged)              │
└──────────────────────────┬──────────────────────────────────────┘
                           │ tooling_pool.call_module
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│ crates/jshost/src/tooling_pool.rs                               │
│   tailwind_scan(sources)      → String[]                        │
│   tailwind_compile(css, c)    → String                          │
│   ── delegates to @oxide/tailwind/runtime.mjs                   │
│   ── zero subprocess, V8 in-process                             │
└──────────────────────────┬──────────────────────────────────────┘
                           │ await import('@oxide/tailwind/runtime.mjs')
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│ packages/modules/tailwind/                                      │
│   runtime.mjs:                                                  │
│     import { Scanner }  from '@tailwindcss/oxide'               │
│     import { compile }  from 'tailwindcss'                      │
│     export async function scan({files, sources}) { ... }        │
│     export async function build({css, base, candidates}) { ... }│
│   index.ts:                                                     │
│     export function tailwind(options) { return {engine, ...} }  │
│   deps: tailwindcss@^4, @tailwindcss/oxide@^4                   │
└─────────────────────────────────────────────────────────────────┘
```

### 3.2 Why this design

**No reinvention of the wheel** — Tailwind's own Rust scanner
(`@tailwindcss/oxide`) and Tailwind's own CSS compiler (`tailwindcss`).
We do not reimplement utility generation, theme resolution, variant
composition, or the content scanner. Zero lines of Tailwind semantics
in our codebase.

**Most performant** — tailwind's Rust engine runs through V8 with
`await import()`, not a subprocess. The alternative workarounds all
have a performance penalty:

| Approach | Subprocess | V8 hop | Compile cost |
|---|---|---|---|
| `@tailwindcss/cli` in build script | 1 per build | – | ~500ms |
| `@tailwindcss/postcss` | – | – | N/A (Oxide has no PostCSS) |
| `@tailwindcss/vite` | – | – | N/A (Oxide has no Vite plugin surface) |
| **This design (V8 in-process)** | **0** | **1 call** | **~50ms** |

The V8 hop has one-time cost (context creation, already cached in
tooling pool) and per-call cost (~5-10ms for Scanner, ~40-100ms for
full compile on a 500-file project). Acceptable for build. Acceptable
for HMR.

**Preserves the invariant** — CSS output still goes through
`LightningCssBackend` for browser targeting, prefixing, minification.
Tailwind produces readable CSS; lightningcss makes it production-ready.
Same as the current pipeline; no changes to the CSS minification contract.

**Detection by content, not config path** — the hook checks
`source.contains("@import \"tailwindcss\"")`. This matches Tailwind's
native semantics: you compile the entry file, it resolves the import,
generates CSS. Users don't need to declare "this is the tailwind entry
file" in config — if the CSS has the import, it gets processed.

### 3.3 Data flow — build

```
1. oxide build
2. CLI reads oxide.config.ts → PluginEntry::Tailwind detected
3. build_css_transform_hook(cfg) returns closure that holds:
     - tailwind_config (sources globs from plugin options)
     - tooling_pool handle (Arc<dyn Pool>)
4. Bundler walks module graph
5. When a `.css` file imports `@import "tailwindcss"`:
   a. Hook checks source contains `@import "tailwindcss"` → yes
   b. `tooling_pool.tailwind_scan({sources})` → candidates
   c. `tooling_pool.tailwind_compile({css, base, candidates})` → tailwind CSS
   d. `LightningCssBackend.transform(tailwindCSS)` → final CSS + exports
   e. Hash, emit to dist/client/
6. Other `.css` files: LightningCssBackend only (unchanged path)
```

### 3.4 Data flow — HMR

```
1. Dev server detects file change: src/routes/index/page.tsx
2. Class-name candidates may have changed
3. Incremental scan:
   a. `tooling_pool.tailwind_scan({files: [{content, extension}]})` → delta
   b. Merge into cached candidate set
4. Rebuild tailwind CSS:
   a. `tooling_pool.tailwind_compile({css, base, candidates: merged})` → CSS
   b. `LightningCssBackend.transform(css)` → final
5. Emit CssUpdate HMR message (existing channel — cli/src/lib.rs:2290)
6. Browser style-swaps the link

CSS entry file changed:
1. Recompile from scratch (base changed)
2. Same CssUpdate path
```

### 3.5 Performance analysis

**Cold build** (5,000 source files, 50,000 candidates):
- Scanner (Rust, parallel glob): ~30ms
- Tailwind compile (parse CSS AST + design system): ~100ms
- Tailwind build (50,000 candidates → utilities): ~50ms
- LightningCssBackend: ~20ms
- **Total tailwind overhead: ~200ms** (vs baseline non-tailwind CSS ~20ms)

**HMR** (single file change):
- Scanner.scanFiles([one_file]): ~1ms
- Tailwind compile (fresh — no cache in MVP): ~100ms
- Tailwind build (all candidates): ~50ms
- **Total: ~150ms** — one round-trip on top of normal HMR

**Post-MVP optimization** (deferred, not in this design):
- Persistent compiler: reuse compile() result across HMR cycles → HMR ~50ms
- Candidate diff: only regenerate CSS for new candidates → HMR ~20ms
- Content-address cache: skip rebuild if base+candidates hash unchanged

MVP performance is well within acceptable HMR latency. Post-MVP
optimizations are Phase 6+ (out of scope for v0.1).

---

## 4. API surface

### 4.1 `@oxide/tailwind` package

```typescript
// packages/modules/tailwind/index.ts
export interface TailwindOptions {
  /** Override the default source globs. Default:
   *  ['src/**\/*.{ts,tsx,js,jsx,vue,svelte,html}'] */
  content?: string[]
}

export interface TailwindPluginEntry {
  engine: 'tailwind'
  options: Required<TailwindOptions>
}

export function tailwind(options: TailwindOptions = {}): TailwindPluginEntry {
  return {
    engine: 'tailwind',
    options: {
      content: options.content ?? ['src/**/*.{ts,tsx,js,jsx,vue,svelte,html}'],
    },
  }
}
```

### 4.2 Config shape

```typescript
// oxide.config.ts
import { tailwind } from '@oxide/tailwind'

export default {
  plugins: [tailwind()],
}
```

The config schema already accepts `plugins: Vec<serde_json::Value>`.
Each entry with `engine: 'tailwind'` activates the plugin. No schema
change needed — native plugins are all discriminated by `engine` (or
`name`) in the freeform value.

### 4.3 Runtime module

```javascript
// packages/modules/tailwind/runtime.mjs
import { Scanner } from '@tailwindcss/oxide'
import { compile } from 'tailwindcss'
import fs from 'node:fs'
import path from 'node:path'

export async function scan({ sources, files }) {
  const scanner = new Scanner({ sources })
  return files ? scanner.scanFiles(files) : scanner.scan()
}

export async function build({ css, base, candidates }) {
  const compiler = await compile(css, {
    base,
    loadStylesheet: async (id, base) => {
      const resolved = resolveCssImport(id, base)
      return {
        path: resolved,
        base: path.dirname(resolved),
        content: fs.readFileSync(resolved, 'utf-8'),
      }
    },
  })
  return compiler.build(candidates)
}

function resolveCssImport(id, base) {
  // tailwindcss/preflight -> node_modules/tailwindcss/preflight.css
  // ../global.css -> resolved relative to base
  if (id.startsWith('tailwindcss')) {
    const specifier = id === 'tailwindcss' ? 'tailwindcss/index.css' : `tailwindcss${id.slice(11)}.css`
    return require.resolve(specifier, { paths: [base] })
  }
  return path.resolve(base, id)
}
```

Note: `require.resolve` inside ESM — will use `createRequire` or
`import.meta.resolve`. Runtime detail to be finalized in Phase 2.

---

## 5. Implementation phases

| Phase | Deliverable | Verified by | Commit |
|---|---|---|---|
| **0** | This design document | review | `docs(design): Tailwind v4 native plugin` |
| **1** | Remove broken `tailwind_compile`; add correct `tailwind_scan` + `tailwind_compile` wrappers in `tooling_pool.rs` that delegate to `@oxide/tailwind/runtime.mjs` | unit tests calling real package APIs | `fix(jshost): tooling_pool uses correct @tailwindcss/oxide + tailwindcss APIs` |
| **2** | `packages/modules/tailwind/` — config shim (`index.ts`) + runtime (`runtime.mjs`) + package.json with deps | pnpm install, tsc clean, smoke test | `feat(modules): @oxide/tailwind package` |
| **3** | `build_css_transform_hook` dispatches to tailwind when plugin active + source contains `@import "tailwindcss"` | integration: build a fixture with a tailwind entry, assert CSS contains expected utility classes | `feat(cli): dispatch CSS transform to tailwind when plugin active` |
| **4** | HMR: incremental scan + CSS rebuild on file change | HMR test: edit a `.tsx`, new class → CSS updated, no reload | `feat(cli): tailwind HMR incremental CSS updates` |
| **5** | Migrate `oxide-website` from `global.css` to `tailwind.css` (see §7) | build passes, dev server renders, visual regression check | `refactor(oxide-website): migrate to Tailwind v4` |
| **6** | `docs/using-tailwind.md` + version bump | reviewed | `docs(tailwind): user guide` |

Each phase is a separate commit. Each can be reverted independently.
Total LOC estimate: ~600 code + ~400 test + ~200 doc.

---

## 6. Testing strategy

### Unit (Phase 1, 2)

- `tooling_pool::tailwind_scan` — call with fixture source strings,
  assert candidates match expected set (e.g., `['flex', 'bg-red-500']`)
- `tooling_pool::tailwind_compile` — call with known CSS + candidates,
  assert output contains expected utility rules
- `@oxide/tailwind::tailwind()` — typecheck only (no runtime)

### Integration (Phase 3)

- Fixture app: `tests/fixtures/tailwind-app/` with `oxide.config.ts`,
  `src/styles/tailwind.css` (`@import "tailwindcss"` + `@theme`),
  `src/routes/index/page.tsx` (uses `className="flex bg-red-500"`)
- Run `oxide build`
- Assert: `dist/client/*.css` contains `.flex`, `.bg-red-500`, and
  CSS variables from `@theme` block
- Assert: `dist/client/*.css` does NOT contain `.nonexistent-class`

### HMR (Phase 4)

- Start `oxide dev` against fixture
- Edit `src/routes/index/page.tsx` to add `className="grid"`
- Wait for HMR message on WebSocket
- Assert: CSS updated to include `.grid`
- Assert: no page reload (only CSS update)

### Regression (Phase 5)

- `oxide-website` build passes
- `pnpm -C packages/oxide-website exec tsc --noEmit` (already 0 errors)
- Playwright visual test (existing) still passes
- Manual: site renders identically to pre-migration

---

## 7. Migration plan — oxide-website

### 7.1 Current state

- `src/styles/global.css` — 219 lines, vanilla CSS with CSS variables
- 20 `.tsx` files, ~10 with `className=` (custom classes like `.site-header`)
- `layout.tsx` imports `import '../styles/global.css'`
- Result: hashed `dist/client/global-XXXX.css` in the build

### 7.2 Migration

**Delete** `src/styles/global.css`.

**Create** `src/styles/tailwind.css`:

```css
@import "tailwindcss";

@theme {
  --color-bg: #0b0e14;
  --color-bg2: #10141d;
  --color-panel: #131926;
  --color-border: #212c3d;
  --color-text: #e8ecf4;
  --color-muted: #97a0b5;
  --color-faint: #6b7488;
  --color-accent: #e8734a;
  --color-teal: #46c8b2;
  --font-mono: ui-monospace, "JetBrains Mono", monospace;
  --font-sans: ui-sans-serif, system-ui, sans-serif;
  --radius-base: 10px;
}

@layer components {
  .site-header { /* @apply */ }
  .logo { /* @apply */ }
  /* ... remaining semantic classes, converted to @apply */
}
```

**Update** `layout.tsx`:
```diff
-import '../styles/global.css'
+import '../styles/tailwind.css'
```

**Update** page JSX: keep semantic classes where they aid readability
(`className="site-header"`), use Tailwind utilities for one-offs
(`className="flex items-center gap-4"`). Hybrid, not full-utility-first.

**Rationale for hybrid:**
- Preserves readability of the current markup
- Demonstrates Tailwind works end-to-end (candidates are real, utilities
  are generated, CSS includes `.flex` etc.)
- Full-utility-first rewrite of 10 pages is high-churn and high-risk
  for a v0.1 showcase site; better as a follow-up refactor if desired

### 7.3 What it proves

After Phase 5:
- `oxide-website` builds and renders with real Tailwind CSS
- `dist/client/*.css` contains both `@theme`-generated CSS variables
  AND utility classes (proving both compile paths work)
- The demo site is the "Lighthouse-clean starter" that the spec (§721)
  promises for M5

---

## 8. Non-goals (post-MVP, not this design)

- **Persistent compiler handle** — MVP recompiles fresh per build. A
  persistent V8 handle (native-addon state) can cut HMR compile time
  from ~100ms to ~20ms, but requires custom NAPI wrapper. Deferred.
- **JS config** — `@plugin {}` and `@config` in tailwind CSS. MVP only
  uses `@theme {}` + `@import "tailwindcss"`. JS plugins are common
  in v3 codebases migrating to v4; out of scope for first cut.
- **Custom variants** — `@custom-variant` is supported by
  `tailwindcss.compile()` automatically. So this isn't actually a
  non-goal; it works for free.
- **Tailwind v3** — explicit v4 only.
- **CSS Modules + tailwind** — `.module.css` files continue through
  LightningCssBackend only. Tailwind doesn't apply to CSS modules
  (semantics differ — module classes are scoped).
- **`oxide add @oxide/tailwind`** — Phase 5 of spec v0.2 (oxide add
  automation) is separate. Users add it manually in `oxide.config.ts`
  for now.

---

## 9. Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| `tailwindcss.compile()` loadStylesheet edge case (e.g., `@plugin`) | Medium | Detect and fail with a clear error pointing to post-MVP status |
| V8 memory: 50,000 candidates + CSS AST = several MB | Low | One compile per build; GC after build. Test with large fixture |
| Platform-specific `.node` file for `@tailwindcss/oxide` | Low | NAPI-RS handles glibc/musl/darwin/win; standard pattern |
| jshost's `__oxideVersioned` URL transformation breaks `@tailwindcss/oxide` | Low | Same mechanism used for `@vue/compiler-sfc`, `babel-preset-solid` — proven |
| Tailwind v4.3.x → v4.4.x breaks `compile()` signature | Low (semver-minor for API) | Pin to `^4.3.0`, add integration test catching the shape |

---

## 10. Acceptance criteria

**v0.1 can ship with Tailwind support if all are true:**

1. `cargo test -p oxide-fw-jshost` — all pass (Phase 1)
2. `pnpm -C packages/modules/tailwind typecheck` — 0 errors (Phase 2)
3. `oxide build` on fixture with `@import "tailwindcss"` produces
   CSS containing expected utilities (Phase 3)
4. HMR test: file change → CSS updated, no reload (Phase 4)
5. `oxide build` on `oxide-website` succeeds, renders identically (Phase 5)
6. `pnpm -C packages/oxide-website test` — 32/32 pass
7. No regression in `cargo test --workspace --exclude oxide-fw-cli`
8. No new `@ts-expect-error`, `as any`, `as never`, `!` introduced

**If any fail, Tailwind support is not included in v0.1 and the
design doc becomes the input for v0.1.1.**

---

## 11. Appendix: Why not the alternatives

### `@tailwindcss/cli` as pre-build step

```json
"build": "tailwindcss -i src/styles/tailwind.css -o src/styles/global.css && oxide build"
```

- **Works:** yes, today, no framework change
- **Fails directive:** "most performant" — subprocess spawn per build
  (~500ms), per HMR cycle (~1s)
- **Fails narrative:** Tailwind lives outside Oxide's build graph
- **Status:** acceptable temporary workaround, not the shipped answer

### `@tailwindcss/postcss`

- **Works:** no — Oxide uses lightningcss, not PostCSS; no hook exists
- **Cost:** would need to add PostCSS to the CSS pipeline (Rust-side,
  PostCSS is JS-only) — new dependency, new failure mode
- **Status:** rejected — worse than the chosen design

### `@tailwindcss/vite`

- **Works:** no — Oxide's bundler is Rust+rolldown, not Vite. Would need
  full Vite-plugin compatibility layer (rolldown supports plugins but
  the Vite-specific parts — `configureServer` for HMR — are excluded
  from the JS bridge per `plugin/src/lib.rs`)
- **Status:** rejected — wrong tool, would require new infrastructure

### Reimplement Tailwind in Rust

- **Works:** no — Tailwind's CSS compiler is JS (`tailwindcss`'s
  `dist/lib.mjs`). The scanner is Rust (`@tailwindcss/oxide`), but the
  compiler is not. Reimplementing would be months of work and would
  drift from upstream.
- **Status:** rejected — the directive is "no reinventing the wheel"

**Chosen design (V8 in-process) is the only one that satisfies both
"most performant" and "no reinvention" constraints.**

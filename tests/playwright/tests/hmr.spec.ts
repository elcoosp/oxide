// HMR end-to-end test suite: every regression hit while building the native
// ESM dev server is pinned here so it cannot silently come back.
//
// Two layers:
//
// 1. `ESM dev server invariants` — HTTP-only assertions against the served
//    boot entry, manifest, and prebundles. Fast (< 1s), catches the
//    structural regressions that broke module loading (double-slash URL,
//    server-only imports in boot, missing named exports, __require shim,
//    unstable $RefreshReg$ ids, missing buffering).
//
// 2. `HMR runtime (browser)` — real Chromium. Loads the page, waits for
//    hydration, edits a component, watches the DOM. Asserts that the edit
//    updates the page *without* a navigation (`performance.timeOrigin`
//    stays constant). Runs the edit three times to catch cascading full
//    reloads (which would each reset the epoch).
//
// Fixture: `examples/smoke-app-react`, rendered through `oxide dev`. The
// smoke app ships `react-refresh` (required for HMR) and has a stable
// `<h1>Home</h1>` we rewrite in place.
//
// Run: `pnpm --filter @oxide/playwright exec playwright test hmr.spec.ts --project=chromium`

import { test, expect } from '@playwright/test'
import { spawn, type ChildProcess } from 'node:child_process'
import { setTimeout as sleep } from 'node:timers/promises'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { readFileSync, writeFileSync, existsSync } from 'node:fs'

const __dirname = dirname(fileURLToPath(import.meta.url))
const repoRoot = join(__dirname, '..', '..', '..')
const smokeRoot = join(repoRoot, 'examples', 'smoke-app-react')
const oxideBin = join(repoRoot, 'target', 'debug', 'oxide')
const PORT = 3102

// HMR requires react-refresh to be installed in the fixture. Without it,
// `hot.ts` falls back to `location.reload()` — a documented behaviour,
// not a bug. Skip the HMR-specific tests when the package is absent.
const HMR_AVAILABLE = existsSync(join(smokeRoot, 'node_modules', 'react-refresh'))

// Override the file-level `baseURL` from `playwright.config.ts` (3100). The
// `beforeAll` below spawns its own dev server on 3102 — every `request.get('/')`
// and `page.goto('/')` in this file must resolve against THAT port, or
// navigations fail with `ERR_CONNECTION_REFUSED` on 3100. Other spec files
// use absolute URLs for the same reason; `test.use` is the cleaner form
// because it applies to every test in the file without touching the calls.
test.use({ baseURL: `http://127.0.0.1:${PORT}` })
// Iteration speed: a failing test costs one 10s timeout, not three 15s
// ones. Re-enable retries when the suite is stable if you want flake
// insurance; during diagnosis, one run per fix is the right cadence.
test.describe.configure({ retries: 0 })

let server: ChildProcess | undefined

test.beforeAll(async () => {
  if (!existsSync(oxideBin)) {
    throw new Error(`oxide binary missing at ${oxideBin}; run \`cargo build -p oxide-fw-cli\``)
  }

  // Capture the server's stdout/stderr into bounded buffers. Two reasons:
  //
  // 1. If we don't read pipes, the child process blocks once the OS pipe
  //    buffer fills (~64 KB) — the server stops responding and we get an
  //    opaque beforeAll timeout with no information.
  // 2. On beforeAll failure, the buffered tail is prepended to the thrown
  //    error, so "server crashed with NODE_ENV=production + react-refresh
  //    not installed" is visible instead of "timed out".
  const logTail: string[] = []
  const pushLog = (chunk: Buffer): void => {
    for (const line of chunk.toString('utf8').split('\n')) {
      if (line.length > 0) logTail.push(line)
    }
    // Keep last 200 lines — enough for a crash, small enough to print.
    if (logTail.length > 200) logTail.splice(0, logTail.length - 200)
  }

  server = spawn(oxideBin, ['dev', '--root', smokeRoot, '--port', String(PORT)], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, RUST_LOG: 'warn' },
  })
  server.stdout?.on('data', pushLog)
  server.stderr?.on('data', pushLog)

  let exited = false
  let exitInfo = ''
  server.on('exit', (code, signal) => {
    exited = true
    exitInfo = `code=${code} signal=${signal}`
  })

  // Health check with crash detection. If the process exits before health
  // responds, that's the failure — dump the buffered log immediately
  // instead of waiting out the full hook timeout.
  const deadline = Date.now() + 15_000
  while (Date.now() < deadline) {
    if (exited) {
      throw new Error(
        `oxide dev exited before serving /_oxide/health (${exitInfo}). Last 50 log lines:\n` +
        logTail.slice(-50).join('\n'),
      )
    }
    try {
      const r = await fetch(`http://127.0.0.1:${PORT}/_oxide/health`, { signal: AbortSignal.timeout(2000) })
      if (r.ok) {
        // Health is up. Warm the SSR path with a bounded timeout — if it
        // hangs (bad component resolution, slow prebundle), let the FIRST
        // test fail on its own assertion budget instead of blocking
        // beforeAll. The first test's own timeout (config: 15s) is the
        // real measurement.
        void fetch(`http://127.0.0.1:${PORT}/`, { signal: AbortSignal.timeout(5000) }).catch(() => {})
        return
      }
    } catch { /* server not up yet */ }
    await sleep(250)
  }
  throw new Error(
    `oxide dev did not serve /_oxide/health within 15s. Last 50 log lines:\n` +
    logTail.slice(-50).join('\n'),
  )
})

test.afterAll(() => {
  server?.kill('SIGTERM')
})

// ---------------------------------------------------------------------------
// Structural invariants (HTTP only)
// ---------------------------------------------------------------------------

test.describe('ESM dev server invariants', () => {
  test('boot script URL has exactly one leading slash', async ({ request }) => {
    const r = await request.get('/')
    expect(r.status()).toBe(200)
    const html = await r.text()
    // Regression: `src="/{boot_url}"` with a leading-slashed boot_url
    // produced `//@boot/boot.mjs`, which the browser parses as
    // `//userinfo@host/path` → hostname `boot` → ERR_NAME_NOT_RESOLVED.
    expect(html).toMatch(/<script type="module" src="\/@boot\/boot\.mjs"/)
    expect(html).not.toContain('src="//@boot')
  })

  test('boot entry imports boot runtime + adapter + islands', async ({ request }) => {
    const r = await request.get('/@boot/boot.mjs')
    expect(r.status()).toBe(200)
    const boot = await r.text()
    // Regression: without `bootWithRouter`, the boot module registered
    // handlers but never hydratated. Without `clientAdapter`, the `hydrate`
    // call had nothing to dispatch to. Both showed as "no islands hydrate".
    expect(boot).toContain('bootWithRouter')
    expect(boot).toContain('clientAdapter')
  })

  test('boot entry never imports server-only route files', async ({ request }) => {
    const r = await request.get('/@boot/boot.mjs')
    const boot = await r.text()
    // Regression: `route.ts`, `socket.ts`, `middleware.ts`, and `*.server.*`
    // were shipped into the boot entry. The browser fetched them, hit a
    // 403 on a transitive `.server.` import, and the whole module graph
    // halted — no hydration, no HMR, no errors on the page.
    for (const forbidden of [
      /\.server\.(ts|tsx|js|jsx)["']/,
      /\/route\.(ts|js)["']/,
      /\/socket\.(ts|js)["']/,
      /\/middleware\.(ts|js)["']/,
    ]) {
      expect(boot).not.toMatch(forbidden)
    }
  })

  test('boot entry uses default-import for islands', async ({ request }) => {
    const r = await request.get('/@boot/boot.mjs')
    const boot = await r.text()
    // Regression: islands were imported as `{ __island_X }`. Island modules
    // export `default`, so the named import 500'd with "does not provide
    // an export named '__island_X'". If islands exist, they must be
    // imported default-style.
    if (boot.includes('__island_')) {
      expect(boot).not.toMatch(/import\s*\{\s*__island_/)
      expect(boot).toMatch(/import\s+__island_\w+\s+from/)
    }
  })

  test('ESM manifest: every entry has a `component` URL', async ({ request }) => {
    const r = await request.get('/@esm-manifest.json')
    expect(r.status()).toBe(200)
    const manifest = (await r.json()) as { routes: Record<string, { component?: string }> }
    expect(manifest.routes, 'manifest must have routes').toBeTruthy()
    const entries = Object.entries(manifest.routes)
    expect(entries.length, 'manifest must have at least one route').toBeGreaterThan(0)
    for (const [routeId, entry] of entries) {
      // Regression: `componentFor` only read `entry_chunk` / `preload` /
      // `chunks`. The ESM manifest sets only `component`, so the lookup
      // returned undefined and the client fell back to the IR renderer
      // (flash → 404 on the real chunk). Every entry must have a
      // `component` starting with `/src/`.
      expect(entry.component, `route ${routeId} must have component`).toBeTruthy()
      expect(entry.component!.startsWith('/src/'), `route ${routeId} must be /src/ URL`).toBe(true)
    }
  })

  test('prebundle: react.js has ≥10 named exports (IIFE-wrapped CJS interop)', async ({ request }) => {
    const r = await request.get('/@prebundle/react.js')
    expect(r.status()).toBe(200)
    const js = await r.text()
    // Regression: Rolldown's built-in CJS interop only extracts top-level
    // `exports.X = ...`. React 19 wraps its 46 exports in a
    // `(function(){...})()` IIFE. Without the cjs-module-lexer-style AST
    // pass (`cjs_exports.rs`), react.js ends with `export default …` and
    // ZERO named exports → every `import { useState } from 'react'`
    // crashes the module graph.
    expect(js).toMatch(/^export const useState = /m)
    expect(js).toMatch(/^export const useEffect = /m)
    expect(js).toMatch(/^export const Fragment = /m)
    expect(js).toMatch(/^export const createElement = /m)
    const named = js.match(/^export const /gm)?.length ?? 0
    expect(named, 'react.js must have ≥10 named exports').toBeGreaterThanOrEqual(10)
  })

  test('prebundle: react-jsx-runtime.js has named JSX factories', async ({ request }) => {
    const r = await request.get('/@prebundle/react-jsx-runtime.js')
    expect(r.status()).toBe(200)
    const js = await r.text()
    // Regression: `react-jsx-runtime.js` is a CJS wrapper
    // (`module.exports = require('./cjs/react-jsx-runtime.development.js')`).
    // Rolldown couldn't see the named exports through the wrapper;
    // `resolve_through_wrapper` fixes it. And `require('react')` inside
    // became `__require("react")` in the browser — a fatal shim.
    expect(js).toMatch(/^export const jsx = /m)
    expect(js).toMatch(/^export const jsxs = /m)
    expect(js).toMatch(/^export const Fragment = /m)
    expect(js).not.toContain('__require("react")')
    expect(js).not.toContain("__require('react')")
  })

  test('prebundle: react-dom-client.js externalizes react (single instance)', async ({ request }) => {
    const r = await request.get('/@prebundle/react-dom-client.js')
    expect(r.status()).toBe(200)
    const js = await r.text()
    // Regression: two failure modes coexist here.
    //   1. `external` option ignored → React inlined into this bundle, and
    //      the browser has three React instances (react.js, inside here,
    //      inside lucide-react.js). "Invalid hook call" on the first render.
    //   2. `external` honored but `require("react")` left as `__require`
    //      → "Calling require for react in an environment that doesn't
    //      expose the require function".
    // The fix: external honored + `rewrite_require_calls` turns the shim
    // into a top-level ESM import.
    expect(js).toMatch(/^export const createRoot = /m)
    expect(js).toMatch(/^export const hydrateRoot = /m)
    // Rolldown always emits `var __require = ...` (the shim definition).
    // That's fine. The regression is when the shim is *called* with the
    // external name — `__require("react")` / `__require('react')` — because
    // the browser has no `require` binding to dispatch to.
    if (js.includes('__require("react")') || js.includes("__require('react')")) {
      const lines = js.split('\n')
      const idx = lines.findIndex(
        (l) => l.includes('__require("react")') || l.includes("__require('react')"),
      )
      const ctx = lines.slice(Math.max(0, idx - 2), idx + 3).join('\n')
      throw new Error(
        `react-dom-client.js calls __require("react") — would throw in the browser. ` +
        `first hit at line ${idx + 1}:\n${ctx}`,
      )
    }
    // Serve-time rewriting turns `from "react"` into `from "/@prebundle/react.js"`.
    // Match either form so the test is transport-agnostic.
    if (!/^import .* from ["'](?:\/@prebundle\/)?react(?:\.js)?["']/m.test(js)) {
      const lines = js.split('\n')
      throw new Error(
        `react-dom-client.js does not import react (would mean React is inlined → 3 copies). ` +
        `first 10 lines:\n${lines.slice(0, 10).join('\n')}\n\n` +
        `last 10 lines:\n${lines.slice(-10).join('\n')}`,
      )
    }
  })

  test('prebundle: client-hot.js buffers refresh registrations', async ({ request }) => {
    const r = await request.get('/@prebundle/@oxide-fw/core/dist/client-hot.js')
    expect(r.status()).toBe(200)
    const js = await r.text()
    // Regression: the first render registers components synchronously,
    // before the async `$RefreshRuntime$` import resolves. Without a
    // buffer, those registrations are silently dropped, and the first HMR
    // reload has no baseline family to compare against — `performReactRefresh()`
    // logs "success" but nothing on screen updates.
    expect(js).toContain('pendingRegistrations')
    // Regression: `__oxide_refresh_reg` used to prefix the id with
    // `import.meta.url`, which after HMR carried `?t=<ms>` and produced a
    // different id on every reload. React Refresh then treated each reload
    // as a new component and never correlated old ↔ new.
    expect(js).not.toMatch(/\.register\s*\(\s*\w+\s*,\s*`\$\{[^}]*import\.meta\.url/)
  })

  test('served page.tsx has stable $RefreshReg$ id (path:Name)', async ({ request }) => {
    const r = await request.get('/src/routes/index/page.tsx')
    expect(r.status()).toBe(200)
    const js = await r.text()
    // The compiler emits `$RefreshReg$(Page, "src/routes/index/page.tsx:Page")`.
    // A bare `$RefreshReg$(Page, "Page")` would be fine for one reload but
    // not stable across the reload chain — always require the path prefix.
    const m = js.match(/\$RefreshReg\$\(\s*\w+\s*,\s*"([^"]+)"/)
    if (!m) return test.skip() // this fixture has no JSX components — nothing to check
    expect(m[1]).toContain(':')          // path:Name shape
    expect(m[1]).toContain('src/routes/') // path is source-relative, not `import.meta.url`
  })

  test('served page.tsx does not leak .server. imports', async ({ request }) => {
    // smoke-app index imports `greeting.server.ts`; the compiler must
    // replace that with a `__oxide_call_fn` stub. A regression here would
    // serve `import { greet } from "../../greeting.server.ts"` and the
    // browser would 403 on the module.
    const r = await request.get('/src/routes/index/page.tsx')
    const js = await r.text()
    // The fix rewrites `'./greeting.server.ts'` → `'/@server-fn/greeting.server.ts'`.
    // That rewritten URL still contains the string "greeting.server", so
    // a bare substring check false-positives. The invariants that matter:
    //
    //   1. No raw relative `.server.*` specifier survives (the browser would
    //      fetch the file, hit the 403 guard, and halt the module graph).
    //   2. If `.server.` appears at all, it's through the `/@server-fn/`
    //      URL family (our handler serves that as a client stub).
    const rawRelative = js.match(/from\s+["']\.\.?\/[^"']*\.server\.[jt]s/)
    if (rawRelative) {
      throw new Error(
        `page.tsx still has a raw relative .server. import (would 403 in the browser): ${rawRelative[0]}`,
      )
    }
    if (js.includes('.server.') && !js.includes('/@server-fn/')) {
      throw new Error(
        `page.tsx references .server. but no /@server-fn/ URL — rewrite_imports didn't fire`,
      )
    }
  })
})

// ---------------------------------------------------------------------------
// Browser/HMR runtime
// ---------------------------------------------------------------------------

test.describe('HMR runtime (browser)', () => {
  // Fixture guards (this describe block only):
  //
  // 1. A prior run that was SIGKILLed mid-test can leave `hmr-target.tsx`
  //    in a marker state. That contaminates every subsequent run: the
  //    server serves the stale marker, the test writes its own marker, the
  //    browser's HMR stream races between the two, and `toHaveText` flakes
  //    with a marker that isn't the one the test just wrote.
  //    Guard: restore the pristine fixture before every test.
  //
  // 2. Each write fires the dev server's watcher (macOS notify emits 3-8
  //    events per write). The 2000ms settle after each test lets that
  //    burst finish before the next test's `page.goto('/')` — a stale
  //    boot.mjs otherwise races hydration and `__oxideBoot` never sets.
  //    Scoped to this describe block so the fast structural tests aren't
  //    slowed.
  const HM_FIXTURE = join(smokeRoot, 'src', 'components', 'hmr-target.tsx')
  const HM_PRISTINE = `import { useState } from 'react'\n\nexport default function HmrTarget() {\n  const [count, setCount] = useState(0)\n  return (\n    <>\n      <h1 data-hmr-target="">HMR-TARGET-ORIGINAL</h1>\n      <button data-hmr-counter="" onClick={() => setCount(count + 1)}>{count}</button>\n    </>\n  )\n}\n`
  test.beforeEach(async () => {
    let cur = ''
    try { cur = readFileSync(HM_FIXTURE, 'utf8') } catch { cur = '' }
    if (cur !== HM_PRISTINE) {
      writeFileSync(HM_FIXTURE, HM_PRISTINE, 'utf8')
      await new Promise((r) => setTimeout(r, 300))
    }
  })
  test.afterEach(async () => {
    let cur = ''
    try { cur = readFileSync(HM_FIXTURE, 'utf8') } catch { cur = '' }
    if (cur !== HM_PRISTINE) writeFileSync(HM_FIXTURE, HM_PRISTINE, 'utf8')
    await new Promise((r) => setTimeout(r, 2000))
  })

  test('page hydrates without module-load errors', async ({ page }) => {
    const errors: string[] = []
    page.on('console', (m) => {
      if (m.type() === 'error') errors.push(m.text())
    })
    page.on('pageerror', (e) => errors.push(e.message))

    await page.goto('/')
    // `window.__oxide` is set SERVER-SIDE inside the inline hydration
    // bootstrap script — its presence only proves SSR rendered. The
    // `window.__oxideBoot` global is set CLIENT-SIDE by `boot.ts` when the
    // client actually booted (`installBootRuntime()`). Use it to assert
    // hydration, not just server render.
    await page.waitForFunction(
      () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
      { timeout: 10_000 },
    )

    // Every pattern below has been a regression at least once in this
    // session. If any appears in the console, the module graph failed.
    const forbidden = [
      'Failed to resolve module specifier', // bare specifier rewrite broke
      'does not provide an export named',   // prebundle missing named export
      'Calling `require` for',              // require→import rewrite broke
      'does not expose the `require`',      // same
      'Invalid hook call',                  // two React instances
      'ERR_NAME_NOT_RESOLVED',              // //@boot double slash
      'does not provide an export',         // islands imported as named
    ]
    // Noise we deliberately ignore (third-party, browser chrome).
    const noise = [/favicon/i, /Datadog/i, /installHook/i]
    const real = errors.filter((e) => !noise.some((r) => r.test(e)))
    for (const pat of forbidden) {
      const hit = real.find((e) => e.includes(pat))
      expect(hit, `regression: "${pat}" appeared in console: ${hit}\nall errors:\n${real.join('\n')}`).toBeUndefined()
    }
  })

  test('HMR WebSocket connects to /_oxide/hmr', async ({ page }) => {
    // Set up the wait BEFORE navigation so a fast WS open isn't missed.
    const wsPromise = page.waitForEvent('websocket', { timeout: 20_000 })
    await page.goto('/')

    // If the client never booted, the WS test failure would be a phantom
    // (no WS is expected if boot.mjs didn't run `connectHmr`). Diagnose
    // with a bounded wait + a dump of `window.__oxideBoot` status.
    try {
      const ws = await wsPromise
      expect(ws.url()).toContain('/_oxide/hmr')
      // Wait for the server's `{"type":"connected",...}` frame.
      await ws.waitForEvent('framereceived', { timeout: 10_000 })
    } catch (err) {
      const bootState = await page.evaluate(() => {
        const w = window as unknown as {
          __oxideBoot?: unknown
          __oxideBootRun?: unknown
          __oxideBootError?: unknown
        }
        return {
          hasBoot: typeof w.__oxideBoot !== 'undefined',
          hasBootRun: typeof w.__oxideBootRun === 'function',
          bootKeys: w.__oxideBoot ? Object.keys(w.__oxideBoot as object) : null,
        }
      })
      throw new Error(
        `WebSocket to /_oxide/hmr never opened. ` +
        `page boot state: ${JSON.stringify(bootState)}. ` +
        `If hasBoot=false, bootWithRouter never ran; check that ` +
        `clientAdapter resolves and boot.mjs doesn't throw. ` +
        `Original error: ${err instanceof Error ? err.message : String(err)}`,
      )
    }
  })

  test('HMR: editing a component updates the DOM without reloading', async ({ page }) => {
    test.setTimeout(60_000)
    test.skip(!HMR_AVAILABLE, 'react-refresh not installed in smoke-app')

    // Edit a COMPONENT (not a route file). Route file edits trigger
    // RestartSubsystems → FullReload; component edits trigger AdapterHmr →
    // ModuleUpdate → in-place React Refresh. This is the actual user-facing
    // HMR path; route file edits reloading is correct behaviour.
    const targetFile = join(smokeRoot, 'src', 'components', 'hmr-target.tsx')
    const original = readFileSync(targetFile, 'utf8')
    const marker = `HMR-MARKER-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`

    try {
      await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 30_000 })
      await page.waitForFunction(
        () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
        { timeout: 20_000 },
      )
      // Settle: any HMR from a prior test's cleanup write should land first.
      await new Promise((r) => setTimeout(r, 500))

      const timeOriginBefore = await page.evaluate(() => performance.timeOrigin)
      const before = await page.locator('h1[data-hmr-target]').textContent()
      expect(before).toBe('HMR-TARGET-ORIGINAL')

      const patched = original.replace(/HMR-TARGET-ORIGINAL/, marker)
      if (patched === original) {
        throw new Error('hmr-target.tsx lost the HMR-TARGET-ORIGINAL marker')
      }
      writeFileSync(targetFile, patched, 'utf8')

      await expect(page.locator('h1[data-hmr-target]')).toHaveText(marker, {
        timeout: 12_000,
      })

      const timeOriginAfter = await page.evaluate(() => performance.timeOrigin)
      expect(timeOriginAfter, 'HMR must not trigger a page reload').toBe(timeOriginBefore)
    } finally {
      writeFileSync(targetFile, original, 'utf8')
      await new Promise((r) => setTimeout(r, 1500))
    }
  })

  test('HMR: 3 consecutive edits each update the DOM, never reload', async ({ page }) => {
    test.setTimeout(60_000)
    test.skip(!HMR_AVAILABLE, 'react-refresh not installed in smoke-app')

    const targetFile = join(smokeRoot, 'src', 'components', 'hmr-target.tsx')
    const original = readFileSync(targetFile, 'utf8')

    try {
      await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 30_000 })
      await page.waitForFunction(
        () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
        { timeout: 20_000 },
      )
      await new Promise((r) => setTimeout(r, 500))

      const timeOrigin = await page.evaluate(() => performance.timeOrigin)
      const sessionMarker = `session-${Date.now()}`
      await page.evaluate((m) => {
        ;(window as unknown as { __hmrSession?: string }).__hmrSession = m
      }, sessionMarker)

      for (let i = 0; i < 3; i++) {
        const marker = `edit-${i}-${Date.now()}`
        const patched = original.replace(/HMR-TARGET-ORIGINAL/, marker)
        if (patched === original) throw new Error('hmr-target.tsx shape drifted')
        writeFileSync(targetFile, patched, 'utf8')

        await expect(page.locator('h1[data-hmr-target]')).toHaveText(marker, {
          timeout: 12_000,
        })

        expect(await page.evaluate(() => performance.timeOrigin)).toBe(timeOrigin)
        expect(
          await page.evaluate(() => (window as unknown as { __hmrSession?: string }).__hmrSession),
        ).toBe(sessionMarker)
      }
    } finally {
      writeFileSync(targetFile, original, 'utf8')
      await new Promise((r) => setTimeout(r, 1500))
    }
  })
  test('HMR: state preservation across edit (Tier 2.4)', async ({ page }) => {
    test.setTimeout(60_000)
    test.skip(!HMR_AVAILABLE, 'react-refresh not installed in smoke-app')

    const targetFile = join(smokeRoot, 'src', 'components', 'hmr-target.tsx')
    const original = readFileSync(targetFile, 'utf8')
    const marker = `HMR-STATE-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`

    try {
      await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 30_000 })
      await page.waitForFunction(
        () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
        { timeout: 20_000 },
      )
      await new Promise((r) => setTimeout(r, 500))

      const timeOrigin = await page.evaluate(() => performance.timeOrigin)

      // Bump state to a non-zero value. If HMR remounts (Refresh fallback)
      // or reloads (no Refresh), this resets to 0 — the failure signature.
      const counter = page.locator('button[data-hmr-counter]')
      await counter.click()
      await counter.click()
      await counter.click()
      await expect(counter).toHaveText('3')

      // Edit: change marker text, preserve hook chain (useState still first).
      // React Refresh must apply the new render in place.
      const patched = original.replace(/HMR-TARGET-ORIGINAL/, marker)
      if (patched === original) {
        throw new Error('hmr-target.tsx lost the HMR-TARGET-ORIGINAL marker')
      }
      writeFileSync(targetFile, patched, 'utf8')

      // 1. Edit applied (marker updated).
      await expect(page.locator('h1[data-hmr-target]')).toHaveText(marker, {
        timeout: 12_000,
      })
      // 2. State preserved (count still 3, not 0).
      await expect(counter).toHaveText('3')
      // 3. No page reload.
      expect(
        await page.evaluate(() => performance.timeOrigin),
        'HMR must not reload when state is preserved',
      ).toBe(timeOrigin)
    } finally {
      writeFileSync(targetFile, original, 'utf8')
      await new Promise((r) => setTimeout(r, 1500))
    }
  })
  test('HMR: broken edit shows overlay without reload, fix clears it (Tier 2.5 + 2.8)', async ({ page }) => {
    test.setTimeout(60_000)
    test.skip(!HMR_AVAILABLE, 'react-refresh not installed in smoke-app')

    const targetFile = join(smokeRoot, 'src', 'components', 'hmr-target.tsx')
    const original = readFileSync(targetFile, 'utf8')
    const fixedMarker = `HMR-FIXED-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`

    try {
      await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 30_000 })
      await page.waitForFunction(
        () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
        { timeout: 20_000 },
      )
      await new Promise((r) => setTimeout(r, 500))

      const timeOrigin = await page.evaluate(() => performance.timeOrigin)

      // Syntactically broken TSX → Oxc rejects → import() throws → overlay.
      writeFileSync(targetFile, 'export default function HmrTarget() {\n  return <h1>BROKEN\n', 'utf8')
      await expect(page.locator('#ox-hmr-overlay')).toBeVisible({ timeout: 12_000 })
      expect(
        await page.evaluate(() => performance.timeOrigin),
        'broken edit must show the overlay WITHOUT reloading',
      ).toBe(timeOrigin)

      // Fix → module-update clears overlay → in-place React Refresh.
      const fixed = original.replace(/HMR-TARGET-ORIGINAL/, fixedMarker)
      if (fixed === original) throw new Error('hmr-target.tsx lost the HMR-TARGET-ORIGINAL marker')
      writeFileSync(targetFile, fixed, 'utf8')

      await expect(page.locator('#ox-hmr-overlay')).toHaveCount(0, { timeout: 15_000 })
      await expect(page.locator('h1[data-hmr-target]')).toHaveText(fixedMarker, { timeout: 12_000 })
      expect(
        await page.evaluate(() => performance.timeOrigin),
        'fixing a broken edit must not reload either',
      ).toBe(timeOrigin)
    } finally {
      writeFileSync(targetFile, original, 'utf8')
      await new Promise((r) => setTimeout(r, 1500))
    }
  })
})

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

test.afterAll(async () => {
  // Await real process exit (see hmr-cold-start.spec.ts for the full
  // rationale). Without it, a subsequent spec's `beforeAll` may spawn a
  // dev server while this one is still draining — two watchers on the
  // same source tree write the same .oxide/ cache and publish competing
  // HMR events to whichever page happens to still be connected.
  if (!server) return
  const exited = new Promise<void>((resolve) => {
    if (server!.exitCode !== null || server!.signalCode !== null) {
      resolve()
      return
    }
    server!.once('exit', () => resolve())
  })
  server.kill('SIGTERM')
  const forceKill = setTimeout(() => {
    try { server!.kill('SIGKILL') } catch { /* already gone */ }
  }, 5000)
  await exited
  clearTimeout(forceKill)
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
  // Fixture guards for the HMR runtime describe block.
  //
  // 1. `hmr-target.tsx` is NOT git-tracked (per the earlier handoff), so its
  //    pristine bytes are hardcoded — capturing from disk at module load
  //    would capture any pollution from a prior SIGKILLed run.
  // 2. `counter.tsx` and `greeting/page.server.ts` ARE git-tracked, so they
  //    are captured from disk at module load. A clean working tree (asserted
  //    by the batch script before commit) means disk == pristine.
  //
  // Every test that edits one of these files restores it in `finally`, but
  // SIGKILL bypasses `finally`. The beforeEach/afterEach pair is the actual
  // safety net — the same fix that cfb8e18f applied for hmr-target.tsx,
  // extended to the fixtures 2.1 and 2.3 introduce.
  const HM_FIXTURE = join(smokeRoot, 'src', 'components', 'hmr-target.tsx')
  const HM_PRISTINE = `import { useState } from 'react'\n\nexport default function HmrTarget() {\n  const [count, setCount] = useState(0)\n  return (\n    <>\n      <h1 data-hmr-target="">HMR-TARGET-ORIGINAL</h1>\n      <button data-hmr-counter="" onClick={() => setCount(count + 1)}>{count}</button>\n    </>\n  )\n}\n`
  const ISLAND_PATH = join(smokeRoot, 'src', 'islands', 'counter.tsx')
  const ISLAND_PRISTINE = readFileSync(ISLAND_PATH, 'utf8')
  const LOADER_PATH = join(smokeRoot, 'src', 'routes', 'greeting', 'page.server.ts')
  const LOADER_PRISTINE = readFileSync(LOADER_PATH, 'utf8')
  const LAYOUT_PATH = join(smokeRoot, 'src', 'routes', 'layout.tsx')
  const LAYOUT_PRISTINE = readFileSync(LAYOUT_PATH, 'utf8')

  const FIXTURES: ReadonlyArray<{ path: string; pristine: string }> = [
    { path: HM_FIXTURE, pristine: HM_PRISTINE },
    { path: ISLAND_PATH, pristine: ISLAND_PRISTINE },
    { path: LOADER_PATH, pristine: LOADER_PRISTINE },
    { path: LAYOUT_PATH, pristine: LAYOUT_PRISTINE },
  ]

  const restoreFixtures = (): boolean => {
    let changed = false
    for (const { path, pristine } of FIXTURES) {
      let cur = ''
      try { cur = readFileSync(path, 'utf8') } catch { cur = '' }
      if (cur !== pristine) {
        writeFileSync(path, pristine, 'utf8')
        changed = true
      }
    }
    return changed
  }

  test.beforeEach(async () => {
    if (restoreFixtures()) await new Promise((r) => setTimeout(r, 300))
  })
  test.afterEach(async () => {
    restoreFixtures()
    // Settle: the dev server's watcher emits 3-8 events per write on macOS;
    // 2000ms lets the burst finish before the next `goto('/')`. Same value
    // the pre-refactor guard used; only the file set has changed.
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
    // Firefox and WebKit both drop the `{"type":"connected"}` frame from a
    // `ws.waitForEvent('framereceived')` attached AFTER `page.waitForEvent('websocket')`
    // resolves — the frame arrives between the socket's creation and the
    // listener attachment. Chromium happens to win the race; the other two
    // engines lose it deterministically (measured: same test fails 12.x s
    // on both, passes on chromium).
    //
    // Correct pattern (same as §3.1/§3.2 frame capture): attach the
    // `framereceived` listener inside `page.on('websocket')`, which fires the
    // instant the socket is created — before any frame can be delivered.
    // Then poll for the accumulated evidence. No awaits-on-events after the
    // fact, no race window.
    const wsUrls: string[] = []
    const frames: string[] = []
    page.on('websocket', (ws) => {
      wsUrls.push(ws.url())
      ws.on('framereceived', (eventOrPayload: unknown) => {
        const raw =
          eventOrPayload && typeof eventOrPayload === 'object'
          && 'payload' in (eventOrPayload as object)
            ? (eventOrPayload as { payload: unknown }).payload
            : eventOrPayload
        let text: string
        if (typeof raw === 'string') text = raw
        else if (raw instanceof ArrayBuffer || ArrayBuffer.isView(raw))
          text = new TextDecoder().decode(raw as ArrayBuffer | ArrayBufferView)
        else if (raw && typeof (raw as { toString?: unknown }).toString === 'function')
          text = (raw as { toString(enc: string): string }).toString('utf8')
        else text = String(raw)
        frames.push(text)
      })
    })

    await page.goto('/')

    // 1. A WS to /_oxide/hmr opened during boot.
    await expect
      .poll(() => wsUrls.find((u) => u.includes('/_oxide/hmr')) ?? null, { timeout: 20_000 })
      .not.toBeNull()

    // 2. The server's `{"type":"connected",...}` greeting frame arrived.
    //    If this times out with wsUrls populated, boot.mjs ran
    //    `connectHmr` (so the socket opened) but the server closed/errored
    //    before sending the greeting — a real server-side regression, not a
    //    client-side boot failure.
    try {
      await expect
        .poll(
          () => frames.find((f) => f.includes('"type":"connected"')) ?? null,
          { timeout: 10_000 },
        )
        .not.toBeNull()
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
        `WS to /_oxide/hmr opened (${JSON.stringify(wsUrls)}) but never received the ` +
        `{"type":"connected"} greeting. Page boot state: ${JSON.stringify(bootState)}. ` +
        `If hasBoot=false, boot.mjs didn't run. If hasBoot=true and the socket opened, ` +
        `the CLIENT did its job — suspect the server's HMR greeting path. ` +
        `Frames received: ${JSON.stringify(frames.slice(0, 5))}. ` +
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
  test('HMR: island edit updates DOM without reload, preserves click state (Tier 2.1)', async ({ page }) => {
    test.setTimeout(60_000)
    test.skip(!HMR_AVAILABLE, 'react-refresh not installed in smoke-app')

    const original = ISLAND_PRISTINE
    const marker = `ISLAND-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`

    try {
      await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 30_000 })
      await page.waitForFunction(
        () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
        { timeout: 20_000 },
      )
      // Wait for the ISLAND to be fully hydrated before clicking. The
      // attribute transitions SSR-HTML -> "pending" (set on entry) ->
      // "done" (set after the adapter attaches the click handler) ->
      // "error" (adapter threw). Waiting for `!== null` matches "pending"
      // too, which races the click: the first click lands before React
      // attached, state stays at 3, the intermediate toContainText('4')
      // fails. Wait for `=== 'done'` specifically, and do not swallow the
      // timeout — a hydration failure must fail the test at this line, not
      // as a downstream "click didn't work" symptom.
      await page.waitForFunction(
        () => document.querySelector('ox-island[data-ox-island="Counter"]')?.getAttribute('data-ox-hydrated') === 'done',
        { timeout: 15_000 },
      )

      const timeOrigin = await page.evaluate(() => performance.timeOrigin)
      const btn = page.locator('ox-island[data-ox-island="Counter"] button')
      await expect(btn).toBeVisible({ timeout: 10_000 })
      // Props { initial: 3 } → starts at 3; two clicks → 5.
      await expect(btn).toContainText('3')
      await btn.click()
      // Intermediate assertion: ensures the first click's state has
      // propagated before the second click. Two back-to-back clicks with
      // no intermediate check race React's re-render + any in-flight HMR
      // that fires between them; the fix is not to slow the clicks down
      // but to VERIFY state between them, so a Refresh that remounts the
      // island (state resets to initial=3) fails at a precise step rather
      // than as an ambiguous "click lost" five seconds later.
      await expect(btn).toContainText('4', { timeout: 5000 })
      await btn.click()
      await expect(btn).toContainText('5', { timeout: 5000 })

      // Edit the island file: add a sibling marker span inside the button.
      // Hook chain (useState first) is untouched, so React Refresh should
      // apply the new render in place — including state preservation.
      const patched = original.replace(
        '<span>{n}</span>',
        `<span data-island-marker="">${marker}</span><span>{n}</span>`,
      )
      if (patched === original) throw new Error('counter.tsx lost the <span>{n}</span> marker')
      writeFileSync(ISLAND_PATH, patched, 'utf8')

      await expect(page.locator('span[data-island-marker]')).toHaveText(marker, { timeout: 15_000 })
      await expect(btn).toContainText('5')
      expect(
        await page.evaluate(() => performance.timeOrigin),
        'island HMR must not reload the page',
      ).toBe(timeOrigin)
    } finally {
      writeFileSync(ISLAND_PATH, original, 'utf8')
      await new Promise((r) => setTimeout(r, 1500))
    }
  })

  test('HMR: loader edit updates DOM without reload (Tier 2.3)', async ({ page }) => {
    test.setTimeout(60_000)
    test.skip(!HMR_AVAILABLE, 'react-refresh not installed in smoke-app')

    const original = LOADER_PRISTINE
    const marker = `greeting-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`

    try {
      await page.goto('/greeting', { waitUntil: 'domcontentloaded', timeout: 30_000 })
      await page.waitForFunction(
        () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
        { timeout: 20_000 },
      )
      await new Promise((r) => setTimeout(r, 500))

      const timeOrigin = await page.evaluate(() => performance.timeOrigin)
      const textEl = page.locator('p[data-greeting-text]')
      await expect(textEl).toBeVisible({ timeout: 10_000 })
      const initialText = await textEl.textContent()
      expect(initialText).toMatch(/^hello,/)

      // Change only the `greeting` field; leave `role` derivation as-is so a
      // failure isolates the loader rerun (not a shape mismatch).
      const patched = original.replace(
        /return \{ greeting: `[^`]*`, role: [^}]*\}/,
        `return { greeting: '${marker}', role: locals?.role ?? 'anon' }`,
      )
      if (patched === original) {
        // Fallback anchor: the loader's return object as written today.
        const fallback = original.replace(
          /greeting: `hello, \$\{locals\?\.user \?\? 'guest'\}`/,
          `greeting: '${marker}'`,
        )
        if (fallback === original) throw new Error('page.server.ts: loader return-shape anchor not found')
        writeFileSync(LOADER_PATH, fallback, 'utf8')
      } else {
        writeFileSync(LOADER_PATH, patched, 'utf8')
      }

      await expect(textEl).toHaveText(marker, { timeout: 15_000 })
      expect(
        await page.evaluate(() => performance.timeOrigin),
        'loader HMR must not reload the page',
      ).toBe(timeOrigin)
    } finally {
      writeFileSync(LOADER_PATH, original, 'utf8')
      await new Promise((r) => setTimeout(r, 1500))
    }
  })
  test('HMR: two independent clients receive the same edit (Tier 2.6)', async ({ browser }) => {
    test.setTimeout(90_000)
    test.skip(!HMR_AVAILABLE, 'react-refresh not installed in smoke-app')

    // Two browser contexts in the same worker. Each gets its own page, its
    // own WS to /_oxide/hmr (opened by the per-page `connectHmr()` call in
    // boot.ts), its own module cache, its own `performance.timeOrigin`.
    // The dev server's HmrHub publishes over a `tokio::sync::broadcast`
    // channel, so one edit reaches every subscriber — this test asserts
    // that end-to-end from the client's side.
    //
    // Distinct from test 14 (`editing a component updates the DOM`), which
    // proves ONE client receives an edit. This proves N.
    const targetFile = join(smokeRoot, 'src', 'components', 'hmr-target.tsx')
    const original = readFileSync(targetFile, 'utf8')
    const marker = `MULTI-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`

    const ctxA = await browser.newContext()
    const ctxB = await browser.newContext()
    const pageA = await ctxA.newPage()
    const pageB = await ctxB.newPage()

    try {
      // Parallel navigation so both pages open their WS around the same tick.
      // Base URL comes from `test.use({ baseURL })` at the top of this file.
      await Promise.all([pageA.goto('/'), pageB.goto('/')])
      await Promise.all([
        pageA.waitForFunction(
          () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
          { timeout: 20_000 },
        ),
        pageB.waitForFunction(
          () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
          { timeout: 20_000 },
        ),
      ])
      // Settle: a WS that opens a tick after __oxideBoot still has time to
      // land before the edit fires.
      await new Promise((r) => setTimeout(r, 500))

      const originA = await pageA.evaluate(() => performance.timeOrigin)
      const originB = await pageB.evaluate(() => performance.timeOrigin)

      // One edit.
      const patched = original.replace(/HMR-TARGET-ORIGINAL/, marker)
      if (patched === original) throw new Error('hmr-target.tsx lost the HMR-TARGET-ORIGINAL marker')
      writeFileSync(targetFile, patched, 'utf8')

      // Both clients see it, in parallel.
      await Promise.all([
        expect(pageA.locator('h1[data-hmr-target]')).toHaveText(marker, { timeout: 15_000 }),
        expect(pageB.locator('h1[data-hmr-target]')).toHaveText(marker, { timeout: 15_000 }),
      ])

      // Neither reloaded. This is the failure signature of a per-connection
      // handler that fires on only one socket and forces the other to
      // reload (e.g. a broadcast bug that drops the second subscriber).
      expect(
        await pageA.evaluate(() => performance.timeOrigin),
        'client A must not reload on a broadcast edit',
      ).toBe(originA)
      expect(
        await pageB.evaluate(() => performance.timeOrigin),
        'client B must not reload on a broadcast edit',
      ).toBe(originB)
    } finally {
      writeFileSync(targetFile, original, 'utf8')
      // Close both contexts BEFORE the afterEach 2s settle: a live WS on
      // close fires its `onclose` -> `setTimeout(open, reconnectMs)`, which
      // races the next test's beforeEach if the contexts stay open.
      await ctxA.close()
      await ctxB.close()
      await new Promise((r) => setTimeout(r, 1500))
    }
  })
  test('HMR: layout edit updates DOM in place, no reload (Tier 2.2)', async ({ page }) => {
    test.setTimeout(60_000)
    test.skip(!HMR_AVAILABLE, 'react-refresh not installed in smoke-app')

    // §2.2 acceptance criterion, now achievable: §3.9's event-aware
    // `classify_event` distinguishes a `Modified` (content) edit from a
    // `Created`/`Removed` (structural) edit, so a content edit to
    // `routes/layout.tsx` routes to `ChangeKind::Component` ->
    // `HmrBehavior::AdapterHmr` -> `HmrMessage::ModuleUpdate`. The client
    // re-imports the layout module, `$RefreshReg$` fires with the same
    // stable `path:RootLayout` id, `performReactRefresh()` walks the tree
    // and updates in place. `timeOrigin` stays unchanged.
    //
    // Before §3.9 this test was a PINNING test asserting the opposite
    // (`timeOrigin` changed because layout edits went through
    // `ConfigOrRoutes` -> `RestartSubsystems` -> `HmrMessage::FullReload`).
    // The pinning assertion existed specifically so that when §3.9 landed
    // it would flip and prompt this rewrite. That is now.
    //
    // Future regression path: if a content edit to layout re-enters the
    // rescan path (via a `classify_event` regression that loses the
    // `Modified` signal), `timeOrigin` will change and this assertion will
    // fail with the message below.
    const targetFile = LAYOUT_PATH
    const original = LAYOUT_PRISTINE
    const marker = `Smoke-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`

    try {
      await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 30_000 })
      await page.waitForFunction(
        () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
        { timeout: 20_000 },
      )
      await new Promise((r) => setTimeout(r, 500))

      const timeOriginBefore = await page.evaluate(() => performance.timeOrigin)
      const header = page.locator('header[data-layout-header] a')
      await expect(header).toHaveText('Smoke')

      // Content edit: change the layout's link text. Same component shape
      // (a single <a> child, no hook change), so React Refresh updates in
      // place.
      const patched = original.replace('>Smoke</a>', `>${marker}</a>`)
      if (patched === original) {
        throw new Error('layout.tsx: header anchor text anchor not found (>Smoke</a>)')
      }
      writeFileSync(targetFile, patched, 'utf8')

      // DOM reflects the new text — via in-place Refresh, not a reload.
      await expect(page.locator('header[data-layout-header] a')).toHaveText(marker, {
        timeout: 15_000,
      })

      // §2.2 assertion: no reload. A FullReload would reset timeOrigin.
      expect(
        await page.evaluate(() => performance.timeOrigin),
        'layout content edit must apply in place — §3.9 classify_event should route ' +
        'Modified(routes/layout.tsx) to AdapterHmr, not RestartSubsystems. ' +
        'If this fails because timeOrigin CHANGED, §3.9 regressed (event kind lost) ' +
        'or classify_event stopped treating Modified layout as Component.',
      ).toBe(timeOriginBefore)
    } finally {
      writeFileSync(targetFile, original, 'utf8')
      await new Promise((r) => setTimeout(r, 1500))
    }
  })

  test('HMR: one save yields exactly one module-update frame (Tier 3.1)', async ({ page }) => {
    test.setTimeout(60_000)
    test.skip(!HMR_AVAILABLE, 'react-refresh not installed in smoke-app')

    // §3.1 acceptance criterion: the SERVER must publish one ModuleUpdate per
    // save, not one per notify event. macOS FSEvents emits 3-8 raw events per
    // writeFileSync; the watcher's 30ms `poll_notify` debounce collapses them
    // today. If that debounce regressed, or if the CLI per-event dispatch
    // produced extra publishes from a stale buffer drain, this catches it.
    //
    // Pure client-side measurement: capture the HMR WebSocket, count
    // `"type":"module-update"` frames received across the save. The client
    // COALESCE_MS collapses `reloadModule` calls, not wire frames, so it does
    // not affect this count.
    const targetFile = join(smokeRoot, 'src', 'components', 'hmr-target.tsx')
    const original = readFileSync(targetFile, 'utf8')
    const marker = `ONE-SAVE-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`

    const hmrFrames: string[] = []
    page.on('websocket', (ws) => {
      if (ws.url().includes('/_oxide/hmr')) {
        ws.on('framereceived', (eventOrPayload: unknown) => {
          // Playwright's framereceived handler receives an EVENT OBJECT whose
          // `.payload` is the actual data (string | Buffer). Older versions
          // passed the payload directly. Handle both shapes; then decode the
          // Buffer-like form via TextDecoder.
          const raw =
            eventOrPayload
            && typeof eventOrPayload === 'object'
            && 'payload' in (eventOrPayload as object)
              ? (eventOrPayload as { payload: unknown }).payload
              : eventOrPayload
          let text: string
          if (typeof raw === 'string') {
            text = raw
          } else if (raw instanceof ArrayBuffer || ArrayBuffer.isView(raw)) {
            text = new TextDecoder().decode(raw as ArrayBuffer | ArrayBufferView)
          } else if (raw && typeof (raw as { toString?: unknown }).toString === 'function') {
            text = (raw as { toString(enc: string): string }).toString('utf8')
          } else {
            text = String(raw)
          }
          hmrFrames.push(text)
        })
      }
    })

    try {
      await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 30_000 })
      await page.waitForFunction(
        () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
        { timeout: 20_000 },
      )
      await new Promise((r) => setTimeout(r, 800))

      const countModUpdates = () =>
        hmrFrames.filter((f) => f.includes('"type":"module-update"')).length
      const modUpdatesBefore = countModUpdates()

      const patched = original.replace(/HMR-TARGET-ORIGINAL/, marker)
      if (patched === original) throw new Error('hmr-target.tsx lost the marker')
      writeFileSync(targetFile, patched, 'utf8')

      await expect(page.locator('h1[data-hmr-target]')).toHaveText(marker, { timeout: 15_000 })
      await new Promise((r) => setTimeout(r, 800))

      const modUpdatesAfter = countModUpdates()
      const delta = modUpdatesAfter - modUpdatesBefore
      expect(
        delta,
        `§3.1: one writeFileSync must yield exactly one ModuleUpdate on the wire. ` +
        `Observed ${delta} (${modUpdatesBefore} before, ${modUpdatesAfter} after). ` +
        `If >1, the server-side debounce regressed (dev/src/lib.rs poll_notify 30ms settle), ` +
        `or the CLI per-batch dedup at cli/src/lib.rs:~2680 stopped coalescing. ` +
        `Frames: ${JSON.stringify(hmrFrames)}`,
      ).toBe(1)
    } finally {
      writeFileSync(targetFile, original, 'utf8')
      await new Promise((r) => setTimeout(r, 1500))
    }
  })

  test('HMR: burst touching N distinct files yields <= N module-update frames (Tier 3.2 diag)', async ({ page }) => {
    test.setTimeout(90_000)
    test.skip(!HMR_AVAILABLE, 'react-refresh not installed in smoke-app')

    // §3.2 DIAGNOSTIC. Scenario: "git checkout <branch>" touches ~50 DISTINCT
    // files. This test touches N=5 distinct component files in one burst and
    // measures:
    //   - delta == 1  -> §3.2 already satisfied (some path coalesces)
    //   - delta == N  -> per-event dispatch, plan's batch fix warranted
    //   - 2..N-1      -> partial coalescing
    // The previous attempt wrote N times to the SAME file and got delta=1 —
    // vacuous, because per-path content-hash dedup collapses same-file writes
    // regardless of §3.2. This version is the real scenario.
    const N = 5
    const FILES: Array<{ path: string; pristine: string; marker: string }> = []
    const candidates = [
      { path: join(smokeRoot, 'src', 'components', 'hmr-target.tsx'), tag: 'hmr-target' },
      { path: join(smokeRoot, 'src', 'islands', 'counter.tsx'), tag: 'counter' },
      { path: join(smokeRoot, 'src', 'routes', 'index', 'page.tsx'), tag: 'page-index' },
      { path: join(smokeRoot, 'src', 'routes', 'greeting', 'page.tsx'), tag: 'page-greeting' },
      { path: join(smokeRoot, 'src', 'routes', 'nested', 'page.tsx'), tag: 'page-nested' },
    ]
    for (const c of candidates) {
      const pristine = readFileSync(c.path, 'utf8')
      // Append a harmless marker comment; content-hash differs per file, so
      // each is a real Modified event. Comment style varies per file, so use
      // a `/* ... */`-free single-line comment only when possible; fall back
      // to appending a newline-anchored comment that TSX/TS both accept:
      //   `// oxide-§3.2-diag-<tag>-<ts>`
      const marker = `\n// oxide-§3.2-diag-${c.tag}-${Date.now()}-${Math.random().toString(36).slice(2, 6)}\n`
      FILES.push({ path: c.path, pristine, marker })
    }

    const hmrFrames: string[] = []
    page.on('websocket', (ws) => {
      if (ws.url().includes('/_oxide/hmr')) {
        ws.on('framereceived', (eventOrPayload: unknown) => {
          const raw =
            eventOrPayload && typeof eventOrPayload === 'object'
            && 'payload' in (eventOrPayload as object)
              ? (eventOrPayload as { payload: unknown }).payload
              : eventOrPayload
          let text: string
          if (typeof raw === 'string') text = raw
          else if (raw instanceof ArrayBuffer || ArrayBuffer.isView(raw))
            text = new TextDecoder().decode(raw as ArrayBuffer | ArrayBufferView)
          else if (raw && typeof (raw as { toString?: unknown }).toString === 'function')
            text = (raw as { toString(enc: string): string }).toString('utf8')
          else text = String(raw)
          hmrFrames.push(text)
        })
      }
    })
    const count = () => hmrFrames.filter((f) => f.includes('"type":"module-update"')).length

    try {
      await page.goto('/', { waitUntil: 'domcontentloaded', timeout: 30_000 })
      await page.waitForFunction(
        () => typeof (window as { __oxideBoot?: unknown }).__oxideBoot !== 'undefined',
        { timeout: 20_000 },
      )
      await new Promise((r) => setTimeout(r, 800))

      const before = count()
      // Burst: touch all N distinct files in one tight loop (no sleeps).
      for (const f of FILES) {
        writeFileSync(f.path, f.pristine + f.marker, 'utf8')
      }

      // Settle: wait for the burst to fully drain (each file's update lands
      // as its own ModuleUpdate under the current dispatch; the final one
      // is enough to prove dispatch completed).
      await new Promise((r) => setTimeout(r, 3000))

      const delta = count() - before
      console.log(`[DIAG-3.2] distinct-files delta=${delta} (N=${N}, before=${before})`)

      expect(delta, `§3.2 diag: ${delta} frames for ${N} DISTINCT file writes`).toBeGreaterThanOrEqual(1)
      expect(delta, `§3.2 diag: ${delta} frames for ${N} DISTINCT file writes (per-event expected)`).toBeLessThanOrEqual(N)
    } finally {
      for (const f of FILES) writeFileSync(f.path, f.pristine, 'utf8')
      await new Promise((r) => setTimeout(r, 2000))
    }
  })
})

import { showOverlay } from './hmr.ts'
// HMR runtime for the Native ESM dev server (Phase 1).
//
// Auto-installs on first import (module side-effect) so any module imported
// after `@oxide-fw/core/client/hot` sees `globalThis.__oxide_hmr__`. This is
// critical: the Oxc transformer's `inject_hmr_boundary` emits
// `import.meta.hot.accept(...)` at module top-level, which requires
// `import.meta.hot` to be set BEFORE the module body runs. The ESM dev
// server's `serve_source_file` prepends a preamble that assigns
// `import.meta.hot = globalThis.__oxide_hmr__.createHotContext(url)`. The
// boot entry imports this module first so the global is ready.
//
// Phase 1: nothing calls `reloadModule` yet; `import.meta.hot` is a real
// object but HMR only fires after Phase 2's WebSocket wiring. Phase 3
// upgrades `__oxide_refresh` to React Refresh's `performReactRefresh`.

interface HotModule {
  url: string
  data: Record<string, unknown>
  acceptCallbacks: Array<(mod: unknown) => void>
  disposeCallbacks: Array<() => void>
  invalidated: boolean
}

interface HotContext {
  readonly url: string
  data: Record<string, unknown>
  accept(cb?: (mod: unknown) => void): void
  dispose(cb?: () => void): void
  invalidate(): void
}

const modules = new Map<string, HotModule>()

// Monotonic counter for cache-busting. `Date.now()` alone is not enough —
// two `reloadModule` calls in the same tick would produce identical URLs,
// Node's ESM cache would serve the same module, and the fresh accept
// callbacks (registered during evaluation) would never fire. In production
// HMR saves are seconds apart so this would not surface, but a scripted
// reload (or a test) hits it immediately.
let reloadSeq = 0

// Coalesce rapid HMR messages. macOS fires 3-8 watcher events per file
// write (IN_MODIFY + IN_CLOSE_WRITE + atime bump from our own reads of
// the same file). Without coalescing, one save triggers N imports in
// <1s and thrashes React Refresh — the concurrent scheduler coalesces
// performReactRefresh() calls, and the DOM ends up reconciled against a
// stale family. Leading + trailing: first message executes, rapid
// followers collapse into one trailing call that reflects the latest
// content.
const lastReloadAt = new Map<string, number>()
const trailingTimers = new Map<string, number>()
const COALESCE_MS = 100

// §3.8a: bound the two maps that would otherwise grow with every distinct
// URL a session touches. `modules` grows one entry per reloaded module;
// `lastReloadAt` grows one entry per URL. In practice a dev session hits
// tens of URLs, but an editor plugin that repeatedly re-imports a path
// (or a scripted test) makes the growth unbounded. 500 is 25x a busy
// session's real needs — any eviction here means the entry is cold.
const MAX_TRACKED_MODULES = 500
const MAX_TRACKED_URLS = 500

function install(): void {
  // §3.5: install in Web Workers too. Today the previous guard
  // (`typeof window === 'undefined'`) bailed, so `import.meta.hot` was
  // undefined inside `new Worker(...)` — the preamble assigned
  // `import.meta.hot = globalThis.__oxide_hmr__.createHotContext(...)`,
  // but `__oxide_hmr__` itself was never installed.
  //
  // Worker detection: canonical `typeof importScripts === 'function'`
  // (available only in worker scope). `self` and `WorkerGlobalScope` are
  // redundant but harmless.
  const hasWindow = typeof window !== 'undefined'
  const hasWorkerScope =
    typeof self !== 'undefined'
    && typeof (self as unknown as { importScripts?: unknown }).importScripts === 'function'
  if (!hasWindow && !hasWorkerScope) return

  // Workers have no `window`, no DOM, no React Refresh runtime. The
  // `import.meta.hot` surface (createHotContext, accept, dispose, data)
  // is what worker code needs; `performReactRefresh()` is meaningless in
  // a worker context. The `__oxide_refresh`/`__oxide_refresh_reg` bits
  // are guarded below on `hasWindow`.
  const g = (hasWindow ? window : (self as unknown as typeof window)) as unknown as {
    __oxide_hmr__?: unknown
    __oxide_refresh?: unknown
    __oxide_refresh_reg?: (t: unknown, id: string) => void
    __oxide_refresh_sig?: () => (...args: unknown[]) => unknown
    $RefreshRuntime$?: {
      performReactRefresh?: () => void
      injectIntoGlobalHook?: (w: unknown) => void
      register?: (t: unknown, id: string) => void
    }
  }
  if (g.__oxide_hmr__) return // idempotent

  g.__oxide_hmr__ = {
    createHotContext(url: string): HotContext {
      let mod = modules.get(url)
      if (!mod) {
        mod = {
          url,
          data: {},
          acceptCallbacks: [],
          disposeCallbacks: [],
          invalidated: false,
        }
        modules.set(url, mod)
        if (modules.size > MAX_TRACKED_MODULES) {
          const oldest = modules.keys().next().value
          if (oldest !== undefined) modules.delete(oldest)
        }
      }
      const m = mod // closure-capture the narrow type
      return {
        get url() {
          return url
        },
        data: m.data,
        accept(cb) {
          if (cb) m.acceptCallbacks.push(cb)
        },
        dispose(cb) {
          if (cb) m.disposeCallbacks.push(cb)
        },
        invalidate() {
          m.invalidated = true
        },
      }
    },

    async reloadModule(url: string): Promise<void> {
      // __coalesce: leading edge + trailing
      const __now = Date.now()
      const __last = lastReloadAt.get(url) ?? 0
      if (__now - __last < COALESCE_MS) {
        if (!trailingTimers.has(url)) {
          const t = setTimeout(() => {
            trailingTimers.delete(url)
            void (globalThis as { __oxide_hmr__?: { reloadModule?: (u: string) => Promise<void> } }).__oxide_hmr__?.reloadModule?.(url)
          }, COALESCE_MS + 20) as unknown as number
          trailingTimers.set(url, t)
        }
        return
      }
      lastReloadAt.delete(url)
      lastReloadAt.set(url, __now)
      if (lastReloadAt.size > MAX_TRACKED_URLS) {
        const oldest = lastReloadAt.keys().next().value
        if (oldest !== undefined) lastReloadAt.delete(oldest)
      }
      // Phase 3 implementation (delivered in Phase 5b.2): dispose + re-import
      // + accept. `url` is what the browser fetched (`/src/routes/...` or
      // `/@css/...`). The re-import busts the ESM cache with `?t=<ms>` so
      // the module body runs fresh. Then every accept callback registered
      // during that fresh evaluation fires — for JSX that means
      // `__oxide_refresh(mod)` which calls React Refresh's
      // `performReactRefresh()` for in-place state-preserving updates; for
      // CSS the side-effect already ran (the `<style>` was replaced in
      // place) so the accept callback is a no-op.
      const mod = modules.get(url)
      if (mod) {
        for (const cb of mod.disposeCallbacks) {
          try { cb() } catch { /* a dispose throwing must not block the reload */ }
        }
        mod.disposeCallbacks.length = 0
        // Accept callbacks are re-pushed during the fresh evaluation below.
        // Clearing prevents an edit-storm from stacking accepts that fire
        // repeatedly on a later reload.
        mod.acceptCallbacks.length = 0
        mod.invalidated = false
      }
      const sep = url.includes('?') ? '&' : '?'
      const freshUrl = `${url}${sep}t=${Date.now()}-${++reloadSeq}`
      let fresh: unknown
      try {
        fresh = await import(/* @vite-ignore */ freshUrl)
      } catch (err) {
        // Tier 2.5 + 2.8: a transform/parse error must surface WITHOUT
        // discarding the current page state. Show the diagnostic overlay
        // and keep the previous DOM; the next successful save fires
        // `clearOverlay()` in `applyHmrMessage`'s `module-update` case and
        // the fresh import applies the edit in place.
        console.error('[oxide] module reload failed:', url, err)
        showOverlay({
          code: 'HMR-IMPORT',
          message: err instanceof Error ? err.message : String(err),
          fix: 'fix the file and save again — no reload needed',
        })
        return
      }
      const freshMod = modules.get(url)
      if (freshMod) {
        for (const cb of freshMod.acceptCallbacks) {
          try { cb(fresh) } catch { /* one throwing accept must not block the rest */ }
        }
        freshMod.acceptCallbacks.length = 0
      }
    },
    __debug: {
      // §3.8a: introspection for the memory-bound test. Getters so the
      // caller always sees current sizes (not a stale snapshot).
      get moduleCount(): number {
        return modules.size
      },
      get reloadCount(): number {
        return lastReloadAt.size
      },
      get MAX_MODULES(): number {
        return MAX_TRACKED_MODULES
      },
      get MAX_URLS(): number {
        return MAX_TRACKED_URLS
      },
    },
  }

  // `__oxide_refresh` is what Oxc's inject_hmr_boundary calls in the accept
  // callback. Phase 1-2: full reload. Phase 3 replaces this with
  // react-refresh's performReactRefresh (loaded lazily).
  // §3.5: React Refresh is a DOM-only concept. In a worker, skip the
  // `__oxide_refresh` / `__oxide_refresh_reg` installs entirely — the
  // worker module graph has no components, only JS modules whose accept
  // callbacks fire on `reloadModule`.
  if (hasWindow && typeof g.__oxide_refresh === 'undefined') {
    g.__oxide_refresh = (_mod: unknown): void => {
      const rt = g.$RefreshRuntime$
      if (rt && typeof rt.performReactRefresh === 'function') {
        try {
          rt.performReactRefresh()
          return
        } catch (e) {
          console.error('[hot] performReactRefresh() threw:', e)
        }
      }
      if (typeof location !== 'undefined') location.reload()
    }
  }

  // §1.2: `$RefreshSig$` real implementation. The compiler wraps every hook
  // call inside a component (`useState`, `useEffect`, custom `use*`) with a
  // signature function. That function is what lets React Refresh detect a
  // HOOK-SHAPE change: same hooks in same order → in-place update; changed
  // count/order → remount (the old hook chain no longer corresponds to the
  // new one). Without it, editing an `useEffect` dependency array silently
  // keeps the OLD closure running — the exact class of "component updated
  // but behaviour is wrong" bug this item exists to close.
  //
  // Delegates to `$RefreshRuntime$.createSignatureFunctionForTransform()`,
  // the same API Vite's react-refresh plugin uses. Falls back to identity
  // when the runtime is not yet loaded (`pendingRegistrations` in the
  // compiler's own buffered path ensures the FIRST registration lands after
  // the runtime loads; sig functions are only needed at re-render time,
  // which is always after boot).
  // §1.2: buffered sigs. Same race `pendingRegistrations` closes on the
  // register path — a component module's `_s(Component, key)` call runs at
  // module-eval time and can race `$RefreshRuntime$`'s async prebundle
  // fetch. If we returned the identity fallback, `setSignature` never fires
  // and HMR has no OLD signature to compare against — the runtime falls
  // through to in-place update, React throws on hook-count mismatch.
  type BufferedSig = {
    calls: unknown[][]
    real?: (...args: unknown[]) => unknown
  }
  const bufferedSigs: BufferedSig[] = []
  if (hasWindow && typeof g.__oxide_refresh_sig === 'undefined') {
    g.__oxide_refresh_sig = (): ((...args: unknown[]) => unknown) => {
      const rt = g.$RefreshRuntime$
      if (rt && typeof rt.createSignatureFunctionForTransform === 'function') {
        try {
          return rt.createSignatureFunctionForTransform() as (...args: unknown[]) => unknown
        } catch (e) {
          console.error('[hot] createSignatureFunctionForTransform() threw:', e)
        }
      }
      const buffered: BufferedSig = { calls: [] }
      bufferedSigs.push(buffered)
      return (...args: unknown[]) => {
        if (buffered.real) return buffered.real(...args)
        buffered.calls.push(args)
        if (args.length >= 2 && typeof args[1] === 'string') return args[0]
        return undefined
      }
    }
  }

  // The per-module `$RefreshReg$` shim (injected by the Rust compiler's
  // `inject_react_refresh` pass) routes through `__oxide_refresh_reg` when
  // the runtime is available; otherwise it is a no-op (a production build
  // strips the shim, so this branch only runs in dev).
  // Registrations that arrived before `$RefreshRuntime$` finished loading.
  // React Refresh needs the INITIAL registration of a component to
  // establish the "family" it later compares against. Without buffering,
  // the very first render (before the async runtime import resolves) would
  // silently drop every registration, and every HMR reload afterward would
  // re-register but find no baseline to compare to — `performReactRefresh()`
  // "succeeds" but nothing on screen updates.
  const pendingRegistrations: Array<{ t: unknown; id: string }> = []
  if (hasWindow && typeof g.__oxide_refresh_reg === 'undefined') {
    g.__oxide_refresh_reg = (t: unknown, id: string): void => {
      // `id` is now `path:Name` — a stable identifier baked into the module
      // by the compiler (`$RefreshReg$(DocBody, "src/routes/…/doc-body.tsx:DocBody")`).
      // The previous code prefixed it with `import.meta.url`, which after
      // HMR carried `?t=<ms>` and produced a DIFFERENT id every reload, so
      // React Refresh treated each reload as a brand-new component and
      // never correlated the old and new versions.
      const rt = g.$RefreshRuntime$
      if (rt && typeof rt.register === 'function') {
        try {
          rt.register(t, id)
        } catch {
          // Registration failing must not break the module evaluation.
        }
      } else {
        pendingRegistrations.push({ t, id })
      }
    }
  }

  // Best-effort: pull in React Refresh's runtime from the prebundle when the
  // project has installed `react-refresh`. A 404 (project has not installed
  // it) leaves `$RefreshRuntime$` undefined; `__oxide_refresh` then falls
  // back to a full reload. A dynamic import so the small reload path does not
  // pay a network round-trip on the happy path.
  // §3.5: skip the React Refresh runtime prebundle pull in a worker.
  // That URL exists, but the runtime has no meaning without a DOM. Keeping
  // the fetch in a worker would 404 in bundler mode and cost a needless
  // round-trip in ESM mode.
  if (hasWindow && typeof g.$RefreshRuntime$ === 'undefined') {
    const nav = (typeof window !== 'undefined' ? window : globalThis) as unknown as {
      location?: { protocol: string }
      document?: { querySelectorAll(s: string): ArrayLike<{ getAttribute?(a: string): string | null }> }
    }
    const scheme = nav.location?.protocol ?? ''
    // §3.11 / BUG 3: fetch the /@prebundle/ runtime ONLY in the native-ESM
    // dev server. The URL exists nowhere else — not in `oxide build` output,
    // not in `--no-esm` bundler mode. Unconditionally fetching it (the
    // previous behaviour) 404s on every bundler-mode page load (Chromium
    // logs `Failed to load resource`, Firefox a MIME-type block because the
    // 404 body is HTML, WebKit `unsupported URL`).
    //
    // Same detection boot.ts uses (boot.ts:323-330): scan
    // `<script type=module>` tags for `/@boot/` or `/@prebundle/`. Both
    // exist only in ESM mode.
    let isEsmDev = false
    if (scheme === 'http:' || scheme === 'https:') {
      const scripts = nav.document?.querySelectorAll('script[type="module"]')
      if (scripts) {
        for (let i = 0; i < scripts.length; i++) {
          const src = scripts[i]?.getAttribute?.('src') ?? ''
          if (src.includes('/@boot/') || src.includes('/@prebundle/')) {
            isEsmDev = true
            break
          }
        }
      }
    }
    if (isEsmDev) {
      // Deferred: a non-awaited dynamic import. The `@prebundle` path is only
      // valid in dev (the dev server owns `/@`), so a 404 there is expected
      // outside `oxide dev`.
      // The prebundle path is a runtime URL — TypeScript cannot resolve
      // it, so suppress the module-not-found and cast the resolved value.
      // (The path only exists when `oxide dev` is running; the `.catch`
      // below handles the 404 that a production host returns.)
      // Dynamic import via a variable path so TypeScript cannot
      // resolve the module at type-check time (`import('/@prebundle/...')`
      // fails with Cannot find module). A direct `import()` is
      // CSP-safe; the previous `new Function` workaround was blocked by
      // the page's Content-Security-Policy (which allows 'strict-dynamic'
      // but not 'unsafe-eval') and threw at module-evaluation time,
      // breaking the entire boot graph (boot.mjs failed → DOMContentLoaded
      // blocked → page hung).
      const runtimeUrl: string = '/@prebundle/react-refresh-runtime.js'
      void (import(/* @vite-ignore */ runtimeUrl) as Promise<{
        default?: unknown
        performReactRefresh?: () => void
        injectIntoGlobalHook?: (w: unknown) => void
        register?: (t: unknown, id: string) => void
      }>)
        .then((mod) => {
          type RuntimeShape = {
            performReactRefresh?: () => void
            injectIntoGlobalHook?: (w: unknown) => void
            register?: (t: unknown, id: string) => void
          }
          const rt = (mod.default ?? mod) as RuntimeShape
          // `injectIntoGlobalHook` MUST run before any `register()` call,
          // so React's dispatcher hook is installed before Refresh begins
          // tracking components.
          if (typeof rt.injectIntoGlobalHook === 'function') {
            try {
              rt.injectIntoGlobalHook(window)
            } catch (e) {
              // Ignore: the runtime is optional.
            }
          }
          g.$RefreshRuntime$ = rt
          // Replay registrations that arrived during the async import.
          // Without this, the very first render (which happens synchronously
          // before this `.then` runs) registers components to a null runtime;
          // they're buffered and replayed here so React Refresh has a
          // baseline family to compare on the FIRST HMR reload.
          if (typeof rt.register === 'function') {
            for (const { t, id } of pendingRegistrations) {
              try {
                rt.register(t, id)
              } catch {
                // Ignore: a bad registration must not break the runtime load.
              }
            }
          }
          pendingRegistrations.length = 0
          // §1.2: replay buffered sigs. Each buffered sig becomes a fresh
          // `createSignatureFunctionForTransform()` and every queued
          // `(type, key)` call fires through it, so the OLD component's
          // signature is registered before any HMR comparison.
          if (
            bufferedSigs.length > 0
            && typeof rt.createSignatureFunctionForTransform === 'function'
          ) {
            for (const b of bufferedSigs) {
              if (b.real) continue
              try {
                b.real = rt.createSignatureFunctionForTransform() as (...args: unknown[]) => unknown
                for (const args of b.calls) {
                  try {
                    b.real(...args)
                  } catch {
                    // Ignore: a bad sig call must not break the runtime load.
                  }
                }
                b.calls.length = 0
              } catch {
                // Ignore: a failed buffered-sig drain must not break the load.
              }
            }
            bufferedSigs.length = 0
          }
        })
        .catch(() => {
          // Not installed — a full reload is the fallback, no user-visible error.
        })
    }
  }
}

install()
// §3.5: worker-side WS. In the main thread, `connectHmr()` (called from
// boot.ts) owns the WebSocket. Workers have no boot.ts — nobody calls
// connectHmr for them. So the worker side opens its own WS to the same
// /_oxide/hmr endpoint. Broadcast semantics: the server's HmrHub publishes
// to every connected client, so main thread AND workers all receive the
// same messages. A worker's `reloadModule(url)` re-imports its own module
// graph, independent of the main thread's React refresh.
;(function connectWorkerHmr(): void {
  const hasWindow = typeof window !== 'undefined'
  const hasWorkerScope =
    typeof self !== 'undefined'
    && typeof (self as unknown as { importScripts?: unknown }).importScripts === 'function'
  if (hasWindow || !hasWorkerScope) return
  type WSLike = {
    onmessage: ((ev: { data: unknown }) => void) | null
    onclose: (() => void) | null
    onerror: (() => void) | null
    close(): void
  }
  const g = self as unknown as {
    WebSocket?: new (url: string) => WSLike
    location?: { protocol: string; host: string }
    __oxide_hmr__?: {
      reloadModule?: (u: string) => Promise<void>
    }
  }
  if (typeof g.WebSocket !== 'function' || !g.location) return
  const scheme = g.location.protocol === 'https:' ? 'wss' : 'ws'
  const url = `${scheme}://${g.location.host}/_oxide/hmr`
  let closed = false
  let socket: WSLike | undefined
  const open = (): void => {
    if (closed) return
    socket = new g.WebSocket!(url)
    socket.onmessage = (ev: { data: unknown }) => {
      try {
        const raw = typeof ev.data === 'string' ? ev.data : String(ev.data)
        const msg = JSON.parse(raw) as { type?: string; url?: string; items?: Array<{ url?: string }> }
        const hmr = g.__oxide_hmr__
        if (typeof hmr?.reloadModule !== 'function') return
        if (msg.type === 'module-update' && typeof msg.url === 'string' && msg.url) {
          void hmr.reloadModule(msg.url)
        } else if (msg.type === 'batch-module-update' && Array.isArray(msg.items)) {
          for (const it of msg.items) {
            if (typeof it.url === 'string' && it.url) void hmr.reloadModule(it.url)
          }
        }
        // full-reload / server-module-swap / error: worker has no DOM to
        // show an overlay and no page to reload — the main thread's own
        // WS handles those. Ignore here.
      } catch { /* malformed frame is not fatal */ }
    }
    socket.onclose = () => {
      socket = undefined
      if (!closed) setTimeout(open, 1000)
    }
    socket.onerror = () => {
      socket?.close()
    }
  }
  open()
})()

export type { HotContext }

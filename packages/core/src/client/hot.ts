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

function install(): void {
  if (typeof window === 'undefined') return
  const g = window as unknown as {
    __oxide_hmr__?: unknown
    __oxide_refresh?: unknown
    __oxide_refresh_reg?: (t: unknown, id: string) => void
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
      lastReloadAt.set(url, __now)
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
        // A transform/parse error after save should surface to the user —
        // fall through to a full reload so the diagnostic overlay (which the
        // dev server publishes) can render.
        console.error('[oxide] module reload failed:', url, err)
        if (typeof location !== 'undefined') location.reload()
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
  }

  // `__oxide_refresh` is what Oxc's inject_hmr_boundary calls in the accept
  // callback. Phase 1-2: full reload. Phase 3 replaces this with
  // react-refresh's performReactRefresh (loaded lazily).
  if (typeof g.__oxide_refresh === 'undefined') {
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
  if (typeof g.__oxide_refresh_reg === 'undefined') {
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
  if (typeof g.$RefreshRuntime$ === 'undefined') {
    const nav = (typeof window !== 'undefined' ? window : globalThis) as unknown as {
      location?: { protocol: string }
    }
    const scheme = nav.location?.protocol ?? ''
    if (scheme === 'http:' || scheme === 'https:') {
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
        })
        .catch(() => {
          // Not installed — a full reload is the fallback, no user-visible error.
        })
    }
  }
}

install()

export type { HotContext }

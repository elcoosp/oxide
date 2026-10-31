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

function install(): void {
  if (typeof window === 'undefined') return
  const g = window as unknown as {
    __oxide_hmr__?: unknown
    __oxide_refresh?: unknown
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
      // Phase 1: no-op — nothing calls this yet.
      // Phase 2: full reload (WebSocket module-update handler).
      // Phase 3: dispose + reimport + performReactRefresh.
      if (typeof location !== 'undefined') location.reload()
    },
  }

  // `__oxide_refresh` is what Oxc's inject_hmr_boundary calls in the accept
  // callback. Phase 1-2: full reload. Phase 3 replaces this with
  // react-refresh's performReactRefresh (loaded lazily).
  if (typeof g.__oxide_refresh === 'undefined') {
    g.__oxide_refresh = (_mod: unknown): void => {
      if (typeof location !== 'undefined') location.reload()
    }
  }
}

install()

export type { HotContext }

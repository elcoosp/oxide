// @oxide/vue real client page root (spec §8.4 + §10.3).
//
// The manifest-driven `update()` / `hydrate()` / `mount()` path in
// ./hydrate.ts needs Vue's own client runtime to mount (or hydrate) the
// REAL page component a route's client chunk exports — the same job
// `createSSRApp().mount()` does for islands in ./hydrate-island-real.ts,
// but for the whole page root. This module loads `vue` lazily and cached
// (the convention of ./real-ssr.ts on the server side): when the optional
// peer dep is absent the page path stays on the IR fallback and nothing
// here throws.

export interface VueClientLike {
  createSSRApp(
    component: unknown,
    props?: Record<string, unknown>,
  ): { mount(container: Element | string): unknown; unmount(): void }
  version?: string
}

/** Uniform handle the page path drives regardless of framework. */
export interface RealPageRoot {
  /** Re-mount the current component with fresh props (navigation swap). */
  update(props: Record<string, unknown>): void
  /** Unmount the app (route teardown / client-root dispose). */
  dispose(): void
}

let cached: VueClientLike | undefined
let attempted = false

async function load(): Promise<VueClientLike | undefined> {
  if (cached) return cached
  if (attempted) return undefined
  attempted = true
  try {
    // Dynamic so the runtime is code-split: a page only pays for Vue's
    // client runtime when a manifest-driven page render actually happens.
    const mod = (await import('vue')) as unknown as { default?: VueClientLike } & VueClientLike
    const vue = (mod.default ?? mod) as VueClientLike
    if (typeof vue.createSSRApp !== 'function') return undefined
    cached = vue
    return cached
  } catch {
    // Optional peer not installed → the IR fallback owns the page.
    return undefined
  }
}

/** True when `vue` is resolvable at runtime. */
export async function isVueClientAvailable(): Promise<boolean> {
  return (await load()) !== undefined
}

/**
 * Mount (or hydrate) a REAL page component into the page container with
 * Vue's own client runtime.
 *
 * - `hydrate: true` → `createSSRApp(...).mount()` hydrates the server-rendered
 *   markup in `container` in place (§10.3 — the same path the islands use).
 * - `hydrate: false` → fresh `createSSRApp(...).mount()` into the container.
 *
 * Returns `undefined` when Vue is not installed — callers fall back to the
 * IR renderer. Navigation updates unmount the previous app and mount a fresh
 * one (Vue roots are single-component apps; reactivity restarts per route,
 * which matches the "route swap" semantics of the §8.4 navigation flow).
 */
export async function mountVuePageRoot(
  container: Element,
  Component: unknown,
  props: Record<string, unknown>,
  opts: { hydrate?: boolean } = {},
): Promise<RealPageRoot | undefined> {
  const vue = await load()
  if (!vue) return undefined
  const el = container
  const mountFresh = (p: Record<string, unknown>): { unmount(): void; mount(el: unknown): void } => {
    el.innerHTML = ''
    const app = vue.createSSRApp(Component, p)
    app.mount(el)
    return app
  }
  let app: { unmount(): void; mount(el: unknown): void }
  if (opts.hydrate) {
    app = vue.createSSRApp(Component, props)
    app.mount(el)
  } else {
    app = mountFresh(props)
  }
  return {
    update(next: Record<string, unknown>): void {
      try {
        app.unmount()
      } catch {
        // unmount throws if the container was already detached; best effort.
      }
      app = mountFresh(next)
    },
    dispose(): void {
      try {
        app.unmount()
      } catch {
        // best effort
      }
    },
  }
}

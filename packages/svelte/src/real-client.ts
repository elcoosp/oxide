// @oxide/svelte real client page root (spec §8.4 + §10.3).
//
// The manifest-driven `update()` / `hydrate()` / `mount()` path in
// ./hydrate.ts needs Svelte's own client runtime to mount (or hydrate) the
// REAL page component a route's client chunk exports — the same job
// `mount(..., { hydrate: true })` does for islands in
// ./hydrate-island-real.ts, but for the whole page root. This module loads
// `svelte` lazily and cached (the convention of ./real-ssr.ts on the server
// side): when the optional peer dep is absent the page path stays on the IR
// fallback and nothing here throws.

export interface SvelteClientLike {
  mount(
    component: unknown,
    opts: { target: Element; props?: Record<string, unknown>; hydrate?: boolean },
  ): unknown
  unmount(component: unknown): void
}

/** Uniform handle the page path drives regardless of framework. */
export interface RealPageRoot {
  /** Re-mount the current component with fresh props (navigation swap). */
  update(props: Record<string, unknown>): void
  /** Unmount the component (route teardown / client-root dispose). */
  dispose(): void
}

let cached: SvelteClientLike | undefined
let attempted = false

async function load(): Promise<SvelteClientLike | undefined> {
  if (cached) return cached
  if (attempted) return undefined
  attempted = true
  try {
    // Dynamic so the runtime is code-split: a page only pays for Svelte's
    // client runtime when a manifest-driven page render actually happens.
    // Svelte 5 exports `mount`/`unmount` from the package root.
    const mod = (await import('svelte')) as unknown as { default?: SvelteClientLike } & SvelteClientLike
    const svelte = (mod.default ?? mod) as SvelteClientLike
    if (typeof svelte.mount !== 'function' || typeof svelte.unmount !== 'function') return undefined
    cached = svelte
    return cached
  } catch {
    // Optional peer not installed → the IR fallback owns the page.
    return undefined
  }
}

/** True when `svelte` (>=5) is resolvable at runtime. */
export async function isSvelteClientAvailable(): Promise<boolean> {
  return (await load()) !== undefined
}

/**
 * Mount (or hydrate) a REAL page component into the page container with
 * Svelte's own client runtime.
 *
 * - `hydrate: true` → `mount(..., { hydrate: true })` attaches to the
 *   server-rendered markup in `container` (§10.3 — the islands path).
 * - `hydrate: false` → fresh `mount(...)` into the container.
 *
 * Returns `undefined` when Svelte is not installed — callers fall back to
 * the IR renderer. Navigation updates unmount the previous component and
 * mount a fresh one (component state restarts per route, matching the
 * "route swap" semantics of the §8.4 navigation flow).
 */
export async function mountSveltePageRoot(
  container: Element,
  Component: unknown,
  props: Record<string, unknown>,
  opts: { hydrate?: boolean } = {},
): Promise<RealPageRoot | undefined> {
  const svelte = await load()
  if (!svelte) return undefined
  const el = container
  const mountFresh = (p: Record<string, unknown>): unknown => {
    el.innerHTML = ''
    return svelte.mount(Component, { target: el, props: p })
  }
  let instance: unknown
  if (opts.hydrate) {
    instance = svelte.mount(Component, { target: el, props, hydrate: true })
  } else {
    instance = mountFresh(props)
  }
  return {
    update(next: Record<string, unknown>): void {
      try {
        svelte.unmount(instance)
      } catch {
        // unmount throws if the container was already detached; best effort.
      }
      instance = mountFresh(next)
    },
    dispose(): void {
      try {
        svelte.unmount(instance)
      } catch {
        // best effort
      }
    },
  }
}

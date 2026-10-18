// @oxide/react real client page root (spec §8.4 + §10.3).
//
// The manifest-driven `update()` / `hydrate()` / `mount()` path in
// ./hydrate.ts needs React's own client runtime to render (or hydrate) the
// REAL page component a route's client chunk exports — the same job
// `react-dom/client` does for islands in ./hydrate-island-real.ts, but for
// the whole page root. This module loads `react` + `react-dom/client`
// lazily and cached (the exact convention of ./real-ssr.ts on the server
// side): when the optional peer deps are absent the page path stays on the
// IR fallback and nothing here throws.
//
// All shapes are structural — the real modules satisfy them, and tests can
// hand-lookalikes via the same dynamic-import seam the server module uses.

export interface ReactClientLike {
  createElement(type: unknown, props: Record<string, unknown> | null): unknown
  version?: string
}

export interface ReactRootLike {
  render(node: unknown): void
  unmount(): void
}

export interface ReactDomClientLike {
  createRoot(container: Element): ReactRootLike
  hydrateRoot(
    container: Element,
    node: unknown,
    options?: { onRecoverableError?: (e: unknown) => void },
  ): ReactRootLike
  version?: string
}

/** Uniform handle the page path drives regardless of framework. */
export interface RealPageRoot {
  /** Re-render the current component with fresh props (navigation swap). */
  update(props: Record<string, unknown>): void
  /** Unmount the tree (route teardown / client-root dispose). */
  dispose(): void
}

let cached: { react: ReactClientLike; client: ReactDomClientLike } | undefined
let attempted = false

async function load(): Promise<{ react: ReactClientLike; client: ReactDomClientLike } | undefined> {
  if (cached) return cached
  if (attempted) return undefined
  attempted = true
  try {
    // Dynamic so both are code-split into a runtime chunk the browser only
    // fetches when a manifest-driven page render actually happens (§6.4).
    const [reactMod, clientMod] = await Promise.all([
      import('react') as Promise<{ default?: ReactClientLike } & ReactClientLike>,
      import('react-dom/client') as Promise<{ default?: ReactDomClientLike } & ReactDomClientLike>,
    ])
    const react = (reactMod.default ?? reactMod) as ReactClientLike
    const client = (clientMod.default ?? clientMod) as ReactDomClientLike
    if (typeof react.createElement !== 'function') return undefined
    if (typeof client.createRoot !== 'function' && typeof client.hydrateRoot !== 'function') {
      return undefined
    }
    cached = { react, client }
    return cached
  } catch {
    // Optional peers not installed → the IR fallback owns the page.
    return undefined
  }
}

/** True when `react` + `react-dom/client` are resolvable at runtime. */
export async function isReactClientAvailable(): Promise<boolean> {
  return (await load()) !== undefined
}

/**
 * Mount (or hydrate) a REAL page component into the page container with
 * React's own client runtime.
 *
 * - `hydrate: true` → `hydrateRoot` attaches to the server-rendered markup
 *   in `container` (§10.3 hydration). On IR-replayed markup React reports a
 *   recoverable mismatch and client-renders the real tree — the correct
 *   end state either way.
 * - `hydrate: false` → `createRoot` + `render` for an empty container
 *   (SPA mount, or a post-navigation fresh paint).
 *
 * Returns `undefined` when React is not installed — callers fall back to
 * the IR renderer. The returned {@link RealPageRoot} re-renders on
 * navigation via `root.render` (a React root updates in place) and unmounts
 * on dispose.
 */
export async function mountReactPageRoot(
  container: Element,
  Component: unknown,
  props: Record<string, unknown>,
  opts: { hydrate?: boolean } = {},
): Promise<RealPageRoot | undefined> {
  const rt = await load()
  if (!rt) return undefined
  let root: ReactRootLike
  if (opts.hydrate) {
    root = rt.client.hydrateRoot(container, rt.react.createElement(Component, props))
  } else {
    root = rt.client.createRoot(container)
    root.render(rt.react.createElement(Component, props))
  }
  return {
    update(next: Record<string, unknown>): void {
      root.render(rt.react.createElement(Component, next))
    },
    dispose(): void {
      try {
        root.unmount()
      } catch {
        // unmount throws if the container was already detached; best effort.
      }
    },
  }
}

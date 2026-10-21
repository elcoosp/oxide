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
  /** Re-render with a NEW component + props (navigation swap). A navigation
   *  to a different route needs a different Component; the closure from
   *  `mountReactPageRoot` captured the ORIGINAL one, so the previous
   *  `update(props)` signature re-rendered the old component with new
   *  props — a client nav never changed the page. */
  update(Component: unknown, props: Record<string, unknown>): void
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
/**
 * §8.2 / §16.3: wrap a page component in its layout chain.
 *
 * The swap logic shared by the initial mount, hydration, and navigation
 * update. The outermost layout is applied last (loops from
 * `layouts.length - 1` down to 0), so `layouts[0]` ends up outermost —
 * the exact tree the SSR adapter emits.
 *
 * Exported for L6(b): the swap logic is what the Playwright navigation
 * spec asserts at the DOM level, but the tree assembly is worth pinning
 * at the unit level — a layout order regression would render
 * Inner(Outer(Page)) which is valid React but wrong markup, and no
 * Playwright assertion today covers that.
 */
export function nestLayouts(
  react: { createElement: (C: unknown, p: Record<string, unknown>) => unknown },
  Component: unknown,
  props: Record<string, unknown>,
  layouts: Array<{ id: string; Component: unknown }>,
): unknown {
  let node: unknown = react.createElement(Component, props)
  for (let i = layouts.length - 1; i >= 0; i--) {
    node = react.createElement(layouts[i].Component, { children: node })
  }
  return node
}

export async function mountReactPageRoot(
  container: Element,
  Component: unknown,
  props: Record<string, unknown>,
  opts: {
    hydrate?: boolean
    // §8.2: layout components to nest, OUTERMOST first. Empty ⇒ page only.
    layouts?: Array<{ id: string; Component: unknown }>
  } = {},
): Promise<RealPageRoot | undefined> {
  const rt = await load()
  if (!rt) return undefined
  const layouts = opts.layouts ?? []
  // Build `<Outer><Inner>…<Page props/></Inner></Outer>` — outermost first
  // by wrapping the page then each layout from the inside out. The SSR
  // adapter produces this exact tree; a bare page here throws React #418
  // on the layout markup.

  let root: ReactRootLike
  if (opts.hydrate) {
    root = rt.client.hydrateRoot(container, nestLayouts(rt.react, Component, props, layouts))
  } else {
    root = rt.client.createRoot(container)
    root.render(nestLayouts(rt.react, Component, props, layouts))
  }
  return {
    update(C: unknown, next: Record<string, unknown>): void {
      root.render(nestLayouts(rt.react, C, next, layouts))
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

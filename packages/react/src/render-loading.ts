// §8.4 loading boundaries: render a route's `loading.tsx` component into an
// overlay element while a client-side navigation's `/__data` fetch is in
// flight. Uses `react-dom/client`'s `createRoot` (real React, so hooks and
// Suspense work). The disposer unmounts the root; the router calls it right
// before `adapter.update` swaps the tree.
import type { Disposer } from '@oxide-fw/core/types'

/** Narrow an unknown loading component to a React function component. Same
 * pattern as hydrate-island-real's helper — runtime guard in the body,
 * narrowing without a cast at the callsite. */
function assertFunctionComponent(
  v: unknown,
): asserts v is import('react').FunctionComponent<Record<string, unknown>> {
  if (typeof v !== 'function') {
    throw new TypeError('reactRenderLoading: Component must be a React function component')
  }
}


export interface ReactDomClientRootLike {
  createRoot(container: Element): { render(node: unknown): void; unmount(): void }
}

// Real module types — this lets us keep `createElement` strict so the
// narrowed FunctionComponent argument type-checks without casts.
let cachedRoot: typeof import('react-dom/client') | undefined
let cachedReact: typeof import('react') | undefined

async function loadReactRuntime(): Promise<{
  root: typeof import('react-dom/client')
  react: typeof import('react')
}> {
  if (cachedRoot && cachedReact) return { root: cachedRoot, react: cachedReact }
  const [reactMod, clientMod] = await Promise.all([
    import('react'),
    import('react-dom/client'),
  ])
  cachedReact = reactMod
  cachedRoot = clientMod
  return { root: cachedRoot, react: cachedReact }
}

/**
 * Render a loading component into `overlay` with React's own client runtime.
 *
 * `createRoot(...)` mounts fresh (no SSR markup to hydrate — the overlay is
 * empty), so the loading component renders normally. The returned disposer
 * unmounts; callers should invoke it synchronously before the tree swaps so
 * the overlay is gone when the new page paints.
 *
 * **Important:** this is async internally (dynamic imports). The router's
 * `onNavigateStart` is synchronous. We mount into the overlay only after the
 * runtime loads; the overlay div exists immediately, so there is no visual
 * gap. Callers that need to synchronously dispose (e.g. navigation
 * superseded) get a disposer that safely no-ops if the mount has not
 * finished yet.
 */
export function reactRenderLoading(
  overlay: Element,
  _input: unknown,
  Component: unknown,
): Disposer {
  let root: { unmount(): void; render(node: unknown): void } | undefined
  let cancelled = false
  void loadReactRuntime()
    .then(({ root: rdc, react }) => {
      if (cancelled) return
      root = rdc.createRoot(overlay)
      assertFunctionComponent(Component)
      root.render(react.createElement(Component, {}))
    })
    .catch((e) => {
      console.error?.('oxide: renderLoading (react) failed', e)
    })
  return () => {
    cancelled = true
    try {
      root?.unmount()
    } catch {
      // unmount throws if the overlay was already detached; best effort.
    }
    root = undefined
  }
}

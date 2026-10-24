// §8.4 loading boundaries: render a route's `loading.tsx` component into an
// overlay element while a client-side navigation's `/__data` fetch is in
// flight. Uses `react-dom/client`'s `createRoot` (real React, so hooks and
// Suspense work). The disposer unmounts the root; the router calls it right
// before `adapter.update` swaps the tree.
import type { Disposer } from '../../core/src/types.ts'

export interface ReactDomClientRootLike {
  createRoot(container: Element): { render(node: unknown): void; unmount(): void }
}

let cachedRoot: ReactDomClientRootLike | undefined
let cachedReact: { createElement: (...args: unknown[]) => unknown } | undefined

async function loadReactRuntime(): Promise<{
  root: ReactDomClientRootLike
  react: { createElement: (...args: unknown[]) => unknown }
}> {
  if (cachedRoot && cachedReact) return { root: cachedRoot, react: cachedReact }
  const [reactMod, clientMod] = await Promise.all([
    import('react'),
    import('react-dom/client'),
  ])
  cachedReact = reactMod as unknown as typeof cachedReact
  cachedRoot = clientMod as unknown as ReactDomClientRootLike
  return { root: cachedRoot!, react: cachedReact! }
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
  _input: { url: string; route: string; params: Record<string, string | string[]>; data: unknown },
  Component: unknown,
): Disposer {
  let root: { unmount(): void } | undefined
  let cancelled = false
  void loadReactRuntime()
    .then(({ root: rdc, react }) => {
      if (cancelled) return
      root = rdc.createRoot(overlay)
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

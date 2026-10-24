// §8.4 loading boundaries: render a route's `loading.tsx` component into an
// overlay element while a client-side navigation's `/__data` fetch is in
// flight. Uses Solid's `render` (client-side render, not `hydrate` — the
// overlay is empty). Disposer disposes the reactive scope.
import type { Disposer } from '../../core/src/types.ts'

interface SolidWebLike {
  render(fn: () => unknown, element: Element): () => void
}

let cachedWeb: SolidWebLike | undefined
let cachedCreateElement: ((type: unknown, props: unknown) => unknown) | undefined

async function loadSolid(): Promise<{
  web: SolidWebLike
  createElement: (type: unknown, props: unknown) => unknown
}> {
  if (cachedWeb && cachedCreateElement) return { web: cachedWeb, createElement: cachedCreateElement }
  let web: SolidWebLike
  try {
    web = (await import('@solidjs/web')) as unknown as SolidWebLike
  } catch {
    web = (await import('@solidjs/web')) as unknown as SolidWebLike
  }
  const solid = (await import('solid-js')) as unknown as {
    createElement?: (type: unknown, props: unknown) => unknown
  }
  if (typeof solid.createElement !== 'function') {
    throw new Error('renderLoading (solid): solid-js has no createElement export')
  }
  cachedWeb = web
  cachedCreateElement = solid.createElement
  return { web, createElement: solid.createElement }
}

export function solidRenderLoading(
  overlay: Element,
  _input: { url: string; route: string; params: Record<string, string | string[]>; data: unknown },
  Component: unknown,
): Disposer {
  let dispose: (() => void) | undefined
  let cancelled = false
  void loadSolid()
    .then(({ web, createElement }) => {
      if (cancelled) return
      dispose = web.render(() => createElement(Component, {}), overlay)
    })
    .catch((e) => {
      console.error?.('oxide: renderLoading (solid) failed', e)
    })
  return () => {
    cancelled = true
    try {
      dispose?.()
    } catch {
      // best-effort
    }
    dispose = undefined
  }
}

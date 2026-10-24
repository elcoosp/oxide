// §8.4 loading boundaries: render a route's `loading.tsx` component into an
// overlay element while a client-side navigation's `/__data` fetch is in
// flight. Uses Solid's `render` (client-side render, not `hydrate` — the
// overlay is empty). Disposer disposes the reactive scope.
import type { Disposer } from '../../core/src/types.ts'

/** Runtime type guard for @solidjs/web. `await import('@solidjs/web')` in
 * ESM returns the namespace with renderer functions at top level; no
 * CJS-interop `.default` exists for a true-ESM package. */
function isSolidWebLikeForLoading(v: unknown): v is SolidWebLike {
  if (v === null || typeof v !== 'object') return false
  return typeof Reflect.get(v, 'renderToString') === 'function'
    || typeof Reflect.get(v, 'renderToStringAsync') === 'function'
    || typeof Reflect.get(v, 'renderToStream') === 'function'
}

/** Solid core factory. Both keys expose the same callable signature — the
 *  runtime passes whatever the user's bundler emitted (Solid 2's
 *  `createElement`, Solid 1's `createComponent`). Declaring them with the
 *  same signature lets callers pick either without a variance fight. */
interface SolidFactoryForLoading {
  createElement?: (type: unknown, props: unknown) => unknown
  createComponent?: (type: unknown, props: unknown) => unknown
}

function isSolidFactoryForLoading(v: unknown): v is SolidFactoryForLoading {
  if (v === null || typeof v !== 'object') return false
  return typeof Reflect.get(v, 'createElement') === 'function'
    || typeof Reflect.get(v, 'createComponent') === 'function'
}


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
  const webMod: unknown = await import('@solidjs/web')
  if (!isSolidWebLikeForLoading(webMod)) {
    throw new Error('renderLoading (solid): @solidjs/web has no renderer export')
  }
  const web: SolidWebLike = webMod
  const solidMod: unknown = await import('solid-js')
  if (!isSolidFactoryForLoading(solidMod)) {
    throw new Error('renderLoading (solid): solid-js has no createElement/createComponent export')
  }
  const create = solidMod.createElement ?? solidMod.createComponent
  if (typeof create !== 'function') {
    throw new Error('renderLoading (solid): solid-js factory is not a function')
  }
  cachedWeb = web
  cachedCreateElement = create
  return { web, createElement: create }
}

export function solidRenderLoading(
  overlay: Element,
  _input: unknown,
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

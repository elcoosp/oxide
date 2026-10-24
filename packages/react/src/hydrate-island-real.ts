// Real React island hydration via `react-dom/client`. The IR-based
// `hydrateIsland` in ./hydrate.ts replays deterministic markup and only
// works for components that don't use hooks — any real app needs React's
// own hydration so `useState` and event handlers survive.
//
// `react-dom/client` is loaded lazily: an island page loads it when the
// first island's strategy fires (typically immediately for `load`, or on
// idle / intersection for the other strategies). A page with no islands
// never fetches it. This turns the island runtime into a code-split the
// browser only pays for when it will actually hydrate something.
//
// The returned value is a `Promise<Disposer>`; `packages/core/src/client/
// islands.ts` awaits it before marking the host hydrated.

import type { FunctionComponent } from 'react'
import type { Disposer } from '../../core/src/types.ts'
import type { OxElement, ClientOptions } from './hydrate.ts'

/** Structural shape of `react-dom/client` — matches the real module. */
export interface ReactDomClientLike {
  hydrateRoot(
    container: Element,
    children: unknown,
    options?: { onRecoverableError?: (e: unknown) => void },
  ): { unmount(): void }
}

let cachedClient: ReactDomClientLike | undefined
let cachedReact: { createElement: (...args: unknown[]) => unknown } | undefined

async function loadReact(): Promise<{
  client: ReactDomClientLike
  react: { createElement: (...args: unknown[]) => unknown }
}> {
  if (cachedClient && cachedReact) {
    return { client: cachedClient, react: cachedReact }
  }
  // `react` and `react-dom/client` are dynamic here so both are code-split
  // into a runtime chunk loaded only on first island hydration. The bundler
  // dedupes: a project that already imports React elsewhere shares the same
  // chunk.
  const [reactMod, clientMod] = await Promise.all([
    import('react'),
    import('react-dom/client'),
  ])
  cachedReact = reactMod as unknown as typeof cachedReact
  cachedClient = clientMod as unknown as ReactDomClientLike
  return { client: cachedClient, react: cachedReact! }
}

/**
 * Hydrate a single `<ox-island>` host with React's own client runtime.
 *
 * `mod` is the dynamic-import result of the island chunk; its `default`
 * export is the React component the server rendered. React's hydration
 * preserves the SSR'd markup and only attaches event listeners, so the
 * initial state the server produced stays put until the user interacts.
 *
 * Resolves to a disposer that unmounts the React root; `hydrateIslands`
 * stores it and calls it on page teardown.
 */
export async function reactHydrateIsland(
  host: Element,
  mod: unknown,
  props: unknown,
  _opts: ClientOptions = {},
): Promise<Disposer> {
  const component =
    (mod as { default?: FunctionComponent }).default ??
    (mod as unknown as FunctionComponent)
  if (typeof component !== 'function') {
    throw new TypeError('reactHydrateIsland: island module must export a default component')
  }
  const { client, react } = await loadReact()
  const element = react.createElement(
    component,
    (props as Record<string, unknown>) ?? {},
  )
  const root = client.hydrateRoot(host, element)
  return () => root.unmount()
}

export type { OxElement, ClientOptions }

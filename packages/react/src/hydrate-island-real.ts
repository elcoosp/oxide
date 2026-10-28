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
import type { Disposer } from '@oxide-fw/core/types'
import type { OxElement, ClientOptions } from './hydrate.ts'

/** Structural shape of `react-dom/client` — matches the real module. */
export interface ReactDomClientLike {
  hydrateRoot(
    container: Element,
    children: unknown,
    options?: { onRecoverableError?: (e: unknown) => void },
  ): { unmount(): void }
}

// Keep the module types the dynamic import actually produces — no local
// minimal type. Callers narrow at the callsite; the alternative (minimal
// interface + `as unknown as` conversion) defeats the point.

/** Narrow an unknown island module export to a React function component.
 *
 * The FrameworkAdapter.hydrateIsland contract types `mod` as `unknown` — the
 * framework cannot know the module's shape ahead of time. This helper is the
 * honest runtime-guard-then-assert pattern: the body checks
 * `typeof v === 'function'` (what a React function component IS at runtime,
 * no amount of type machinery changes that), and TS uses that guard to narrow
 * `unknown` to `FunctionComponent<Record<string, unknown>>` in the caller —
 * without an `as` cast. A class component (rare in island code, and not what
 * the SSR path emits) would also pass the runtime check; the signature just
 * tells TS what to call it.
 */
function assertFunctionComponent(
  v: unknown,
): asserts v is import('react').FunctionComponent<Record<string, unknown>> {
  if (typeof v !== 'function') {
    throw new TypeError('island module must export a React function component')
  }
}

let cachedClient: typeof import('react-dom/client') | undefined
let cachedReact: typeof import('react') | undefined

async function loadReact(): Promise<{
  client: typeof import('react-dom/client')
  react: typeof import('react')
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
  cachedReact = reactMod
  cachedClient = clientMod
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
  // `mod` is the dynamic-import result of the island chunk; its `default`
  // export is the React component the server rendered. Some bundlers wrap
  // the module without a `default` (an interop shape); in that case the
  // module itself is the component function. Narrow through runtime checks
  // (`typeof === 'function'`) instead of casting — if neither shape is a
  // function, the throw is the honest failure.
  const maybeDefault = (mod as { default?: unknown }).default
  const candidate: unknown = maybeDefault ?? mod
  assertFunctionComponent(candidate)
  const component = candidate
  const { client, react } = await loadReact()
  const element = react.createElement(
    component,
    (props as Record<string, unknown>) ?? {},
  )
  const root = client.hydrateRoot(host, element)
  return () => root.unmount()
}

export type { OxElement, ClientOptions }

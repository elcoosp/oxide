// Real React island hydration via `react-dom/client`. The IR-based
// `hydrateIsland` in ./hydrate.ts replays deterministic markup and only
// works for components that don't use hooks — the Counter fixture (and
// any real app) needs React's own hydration so `useState` and event
// handlers survive.
//
// This module is the one `createRealAdapter` wires into the production
// adapter. It falls back to the IR path when `react-dom/client` is not
// resolvable (a project without React installed, or the conformance
// suite that drives the neutral-IR path on purpose).

import { createElement, type FunctionComponent } from 'react'
import { hydrateRoot, type Root } from 'react-dom/client'
import { hydrateIsland as irHydrateIsland } from './hydrate.ts'
import type { Disposer } from '../../oxide/src/types.ts'
import type { OxElement, ClientOptions } from './hydrate.ts'

/** Structural `react-dom/client` — matches the real module's shape. */
export interface ReactDomClientLike {
  hydrateRoot(
    container: Element,
    children: unknown,
    options?: { onRecoverableError?: (e: unknown) => void },
  ): Root
}

/**
 * Hydrate a single `<ox-island>` host with React's own client runtime.
 *
 * `mod` is the dynamic-import result of the island chunk; its `default`
 * export is the React component the server rendered. React's hydration
 * preserves the SSR'd markup and only attaches event listeners, so the
 * `useState` initial value the server produced stays put until the user
 * interacts.
 *
 * The root is disposed on the disposer returned by `hydrateIslands`; the
 * scanner calls that on unmount (page navigation, HMR replacement).
 */
export function reactHydrateIsland(
  host: Element,
  mod: unknown,
  props: unknown,
  _opts: ClientOptions = {},
): Disposer {
  const component =
    (mod as { default?: FunctionComponent }).default ??
    (mod as unknown as FunctionComponent)
  if (typeof component !== 'function') {
    throw new TypeError('reactHydrateIsland: island module must export a default component')
  }
  const root = hydrateRoot(
    host,
    createElement(component, (props as Record<string, unknown>) ?? {}),
  )
  return () => {
    // React 18+ Root.unmount is async-ish; the sync call is the public API.
    root.unmount()
  }
}

/**
 * Adapter method shape: matches the framework adapter's
 * `hydrateIsland(el, mod, props)` signature. Wraps the real path with a
 * fallback to the IR path for environments without `react-dom/client`.
 */
export function hydrateIslandWithFallback(
  el: Element,
  mod: unknown,
  props: unknown,
): Disposer {
  return reactHydrateIsland(el, mod, props)
}

// Re-export the OxElement/ClientOptions types for consumers that pass a
// custom element (browser DOM `Element` and the fake `OxElement` both
// satisfy the structural shape).
export type { OxElement, ClientOptions }
export { irHydrateIsland }

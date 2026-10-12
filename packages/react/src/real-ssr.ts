// @oxide/react real React SSR path (spec §7).
//
// The IR-based renderer in ./render.ts is the NEUTRAL fallback: it runs with
// no framework deps installed and drives the §22 conformance suite. This
// module is the REAL path: it dynamically imports `react` and
// `react-dom/server` and renders actual React components through the real
// upstream renderer. When those packages are installed (they are optional
// peer deps of this adapter), the pipeline should prefer this path so users
// get full React SSR semantics — hooks, context, useId, Suspense,
// React.createElement trees compiled by Oxc.
//
// Node 24 type stripping: this file is .ts, not .tsx — components supplied
// by callers are opaque functions receiving props and returning React
// elements. Build them with React.createElement, or rely on the Oxc JSX
// transform at compile time.
//
// Imports are dynamic and cached so the package keeps working (falling back
// to the IR renderer) when react/react-dom are absent.

import type { RenderInput, HeadTags } from '../../oxide/src/types.ts'

/** Minimal structural type of the React module we depend on. */
export interface ReactLike {
  createElement(type: unknown, props: Record<string, unknown> | null, ...children: unknown[]): unknown
  Fragment: unknown
  version?: string
}

/** Minimal structural type of the `react-dom/server` module we depend on. */
export interface ReactDomServerLike {
  renderToString(element: unknown): string
  version?: string
}

let cached: { react: ReactLike; server: ReactDomServerLike } | undefined
let attempted = false

async function load(): Promise<{ react: ReactLike; server: ReactDomServerLike } | undefined> {
  if (cached) return cached
  if (attempted) return undefined
  attempted = true
  try {
    const reactMod = (await import('react')) as unknown as { default?: ReactLike } & ReactLike
    const serverMod = (await import('react-dom/server')) as unknown as ReactDomServerLike
    const react = (reactMod.default ?? reactMod) as ReactLike
    if (typeof react.createElement !== 'function') return undefined
    if (typeof serverMod.renderToString !== 'function') return undefined
    cached = { react, server: serverMod }
    return cached
  } catch {
    return undefined
  }
}

/** True when `react` + `react-dom/server` are resolvable at runtime. */
export async function isReactAvailable(): Promise<boolean> {
  return (await load()) !== undefined
}

/** The resolved React module, or undefined when not installed. */
export async function getReact(): Promise<ReactLike | undefined> {
  return (await load())?.react
}

/** The resolved `react-dom/server` module, or undefined when not installed. */
export async function getReactDomServer(): Promise<ReactDomServerLike | undefined> {
  return (await load())?.server
}

/**
 * Render a React component to its HTML string via `react-dom/server`'s
 * `renderToString`. Throws when React is not installed — callers should
 * check `isReactAvailable()` first, or fall back to the IR renderer in
 * ./render.ts.
 */
export async function renderReactToString(
  Component: (props: Record<string, unknown>) => unknown,
  props: Record<string, unknown> = {},
): Promise<string> {
  const deps = await load()
  if (!deps) {
    throw new Error(
      '@oxide/react: real SSR requires `react` and `react-dom/server` to be installed',
    )
  }
  const element = deps.react.createElement(Component, props)
  return deps.server.renderToString(element)
}

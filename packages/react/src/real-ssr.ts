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

import type { RenderInput, HeadTags } from '../../core/src/types.ts'

/** Minimal structural type of the React module we depend on. */
export interface ReactLike {
  createElement(type: unknown, props: Record<string, unknown> | null, ...children: unknown[]): unknown
  Fragment: unknown
  version?: string
}

/** Minimal structural type of the `react-dom/server` module we depend on. */
export interface ReactDomServerLike {
  renderToString(element: unknown): string
  /**
   * React 19's streaming renderer (available on `react-dom/server.browser`,
   * `.edge`, `.node`, `.bun` — every non-`react-server` entry). Resolves to
   * a `ReadableStream<Uint8Array>` the caller can pump into its response.
   * Optional: an older React 18 may only have the `renderToPipeableStream`
   * Node shape, so the caller feature-detects.
   */
  renderToReadableStream?(
    element: unknown,
    options?: { onError?: (e: unknown) => void },
  ): Promise<ReadableStream<Uint8Array>>
  version?: string
}

let cached: { react: ReactLike; server: ReactDomServerLike } | undefined
let attempted = false
// BUG-09: per-adapter load error (read by the shared helper via lastError).
// The old `__oxideReactLoadError` global leaked internals across adapters.
let lastLoadError: string | undefined
export function getLastLoadError(): string | undefined {
  return lastLoadError
}

async function load(): Promise<{ react: ReactLike; server: ReactDomServerLike } | undefined> {
  if (cached) return cached
  if (attempted) return undefined
  attempted = true
  try {
    const reactMod = (await import('react')) as unknown as { default?: ReactLike } & ReactLike
    const serverMod = (await import('react-dom/server')) as unknown as ReactDomServerLike
    const react = (reactMod.default ?? reactMod) as ReactLike
    if (typeof react.createElement !== 'function') {
      lastLoadError =
        'react imported but createElement is ' + typeof (react as { createElement?: unknown }).createElement +
        '; keys=' + Object.keys(reactMod as object).join(',') +
        '; defaultKeys=' + (reactMod.default ? Object.keys(reactMod.default as object).join(',') : 'none')
      return undefined
    }
    if (typeof serverMod.renderToString !== 'function') {
      lastLoadError =
        'react-dom/server imported but renderToString is ' + typeof (serverMod as { renderToString?: unknown }).renderToString
      return undefined
    }
    cached = { react, server: serverMod }
    return cached
  } catch (e) {
    lastLoadError =
      'import threw: ' + ((e as { message?: string })?.message ?? String(e))
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
/**
 * Structural shape of a child descriptor (§8.2). `makeRealAdapter` hands the
 * layout a `children` value of `{ Component, props }` — a plain descriptor,
 * not a live React element. That lets the layout chain be framework-agnostic
 * (Vue and Svelte use the same wire shape). React, however, can only render
 * a real element as `props.children`, so we expand each descriptor into
 * `React.createElement(descriptor.Component, descriptor.props)` before the
 * top-level `createElement` call.
 */
interface ChildDescriptor {
  Component: (props: Record<string, unknown>) => unknown
  props: Record<string, unknown>
}

function isChildDescriptor(v: unknown): v is ChildDescriptor {
  return (
    typeof v === 'object' &&
    v !== null &&
    'Component' in v &&
    'props' in v &&
    typeof (v as { Component: unknown }).Component !== 'undefined'
  )
}

function expandDescriptor(
  react: ReactLike,
  descriptor: ChildDescriptor,
): unknown {
  const props: Record<string, unknown> = { ...descriptor.props }
  // Nested layout: `props.children` is another descriptor — expand it
  // depth-first so innermost pages become elements first. A layout with N
  // outer layouts is N descriptors deep.
  if (isChildDescriptor(props.children)) {
    props.children = expandDescriptor(react, props.children)
  }
  return react.createElement(descriptor.Component, props)
}

/**
 * Render a React component to its HTML string via `react-dom/server`'s
 * `renderToString`. Descriptor children (`{ Component, props }`) are expanded
 * to real React elements before the outer `createElement` call — see
 * `expandDescriptor`.
 *
 * Throws when React is not installed — callers should check
 * `isReactAvailable()` first, or fall back to the IR renderer in
 * ./render.ts.
 */
export async function renderReactToString(
  Component: unknown,
  props: Record<string, unknown> = {},
): Promise<string> {
  const deps = await load()
  if (!deps) {
    throw new Error(
      '@oxide/react: real SSR requires `react` and `react-dom/server` to be installed',
    )
  }
  // §8.2: a layout receives `props.children` as a descriptor. Expand it into
  // a React element first so the layout can render `{props.children}` as a
  // normal JSX child. The outermost call also accepts a plain component (a
  // page with no layout), which has no descriptor children.
  const expanded: Record<string, unknown> = { ...props }
  if (isChildDescriptor(expanded.children)) {
    expanded.children = expandDescriptor(deps.react, expanded.children)
  }
  const element = deps.react.createElement(Component, expanded)
  return deps.server.renderToString(element)
}

/**
 * Stream a React component's HTML via `renderToReadableStream` (React 19).
 * The returned stream yields the shell immediately and boundary content as
 * it resolves — the TTFB property streaming exists for. The shell-first
 * chunking is preserved end-to-end: the worker emits each chunk as a raw
 * binary frame, and the pipeline's `render_stream_with_recovery` passes them
 * straight to the client (only splicing the recovery boundary if a mid-stream
 * error occurs).
 *
 * Falls back to `[renderReactToString]`'s single-chunk result when the
 * runtime has no streaming API — a caller that drains the stream still gets
 * the same HTML, just as one chunk.
 */
export async function renderReactToStream(
  Component: unknown,
  props: Record<string, unknown> = {},
): Promise<ReadableStream<Uint8Array>> {
  const deps = await load()
  if (!deps) {
    throw new Error(
      '@oxide/react: real SSR requires `react` and `react-dom/server` to be installed',
    )
  }
  // Same descriptor-expansion as `renderReactToString`: a layout receives
  // `props.children` as `{ Component, props }`; React needs a live element.
  const expanded: Record<string, unknown> = { ...props }
  if (isChildDescriptor(expanded.children)) {
    expanded.children = expandDescriptor(deps.react, expanded.children)
  }
  const element = deps.react.createElement(Component, expanded)

  if (typeof deps.server.renderToReadableStream === 'function') {
    // React 19 streams natively. `onError` reports a thrown component; the
    // stream itself then closes early. That error surfaces to the caller as
    // the pipeline's recovery boundary (render_stream_with_recovery).
    return deps.server.renderToReadableStream(element, {
      onError: (e) => {
        console.error?.('oxide: React streaming SSR error', e)
      },
    })
  }
  // Older React (no streaming API): synthesize a single-chunk stream from
  // `renderToString` so the caller sees a uniform interface.
  const html = deps.server.renderToString(element)
  const bytes = new TextEncoder().encode(html)
  return new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(bytes)
      controller.close()
    },
  })
}

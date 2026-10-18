// @oxide/svelte real Svelte SSR path (spec §7).
//
// The IR-based renderer in ./render.ts is the NEUTRAL fallback. This module
// is the REAL path: it dynamically imports `svelte/server` (Svelte 5's
// `render(Component, { props })`) and returns the rendered body.
//
// Imports are dynamic and cached so the package keeps working (falling back
// to the IR renderer) when svelte is absent.

export interface SvelteServerLike {
  render(
    component: unknown,
    options?: { props?: Record<string, unknown> },
  ): { head?: string; body?: string; html?: string }
  version?: string
}

let cached: SvelteServerLike | undefined
let attempted = false
// BUG-09: per-adapter load error detail.
let lastLoadError: string | undefined

/**
 * Structural shape of the `svelte` core module's snippet factory (Svelte 5).
 * Snippets are how a programmatic caller passes slot content to a component:
 * a layout's `{@render children()}` invokes the snippet we hand it as the
 * `children` prop. Optional — an older Svelte without it keeps the raw
 * descriptor in `props.children` (the pre-descriptor behavior).
 */
export interface SvelteSnippetFactoryLike {
  createRawSnippet?(render: (anchor: unknown) => { html: string } | string): unknown
  version?: string
}

let cachedCore: SvelteSnippetFactoryLike | undefined

async function loadCore(): Promise<SvelteSnippetFactoryLike | undefined> {
  if (cachedCore) return cachedCore
  try {
    const mod = (await import('svelte')) as unknown as
      { default?: SvelteSnippetFactoryLike } & SvelteSnippetFactoryLike
    cachedCore = (mod.default ?? mod) as SvelteSnippetFactoryLike
    return cachedCore
  } catch {
    // Core runtime absent → descriptor children stay descriptors.
    return undefined
  }
}

export function getLastLoadError(): string | undefined {
  return lastLoadError
}

async function load(): Promise<SvelteServerLike | undefined> {
  if (cached) return cached
  if (attempted) return undefined
  attempted = true
  try {
    const mod = (await import('svelte/server')) as unknown as { default?: SvelteServerLike } & SvelteServerLike
    const server = (mod.default ?? mod) as SvelteServerLike
    if (typeof server.render !== 'function') {
      lastLoadError = 'svelte/server imported but render is ' + typeof server.render
      return undefined
    }
    cached = server
    return cached
  } catch (e) {
    lastLoadError = 'import threw: ' + ((e as { message?: string })?.message ?? String(e))
    return undefined
  }
}

/** True when `svelte/server` (Svelte 5) is resolvable at runtime. */
export async function isSvelteAvailable(): Promise<boolean> {
  return (await load()) !== undefined
}

/**
 * Structural shape of a layout child descriptor (§8.2). `makeRealAdapter`
 * hands a layout a `children` value of `{ Component, props }` — a plain
 * descriptor, not a mounted component, so the layout chain stays framework-
 * agnostic on the wire.
 */
interface ChildDescriptor {
  Component: unknown
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

/**
 * §8.2: expand a layout child descriptor chain, depth-first, into Svelte 5's
 * slot idiom. Svelte consumes children through SNIPPET props
 * (`{@render children()}`), so the child chain is rendered to HTML with
 * `svelte/server` and wrapped in a `createRawSnippet` — the layout then
 * emits the child markup verbatim from its slot.
 *
 * Tradeoff (documented): inside a layout chain the child is static raw HTML
 * — its own client-side hydration happens through the page-root/island
 * paths, not inside the snippet. When `svelte` core (createRawSnippet) or
 * the server renderer is unavailable the descriptor is left in place
 * unchanged (the pre-descriptor behavior).
 */
async function expandSvelteProps(props: Record<string, unknown>): Promise<Record<string, unknown>> {
  if (!isChildDescriptor(props.children)) return props
  const child = props.children
  // Depth-first: the innermost chain expands first, so nested layouts
  // (L1(L2(page))) each see a snippet for everything below them.
  const childProps = await expandSvelteProps(child.props)
  const [core, server] = await Promise.all([loadCore(), load()])
  if (!core || typeof core.createRawSnippet !== 'function' || !server) return props
  const out = server.render(child.Component, { props: childProps })
  const html = typeof out.body === 'string' ? out.body : typeof out.html === 'string' ? out.html : ''
  const snippet = core.createRawSnippet(() => ({ html }))
  return { ...props, children: snippet }
}

/** The resolved Svelte server renderer, or undefined when not installed. */
export async function getSvelteServer(): Promise<SvelteServerLike | undefined> {
  return await load()
}

/**
 * Render a Svelte 5 component to HTML via `svelte/server`. Returns `.body`
 * when present (Svelte 5's `render` returns `{ head, body }`); `.html` for a
 * Svelte 4-style response. A layout child descriptor (`props.children =
 * { Component, props }`, §8.2) is expanded into a raw snippet first — see
 * {@link expandSvelteProps}. Throws when Svelte is not installed.
 */
export async function renderSvelteToString(
  Component: unknown,
  props: Record<string, unknown> = {},
): Promise<string> {
  const server = await load()
  if (!server) {
    throw new Error('@oxide/svelte: real SSR requires `svelte` (>=5) to be installed')
  }
  const effectiveProps = await expandSvelteProps(props)
  const out = server.render(Component, { props: effectiveProps })
  if (typeof out.body === 'string') return out.body
  if (typeof out.html === 'string') return out.html
  throw new Error('@oxide/svelte: svelte/server render() returned no body/html field')
}

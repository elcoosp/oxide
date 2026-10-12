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

async function load(): Promise<SvelteServerLike | undefined> {
  if (cached) return cached
  if (attempted) return undefined
  attempted = true
  try {
    const mod = (await import('svelte/server')) as unknown as { default?: SvelteServerLike } & SvelteServerLike
    const server = (mod.default ?? mod) as SvelteServerLike
    if (typeof server.render !== 'function') return undefined
    cached = server
    return cached
  } catch {
    return undefined
  }
}

/** True when `svelte/server` (Svelte 5) is resolvable at runtime. */
export async function isSvelteAvailable(): Promise<boolean> {
  return (await load()) !== undefined
}

/** The resolved Svelte server renderer, or undefined when not installed. */
export async function getSvelteServer(): Promise<SvelteServerLike | undefined> {
  return await load()
}

/**
 * Render a Svelte 5 component to HTML via `svelte/server`. Returns `.body`
 * when present (Svelte 5's `render` returns `{ head, body }`); `.html` for a
 * Svelte 4-style response. Throws when Svelte is not installed.
 */
export async function renderSvelteToString(
  Component: unknown,
  props: Record<string, unknown> = {},
): Promise<string> {
  const server = await load()
  if (!server) {
    throw new Error('@oxide/svelte: real SSR requires `svelte` (>=5) to be installed')
  }
  const out = server.render(Component, { props })
  if (typeof out.body === 'string') return out.body
  if (typeof out.html === 'string') return out.html
  throw new Error('@oxide/svelte: svelte/server render() returned no body/html field')
}

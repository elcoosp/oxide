// @oxide/solid real Solid SSR path (spec §7).
//
// The IR-based renderer in ./render.ts is the NEUTRAL fallback. This module
// is the REAL path: it dynamically imports `solid-js/web` and calls
// `renderToString` / `renderToStringAsync` on the supplied component
// factory.
//
// Imports are dynamic and cached so the package keeps working (falling back
// to the IR renderer) when solid-js is absent.

export interface SolidWebLike {
  renderToString(fn: () => unknown): string
  renderToStringAsync(fn: () => unknown): Promise<string>
  version?: string
}

let cached: SolidWebLike | undefined
let attempted = false

async function load(): Promise<SolidWebLike | undefined> {
  if (cached) return cached
  if (attempted) return undefined
  attempted = true
  try {
    const mod = (await import('solid-js/web')) as unknown as SolidWebLike
    if (typeof mod.renderToStringAsync !== 'function') return undefined
    cached = mod
    return cached
  } catch {
    return undefined
  }
}

/** True when `solid-js/web` is resolvable at runtime. */
export async function isSolidAvailable(): Promise<boolean> {
  return (await load()) !== undefined
}

/** The resolved `solid-js/web` module, or undefined when not installed. */
export async function getSolidWeb(): Promise<SolidWebLike | undefined> {
  return await load()
}

/**
 * Render a Solid component factory to HTML via `solid-js/web`. Uses the
 * async variant so Suspense/resources resolve. The factory is called with
 * no arguments and must return the root element (built via `solid-js`'s JSX,
 * compiled at build time, or via `solid-js/web`'s SSR element helpers).
 */
export async function renderSolidToString(fn: () => unknown): Promise<string> {
  const web = await load()
  if (!web) {
    throw new Error('@oxide/solid: real SSR requires `solid-js` to be installed')
  }
  return await web.renderToStringAsync(fn)
}

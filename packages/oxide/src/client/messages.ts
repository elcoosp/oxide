// Client-side message catalogs (§16-D).
//
// The build emits one JSON file per locale at `/client/messages/<locale>.json`
// (`{ locale, messages: { "nav.home": "Home", … } }`). This module is the
// browser half: it fetches the active locale's catalog once, caches it, and
// answers `t(key, values)` from a plain object.
//
// Why a plain object rather than ICU at runtime: the server formats ICU
// messages (plural/select) at render time via `oxide-assets::i18n::format`.
// An island is already hydrated with the *rendered* text in its props — a
// client `t()` call is for messages the island composes after hydration
// (a counter label, a toast), which are overwhelmingly plain substitutions.
// An ICU placeholder (`{n}`) is substituted; a `plural`/`select` expression
// is not — an island that needs one should use the server-rendered value in
// its props. Documented limitation, not a silent one.

/** A loaded locale catalog, keyed by message id. */
export type Messages = Readonly<Record<string, string>>

/** The JSON shape the build emits. */
interface CatalogFile {
  locale: string
  messages: Record<string, string>
}

const cache = new Map<string, Messages>()
let activeLocale: string | null = null
let inflight: Promise<Messages> | null = null

/** The locale the page was rendered in — `<html lang>` is authoritative
 *  (the server sets it from the resolved locale), falling back to the
 *  document's `data-locale` if present. */
function detectLocale(): string {
  if (activeLocale) return activeLocale
  const doc = globalThis.document
  if (doc?.documentElement?.lang) {
    activeLocale = doc.documentElement.lang
    return activeLocale
  }
  const dl = doc?.documentElement?.getAttribute?.('data-locale')
  if (dl) {
    activeLocale = dl
    return activeLocale
  }
  activeLocale = 'en'
  return activeLocale
}

/** Derive the route key the build uses for a per-route subset. The build
 *  keys on the route directory with any trailing `/page` stripped and slashes
 *  replaced by underscores (`routes/blog/[slug]` → `blog_[slug]`); this
 *  reproduces it from the ODF payload's `route` field, or from
 *  `location.pathname` when the payload is absent. */
function routeKeyFromPage(): string | null {
  const g = globalThis as {
    __oxide?: { route?: string }
  }
  const route = g.__oxide?.route
  if (typeof route === 'string' && route.length > 0) {
    const stripped = route.endsWith('/page') ? route.slice(0, -5) : route
    const rel = stripped.startsWith('routes/') ? stripped.slice(7) : stripped
    return rel.replace(/\//g, '_')
  }
  return null
}

/** Load (and cache) the active locale's catalog. Tries the per-route subset
 *  first (`/client/messages/<locale>/<route>.json`), falling back to the full
 *  catalog (`/client/messages/<locale>.json`). Resolves to an empty map on
 *  any failure — a missing catalog must not break a page; `t()` then returns
 *  the key, which is a visible-and-recoverable degradation, not a crash. */
export async function loadMessages(locale?: string): Promise<Messages> {
  const l = locale ?? detectLocale()
  const cached = cache.get(l)
  if (cached) return cached
  if (inflight) return inflight
  inflight = (async () => {
    try {
      // Start from the full catalog, then overlay the route's subset. The
      // subset is a *preference*, not a replacement: a route that names a
      // few keys must still see shared keys (`nav.home`) that live in the
      // full catalog. Previously the first successful subset fetch
      // replaced the catalog wholesale.
      const merged: Record<string, string> = {}
      let subset: Record<string, string> | null = null
      const routeKey = routeKeyFromPage()
      if (routeKey) {
        try {
          const sub = await globalThis.fetch(
            `/client/messages/${encodeURIComponent(l)}/${encodeURIComponent(routeKey)}.json`,
          )
          if (sub.ok) {
            const body = (await sub.json()) as CatalogFile
            subset = body.messages ?? {}
          }
        } catch {
          // fall through to the full catalog
        }
      }
      try {
        const res = await globalThis.fetch(`/client/messages/${encodeURIComponent(l)}.json`)
        if (res.ok) {
          const body = (await res.json()) as CatalogFile
          Object.assign(merged, body.messages ?? {})
        }
      } catch {
        // A missing full catalog is not fatal; the subset below may still
        // cover what this route needs.
      }
      // Subset wins over the full catalog for the keys it carries, but the
      // full catalog stays for every shared key it does not.
      if (subset) Object.assign(merged, subset)
      const msgs: Messages = Object.freeze(merged)
      cache.set(l, msgs)
      return msgs
    } catch {
      return {}
    } finally {
      inflight = null
    }
  })()
  return inflight
}

/** Synchronously look up a message. Returns `key` when the catalog has not
 *  been loaded yet or the key is absent — a caller that needs the value
 *  before load can `await loadMessages()` first. */
export function t(key: string, values?: Record<string, string | number>): string {
  const l = detectLocale()
  const msgs = cache.get(l) ?? {}
  // Catalog keys are namespaced by the locale filename they came from
  // (`locales/en/common.json` → `common.nav.home`), but authors write
  // `t('nav.home')`: the namespace is a file-location artifact, not part of
  // the message id. Fall back to a `.`-boundary suffix match, which is the
  // same rule the build uses when it selects a per-route subset.
  let out = msgs[key]
  if (out === undefined) {
    const suffix = '.' + key
    for (const k of Object.keys(msgs)) {
      if (k.endsWith(suffix)) {
        out = msgs[k]
        break
      }
    }
  }
  out = out ?? key
  if (values) {
    out = out.replace(/\{(\w+)\}/g, (_, name: string) =>
      name in values ? String(values[name]) : `{${name}}`,
    )
  }
  return out
}

/** Test-only: reset the module-level cache. */
export function _resetMessagesForTest(): void {
  cache.clear()
  activeLocale = null
  inflight = null
}

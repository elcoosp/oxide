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

/** Load (and cache) the active locale's catalog. Resolves to an empty map on
 *  any failure — a missing catalog must not break a page; `t()` then returns
 *  the key, which is a visible-and-recoverable degradation, not a crash. */
export async function loadMessages(locale?: string): Promise<Messages> {
  const l = locale ?? detectLocale()
  const cached = cache.get(l)
  if (cached) return cached
  if (inflight) return inflight
  inflight = (async () => {
    try {
      const res = await globalThis.fetch(`/client/messages/${encodeURIComponent(l)}.json`)
      if (!res.ok) return {}
      const body = (await res.json()) as CatalogFile
      const msgs: Messages = Object.freeze({ ...(body.messages ?? {}) })
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
  let out = msgs[key] ?? key
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

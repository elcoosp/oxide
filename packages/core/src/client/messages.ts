// Client-side message catalogs (§16-D).
//
// The build emits one JSON file per locale at `/client/messages/<locale>.json`
// (`{ locale, messages: { "nav.home": "Home", … } }`). This module is the
// browser half: it fetches the active locale's catalog once, caches it, and
// answers `t(key, values)` from a plain object.
//
// Messages are ICU MessageFormat strings. The server formats them with
// `oxide-assets::i18n::format`; the browser half below implements the same
// core subset without a dependency: placeholders, `plural`, `selectordinal`,
// `select`, `number`, `date` and `time`, including nested messages and the
// `offset:` form. An island that hydrates with server-rendered text still
// gets the same strings when it formats a message itself after hydration.

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
/**
 * §16.4: whether this build emitted any message catalogs.
 *
 * Set from `client.manifest.json`'s `i18n_locales` (the CLI stamps it at
 * build time). `undefined` means "not told" — the fetch proceeds, which is
 * the pre-existing behaviour for a caller that never registered a manifest.
 * An empty array means "the project has no i18n", and the fetch is skipped:
 * without this, every page load of a non-i18n app requests
 * `/client/messages/<locale>.json`, gets a 404, and Chromium reports it as a
 * console error.
 */
let hasCatalogs: boolean | undefined

/** Register the build's locale list (from the client manifest). */
export function setMessageLocales(locales: string[] | undefined): void {
  hasCatalogs = locales === undefined ? undefined : locales.length > 0
}

export async function loadMessages(locale?: string): Promise<Messages> {
  const l = locale ?? detectLocale()
  const cached = cache.get(l)
  if (cached) return cached
  if (inflight) return inflight
  if (hasCatalogs === false) {
    // The build emitted no catalogs; asking would 404.
    return {}
  }
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
  if (values) out = formatIcu(out, values, l)
  return out
}

/** Format an ICU MessageFormat string with the browser's own `Intl` APIs.
 *  Unknown placeholders are left intact (matching the previous substitution
 *  behaviour) so a missing value is visible rather than silently empty. */
export function formatIcu(
  message: string,
  values: Readonly<Record<string, string | number>>,
  locale = detectLocale(),
  pound?: number,
): string {
  let out = ''
  let i = 0
  while (i < message.length) {
    const c = message[i]!
    if (c === "'") {
      const quoted = readQuoted(message, i)
      if (quoted) {
        out += quoted.value
        i = quoted.next
      } else {
        out += c
        i += 1
      }
      continue
    }
    if (c === '#') {
      out += pound === undefined ? '#' : formatNumber(pound, locale)
      i += 1
      continue
    }
    if (c !== '{') {
      out += c
      i += 1
      continue
    }
    const close = findClosingBrace(message, i)
    if (close < 0) {
      out += message.slice(i)
      break
    }
    out += formatArgument(message.slice(i + 1, close), values, locale, pound)
    i = close + 1
  }
  return out
}

function readQuoted(s: string, start: number): { value: string; next: number } | null {
  const next = s[start + 1]
  if (next === "'") return { value: "'", next: start + 2 }
  // ICU starts a quoted literal only before syntax; an ordinary apostrophe
  // in prose ("l'utilisateur") is a literal, not a quote.
  if (next === undefined || !"{#}|".includes(next)) return null
  let value = ''
  let i = start + 1
  while (i < s.length) {
    if (s[i] === "'") {
      if (s[i + 1] === "'") {
        value += "'"
        i += 2
        continue
      }
      return { value, next: i + 1 }
    }
    value += s[i]
    i += 1
  }
  return { value, next: i }
}

function findClosingBrace(s: string, open: number): number {
  let depth = 0
  let i = open
  while (i < s.length) {
    const c = s[i]!
    if (c === "'") {
      const quoted = readQuoted(s, i)
      if (quoted) {
        i = quoted.next
        continue
      }
    }
    if (c === '{') depth += 1
    else if (c === '}') {
      depth -= 1
      if (depth === 0) return i
    }
    i += 1
  }
  return -1
}

function formatArgument(
  inner: string,
  values: Readonly<Record<string, string | number>>,
  locale: string,
  pound?: number,
): string {
  const comma = inner.indexOf(',')
  if (comma < 0) {
    const name = inner.trim()
    if (!(name in values)) return `{${name}}`
    return String(values[name])
  }
  const name = inner.slice(0, comma).trim()
  const rest = inner.slice(comma + 1).trim()
  if (!(name in values)) return `{${inner}}`
  const value = values[name]
  if (value === undefined) return `{${inner}}`
  const second = rest.indexOf(',')
  const type = (second < 0 ? rest : rest.slice(0, second)).trim()
  const style = second < 0 ? '' : rest.slice(second + 1).trim()
  switch (type) {
    case 'plural':
    case 'selectordinal':
      return formatPlural(value, style, locale, type === 'selectordinal', values, pound)
    case 'select':
      return formatSelect(value, style, locale, values, pound)
    case 'number':
      return formatNumberValue(value, style, locale)
    case 'date':
    case 'time':
      return formatDateTime(value, style, locale, type)
    default:
      return String(value)
  }
}

interface IcuOptions {
  options: Map<string, string>
  offset: number
}

function parseOptions(source: string): IcuOptions {
  const options = new Map<string, string>()
  let offset = 0
  let i = 0
  while (i < source.length) {
    while (i < source.length && /\s/.test(source.charAt(i))) i += 1
    if (i >= source.length) break
    const keyStart = i
    while (i < source.length && !/\s/.test(source[i]!) && source[i] !== '{') i += 1
    const key = source.slice(keyStart, i)
    while (i < source.length && /\s/.test(source.charAt(i))) i += 1
    if (key.startsWith('offset:')) {
      const n = Number(key.slice('offset:'.length))
      if (Number.isFinite(n)) offset = n
      continue
    }
    if (source[i] !== '{') {
      // Malformed option: skip one character so the loop cannot spin.
      i += 1
      continue
    }
    const close = findClosingBrace(source, i)
    if (close < 0) break
    options.set(key, source.slice(i + 1, close))
    i = close + 1
  }
  return { options, offset }
}

function formatPlural(
  value: string | number,
  style: string,
  locale: string,
  ordinal: boolean,
  values: Readonly<Record<string, string | number>>,
  outerPound?: number,
): string {
  const n = Number(value)
  if (!Number.isFinite(n)) return String(value)
  const { options, offset } = parseOptions(style)
  const exact = options.get('=' + String(n - offset))
  if (exact !== undefined) return formatIcu(exact, values, locale, n - offset)
  let category = 'other'
  try {
    category = new Intl.PluralRules(locale, { type: ordinal ? 'ordinal' : 'cardinal' }).select(n - offset)
  } catch {
    // Invalid locale: keep the English categories deterministic.
    category = n - offset === 1 ? 'one' : 'other'
  }
  const branch = options.get(category) ?? options.get('other')
  return branch === undefined ? '' : formatIcu(branch, values, locale, n - offset)
}

function formatSelect(
  value: string | number,
  style: string,
  locale: string,
  values: Readonly<Record<string, string | number>>,
  pound?: number,
): string {
  const { options } = parseOptions(style)
  const branch = options.get(String(value)) ?? options.get('other')
  return branch === undefined ? '' : formatIcu(branch, values, locale, pound)
}

function formatNumber(n: number, locale: string): string {
  try {
    return new Intl.NumberFormat(locale).format(n)
  } catch {
    return String(n)
  }
}

function formatNumberValue(value: string | number, style: string, locale: string): string {
  const n = Number(value)
  if (!Number.isFinite(n)) return String(value)
  const s = style.trim()
  try {
    if (s === 'integer') return new Intl.NumberFormat(locale, { maximumFractionDigits: 0 }).format(n)
    if (s === 'percent') return new Intl.NumberFormat(locale, { style: 'percent' }).format(n)
    if (s.startsWith('currency:')) {
      return new Intl.NumberFormat(locale, {
        style: 'currency',
        currency: s.slice('currency:'.length),
      }).format(n)
    }
  } catch {
    // fall through to plain number formatting
  }
  return formatNumber(n, locale)
}

function formatDateTime(
  value: string | number,
  style: string,
  locale: string,
  kind: 'date' | 'time',
): string {
  const d = new Date(value)
  if (Number.isNaN(d.getTime())) return String(value)
  const s = style.trim()
  const dateStyle = ['short', 'medium', 'long', 'full'].includes(s)
    ? (s as 'short' | 'medium' | 'long' | 'full')
    : undefined
  try {
    return new Intl.DateTimeFormat(locale, {
      ...(kind === 'date' ? { dateStyle: dateStyle ?? 'medium' } : {}),
      ...(kind === 'time' ? { timeStyle: dateStyle ?? 'medium' } : {}),
    }).format(d)
  } catch {
    return String(value)
  }
}

/** Test-only: reset the module-level cache. */
export function _resetMessagesForTest(): void {
  cache.clear()
  activeLocale = null
  inflight = null
}

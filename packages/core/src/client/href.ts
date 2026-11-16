// V2-E02 — the runtime of the generated `href` overloads and the typed
// `goto` (spec §V2-E02).
//
// Typegen (`crates/router/src/typegen.rs`) emits the per-route `href`
// overloads into `.oxide/types/routes.d.ts`; this module is the RUNTIME
// those declarations bind to. The contract, in the generated overload
// order:
//   - params fill the route's dynamic segments — every spelling the build
//     emits: `[name]` (the scanner), `_name_` (the manifest's normalized
//     form), `:name` (hand-written tables), and `[...name]` (rest, array
//     joined by `/`);
//   - params the route does not consume ride the query string (the raw
//     `searchParams` record dimension stays reachable);
//   - `opts.search` serializes as the query (arrays repeat the key, the
//     `searchToQueryString` rules), `opts.hash` appends `#…`;
//   - an i18n `locale` prefixes `/{locale}` (the `prerendered/{locale}/…`
//     convention, `crates/assets/src/i18n.rs`) — except when it equals the
//     `defaultLocale` given in opts, which stays unprefixed.
//
// Leniency note: a param the route declares but the caller omits leaves
// the token verbatim. The generated overload is the TYPE check (TS rejects
// the call); the runtime never throws in a render path — a broken href is
// a visible bug, not a crashed page.

import type { Router } from './router.ts'

/** The options object of a search-carrying `href` overload. */
export interface HrefOpts {
  /** The typed search object — serialized as the query string. */
  search?: unknown
  /** The fragment, with or without the leading `#`. */
  hash?: string
  /** The i18n locale; prefixed `/{locale}` unless it is the default. */
  locale?: string
  /** The project's default locale — a `locale` equal to it stays unprefixed. */
  defaultLocale?: string
}

/** The navigation options the typed `goto` accepts (href opts + replace). */
export interface GotoOpts extends HrefOpts {
  /** History semantics of the navigation (default `false` = push). */
  replace?: boolean
}

/** A param value: a path segment, or an array for a `[...rest]` segment. */
export type HrefParam = string | number | boolean | Array<string | number | boolean>

/** The router surface `goto` needs (the client router satisfies it). */
export type GotoRouter = Pick<Router, 'goto'>

/** The known opts keys — an object with ONLY these keys is opts, not params. */
const OPTS_KEYS = new Set(['search', 'hash', 'locale', 'defaultLocale', 'replace'])

function isOptsObject(v: unknown): v is HrefOpts {
  if (v === undefined || v === null || typeof v !== 'object' || Array.isArray(v)) return false
  const keys = Object.keys(v as Record<string, unknown>)
  if (keys.length === 0) return false
  return keys.every((k) => OPTS_KEYS.has(k))
}

/**
 * Fill a route pattern's dynamic segments from `params`. Recognizes every
 * spelling the build emits (`[name]`, the manifest's `_name_`, `:name`,
 * `[...name]`); an unconsumed or missing param never throws — leftovers
 * come back as the query-string half, missing params leave the token.
 */
function fillSegments(
  route: string,
  params: Record<string, HrefParam> | undefined,
): { path: string; leftover: Array<[string, string]> } {
  const leftover: Array<[string, string]> = []
  if (params === undefined || Object.keys(params).length === 0) {
    return { path: route, leftover }
  }
  const consumed = new Set<string>()
  const path = route
    .split('/')
    .map((seg) => {
      let name: string | undefined
      let rest = false
      if (seg.startsWith('[') && seg.endsWith(']') && seg.length > 2) {
        const inner = seg.slice(1, -1)
        if (inner.startsWith('...')) {
          name = inner.slice(3)
          rest = true
        } else {
          name = inner
        }
      } else if (seg.startsWith(':') && seg.length > 1) {
        name = seg.slice(1)
      } else if (seg.length > 2 && seg.startsWith('_') && seg.endsWith('_')) {
        name = seg.slice(1, -1)
      }
      if (name === undefined || !(name in params)) return seg
      consumed.add(name)
      const v = params[name]
      if (rest) {
        const list = Array.isArray(v) ? v : [v]
        return list.map(String).join('/')
      }
      return String(v)
    })
    .join('/')
  for (const [k, v] of Object.entries(params)) {
    if (consumed.has(k)) continue
    if (Array.isArray(v)) {
      for (const item of v) leftover.push([k, String(item)])
    } else {
      leftover.push([k, String(v)])
    }
  }
  return { path, leftover }
}

/** Serialize the query half: the leftover params (route identity) first, then `opts.search` (arrays repeat, null skipped). */
function queryString(opts: HrefOpts | undefined, leftover: Array<[string, string]>): string {
  const sp = new URLSearchParams()
  for (const [k, v] of leftover) sp.append(k, v)
  const search = opts?.search
  if (search !== undefined && search !== null && typeof search === 'object') {
    for (const [k, v] of Object.entries(search as Record<string, unknown>)) {
      if (v === undefined || v === null) continue
      if (Array.isArray(v)) {
        for (const item of v) {
          if (item !== undefined && item !== null) sp.append(k, String(item))
        }
      } else {
        sp.append(k, String(v))
      }
    }
  }
  const s = sp.toString()
  return s === '' ? '' : `?${s}`
}

/** The fragment half: `#…` with the leading `#` normalized. */
function fragment(hash: string | undefined): string {
  if (hash === undefined || hash === '') return ''
  return hash.startsWith('#') ? hash : `#${hash}`
}

/** The locale half: `/{locale}` prefix unless the locale IS the default. The locale may ride the positional form or opts. */
function localePrefix(loc: string | undefined, opts: HrefOpts | undefined): string {
  const locale = loc ?? opts?.locale
  if (locale === undefined) return ''
  if (opts?.defaultLocale !== undefined && locale === opts.defaultLocale) return ''
  return `/${locale}`
}

/**
 * The runtime `href` — the implementation the generated overloads declare.
 * Accepts the overload shapes positionally: `(route)`, `(route, params)`,
 * `(route, params, opts)`, `(route, params, opts, locale)`,
 * `(route, opts)`, `(route, opts, locale)` and the bare i18n form
 * `(route, undefined, locale)`. An object whose keys are all known opts
 * keys is opts; anything else is params (an EMPTY object is params — no
 * segments to fill).
 */
export function hrefFor(
  route: string,
  paramsOrOpts?: Record<string, HrefParam> | HrefOpts,
  optsOrLocale?: HrefOpts | string,
  locale?: string,
): string {
  let params: Record<string, HrefParam> | undefined
  let opts: HrefOpts | undefined
  let loc = locale
  if (typeof paramsOrOpts === 'string') {
    loc = paramsOrOpts
  } else if (paramsOrOpts !== undefined) {
    if (isOptsObject(paramsOrOpts)) opts = paramsOrOpts
    else params = paramsOrOpts
  }
  if (typeof optsOrLocale === 'string') {
    loc = optsOrLocale
  } else if (optsOrLocale !== undefined) {
    // The third position may refine the opts when the second carried
    // params; keys merge with the third-position opts winning.
    opts = { ...(opts ?? {}), ...optsOrLocale }
  }
  const { path, leftover } = fillSegments(route, params)
  const q = queryString(opts, leftover)
  return `${localePrefix(loc, opts)}${path}${q}${fragment(opts?.hash)}`
}

/**
 * The typed `goto`: build the href exactly as {@link hrefFor} does and
 * hand it to the client router — the navigation entry the adapters
 * re-export with their project's generated overloads.
 */
export function goto(
  router: GotoRouter,
  route: string,
  paramsOrOpts?: Record<string, HrefParam> | GotoOpts,
  optsOrLocale?: GotoOpts | string,
  locale?: string,
): Promise<void> {
  let opts: GotoOpts | undefined
  let params: Record<string, HrefParam> | undefined
  let loc = locale
  if (typeof paramsOrOpts === 'string') {
    loc = paramsOrOpts
  } else if (paramsOrOpts !== undefined) {
    if (isOptsObject(paramsOrOpts)) opts = paramsOrOpts
    else params = paramsOrOpts
  }
  if (typeof optsOrLocale === 'string') {
    loc = optsOrLocale
  } else if (optsOrLocale !== undefined) {
    opts = { ...(opts ?? {}), ...optsOrLocale }
  }
  const href = hrefFor(route, params, opts, loc)
  return router.goto(href, { replace: opts?.replace ?? false })
}

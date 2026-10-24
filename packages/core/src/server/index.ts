// Server runtime APIs — the framework-neutral data layer (spec §9).
// Every route module (`page.server.ts`, `layout.server.ts`, `*.server.ts`)
// imports from here. Tag keys are plain strings so compiled stubs, the Rust
// pipeline and user code agree without symbol coordination.

import type {
  ActionEvent,
  CookieOptions,
  CookieStore,
  DeferredPlaceholder,
  DeferredRegistry,
  LoadEvent,
  ServerFnEvent,
  SocketContext,
  SocketHandlers,
  StandardSchema,
} from './json.ts'

export type {
  ActionEvent, CookieOptions, CookieStore, DeferredPlaceholder, DeferredRegistry,
  LoadEvent, LoadResult, ServerFnEvent, SocketContext, SocketHandlers, StandardSchema,
} from './json.ts'

/** Tag key on load fns (`fn.$$oxide === 'load'`). */
export const OXIDE_TAG = '$$oxide'

// ---------------------------------------------------------------------------
// defineLoad (§9.1, §9.2)
// ---------------------------------------------------------------------------

/**
 * Tag a load function. Calling the returned fn runs the load with the event's
 * `defer` wired to a per-request id registry: promises registered via
 * `event.defer(p)` get `{__deferred: id}` placeholders immediately, and any
 * standalone `defer(p)` markers found in the returned data are converted on
 * the way out (Value::Deferred-compatible shape, §9.5). The Rust render
 * pipeline consumes the registry via `event.__oxideDeferredRegistry`.
 */
export function defineLoad<D extends Record<string, unknown>>(
  fn: (event: LoadEvent) => Promise<D> | D,
): ((event: LoadEvent) => Promise<D>) & { $$oxide: 'load' } {
  const wrapped = async (event: LoadEvent): Promise<D> => {
    const holder = event as LoadEvent & { __oxideDeferredRegistry?: DeferredRegistry }
    const registry: DeferredRegistry =
      holder.__oxideDeferredRegistry instanceof Map ? holder.__oxideDeferredRegistry : new Map()
    holder.__oxideDeferredRegistry = registry
    let nextId = 1
    const allocate = () => nextId++
    const ev: LoadEvent = {
      ...event,
      defer: (promise: Promise<unknown>) => {
        const id = allocate()
        registry.set(id, promise)
        return { __deferred: id }
      },
    }
    const data = await fn(ev)
    return convertDefers(data, registry, allocate) as D
  }
  return Object.assign(wrapped, { $$oxide: 'load' as const })
}

/** A standalone deferred marker (what the exported [defer] returns). */
function isDeferMarker(v: unknown): v is { __oxide_defer: true; promise: Promise<unknown> } {
  return (
    typeof v === 'object' && v !== null &&
    (v as { __oxide_defer?: unknown }).__oxide_defer === true &&
    typeof (v as { promise?: unknown }).promise === 'object'
  )
}

/**
 * Replace standalone [defer] markers in a load result with
 * `{__deferred: id}` placeholders and register their promises. Cycle-safe
 * (WeakSet) and depth-capped (§17.1 parity with the ODF decoder).
 */
function convertDefers(value: unknown, registry: DeferredRegistry, allocate: () => number, depth = 1, seen = new WeakSet<object>()): unknown {
  if (depth > 512 || value === null || typeof value !== 'object') return value
  if (isDeferMarker(value)) {
    const id = allocate()
    registry.set(id, value.promise)
    return { __deferred: id } as DeferredPlaceholder
  }
  if (seen.has(value)) return value
  seen.add(value)
  if (Array.isArray(value)) {
    for (let i = 0; i < value.length; i++) {
      value[i] = convertDefers(value[i], registry, allocate, depth + 1, seen)
    }
    return value
  }
  if (value instanceof Map) {
    for (const [k, v] of value) value.set(k, convertDefers(v, registry, allocate, depth + 1, seen))
    return value
  }
  if (value instanceof Set) {
    return new Set([...value].map((v) => convertDefers(v, registry, allocate, depth + 1, seen)))
  }
  const obj = value as Record<string, unknown>
  for (const k of Object.keys(obj)) {
    if (k === '__proto__' || k === 'constructor') continue // §17.1 parity
    obj[k] = convertDefers(obj[k], registry, allocate, depth + 1, seen)
  }
  return obj
}

/**
 * Standalone form of `event.defer` (§9.2): wraps a promise in a marker the
 * load pipeline recognizes. Prefer `event.defer()` inside loads — it assigns
 * the streaming id immediately.
 */
export {
  beginBoundaryCapture,
  endBoundaryCapture,
  drainBoundaries,
  registerBoundary,
  deferredIdOf,
  boundaryFallback,
  boundaryContent,
  BOUNDARY_FALLBACK_ATTR,
  BOUNDARY_CONTENT_ATTR,
  type BoundaryRegistration,
} from './boundary.ts'

export function defer(promise: Promise<unknown>): { __oxide_defer: true; promise: Promise<unknown> } {
  return { __oxide_defer: true, promise }
}

// ---------------------------------------------------------------------------
// error() / redirect() sentinels (§9.2)
// ---------------------------------------------------------------------------

/** Thrown by [error]; carries the HTTP status for the response pipeline. */
export class OxideHttpError extends Error {
  status: number
  constructor(status: number, message?: string) {
    super(message ?? httpStatusText(status))
    this.name = 'OxideHttpError'
    this.status = status
  }
}

/** Thrown by [redirect]; the pipeline converts it to a redirect response. */
export class OxideRedirect extends Error {
  location: string
  status: number
  constructor(location: string, status = 303) {
    super(`redirecting to ${location}`)
    this.name = 'OxideRedirect'
    this.location = location
    this.status = status
  }
}

const STATUS_TEXT: Record<number, string> = {
  400: 'Bad Request', 401: 'Unauthorized', 403: 'Forbidden', 404: 'Not Found',
  405: 'Method Not Allowed', 409: 'Conflict', 410: 'Gone', 418: "I'm a teapot",
  422: 'Unprocessable Entity', 429: 'Too Many Requests', 500: 'Internal Server Error',
  502: 'Bad Gateway', 503: 'Service Unavailable', 504: 'Gateway Timeout',
}

function httpStatusText(status: number): string {
  return STATUS_TEXT[status] ?? `HTTP ${status}`
}

/** Abort a load with an HTTP error (§9.2). Throws [OxideHttpError]. */
export function error(status: number, message?: string): never {
  throw new OxideHttpError(status, message)
}

/** Redirect from a load or action. Default 303 (POST→GET safe). Throws. */
export function redirect(location: string, status = 303): never {
  throw new OxideRedirect(location, status)
}

// ---------------------------------------------------------------------------
// Actions / meta / sockets (§9.1, §9.6)
// ---------------------------------------------------------------------------

export interface ActionDefinition<I = unknown> {
  /** Optional Standard Schema validator; runs before `run` (§9.3). */
  input?: StandardSchema<I>
  run(ctx: ActionEvent): Promise<unknown> | unknown
}

/** Tag an action definition (form-first, §9.3). */
export function defineAction<I>(def: {
  input?: StandardSchema<I>
  run(ctx: ActionEvent & { input: I }): Promise<unknown> | unknown
}): { $$oxide: 'action'; input?: StandardSchema<I>; run(ctx: ActionEvent & { input: I }): Promise<unknown> | unknown } {
  return { $$oxide: 'action', input: def.input, run: def.run }
}

/** Tag a meta function: load data → neutral head model (§16.3). */
export function defineMeta<M extends (ctx: { data: unknown; params: Record<string, string | string[]> }) => Record<string, unknown>>(
  fn: M,
): M & { $$oxide: 'meta' } {
  return Object.assign(fn, { $$oxide: 'meta' as const })
}

/** Tag a WebSocket route's handlers (§9.6 — Rust owns framing/backpressure). */
export function defineSocket(handlers: SocketHandlers): SocketHandlers & { $$oxide: 'socket' } {
  return { ...handlers, $$oxide: 'socket' } as SocketHandlers & { $$oxide: 'socket' }
}

// ---------------------------------------------------------------------------
// cache() / 'use cache' helpers (§9.3, §11)
// ---------------------------------------------------------------------------

export interface CacheOptions {
  /** Fresh lifetime in seconds. */
  ttl?: number
  /** Stale-while-revalidate window in seconds (served stale + refreshed). */
  swr?: number
  /**
   * Tags for grouped invalidation (§11.3).
   *
   * A plain array is a fixed set of tags. A function receives the wrapped
   * call's arguments and returns the tags for that call — the `'use cache'`
   * compiler pass emits this form when a `cacheTag(…)` argument is not a
   * literal (e.g. `cacheTag('posts:' + slug)`), so the tag reflects the
   * argument rather than being dropped.
   */
  tags?: string[] | ((...args: unknown[]) => string[])
}

import { encodeOdf } from '../odf-encode.ts'
import { decodeOdf } from '../client/odf-decode.ts'
import { bytesToBase64 } from '../bytes.ts'

/** Deterministic cache key: JSON of args + normalized opts (tags sorted). */
/** Deterministic cache key: JSON of args + normalized opts (tags sorted). */
export function cacheKey(
  args: unknown[],
  opts?: { ttl?: number; swr?: number; tags?: string[] },
): string {
  const normalized = {
    ttl: opts?.ttl ?? null,
    swr: opts?.swr ?? null,
    tags: [...(opts?.tags ?? [])].sort(),
  }
  return JSON.stringify({ args, opts: normalized })
}

interface CacheEntry {
  value: unknown
  expiresAt: number
  staleUntil: number
  tags: string[]
}

/**
 * §9.3: the worker's host-op bridge, installed by `oxide-worker.mjs`. In a
 * Node worker it sends an `op_request` to Rust; in a browser it is absent
 * (there is no Rust host), so every call here falls back to the in-process
 * Map. The declaration makes the call site type-safe without importing the
 * worker.
 */
declare global {
  // eslint-disable-next-line no-var
  var __oxide_op_request:
    | ((op: string, args: unknown) => Promise<unknown>)
    | undefined
}

/** True when a Rust host is reachable through the op bridge. */
function hostOpAvailable(): boolean {
  return typeof globalThis.__oxide_op_request === 'function'
}

const cacheStore = new Map<string, CacheEntry>()
const cachePending = new Map<string, Promise<unknown>>()

/** P-10: bound the in-process fallback cache. Keys derive from caller args,
 *  which an attacker controls, so without a cap a long-lived worker's Map
 *  grows without limit. Expired entries are swept periodically and a hard
 *  entry cap evicts oldest-first. */
const CACHE_MAX_ENTRIES = 10_000
function sweepCache(): void {
  const now = Date.now()
  for (const [k, v] of cacheStore) {
    if (v.staleUntil < now) cacheStore.delete(k)
  }
  if (cacheStore.size > CACHE_MAX_ENTRIES) {
    let drop = cacheStore.size - CACHE_MAX_ENTRIES
    for (const k of cacheStore.keys()) {
      if (drop-- <= 0) break
      cacheStore.delete(k)
    }
  }
}
const cacheSweeper = setInterval(sweepCache, 60_000)
// `unref` so a short-lived process is not kept alive by the sweeper.
;(cacheSweeper as unknown as { unref?: () => void }).unref?.()

/** Drop cached entries carrying any of the given tags (soft→delete here; the
 * Rust TieredCache implements the stale-serve semantics, §11.3). */
export function invalidateTag(...tags: string[]): number {
  let n = 0
  for (const [key, entry] of cacheStore) {
    if (entry.tags.some((t) => tags.includes(t))) {
      cacheStore.delete(key)
      n++
    }
  }
  return n
}

/**
 * The explicit form of the `'use cache'` directive (§9.3): wraps an async fn
 * with an in-memory entry keyed by `cacheKey(args, opts)`. Concurrent misses
 * for the same key single-flight; `swr` serves stale then refreshes in the
 * background.
 */
export function cache<A extends unknown[], R>(
  fn: (...args: A) => Promise<R>,
  opts: CacheOptions = {},
): ((...args: A) => Promise<R>) & { $$oxideCache: true; opts: CacheOptions } {
  const ttlMs = (opts.ttl ?? 0) * 1000
  const swrMs = (opts.swr ?? 0) * 1000
  // Resolve `tags` for one call: a plain array is constant, a function is
  // invoked with the call's arguments. The compiler emits the function form
  // for a non-literal `cacheTag(…)` so the tag can depend on an argument.
  const tagsFor = (args: A): string[] => {
    const t = opts.tags
    const raw = typeof t === 'function' ? (t as (...a: unknown[]) => unknown)(...args) : t
    return Array.isArray(raw) ? raw.filter((x): x is string => typeof x === 'string') : []
  }

  const wrapped = async (...args: A): Promise<R> => {
    // The key includes the resolved tags (sorted), so two calls with the
    // same arguments but different tag thunks do not collide.
    const callTags = tagsFor(args)
    const key = cacheKey(args, { ...opts, tags: callTags })

    // §9.3: when a Rust host is reachable (`oxide-worker.mjs` installed
    // the op bridge), the tiered cache is authoritative — it survives
    // across requests and, with a remote handler, across nodes. The
    // in-process Map is the fallback: the browser has no host, and a
    // worker whose pool has no `CacheHandler` answers `null`.
    if (hostOpAvailable()) {
      const host = await hostGet(key)
      if (host !== undefined) {
        // A host entry is a fresh hit — the Rust tier owns TTL/SWR.
        return host as R
      }
    }

    const now = Date.now()
    const hit = cacheStore.get(key)
    if (hit) {
      if (now < hit.expiresAt) return hit.value as R
      if (swrMs > 0 && now < hit.staleUntil) {
        void refresh(key, args, callTags) // background revalidate, serve stale now
        return hit.value as R
      }
      cacheStore.delete(key)
    }
    const inflight = cachePending.get(key)
    if (inflight) return (await inflight) as R
    return (await runAndStore(key, args, callTags)) as R
  }

  /** §9.3: `cache_get` through the host bridge. `undefined` means "not
   *  cached" — a null reply, or no host, or a host error (the in-process
   *  Map then runs). */
  async function hostGet(key: string): Promise<unknown | undefined> {
    try {
      const v = (await globalThis.__oxide_op_request!('cache_get', { key })) as
        | { body_b64?: string; value?: unknown }
        | null
      if (v === null || v === undefined) return undefined
      // §9.5: the cached bytes are ODF (rich types survive). Decode them.
      // A JSON-encoded fallback (an entry written by an older runtime) is
      // detected by the missing `body_b64` and returned as-is.
      if (typeof v.body_b64 === 'string' && v.body_b64.length > 0) {
        const bin = atob(v.body_b64)
        const bytes = new Uint8Array(bin.length)
        for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i)
        return decodeOdf(bytes)
      }
      return v.value
    } catch {
      // A host error is not fatal — the in-process tier answers.
      return undefined
    }
  }

  /** §9.3: `cache_set` through the host bridge. Best-effort: a failure
   *  leaves the in-process Map as the only copy. */
  function hostSet(key: string, value: unknown, tags: string[]): void {
    if (!hostOpAvailable()) return
    // §9.5: encode the value to ODF so rich types (Date/Map/Set/BigInt/…) and
    // shared references survive the cache round-trip. The same bytes are what
    // the wire format (§9.5) uses, so a cached value and a streamed value are
    // bit-identical. A value that ODF cannot encode falls back to JSON with a
    // warning rather than failing the cache write.
    let body_b64 = ''
    try {
      // P-09: linear-time base64 (chunked), not per-byte string concat.
      body_b64 = bytesToBase64(encodeOdf(value))
    } catch (e) {
      console.warn?.('oxide: cache value is not ODF-encodable; storing JSON', e)
      const json = JSON.stringify(value)
      body_b64 = bytesToBase64(new TextEncoder().encode(json))
    }
    // `ttl`/`swr` are seconds on the wire (§11.3).
    void globalThis
      .__oxide_op_request!('cache_set', {
        key,
        body_b64,
        tags,
        ttl: opts.ttl ?? 0,
        swr: opts.swr,
      })
      .catch(() => {})
  }
  const runAndStore = async (key: string, args: A, tags: string[]): Promise<R> => {
    const p = (async () => {
      try {
        const value = await fn(...args)
        cacheStore.set(key, {
          value,
          expiresAt: Date.now() + ttlMs,
          staleUntil: Date.now() + ttlMs + swrMs,
          tags: [...tags],
        })
        // P-10: keep the fallback Map bounded after each write.
        if (cacheStore.size > CACHE_MAX_ENTRIES) sweepCache()
        // §9.3: mirror to the Rust tier when a host is reachable.
        hostSet(key, value, tags)
        return value
      } finally {
        cachePending.delete(key)
      }
    })()
    cachePending.set(key, p)
    return (await p) as R
  }
  const refresh = (key: string, args: A, tags: string[]): Promise<unknown> => {
    if (cachePending.has(key)) return cachePending.get(key)!
    return runAndStore(key, args, tags)
  }
  return Object.assign(wrapped, { $$oxideCache: true as const, opts })
}

/**
 * Inside a `'use cache'` fn: attach tags to the cache entry (§9.3). The
 * compiler pass threads these to the cache wrapper; standalone it returns a
 * documented marker.
 */
export function cacheTag(...tags: string[]): { __oxideCacheTag: string[] } {
  return { __oxideCacheTag: tags }
}

/** Inside a `'use cache'` fn: override ttl/swr for this entry (§9.3). */
export function cacheLife(life: { ttl?: number; swr?: number; staleIfError?: number }): { __oxideCacheLife: typeof life } {
  return { __oxideCacheLife: life }
}

/**
 * §9.3: the wrapper the compiler's `'use cache'` pass emits.
 *
 * A function
 *
 *     export const getPost = async (slug: string) => {
 *       'use cache'
 *       cacheTag('posts:' + slug)
 *       cacheLife({ ttl: 60, swr: 300 })
 *       return db.post(slug)
 *     }
 *
 * compiles to
 *
 *     export const getPost = __oxide_cache(
 *       async (slug) => db.post(slug),
 *       "<module>", "getPost",
 *       { tags: ["posts:" + slug], life: { ttl: 60, swr: 300 } },
 *     )
 *
 * The options object carries *expressions* the compiler hoisted out of the
 * function body — `tags` and `life` are evaluated at call time inside the
 * wrapper, not at module load, so a tag built from an argument reflects
 * that argument.
 *
 * The wrapper adapts to `cache()`: the compiler's `{tags, life}` becomes
 * the runtime's `{tags, ttl, swr}`. Installing it as a global (like
 * `__oxide_register_fn`) lets the emitted call resolve without every
 * `*.server.ts` importing it by that name.
 */
declare global {
  // eslint-disable-next-line no-var
  var __oxide_cache:
    | (<A extends unknown[], R>(
        fn: (...args: A) => Promise<R>,
        module: string,
        exportName: string,
        opts: {
          tags?: unknown
          life?: { ttl?: number; swr?: number; staleIfError?: number }
        },
      ) => (...args: A) => Promise<R>)
    | undefined
}

if (typeof globalThis.__oxide_cache !== 'function') {
  globalThis.__oxide_cache = <A extends unknown[], R>(
    fn: (...args: A) => Promise<R>,
    _module: string,
    _exportName: string,
    opts: {
      tags?: unknown
      life?: { ttl?: number; swr?: number; staleIfError?: number }
    },
  ): ((...args: A) => Promise<R>) => {
    // `tags` may be a value (an array of strings) or a thunk the compiler
    // emitted for an expression that depends on the arguments. Resolve it
    // per call.
    const tagsOf = (args: A): string[] => {
      const t = typeof opts.tags === 'function' ? (opts.tags as (...a: A) => unknown)(...args) : opts.tags
      if (Array.isArray(t)) return t.filter((x): x is string => typeof x === 'string')
      return typeof t === 'string' ? [t] : []
    }
    // The compiler emits `tags` as a plain array when every `cacheTag(…)`
    // argument is a literal, and as a thunk `(...args) => [...]` when at
    // least one is an expression. `cache()` accepts both, resolving the
    // thunk per call — so a `cacheTag('posts:' + slug)` reaches the cache
    // entry with the value of `slug` for that call.
    return cache(fn, {
      ttl: opts.life?.ttl,
      swr: opts.life?.swr,
      tags: opts.tags as string[] | ((...args: unknown[]) => string[]) | undefined,
    })
  }
}

// ---------------------------------------------------------------------------
// server functions (§9.4)
// ---------------------------------------------------------------------------

type FnRegistry = Record<string, (...args: never[]) => unknown>

const globalRegistry = globalThis as typeof globalThis & {
  __oxide_fn_registry?: FnRegistry
}
const initialRegistry: FnRegistry = Object.create(null)
globalRegistry.__oxide_fn_registry = globalRegistry.__oxide_fn_registry ?? initialRegistry

/** Live id → fn registry (ids are computed by the Rust build, §9.4).
 *  Parameters are typed `never[]` (contravariant bottom) so any concrete
 *  function signature is assignable to it, matching how a heterogeneous
 *  registry actually behaves at runtime — the caller resolves the exact
 *  parameter shape per-id. */
export const __oxide_fn_registry: FnRegistry = globalRegistry.__oxide_fn_registry

/** Look up a registered server fn by its build id. */
export function getServerFn(id: string): ((...args: never[]) => unknown) | undefined {
  return __oxide_fn_registry[id]
}

/**
 * Install the compiler's registration hook as a global.
 *
 * The compiler's server-side transform appends
 * `__oxide_register_fn("<build-id>", <export-name>);` after each module's
 * body — the id is a build-time hash of file, export name and the
 * per-build secret, and the SSR module itself cannot compute it. Rather
 * than force every `.server.ts` file to import a helper under that name,
 * the runtime installs it on `globalThis` here. Because a `.server.ts`
 * module imports `serverFn` (or `defineLoad`, etc.) from this module, this
 * code runs before the appended call.
 *
 * The signature is `(id, fn)` and the effect is the same as
 * `serverFn(id, fn)` — the tagged function is registered under the
 * compiler's id, so `POST /_oxide/fn/<id>` finds it.
 */
declare global {
  // eslint-disable-next-line no-var
  var __oxide_register_fn:
    | ((id: string, fn: (...args: never[]) => unknown) => void)
    | undefined
}
if (typeof globalThis.__oxide_register_fn !== 'function') {
  globalThis.__oxide_register_fn = (id, fn) => {
    __oxide_fn_registry[id] = fn
  }
}

let anonymousFnCounter = 0

/**
 * Tag + register a server function (§9.4). In app code the compiler rewrites
 * `serverFn(fn)` to `serverFn('<stable id>', fn)`; the runtime registers the
 * fn so `POST /_oxide/fn/<id>` can dispatch to it. The returned fn remains
 * directly callable in-process (tests, SSR).
 */
export function serverFn<A extends unknown[], R>(
  fnOrId: ((...args: A) => Promise<R> | R) | string,
  maybeFn?: (...args: A) => Promise<R> | R,
): ((...args: A) => Promise<R> | R) & { $$oxideServerFn: true; id: string } {
  const isIdFirst = typeof fnOrId === 'string'
  const fn = (isIdFirst ? maybeFn : fnOrId) as (...args: A) => Promise<R> | R
  if (typeof fn !== 'function') {
    throw new TypeError('serverFn(fn) or serverFn(id, fn) — a function is required')
  }
  const id = isIdFirst
    ? (fnOrId as string)
    : ((fn as { name?: string }).name || `anonymous-${++anonymousFnCounter}`)
  const tagged = fn as ((...args: A) => Promise<R> | R) & { $$oxideServerFn: true; id: string }
  tagged.$$oxideServerFn = true
  tagged.id = id
  __oxide_fn_registry[id] = tagged
  return tagged
}

// ---------------------------------------------------------------------------
// revalidate collectors (§11.3)
// ---------------------------------------------------------------------------

export type RevalidateEvent = { type: 'tag'; tag: string } | { type: 'path'; path: string }

/** All revalidate calls recorded in order (inspector/tests). */
export const _revalidateLog: RevalidateEvent[] = []

/**
 * Collectors for cache invalidation (§11.3). In actions, prefer
 * `event.revalidate`; this standalone form records into `_revalidateLog` for
 * the inspector and tests.
 */
export const revalidate = {
  tag(tag: string): { type: 'tag'; tag: string } {
    const ev = { type: 'tag', tag } as const
    _revalidateLog.push(ev)
    return ev
  },
  path(path: string): { type: 'path'; path: string } {
    const ev = { type: 'path', path } as const
    _revalidateLog.push(ev)
    return ev
  },
}

/**
 * §16.1: build a `srcset` attribute string from a base image URL and a set
 * of widths. Pairs with the `/_oxide/image` endpoint or a pre-generated
 * `?w=` variant set. A leading `./` or `/` URL is passed through verbatim;
 * `signImageUrl` (below) appends a signature when the project uses signed
 * image URLs.
 *
 * @example
 *   <img
 *     src="/_oxide/image?src=/hero.png&w=1280"
 *     srcset={srcset([640, 1024, 1280, 1920], '/_oxide/image?src=/hero.png')}
 *     sizes="(min-width: 1280px) 1280px, 100vw"
 *     alt="Hero"
 *   />
 */
export function srcset(widths: ReadonlyArray<number>, baseUrl: string): string {
  const seen = new Set<number>()
  const sorted = [...widths].filter((w) => {
    if (!Number.isFinite(w) || w <= 0) return false
    if (seen.has(w)) return false
    seen.add(w)
    return true
  }).sort((a, b) => a - b)
  const sep = baseUrl.includes('?') ? '&' : '?'
  return sorted.map((w) => `${baseUrl}${sep}w=${w} ${w}w`).join(', ')
}
export * from './content.ts'

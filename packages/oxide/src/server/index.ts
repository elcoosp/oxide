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
  SocketHandlers,
  StandardSchema,
} from './json.ts'

export type {
  ActionEvent, CookieOptions, CookieStore, DeferredPlaceholder, DeferredRegistry,
  LoadEvent, LoadResult, ServerFnEvent, SocketHandlers, StandardSchema,
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
  /** Tags for grouped invalidation (§11.3). */
  tags?: string[]
}

/** Deterministic cache key: JSON of args + normalized opts (tags sorted). */
export function cacheKey(args: unknown[], opts?: CacheOptions): string {
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

const cacheStore = new Map<string, CacheEntry>()
const cachePending = new Map<string, Promise<unknown>>()

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
  const wrapped = async (...args: A): Promise<R> => {
    const key = cacheKey(args, opts)
    const now = Date.now()
    const hit = cacheStore.get(key)
    if (hit) {
      if (now < hit.expiresAt) return hit.value as R
      if (swrMs > 0 && now < hit.staleUntil) {
        void refresh(key, args) // background revalidate, serve stale now
        return hit.value as R
      }
      cacheStore.delete(key)
    }
    const inflight = cachePending.get(key)
    if (inflight) return (await inflight) as R
    return (await runAndStore(key, args)) as R
  }
  const runAndStore = async (key: string, args: A): Promise<R> => {
    const p = (async () => {
      try {
        const value = await fn(...args)
        cacheStore.set(key, {
          value,
          expiresAt: Date.now() + ttlMs,
          staleUntil: Date.now() + ttlMs + swrMs,
          tags: [...(opts.tags ?? [])],
        })
        return value
      } finally {
        cachePending.delete(key)
      }
    })()
    cachePending.set(key, p)
    return (await p) as R
  }
  const refresh = (key: string, args: A): Promise<unknown> => {
    if (cachePending.has(key)) return cachePending.get(key)!
    return runAndStore(key, args)
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
    return cache(fn, {
      ttl: opts.life?.ttl,
      swr: opts.life?.swr,
      get tags() {
        // The `cache()` API reads `opts.tags` once, at wrap time; a
        // per-call tag is a future enhancement (the Rust tier keys
        // entries by content hash, not by a fixed list). For now the
        // static portion is what registers.
        const t = opts.tags
        return Array.isArray(t) ? (t as string[]) : []
      },
    })
    // Note: `tagsOf` is unused today because `cache()` reads a fixed tag
    // list; it becomes the source for the per-call tag registration when
    // the Rust L1 tier is wired to the request path.
  }
}

// ---------------------------------------------------------------------------
// server functions (§9.4)
// ---------------------------------------------------------------------------

const globalRegistry = globalThis as typeof globalThis & {
  __oxide_fn_registry?: Record<string, (...args: unknown[]) => unknown>
}
globalRegistry.__oxide_fn_registry ??= Object.create(null)

/** Live id → fn registry (ids are computed by the Rust build, §9.4). */
export const __oxide_fn_registry: Record<string, (...args: unknown[]) => unknown> =
  globalRegistry.__oxide_fn_registry

/** Look up a registered server fn by its build id. */
export function getServerFn(id: string): ((...args: unknown[]) => unknown) | undefined {
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
    | ((id: string, fn: (...args: unknown[]) => unknown) => void)
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
  __oxide_fn_registry[id] = tagged as (...args: unknown[]) => unknown
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

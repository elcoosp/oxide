import { BOUNDARY_CONTENT_ATTR, BOUNDARY_FALLBACK_ATTR, BoundaryRegistration, beginBoundaryCapture, boundaryContent, boundaryFallback, deferredIdOf, drainBoundaries, endBoundaryCapture, registerBoundary } from "./server-boundary.js";
/** Cookie store wrapper (Web API cookies, §9.2). */
interface CookieStore {
  get(name: string): string | undefined;
  set(name: string, value: string, opts?: CookieOptions): void;
  /** Optional delete convenience: set with maxAge 0. */
  delete?(name: string): void;
}
interface CookieOptions {
  domain?: string;
  path?: string;
  maxAge?: number;
  httpOnly?: boolean;
  secure?: boolean;
  sameSite?: 'lax' | 'strict' | 'none';
}
/** The serialized shape on the wire — mirrors ODF `Value::Deferred` (§9.5). */
interface DeferredPlaceholder {
  __deferred: number;
}
interface LoadEvent {
  /** Route params from the matcher (§8.2). */
  params: Record<string, string | string[]>;
  request: Request;
  /** Request-scoped state populated by middleware (§9.2). */
  locals: Record<string, unknown>;
  cookies: CookieStore;
  /**
   * Register a promise for streaming. Returns a placeholder that serializes
   * as `Value::Deferred { id }` (JSON: `{__deferred: id}`); the resolved
   * value streams later via the boundary protocol (§7.1).
   */
  defer(promise: Promise<unknown>): DeferredPlaceholder;
  /** Add headers to the eventual response before it is committed. */
  setHeaders(headers: Record<string, string>): void;
  /** Layout-chain parent data; awaits explicitly (§9.2). */
  parent(): Promise<Record<string, unknown>>;
}
/** Internal: collects deferred promises per request, id → promise. */
type DeferredRegistry = Map<number, Promise<unknown>>;
interface ActionEvent {
  params: Record<string, string | string[]>;
  request: Request;
  locals: Record<string, unknown>;
  cookies: CookieStore;
  /** Validated input (Standard Schema ran before run()). */
  input: unknown;
  /** Form data for progressive enhancement (no-JS posts). */
  form: FormData;
  /** Collect tag/path invalidations (§11.3). */
  revalidate: {
    tag(tag: string): void;
    path(path: string): void;
  };
}
/** Server function event (§9.4): POST /_oxide/fn/<id>. */
interface ServerFnEvent {
  request: Request;
  locals: Record<string, unknown>;
  input: unknown;
}
/**
 * The context every socket phase receives (§9.6). `send` targets the
 * caller; `broadcast` every other socket on the same `socket.ts` route;
 * `sendTo` a specific id from `clients`; `clients` is the live presence
 * list for this route (including the caller).
 */
interface SocketContext {
  headers: Record<string, string>;
  locals: Record<string, unknown>;
  clients: string[];
  send(data: string | Uint8Array): void;
  broadcast(data: string | Uint8Array): void;
  sendTo(socketId: string, data: string | Uint8Array): void;
}
/** WebSocket route handlers (§9.6). */
interface SocketHandlers {
  open(ctx: SocketContext): void | Promise<void>;
  message(ctx: SocketContext & {
    data: string | Uint8Array;
  }): void | Promise<void>;
  close(ctx: SocketContext & {
    code: number;
    reason: string;
  }): void | Promise<void>;
}
/** A Standard Schema validator (structural; any conforming lib). */
interface StandardSchema<I = unknown, O = I> {
  '~standard': {
    validate(value: unknown): Promise<{
      value?: O;
      issues?: Array<{
        message: string;
      }>;
    }> | {
      value?: O;
      issues?: Array<{
        message: string;
      }>;
    };
  };
}
/** Result shape produced by a load wrapper for the render pipeline. */
interface LoadResult {
  /** Data with deferred markers replaced by `Value::Deferred` placeholders. */
  data: Record<string, unknown>;
  /** Streaming promises, id → promise (consumed by the boundary protocol). */
  deferred: DeferredRegistry;
}
/** One entry as emitted by the build. `frontmatter` is the raw parsed
 * map; `body` is the Markdown/MDX source (unrendered — a caller that
 * wants HTML calls `renderMarkdown` from `oxide/server`). */
export interface ContentEntry {
  slug: string;
  frontmatter: Record<string, unknown>;
  body: string;
  /** `true` when the source is `.mdx` — the body needs MDX compilation
   * to render. `renderMarkdown` handles both; a caller doing its own
   * rendering should branch on this. */
  is_mdx?: boolean;
}
/**
 * Read every entry in a collection. Throws when the collection was not
 * built (a typo in the name, or a project that has never run `oxide
 * build`) — the error message names the expected file so a fix is one
 * step away.
 */
export declare function getCollection(name: string): Promise<ContentEntry[]>;
/**
 * Read one entry by slug. Returns `undefined` when the slug is absent
 * from the collection (a miss in a loader is often a 404, not an error).
 * Throws only when the *collection* itself is missing — the same
 * contract as `getCollection`.
 */
export declare function getEntry(collection: string, slug: string): Promise<ContentEntry | undefined>;
/**
 * Test-only: clear the mtime cache. The public surface is
 * `getCollection` / `getEntry`; a test that reads the same file after
 * rewriting it needs a fresh stat.
 */
export declare function __clearContentCache(): void;
/** Tag key on load fns (`fn.$$oxide === 'load'`). */
export declare const OXIDE_TAG = "$$oxide";
/**
 * Tag a load function. Calling the returned fn runs the load with the event's
 * `defer` wired to a per-request id registry: promises registered via
 * `event.defer(p)` get `{__deferred: id}` placeholders immediately, and any
 * standalone `defer(p)` markers found in the returned data are converted on
 * the way out (Value::Deferred-compatible shape, §9.5). The Rust render
 * pipeline consumes the registry via `event.__oxideDeferredRegistry`.
 */
export declare function defineLoad<D extends Record<string, unknown>>(fn: (event: LoadEvent) => Promise<D> | D): ((event: LoadEvent) => Promise<D>) & {
  $$oxide: 'load';
};
export declare function defer(promise: Promise<unknown>): {
  __oxide_defer: true;
  promise: Promise<unknown>;
};
/** Thrown by [error]; carries the HTTP status for the response pipeline. */
export declare class OxideHttpError extends Error {
  status: number;
  constructor(status: number, message?: string);
}
/** Thrown by [redirect]; the pipeline converts it to a redirect response. */
export declare class OxideRedirect extends Error {
  location: string;
  status: number;
  constructor(location: string, status?: number);
}
/** Abort a load with an HTTP error (§9.2). Throws [OxideHttpError]. */
export declare function error(status: number, message?: string): never;
/** Redirect from a load or action. Default 303 (POST→GET safe). Throws. */
export declare function redirect(location: string, status?: number): never;
export interface ActionDefinition<I = unknown> {
  /** Optional Standard Schema validator; runs before `run` (§9.3). */
  input?: StandardSchema<I>;
  run(ctx: ActionEvent): Promise<unknown> | unknown;
}
/** Tag an action definition (form-first, §9.3). */
export declare function defineAction<I>(def: {
  input?: StandardSchema<I>;
  run(ctx: ActionEvent & {
    input: I;
  }): Promise<unknown> | unknown;
}): {
  $$oxide: 'action';
  input?: StandardSchema<I>;
  run(ctx: ActionEvent & {
    input: I;
  }): Promise<unknown> | unknown;
};
/** Tag a meta function: load data → neutral head model (§16.3). */
export declare function defineMeta<M extends (ctx: {
  data: unknown;
  params: Record<string, string | string[]>;
}) => Record<string, unknown>>(fn: M): M & {
  $$oxide: 'meta';
};
/** Tag a WebSocket route's handlers (§9.6 — Rust owns framing/backpressure). */
export declare function defineSocket(handlers: SocketHandlers): SocketHandlers & {
  $$oxide: 'socket';
};
export interface CacheOptions {
  /** Fresh lifetime in seconds. */
  ttl?: number;
  /** Stale-while-revalidate window in seconds (served stale + refreshed). */
  swr?: number;
  /**
   * Tags for grouped invalidation (§11.3).
   *
   * A plain array is a fixed set of tags. A function receives the wrapped
   * call's arguments and returns the tags for that call — the `'use cache'`
   * compiler pass emits this form when a `cacheTag(…)` argument is not a
   * literal (e.g. `cacheTag('posts:' + slug)`), so the tag reflects the
   * argument rather than being dropped.
   */
  tags?: string[] | ((...args: unknown[]) => string[]);
}
/** Deterministic cache key: JSON of args + normalized opts (tags sorted). */
/** Deterministic cache key: JSON of args + normalized opts (tags sorted). */
export declare function cacheKey(args: unknown[], opts?: {
  ttl?: number;
  swr?: number;
  tags?: string[];
}): string;
/**
 * §9.3: the worker's host-op bridge, installed by `oxide-worker.mjs`. In a
 * Node worker it sends an `op_request` to Rust; in a browser it is absent
 * (there is no Rust host), so every call here falls back to the in-process
 * Map. The declaration makes the call site type-safe without importing the
 * worker.
 */
declare global {
  var __oxide_op_request: ((op: string, args: unknown) => Promise<unknown>) | undefined;
}
/** Drop cached entries carrying any of the given tags (soft→delete here; the
 * Rust TieredCache implements the stale-serve semantics, §11.3). */
export declare function invalidateTag(...tags: string[]): number;
/**
 * The explicit form of the `'use cache'` directive (§9.3): wraps an async fn
 * with an in-memory entry keyed by `cacheKey(args, opts)`. Concurrent misses
 * for the same key single-flight; `swr` serves stale then refreshes in the
 * background.
 */
export declare function cache<A extends unknown[], R>(fn: (...args: A) => Promise<R>, opts?: CacheOptions): ((...args: A) => Promise<R>) & {
  $$oxideCache: true;
  opts: CacheOptions;
};
/**
 * Inside a `'use cache'` fn: attach tags to the cache entry (§9.3). The
 * compiler pass threads these to the cache wrapper; standalone it returns a
 * documented marker.
 */
export declare function cacheTag(...tags: string[]): {
  __oxideCacheTag: string[];
};
/** Inside a `'use cache'` fn: override ttl/swr for this entry (§9.3). */
export declare function cacheLife(life: {
  ttl?: number;
  swr?: number;
  staleIfError?: number;
}): {
  __oxideCacheLife: typeof life;
};
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
  var __oxide_cache: (<A extends unknown[], R>(fn: (...args: A) => Promise<R>, module: string, exportName: string, opts: {
    tags?: unknown;
    life?: {
      ttl?: number;
      swr?: number;
      staleIfError?: number;
    };
  }) => (...args: A) => Promise<R>) | undefined;
}
type FnRegistry = Record<string, (...args: never[]) => unknown>;
/** Live id → fn registry (ids are computed by the Rust build, §9.4).
 *  Parameters are typed `never[]` (contravariant bottom) so any concrete
 *  function signature is assignable to it, matching how a heterogeneous
 *  registry actually behaves at runtime — the caller resolves the exact
 *  parameter shape per-id. */
export declare const __oxide_fn_registry: FnRegistry;
/** Look up a registered server fn by its build id. */
export declare function getServerFn(id: string): ((...args: never[]) => unknown) | undefined;
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
  var __oxide_register_fn: ((id: string, fn: (...args: never[]) => unknown) => void) | undefined;
}
/**
 * Tag + register a server function (§9.4). In app code the compiler rewrites
 * `serverFn(fn)` to `serverFn('<stable id>', fn)`; the runtime registers the
 * fn so `POST /_oxide/fn/<id>` can dispatch to it. The returned fn remains
 * directly callable in-process (tests, SSR).
 */
export declare function serverFn<A extends unknown[], R>(fnOrId: ((...args: A) => Promise<R> | R) | string, maybeFn?: (...args: A) => Promise<R> | R): ((...args: A) => Promise<R> | R) & {
  $$oxideServerFn: true;
  id: string;
};
export type RevalidateEvent = {
  type: 'tag';
  tag: string;
} | {
  type: 'path';
  path: string;
};
/** All revalidate calls recorded in order (inspector/tests). */
export declare const _revalidateLog: RevalidateEvent[];
/**
 * Collectors for cache invalidation (§11.3). In actions, prefer
 * `event.revalidate`; this standalone form records into `_revalidateLog` for
 * the inspector and tests.
 */
export declare const revalidate: {
  tag(tag: string): {
    type: 'tag';
    tag: string;
  };
  path(path: string): {
    type: 'path';
    path: string;
  };
};
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
export declare function srcset(widths: ReadonlyArray<number>, baseUrl: string): string;
export { type ActionEvent, BOUNDARY_CONTENT_ATTR, BOUNDARY_FALLBACK_ATTR, type BoundaryRegistration, type CookieOptions, type CookieStore, type DeferredPlaceholder, type DeferredRegistry, type LoadEvent, type LoadResult, type ServerFnEvent, type SocketContext, type SocketHandlers, type StandardSchema, beginBoundaryCapture, boundaryContent, boundaryFallback, deferredIdOf, drainBoundaries, endBoundaryCapture, registerBoundary };
//# sourceMappingURL=server.d.ts.map
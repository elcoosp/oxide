// Event types for the framework-neutral data layer (spec §9.1-§9.2).
// `page.server.ts` / `layout.server.ts` modules receive these per request.
// `Request` is the standard global (Web API); Node 24 provides it natively.

export {}

/** Cookie store wrapper (Web API cookies, §9.2). */
export interface CookieStore {
  get(name: string): string | undefined
  set(name: string, value: string, opts?: CookieOptions): void
  /** Optional delete convenience: set with maxAge 0. */
  delete?(name: string): void
}

export interface CookieOptions {
  domain?: string
  path?: string
  maxAge?: number
  httpOnly?: boolean
  secure?: boolean
  sameSite?: 'lax' | 'strict' | 'none'
}

/** A promise registered for the deferred-boundary streaming protocol (§7.1). */
export interface DeferMarker {
  /** String key (Symbol-safe): the marker flag. */
  __oxide_defer: true
  promise: Promise<unknown>
}

/** The serialized shape on the wire — mirrors ODF `Value::Deferred` (§9.5). */
export interface DeferredPlaceholder {
  __deferred: number
}

export interface LoadEvent {
  /** Route params from the matcher (§8.2). */
  params: Record<string, string | string[]>
  request: Request
  /** Request-scoped state populated by middleware (§9.2). */
  locals: Record<string, unknown>
  cookies: CookieStore
  /**
   * Register a promise for streaming. Returns a placeholder that serializes
   * as `Value::Deferred { id }` (JSON: `{__deferred: id}`); the resolved
   * value streams later via the boundary protocol (§7.1).
   */
  defer(promise: Promise<unknown>): DeferredPlaceholder
  /** Add headers to the eventual response before it is committed. */
  setHeaders(headers: Record<string, string>): void
  /** Layout-chain parent data; awaits explicitly (§9.2). */
  parent(): Promise<Record<string, unknown>>
}

/** Internal: collects deferred promises per request, id → promise. */
export type DeferredRegistry = Map<number, Promise<unknown>>

export interface ActionEvent {
  params: Record<string, string | string[]>
  request: Request
  locals: Record<string, unknown>
  cookies: CookieStore
  /** Validated input (Standard Schema ran before run()). */
  input: unknown
  /** Form data for progressive enhancement (no-JS posts). */
  form: FormData
  /** Collect tag/path invalidations (§11.3). */
  revalidate: {
    tag(tag: string): void
    path(path: string): void
  }
}

/** Server function event (§9.4): POST /_oxide/fn/<id>. */
export interface ServerFnEvent {
  request: Request
  locals: Record<string, unknown>
  input: unknown
}

/** WebSocket route handlers (§9.6). */
export interface SocketHandlers {
  open(ctx: { send(data: string | Uint8Array): void }): void | Promise<void>
  message(ctx: { data: string | Uint8Array; send(data: string | Uint8Array): void }): void | Promise<void>
  close(ctx: { code: number; reason: string }): void | Promise<void>
}

/** A Standard Schema validator (structural; any conforming lib). */
export interface StandardSchema<I = unknown, O = I> {
  '~standard': {
    validate(value: unknown): Promise<{ value?: O; issues?: Array<{ message: string }> }> | { value?: O; issues?: Array<{ message: string }> }
  }
}

/** Result shape produced by a load wrapper for the render pipeline. */
export interface LoadResult {
  /** Data with deferred markers replaced by `Value::Deferred` placeholders. */
  data: Record<string, unknown>
  /** Streaming promises, id → promise (consumed by the boundary protocol). */
  deferred: DeferredRegistry
}

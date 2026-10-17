// Client glue for the core deferred-boundary protocol (spec §7.1, §9.2).
//
// The Rust render crate streams `<template data-ox-c='ID'>…</template>` plus a
// ~300-byte inline swapper that moves the content into the matching
// `<template data-ox-b='ID'>` placeholder and dispatches a document-level
// CustomEvent `ox:swap` (detail: {id}) — see render::SWAPPER_SRC.
//
// This module: (1) rewrites `{__deferred: id}` ODF placeholders (§9.5) in
// client data into promise-likes the adapter idiom can await (React `use()`,
// Vue ref, Svelte await block, Solid resource); (2) resolves those
// promise-likes when the matching `ox:swap` event arrives (detail.id, plus an
// optional value/error extension).

export interface SwapDetail {
  id: number
  /**
   * The **streamed HTML content** of the boundary — the `innerHTML` of the
   * `<template data-ox-c='<id>'>` the server emitted. `render::SWAPPER_SRC`
   * sends this as `detail.value` on every `ox:swap` (before this was
   * `undefined`, so any `DeferredClient` awaiting a loader data slot resolved
   * to nothing). This is HTML, not the raw data payload: a `defer()`ed value
   * in the loader data stream is currently delivered as rendered markup, not
   * as the structured `Value` §9.2 calls for. Carrying the ODF-encoded data
   * alongside (so `DeferredClient` resolves to the decoded value) is a
   * remaining gap — see WORKLOG.
   */
  value?: string
  error?: unknown
}

/** A promise-like the client adapter can await; resolves on ox:swap. */
export class DeferredClient<T = unknown> {
  status: 'pending' | 'resolved' | 'rejected' = 'pending'
  value?: T
  error?: unknown
  readonly promise: Promise<T>
  #resolve!: (v: T) => void
  #reject!: (e: unknown) => void

  constructor() {
    this.promise = new Promise<T>((resolve, reject) => {
      this.#resolve = resolve
      this.#reject = reject
    })
    // avoid unhandled-rejection noise for slots nobody awaits
    this.promise.catch(() => {})
  }

  resolve(value: T): void {
    if (this.status !== 'pending') return
    this.status = 'resolved'
    this.value = value
    this.#resolve(value)
  }

  reject(error: unknown): void {
    if (this.status !== 'pending') return
    this.status = 'rejected'
    this.error = error
    this.#reject(error instanceof Error ? error : new Error(`deferred slot rejected: ${String(error)}`))
  }

  then = <R1 = T, R2 = never>(
    onOk?: ((v: T) => R1 | PromiseLike<R1>) | null,
    onErr?: ((e: unknown) => R2 | PromiseLike<R2>) | null,
  ): Promise<R1 | R2> => this.promise.then(onOk, onErr)

  catch = <R = never>(onErr?: ((e: unknown) => R | PromiseLike<R>) | null): Promise<T | R> =>
    this.promise.catch(onErr)
}

export interface DeferGlue {
  /** Rewrite `{__deferred: id}` placeholders in place → DeferredClient. */
  wire(data: unknown): void
  /** Resolve the slot with a value (e.g. an ODF payload from the stream). */
  resolve(id: number, value: unknown): void
  reject(id: number, error: unknown): void
  readonly registry: Map<number, DeferredClient>
  dispose(): void
}

export interface DeferGlueOptions {
  doc?: Document
  /** Called on every ox:swap (adapter hydration of swapped content, §7.1). */
  onSwap?(detail: SwapDetail): void
  /** Custom event name (default 'ox:swap'). */
  eventName?: string
}

/**
 * Listen for `ox:swap` events on the document and resolve registered
 * deferred slots. `dispose()` removes the listener; later swaps no longer
 * resolve (slots wired after dispose reject as orphaned).
 */
export function attachDeferGlue(options: DeferGlueOptions = {}): DeferGlue {
  const doc = options.doc ?? (globalThis as { document?: Document }).document
  const eventName = options.eventName ?? 'ox:swap'
  const registry = new Map<number, DeferredClient>()
  let dead = false
  if (!doc) throw new TypeError('attachDeferGlue: no document (pass one in non-browser tests)')

  const onSwap = (e: Event): void => {
    const detail = (e as CustomEvent<SwapDetail>).detail
    if (!detail || typeof detail.id !== 'number') return
    options.onSwap?.(detail)
    const slot = registry.get(detail.id)
    if (!slot) return
    if ('error' in detail && detail.error !== undefined && detail.error !== null) {
      slot.reject(detail.error)
    } else {
      slot.resolve(detail.value)
    }
  }
  doc.addEventListener(eventName, onSwap as EventListener)

  return {
    registry,
    resolve(id: number, value: unknown) {
      registry.get(id)?.resolve(value)
    },
    reject(id: number, error: unknown) {
      registry.get(id)?.reject(error)
    },
    wire(data: unknown): void {
      wireDeferreds(data, { registry, dead })
    },
    dispose() {
      dead = true
      doc.removeEventListener(eventName, onSwap as EventListener)
      // orphan any still-pending slots so their promises settle
      for (const [id, slot] of registry) {
        if (slot.status === 'pending') {
          slot.reject(new Error(`deferred slot ${id}: glue disposed before swap`))
        }
      }
    },
  }
}

export interface WireOptions {
  registry?: Map<number, DeferredClient>
  /** Dead glue: newly wired slots reject immediately instead of hanging. */
  dead?: boolean
}

const isPlaceholder = (v: unknown): v is { __deferred: number } =>
  typeof v === 'object' && v !== null &&
  typeof (v as { __deferred?: unknown }).__deferred === 'number' &&
  Object.keys(v as object).length === 1

/**
 * Walk `data` (which came from the ODF decoder) and replace every
 * `{__deferred: id}` placeholder with a [DeferredClient] registered under its
 * id. Mutates containers in place; cycle-safe; depth-capped like the decoder.
 */
export function wireDeferreds(data: unknown, options: WireOptions = {}): unknown {
  const registry = options.registry ?? new Map<number, DeferredClient>()
  walk(data, registry, 1, new WeakSet<object>())
  return data
}

function walk(value: unknown, registry: Map<number, DeferredClient>, depth: number, seen: WeakSet<object>, dead = false): void {
  if (depth > 512 || value === null || typeof value !== 'object') return
  if (isPlaceholder(value)) return // handled by the parent container
  if (seen.has(value)) return
  seen.add(value)

  if (value instanceof Map) {
    for (const [k, v] of value) {
      if (isPlaceholder(v)) value.set(k, slotFor(v.__deferred, registry, dead))
      else walk(v, registry, depth + 1, seen, dead)
    }
    return
  }
  if (value instanceof Set) {
    const items = [...value]
    value.clear()
    for (const item of items) {
      value.add(isPlaceholder(item) ? slotFor(item.__deferred, registry, dead) : (walk(item, registry, depth + 1, seen, dead), item))
    }
    return
  }
  const obj = value as Record<string, unknown>
  for (const key of Object.keys(obj)) {
    const v = obj[key]
    if (isPlaceholder(v)) obj[key] = slotFor(v.__deferred, registry, dead)
    else walk(v, registry, depth + 1, seen, dead)
  }
}

function slotFor(id: number, registry: Map<number, DeferredClient>, dead = false): DeferredClient {
  let slot = registry.get(id)
  if (!slot) {
    slot = new DeferredClient()
    if (dead) slot.reject(new Error(`deferred slot ${id}: glue disposed before wire`))
    registry.set(id, slot)
  }
  return slot
}

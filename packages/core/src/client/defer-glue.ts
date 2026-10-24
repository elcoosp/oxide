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
  // Internally we resolve/reject with `unknown` so this class is COVARIANT
  // in T -- a `DeferredClient<X>` is assignable to `DeferredClient<unknown>`
  // without a cast (enables the heterogeneous registry Map<number,
  // DeferredClient>). The public `resolve(value: T)` still gives callers the
  // type-safe view; `promise: Promise<T>` is the typed consumer surface.
  #resolveUnknown!: (v: unknown) => void
  #rejectUnknown!: (e: unknown) => void

  constructor() {
    let resolveUnknown!: (v: unknown) => void
    let rejectUnknown!: (e: unknown) => void
    const p = new Promise<unknown>((res, rej) => {
      resolveUnknown = res
      rejectUnknown = rej
    })
    this.promise = p as Promise<T>
    this.#resolveUnknown = resolveUnknown
    this.#rejectUnknown = rejectUnknown
    // avoid unhandled-rejection noise for slots nobody awaits
    this.promise.catch(() => {})
  }

  resolve(value: T): void {
    if (this.status !== 'pending') return
    this.status = 'resolved'
    this.value = value
    this.#resolveUnknown(value)
  }

  reject(error: unknown): void {
    if (this.status !== 'pending') return
    this.status = 'rejected'
    this.error = error
    this.#rejectUnknown(error instanceof Error ? error : new Error(`deferred slot rejected: ${String(error)}`))
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
  /**
   * §7.1 / §9.2: resolve a batch of already-resolved values (the `/__data`
   * envelope's `deferred` map). A client navigation carries the loader's
   * resolved `defer()` output; this settles the matching slots so a
   * component awaiting one gets the value instead of hanging. Unknown ids
   * are ignored (a slot may not be wired yet).
   */
  resolveAll(values: Record<string, unknown>): void
  reject(id: number, error: unknown): void
  readonly registry: Map<number, DeferredClient>
  dispose(): void
}

/** Structural interface for a "script-like" element the glue reads. The
 *  glue only ever uses `.getAttribute` and `.textContent`, so a real
 *  `HTMLScriptElement` structurally satisfies this — no cast needed. */
export interface DeferGlueElement {
  getAttribute?(name: string): string | null
  textContent?: string | null
}

/** Structural interface for the "document-like" object the glue attaches to.
 *  Real `Document` satisfies this. A test fake document also satisfies it
 *  (the previously-required `Document` type forced every caller to cast). */
export interface DeferGlueDoc {
  addEventListener(type: string, fn: (ev: unknown) => void): void
  removeEventListener(type: string, fn: (ev: unknown) => void): void
  querySelectorAll?(sel: string): ArrayLike<DeferGlueElement>
  documentElement?: unknown
  readyState?: string
}

/** The `ox:swap` custom-event detail. Narrow with `isSwapEvent`. */
interface SwapEventDetail extends SwapDetail {
  error?: unknown
}

function isSwapEvent(e: unknown): e is { detail: SwapEventDetail } {
  if (typeof e !== 'object' || e === null) return false
  if (!('detail' in e)) return false
  const d = (e as { detail: unknown }).detail
  if (typeof d !== 'object' || d === null) return false
  return 'id' in d && typeof (d as { id: unknown }).id === 'number'
}

export interface DeferGlueOptions {
  doc?: DeferGlueDoc
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
/** Resolve (or reject) one slot from a settle value. A rejected defer is
 *  serialized as `{ __oxide_deferred_error: "..." }` and must reject the
 *  slot rather than resolve it to a shape the page did not expect. */
function resolveSlot(
  registry: Map<number, DeferredClient>,
  id: number,
  value: unknown,
): void {
  const slot = registry.get(id)
  if (!slot) return
  if (
    value &&
    typeof value === 'object' &&
    '__oxide_deferred_error' in (value as Record<string, unknown>)
  ) {
    const err = (value as Record<string, unknown>).__oxide_deferred_error
    slot.reject(new Error(String(err)))
  } else {
    slot.resolve(value)
  }
}

export function attachDeferGlue(options: DeferGlueOptions = {}): DeferGlue {
  const doc: DeferGlueDoc | undefined = options.doc ?? (globalThis as { document?: DeferGlueDoc }).document
  const eventName = options.eventName ?? 'ox:swap'
  const registry = new Map<number, DeferredClient>()
  let dead = false
  if (!doc) throw new TypeError('attachDeferGlue: no document (pass one in non-browser tests)')

  const onSwap = (e: unknown): void => {
    if (!isSwapEvent(e)) return
    const detail = e.detail
    options.onSwap?.(detail)
    const slot = registry.get(detail.id)
    if (!slot) return
    if (detail.error !== undefined && detail.error !== null) {
      slot.reject(detail.error)
    } else {
      slot.resolve(detail.value)
    }
  }
  doc.addEventListener(eventName, onSwap)

  // §7.1 / §9.2: the server injects a `<script type="application/oxide-deferred">`
  // with the resolved `defer()` values as JSON `{ "<id>": value, ... }`. Parse
  // it now (or on DOMContentLoaded if the script is below the glue's attach
  // point) and resolve every matching slot. A rejected defer is serialized
  // as `{ "__oxide_deferred_error": "..." }` — recognized here and rejects
  // the slot instead of resolving it to a shape the page did not expect.
  const resolveFromScripts = (): void => {
    if (typeof doc.querySelectorAll !== 'function') return
    // Method call — `this` is `doc`. No detach dance.
    const scripts = doc.querySelectorAll('script[type="application/oxide-deferred"]')
    for (let i = 0; i < scripts.length; i++) {
      const script = scripts[i]
      if (script === undefined) continue
      const raw = script.textContent ?? ''
      if (raw.length === 0) continue
      let parsed: Record<string, unknown>
      try {
        parsed = JSON.parse(raw) as Record<string, unknown>
      } catch {
        // A malformed deferred payload must not break hydration; skip it.
        continue
      }
      // Two shapes carry a settle:
      //   * batched (pre-B2): the script body is `{ "<id>": value, ... }`;
      //   * per-settle (B2): `data-id="<id>"` and the body is the value.
      // A per-settle script arrives as its own element, possibly *after*
      // DOMContentLoaded — the observer below picks those up.
      const dataId = script.getAttribute?.('data-id')
      if (dataId) {
        const id = Number(dataId)
        if (Number.isFinite(id)) {
          resolveSlot(registry, id, parsed)
        }
        continue
      }
      for (const [idStr, value] of Object.entries(parsed)) {
        const id = Number(idStr)
        if (!Number.isFinite(id)) continue
        resolveSlot(registry, id, value)
      }
    }
  }

  // A streamed settle appends its script after the document has loaded, so
  // `resolveFromScripts` has already run. Watch for the insertions and
  // resolve each new one. The observer disconnects in `dispose`.
  let observer: { disconnect(): void; observe(target: unknown, options?: { childList?: boolean; subtree?: boolean }): void } | null = null
  const MO = (globalThis as { MutationObserver?: new (cb: (m: unknown[]) => void) => {
    observe(t: unknown, o: unknown): void
    disconnect(): void
  } }).MutationObserver
  if (typeof MO === 'function' && doc.documentElement !== undefined) {
    observer = new MO((mutations: unknown[]) => {
      for (const m of mutations) {
        const added = (m as { addedNodes?: ArrayLike<unknown> }).addedNodes
        if (!added) continue
        for (let i = 0; i < added.length; i++) {
          const node = added[i] as { tagName?: string; getAttribute?: (n: string) => string | null }
          if (
            node.tagName === 'SCRIPT' &&
            node.getAttribute?.('type') === 'application/oxide-deferred'
          ) {
            resolveFromScripts()
            return
          }
        }
      }
    })
    observer.observe(doc.documentElement, {
      childList: true,
      subtree: true,
    })
  }
  // Try synchronously, then again on DOMContentLoaded in case the script
  // element had not yet been parsed when the glue attached (the glue attaches
  // during the boot module's execution, which the browser runs after the HTML
  // but a `<script>` below the boot script loads later).
  resolveFromScripts()
  const onReady = (): void => resolveFromScripts()
  if (doc.readyState === 'loading') {
    doc.addEventListener('DOMContentLoaded', onReady)
  }

  return {
    registry,
    resolve(id: number, value: unknown) {
      registry.get(id)?.resolve(value)
    },
    resolveAll(values: Record<string, unknown>) {
      for (const [idStr, value] of Object.entries(values)) {
        const id = Number(idStr)
        if (!Number.isFinite(id)) continue
        registry.get(id)?.resolve(value)
      }
    },
    reject(id: number, error: unknown) {
      registry.get(id)?.reject(error)
    },
    wire(data: unknown): void {
      wireDeferreds(data, { registry, dead })
      // §7.1 / §9.2: the server's deferred script may have been parsed
      // *before* this `wire()` populated the registry. Re-run the script
      // resolution so freshly-wired slots that match a script's ids resolve
      // immediately. Idempotent — `DeferredClient.resolve` ignores a
      // non-pending slot.
      resolveFromScripts()
    },
    dispose() {
      observer?.disconnect()
      observer = null
      dead = true
      doc.removeEventListener(eventName, onSwap)
      doc.removeEventListener('DOMContentLoaded', onReady)
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

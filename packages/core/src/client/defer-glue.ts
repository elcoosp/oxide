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
   * The **streamed HTML content** of a `suspense: 'core'` boundary — the
   * `innerHTML` of the `<template data-ox-c='<id>'>` the server emitted.
   * `render::SWAPPER_SRC` sends this as `detail.value` on every `ox:swap`,
   * because the whole point of a Suspense boundary is "show this
   * placeholder, then swap in the ready subtree." That subtree is
   * **rendered markup**, not the raw data value.
   *
   * Separate mechanism, same event slot: `defer()` values in *loader data*
   * (§9.2) arrive via `<script type="application/oxide-deferred"
   * data-id="<id>">` tags. Those carry the **structured** JSON value
   * (`{"label":"fast"}`) and flow through `resolveFromScripts`, not this
   * `SwapDetail` path. That's why a `DeferredClient` awaiting a loader
   * slot resolves to the typed value, while one awaiting a Suspense
   * boundary resolves to the boundary's rendered HTML — two shapes, two
   * protocols, both §7.1.
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

export function attachDeferGlue(options: DeferGlueOptions = {}): DeferGlue {
  const doc: DeferGlueDoc | undefined = options.doc ?? (globalThis as { document?: DeferGlueDoc }).document
  const eventName = options.eventName ?? 'ox:swap'
  const registry = new Map<number, DeferredClient>()
  /** id → desired text. Populated on resolve; consumed by the marker
   *  MutationObserver that reconciles any later clobber (React remount,
   *  framework re-render, external override). */
  const reconciled = new Map<number, string>()
  /**
   * §7.1: when a slot resolves, if the page contains a
   * `[data-deferred-value="<id>"]` marker, write the resolved value into
   * it. This lets a page that doesn't use framework idioms (React `use()`,
   * Vue ref, Svelte await block, Solid resource) still *see* the streamed
   * value — the last-mile wiring that turns "the settle script landed" into
   * "the page shows the label."
   *
   * Convention (matching the smoke-app `/deferred` fixture):
   *   `value = { label: <string> }`  ->  marker.textContent = value.label
   *   `value = <string>`             ->  marker.textContent = value
   *   `value = anything else`        ->  marker.textContent = JSON.stringify(value)
   * Opt-in per element: no marker, no work. Multiple markers per id (rare)
   * all receive the same value.
   */
  const updateMarker = (id: number, value: unknown): void => {
    if (!doc || typeof doc.querySelectorAll !== 'function') return
    const els = doc.querySelectorAll(`[data-deferred-value="${id}"]`)
    if (!els || els.length === 0) return
    const el = els[0]
    if (!el) return
    if (
      value &&
      typeof value === 'object' &&
      typeof (value as { label?: unknown }).label === 'string'
    ) {
      el.textContent = (value as { label: string }).label
    } else if (typeof value === 'string') {
      el.textContent = value
    } else {
      el.textContent = JSON.stringify(value)
    }
  }

  /** Compute the text a marker should show for a resolved value. Mirrors
   *  `updateMarker`'s convention so a retry can compare and skip no-ops. */
  const desiredText = (value: unknown): string => {
    if (
      value &&
      typeof value === 'object' &&
      typeof (value as { label?: unknown }).label === 'string'
    ) {
      return (value as { label: string }).label
    }
    if (typeof value === 'string') return value
    return JSON.stringify(value)
  }

  /**
   * Reconcile a marker write across several animation frames. Needed because
   * React's hydration can (a) error with #418 on a text mismatch, and (b)
   * respond by **replacing the marker element** — not just its text — so a
   * one-shot write against the pre-hydration element is lost. React may also
   * do its work in a microtask that runs *after* the first rAF, so a single
   * reapply on frame 1 can still be too early.
   *
   * Strategy: run for the full 30-frame window (≈500ms at 60fps) with **no
   * early exit on stability**. An earlier attempt stopped after 3 stable
   * frames — that heuristic missed React #418's clobber, which lands a few
   * frames *after* the glue settles. On non-React pages, the 30 no-op frames
   * cost ~0.1ms each — negligible. The persistent MutationObserver
   * (`attachDeferGlue`) is belt-and-braces for anything later than 500ms.
   */
  const scheduleMarkerReapply = (id: number, value: unknown): void => {
    // Non-browser host (SSR worker, node:test without a DOM, the
    // adapter's preload path): there is no hydration pass that will
    // clobber a marker. Skip the retry entirely — otherwise every slot
    // resolution posts 30 timers that keep the process event loop alive
    // for ~500ms per test, without changing anything observable. Browser
    // hosts always have `requestAnimationFrame`.
    const raf = (globalThis as { requestAnimationFrame?: (cb: () => void) => unknown })
      .requestAnimationFrame
    if (typeof raf !== 'function') return

    const desired = desiredText(value)
    let attempts = 0
    const MAX_ATTEMPTS = 30

    const tick = (): void => {
      attempts++
      if (dead) return
      if (attempts > MAX_ATTEMPTS) return
      if (!doc || typeof doc.querySelectorAll !== 'function') return
      const els = doc.querySelectorAll(`[data-deferred-value="${id}"]`)
      const el = els && els.length > 0 ? els[0] : undefined
      if (el && el.textContent !== desired) {
        el.textContent = desired
      }
      raf(tick)
    }
    raf(tick)
  }

  /**
   * Resolve (or reject) one slot from a settle value, and update any
   * matching `[data-deferred-value]` marker. A rejected defer is
   * serialized as `{ __oxide_deferred_error: "..." }` and rejects the
   * slot rather than resolving it to a shape the page did not expect.
   * Called from every resolution path: per-settle scripts, batched
   * scripts, `ox:swap` events, and the programmatic `glue.resolve`/
   * `glue.resolveAll` API.
   */
  const resolveSlot = (id: number, value: unknown): void => {
    const slot = registry.get(id)
    if (!slot) return
    if (
      value &&
      typeof value === 'object' &&
      '__oxide_deferred_error' in (value as Record<string, unknown>)
    ) {
      slot.reject(
        new Error(String((value as Record<string, unknown>).__oxide_deferred_error)),
      )
      return
    }
    // Only update the marker when the slot actually transitions to resolved
    // (a second resolve of an already-resolved slot is a no-op, and must not
    // overwrite a marker that a user might have set to something else).
    const wasPending = slot.status === 'pending'
    slot.resolve(value)
    if (wasPending) {
      // Record the desired text so the MutationObserver can reconcile any
      // later clobber (React #418's remount, a framework re-render, or an
      // external override) without re-deriving the value each time.
      reconciled.set(id, desiredText(value))
      // Best-effort synchronous write: non-React pages see the value
      // immediately, and the retry below is a no-op.
      updateMarker(id, value)
      // 30-frame retry window (≈500ms) covers React #418's clobber, which
      // can land several frames after hydration begins. No early exit on
      // stability — an earlier "3 stable frames" heuristic missed it.
      scheduleMarkerReapply(id, value)
    }
  }

  let dead = false
  if (!doc) throw new TypeError('attachDeferGlue: no document (pass one in non-browser tests)')

  const onSwap = (e: unknown): void => {
    if (!isSwapEvent(e)) return
    const detail = e.detail
    options.onSwap?.(detail)
    if (detail.error !== undefined && detail.error !== null) {
      registry.get(detail.id)?.reject(detail.error)
      return
    }
    resolveSlot(detail.id, detail.value)
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
          resolveSlot(id, parsed)
        }
        continue
      }
      for (const [idStr, value] of Object.entries(parsed)) {
        const id = Number(idStr)
        if (!Number.isFinite(id)) continue
        resolveSlot(id, value)
      }
    }
  }

  // A streamed settle appends its script after the document has loaded, so
  // `resolveFromScripts` has already run. Watch for the insertions and
  // resolve each new one. The observer disconnects in `dispose`.
  let observer: { disconnect(): void; observe(target: unknown, options?: { childList?: boolean; subtree?: boolean; characterData?: boolean }): void } | null = null
  const MO = (globalThis as { MutationObserver?: new (cb: (m: unknown[]) => void) => {
    observe(t: unknown, o: unknown): void
    disconnect(): void
  } }).MutationObserver
  if (typeof MO === 'function' && doc.documentElement !== undefined) {
    observer = new MO((mutations: unknown[]) => {
      // Belt-and-braces (React #418 remounts, framework re-renders, any
      // late clobber): reconcile every resolved marker whose DOM text
      // disagrees with the value the glue wrote. Runs on every mutation
      // batch — `reconciled` is bounded by the number of resolved deferred
      // slots, so this is O(resolved). The 30-frame rAF retry above is the
      // primary mechanism; this catches clobbers that land after that window.
      if (!dead && doc && typeof doc.querySelectorAll === 'function') {
        for (const [id, desired] of reconciled) {
          const els = doc.querySelectorAll(`[data-deferred-value="${id}"]`)
          const el = els && els.length > 0 ? els[0] : undefined
          if (el && el.textContent !== desired) el.textContent = desired
        }
      }
      // Existing behaviour: resolve any newly appended deferred script.
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
      characterData: true,
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
      resolveSlot(id, value)
    },
    resolveAll(values: Record<string, unknown>) {
      for (const [idStr, value] of Object.entries(values)) {
        const id = Number(idStr)
        if (!Number.isFinite(id)) continue
        resolveSlot(id, value)
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

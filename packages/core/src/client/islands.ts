// Islands loader (spec §10.3, D12) — ~1.5 KB core: schedules hydration of
// `<ox-island>` hosts by strategy (`client:load | idle | visible | media(...)`)
// and dispatches each into the adapter's `hydrateIsland` (§7). Server HTML
// renders islands with ODF-encoded props + a chunk reference (render crate:
// data-ox-island / data-ox-chunk / data-ox-strategy / data-ox-props).
// Pages with no islands ship no framework JavaScript.

import { decodeOdf } from './odf-decode.ts'
import { replayFor } from './replay.ts'

/** Structural island host (real element or test fake). */
export interface IslandHost {
  getAttribute(name: string): string | null
  hasAttribute(name: string): boolean
  setAttribute?(name: string, value: string): void
}

interface IslandsDoc {
  querySelectorAll(sel: string): ArrayLike<IslandHost>
}

export interface IslandsAdapter {
  /** Synchronous (IR replay) or asynchronous (React's `react-dom/client`,
   *  which code-splits its runtime). The caller awaits either. */
  hydrateIsland?(
    el: Element,
    mod: unknown,
    props: unknown,
  ): (() => void) | Promise<() => void>
  hydrate?(root: Element, input: unknown): () => void
}

export interface IslandsOptions {
  adapter: IslandsAdapter
  doc?: IslandsDoc
  /** Load a chunk reference (dynamic import in the browser). */
  loadChunk?: (chunk: string) => Promise<unknown>
  requestIdleCallback?: (cb: () => void) => unknown
  matchMedia?: (query: string) => { matches: boolean; addEventListener?(type: string, cb: () => void): void; removeEventListener?(type: string, cb: () => void): void }
  IntersectionObserver?: new (cb: (entries: Array<{ target: unknown; isIntersecting: boolean }>) => void) => {
    observe(el: unknown): void
    unobserve(el: unknown): void
  }
}

export type ParsedStrategy = { kind: 'load' | 'idle' | 'visible' } | { kind: 'media'; query: string }

/** Parse `data-ox-strategy`: `load | idle | visible | media(query)`. */
export function parseStrategy(raw: string | null): ParsedStrategy {
  const s = (raw ?? 'load').trim()
  if (s.startsWith('media')) {
    const inner = s.slice(s.indexOf('(') + 1, s.lastIndexOf(')')).trim()
    return { kind: 'media', query: inner }
  }
  if (s === 'idle' || s === 'visible') return { kind: s }
  return { kind: 'load' }
}

function b64Bytes(b64: string): Uint8Array {
  const g = globalThis as { atob?: (s: string) => string; Buffer?: { from(s: string, enc: string): Uint8Array } }
  if (typeof g.atob === 'function') {
    const bin = g.atob(b64)
    const bytes = new Uint8Array(bin.length)
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i)
    return bytes
  }
  if (g.Buffer) return new Uint8Array(g.Buffer.from(b64, 'base64'))
  throw new TypeError('islands: no base64 decoder available (atob or Buffer)')
}

/** Decode `data-ox-props`: base64 ODF (§10.3), with a JSON fallback for
 * payloads that predate the ODF props encoding (render test fixtures). */
export function decodeIslandProps(b64: string): unknown {
  const bytes = b64Bytes(b64)
  if (bytes.length >= 4 && bytes[0] === 0x4f && bytes[1] === 0x44 && bytes[2] === 0x46 && bytes[3] === 0x31) {
    return decodeOdf(bytes)
  }
  try {
    return JSON.parse(new TextDecoder().decode(bytes))
  } catch {
    return undefined
  }
}

export interface IslandCtx extends IslandsOptions {
  loadChunk: (chunk: string) => Promise<unknown>
}

/** Hydrate one island now: decode props → load chunk → adapter dispatch.
 *
 * Two attributes bracket the work so consumers (tests, devtools, a
 * readiness check) can distinguish "hydration started" from "hydration
 * finished":
 *   * `data-ox-hydrated="pending"` — set synchronously on entry, serves as
 *     the double-hydration guard.
 *   * `data-ox-hydrated="done"` — set after the adapter's `hydrateIsland`
 *     returns, i.e. once framework listeners are attached. A browser test
 *     that needs to click the island waits for this.
 *   * `data-ox-hydrated="error"` — the adapter threw; the island stays
 *     inert and the attribute records why.
 */
export async function hydrateIsland(el: IslandHost, ctx: IslandCtx): Promise<(() => void) | undefined> {
  const host = el as IslandHost & { 'data-ox-hydrated'?: unknown }
  const state = host.getAttribute('data-ox-hydrated')
  if (state === 'pending' || state === 'done') return undefined // guard
  host.setAttribute?.('data-ox-hydrated', 'pending')
  try {
    const chunk = el.getAttribute('data-ox-chunk') ?? ''
    const propsAttr = el.getAttribute('data-ox-props')
    const props = propsAttr ? decodeIslandProps(propsAttr) : undefined
    const mod = await ctx.loadChunk(chunk)
    const adapter = ctx.adapter
    let disposer: (() => void) | undefined
    if (typeof adapter.hydrateIsland === 'function') {
      // `hydrateIsland` may be synchronous (IR path) or async (React's
      // `react-dom/client` path code-splits its runtime on first use).
      // `await` handles both — a non-promise value resolves immediately.
      disposer = await Promise.resolve(
        adapter.hydrateIsland(el as Element, mod, props),
      )
    } else if (typeof adapter.hydrate === 'function') {
      disposer = adapter.hydrate(el as Element, { props, mod })
    }
    host.setAttribute?.('data-ox-hydrated', 'done')
    // §V2-F03: interactions that landed on this island while it was pending
    // replay now — after the framework listeners are attached, before the
    // caller's microtask continues. No log, no replay: a no-op when capture
    // never started (SSR, tests without startReplay).
    replayFor(el)
    return disposer
  } catch (e) {
    host.setAttribute?.('data-ox-hydrated', 'error')
    throw e
  }
}

/**
 * Schedule every `<ox-island[data-ox-chunk]>` under `doc` by its strategy
 * (§10.3): `load` → now; `idle` → requestIdleCallback (setTimeout fallback);
 * `visible` → IntersectionObserver; `media(q)` → matchMedia now/change.
 * Returns a disposer that cancels pending schedules.
 */
export function hydrateIslands(opts: IslandsOptions): () => void {
  const doc = opts.doc ?? (globalThis as { document?: IslandsDoc }).document
  if (!doc) throw new TypeError('hydrateIslands: no document (pass one in non-browser tests)')
  const loadChunk = opts.loadChunk ?? defaultLoadChunk
  const ric = opts.requestIdleCallback ?? ((cb: () => void) => {
    const g = globalThis as { requestIdleCallback?: (cb: () => void) => unknown }
    return g.requestIdleCallback ? g.requestIdleCallback(cb) : setTimeout(cb, 1)
  })
  const cancelled = { value: false }
  const disposers: Array<() => void> = []
  const schedule = (el: IslandHost): void => {
    const strategy = parseStrategy(el.getAttribute('data-ox-strategy'))
    const run = () => {
      if (cancelled.value) return
      void hydrateIsland(el, { ...opts, loadChunk })
    }
    switch (strategy.kind) {
      case 'load':
        run()
        break
      case 'idle':
        ric(() => run())
        break
      case 'visible': {
        const IO = opts.IntersectionObserver ?? (globalThis as { IntersectionObserver?: IslandsOptions['IntersectionObserver'] }).IntersectionObserver
        if (!IO) {
          run() // no observer available: hydrate rather than never
          break
        }
        const io = new IO((entries) => {
          for (const entry of entries) {
            if (entry.isIntersecting) {
              io.unobserve(entry.target)
              run()
            }
          }
        })
        io.observe(el)
        disposers.push(() => io.unobserve(el))
        break
      }
      case 'media': {
        const mm = opts.matchMedia ?? (globalThis as { matchMedia?: IslandsOptions['matchMedia'] }).matchMedia
        if (!mm) break
        const m = mm(strategy.query)
        if (m.matches) {
          run()
        } else if (m.addEventListener) {
          const onChange = () => {
            if (m.matches) {
              m.removeEventListener?.('change', onChange)
              run()
            }
          }
          m.addEventListener('change', onChange)
          disposers.push(() => m.removeEventListener?.('change', onChange))
        }
        break
      }
    }
  }
  const islands = doc.querySelectorAll('ox-island[data-ox-chunk]')
  for (let i = 0; i < islands.length; i++) {
    const island = islands[i]
    if (island === undefined) continue
    schedule(island)
  }
  return () => {
    cancelled.value = true
    for (const d of disposers) d()
  }
}

function defaultLoadChunk(chunk: string): Promise<unknown> {
  // Dynamic import with a variable specifier; bundlers mark it external via
  // the /* oxide:island */ comment (each island is its own chunk, §10.3).
  // §3.10: catch here so a transient dev-server outage leaves an unhandled
  // rejection only at the caller's log, not at the page — the island is
  // skipped, its SSR markup stays, hydration resumes on a later scan.
  return import(/* oxide:island */ chunk).catch((e: unknown) => {
    console.error?.('[oxide] island chunk fetch failed; island deferred', chunk, e)
    return undefined
  })
}

// ---------------------------------------------------------------------------
// §V2-F01: the mixed-framework registry
// ---------------------------------------------------------------------------

/** One registry island: its framework and its own lazy module load. */
export interface MixedIslandEntry {
  fw: string
  load: () => Promise<unknown>
}

/** The mixed registry the generated `__boot__` entry hands to
 *  `installBootRuntime({ adapters, islands })` (§V2-F01): one lazy
 *  adapter per framework present, one entry per island. */
export interface MixedIslandRegistry {
  adapters: Record<string, () => Promise<{ clientAdapter: IslandsAdapter }>>
  islands: Record<string, MixedIslandEntry>
}

/** Mark a host with the hydration error (the attribute the devtools
 *  overlay and tests read). */
function markMixedError(host: IslandHost, message: string): void {
  host.setAttribute?.('data-ox-hydrated', 'error')
  host.setAttribute?.('data-ox-error', message)
}

/** Hydrate one island through the MIXED registry (§V2-F01's
 *  `hydrateOne`): look the island up by name, load the module and the
 *  island's own adapter in parallel, decode the props, dispatch into
 *  `hydrateIsland`. An unknown island marks an error — never a wrong
 *  adapter's hydration. */
export async function hydrateMixedIsland(
  host: IslandHost,
  reg: MixedIslandRegistry,
): Promise<(() => void) | undefined> {
  const el = host as IslandHost & { 'data-ox-hydrated'?: unknown }
  const state = el.getAttribute('data-ox-hydrated')
  if (state === 'pending' || state === 'done') return undefined // guard
  const name = host.getAttribute('data-ox-island') ?? ''
  const entry = reg.islands[name]
  if (!entry) {
    markMixedError(host, 'unknown island ' + name)
    return undefined
  }
  el.setAttribute?.('data-ox-hydrated', 'pending')
  try {
    const lazy = reg.adapters[entry.fw]
    if (!lazy) throw new Error('no adapter registered for framework ' + entry.fw)
    const [mod, adapterMod] = await Promise.all([entry.load(), lazy()])
    const adapter = adapterMod.clientAdapter
    const propsAttr = host.getAttribute('data-ox-props')
    const props = propsAttr ? decodeIslandProps(propsAttr) : undefined
    let disposer: (() => void) | undefined
    if (typeof adapter.hydrateIsland === 'function') {
      disposer = await Promise.resolve(adapter.hydrateIsland(host as unknown as Element, mod, props))
    } else if (typeof adapter.hydrate === 'function') {
      disposer = adapter.hydrate(host as unknown as Element, { props, mod })
    }
    el.setAttribute?.('data-ox-hydrated', 'done')
    replayFor(host)
    return disposer
  } catch (e) {
    markMixedError(host, e instanceof Error ? e.message : String(e))
    return undefined
  }
}

/** Scan every `<ox-island[data-ox-chunk]>` under `doc` and hydrate
 *  through the mixed registry, by the same strategy schedule the
 *  single-adapter path uses (§10.3). Returns the disposer. */
export function hydrateMixedIslands(
  reg: MixedIslandRegistry,
  opts: {
    doc?: IslandsDoc
    requestIdleCallback?: IslandsOptions['requestIdleCallback']
    matchMedia?: IslandsOptions['matchMedia']
    IntersectionObserver?: IslandsOptions['IntersectionObserver']
  },
): () => void {
  const doc = opts.doc ?? (globalThis as { document?: IslandsDoc }).document
  if (!doc) throw new TypeError('hydrateMixedIslands: no document (pass one in non-browser tests)')
  const ric = opts.requestIdleCallback ?? ((cb: () => void) => {
    const g = globalThis as { requestIdleCallback?: (cb: () => void) => unknown }
    return g.requestIdleCallback ? g.requestIdleCallback(cb) : setTimeout(cb, 1)
  })
  const cancelled = { value: false }
  const schedule = (el: IslandHost): void => {
    const strategy = parseStrategy(el.getAttribute('data-ox-strategy'))
    const run = () => {
      if (cancelled.value) return
      void hydrateMixedIsland(el, reg)
    }
    switch (strategy.kind) {
      case 'load':
        run()
        break
      case 'idle':
        ric(() => run())
        break
      case 'visible': {
        const IO =
          opts.IntersectionObserver ??
          (globalThis as { IntersectionObserver?: IslandsOptions['IntersectionObserver'] })
            .IntersectionObserver
        if (!IO) {
          run()
          break
        }
        const io = new IO((entries) => {
          for (const entry of entries) {
            if (entry.isIntersecting) {
              io.unobserve(entry.target)
              run()
            }
          }
        })
        io.observe(el)
        disposers.push(() => io.unobserve(el))
        break
      }
      case 'media': {
        const mq = (opts.matchMedia ?? defaultMatchMedia)(strategy.query)
        if (mq.matches) {
          run()
        } else {
          const onChange = (e: { matches: boolean }): void => {
            if (e.matches && !cancelled.value) {
              mq.removeEventListener?.('change', onChange)
              run()
            }
          }
          mq.addEventListener?.('change', onChange)
          disposers.push(() => mq.removeEventListener?.('change', onChange))
        }
        break
      }
    }
  }
  const disposers: Array<() => void> = []
  const hosts = doc.querySelectorAll('ox-island[data-ox-chunk]')
  for (const host of hosts) schedule(host)
  return () => {
    cancelled.value = true
    for (const d of disposers) d()
  }
}

function defaultMatchMedia(query: string): { matches: boolean; addEventListener?(t: string, cb: (e: { matches: boolean }) => void): void; removeEventListener?(t: string, cb: (e: { matches: boolean }) => void): void } {
  const g = globalThis as { matchMedia?: (q: string) => { matches: boolean; addEventListener?(t: string, cb: (e: { matches: boolean }) => void): void; removeEventListener?(t: string, cb: (e: { matches: boolean }) => void): void } }
  if (typeof g.matchMedia !== 'function') return { matches: false }
  return g.matchMedia(query)
}

// §V2-F17 — the component-cost dev overlay (the client half).
//
// The spec's dev overlay ("Dev-only (`?ox-cost` or Alt+Shift+O)") highlights
// the hovered element's component and lists the three heaviest. The two other
// halves are landed: the compiler stamps `data-ox-src="path:line"` on every
// component's first element (`crates/compiler/src/data_src.rs`), and the dev
// plane serves `GET /__oxide/cost?src=<path>` (dev-token guarded, D41 —
// `crates/server/src/cost_endpoint.rs`). This module consumes both:
//
// - **activation** is dev-only by construction — the boot mounts it on the
//   ESM dev path only, and it arms on the `?ox-cost` query spelling or the
//   Alt+Shift+O chord; nothing ships in a production bundle's runtime path;
// - **attribution** walks `closest('[data-ox-src]')` from the hovered target
//   and parses the stamp into the `{ path, line }` the endpoint's `?src=`
//   contract takes — an unattributable stamp is `undefined`, never a guess;
// - **the panel** renders the hovered record and the three heaviest
//   components, with the spec's bytes-only note ("Svelte and Solid show
//   bytes only, and the overlay says so") for a null `renderMsP50`;
// - **degradation is loud**: a 403 (the dev-token guard refused), a 404 (the
//   endpoint's documented miss) and a network failure each render their own
//   honest notice — the overlay never fabricates a record the build did not
//   produce (the endpoint's own doctrine).
//
// The dev token reaches the page the dev boot mints it for via
// `window.__oxide.devToken` (the render stage's `application/oxide-dev-token`
// tag); a page without one keeps every other arm working and the fetch
// degrades on the endpoint's 403.

export interface OxSrcRef {
  /** The project-relative source path the cost endpoint's `?src=` takes. */
  path: string
  /** The 1-based stamp line (informational — the endpoint keys on the path). */
  line: number
}

/**
 * Parse a `data-ox-src` stamp into the `{ path, line }` the cost endpoint's
 * `?src=` contract takes. The stamp is `path:line` with a NUMERIC line —
 * anything else (no colon, a non-numeric line, an empty path) is a stamp the
 * overlay cannot attribute, and `undefined` says so instead of guessing.
 */
export function parseOxSrc(attr: string | null | undefined): OxSrcRef | undefined {
  if (!attr) return undefined
  const idx = attr.lastIndexOf(':')
  if (idx <= 0) return undefined
  const path = attr.slice(0, idx)
  const line = attr.slice(idx + 1)
  if (path.length === 0 || !/^\d+$/.test(line)) return undefined
  return { path, line: Number(line) }
}

/**
 * The `?ox-cost` activation parse: an `ox-cost` query KEY present in
 * `search` (any value, including the bare flag spelling) arms the overlay.
 * A key that merely STARTS with the flag (`?ox-costly`) is not the
 * activation. Input is the `location.search` string (possibly empty).
 */
export function costOverlayActivatable(search: string): boolean {
  const query = search.startsWith('?') ? search.slice(1) : search
  if (query.length === 0) return false
  for (const kv of query.split('&')) {
    if (kv.split('=')[0] === 'ox-cost') return true
  }
  return false
}

/** One component's cost record — the endpoint's wire shape, camelCase. */
export interface ComponentCostRecord {
  /** The module's rendered size in bytes. */
  bytes: number
  /** The gzip estimate in bytes. */
  gzip: number
  /** The client chunk the file landed in. */
  chunk: string
  /** The p50 server render time in ms; `null` for the bytes-only adapters. */
  renderMsP50?: number | null
  /** The cache tier the last render answered from, when recorded. */
  cache?: string | null
}

/**
 * The `n` largest records by `bytes`, first-seen order kept on ties (the
 * panel's list is deterministic for equal sizes). `n` past the length is
 * every record; an empty input is an empty list — the list is honest, never
 * padded.
 */
export function heaviest(rows: ComponentCostRecord[], n: number): ComponentCostRecord[] {
  if (n <= 0 || rows.length === 0) return []
  const indexed = rows.map((row, i) => ({ row, i }))
  indexed.sort((a, b) => b.row.bytes - a.row.bytes || a.i - b.i)
  return indexed.slice(0, n).map((e) => e.row)
}

/** The panel's byte units (B with no fraction; KB/MB with one decimal). */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / (1024 * 1024)).toFixed(1)} MB`
}

/** The p50 render line: the measured value, or the spec's bytes-only note. */
function renderLine(renderMsP50: number | null | undefined): string {
  if (renderMsP50 === null || renderMsP50 === undefined) {
    return 'render time unavailable (bytes-only adapter)'
  }
  return `render p50 ${renderMsP50.toFixed(1)} ms`
}

/**
 * The panel's line plan for one record — the strings the panel renders, in
 * order: bytes, gzip, chunk, the render line (the note for a null
 * `renderMsP50`), and the cache tier when the pipeline recorded one. A
 * pure function so the pins drive the exact wording before any DOM exists.
 */
export function costPanelPlan(record: ComponentCostRecord): string[] {
  const lines = [
    `bytes ${formatBytes(record.bytes)}`,
    `gzip ${formatBytes(record.gzip)}`,
    `chunk ${record.chunk}`,
    renderLine(record.renderMsP50),
  ]
  if (record.cache) lines.push(`cache ${record.cache}`)
  return lines
}

/** The dev-plane endpoint the overlay fetches (the server's `COST_ENDPOINT_PATH`). */
const COST_ENDPOINT = '/__oxide/cost'

/** The stamp attribute the compiler injects (`DATA_OX_SRC_ATTR` server-side). */
const OX_SRC_ATTR = 'data-ox-src'

/** The highlight marker the overlay paints onto the hovered component root. */
const HOVER_ATTR = 'data-ox-cost-hover'

/** The panel's marker attribute (and the mount guard's lookup key). */
const PANEL_ATTR = 'data-ox-cost-panel'

/** What the overlay needs from the environment; every field has a global default. */
export interface CostOverlayOptions {
  /** The document to mount into. Default: the global `document`. */
  doc?: Document
  /** The fetch to speak the endpoint with. Default: the global `fetch`. */
  fetchFn?: typeof fetch
  /** The location to read `?ox-cost` from. Default: the global `location`. */
  location?: { search: string }
  /**
   * The dev session token for the D41 guard. Default:
   * `window.__oxide.devToken` (the dev boot's page wire); empty/absent
   * sends no `token=` and the endpoint's 403 degrades loud.
   */
  devToken?: string
}

/** The `window.__oxide` boot global, extended with the dev-plane token. */
interface OxideGlobal {
  __oxide?: { devToken?: string }
}

interface OverlayState {
  dispose: () => void
  armed: boolean
  panel: HTMLElement | undefined
  hovered: Element | undefined
  records: Map<string, ComponentCostRecord>
  inflight: Map<string, Promise<ComponentCostRecord | null>>
  text: string
}

// One overlay per document (the boot's idempotency doctrine — the same
// shape the island scan's `data-ox-hydrated` guard provides): a re-mount
// while armed returns the LIVE disposer instead of a second panel.
const mounted = new WeakMap<Document, OverlayState>()

/** The cost endpoint URL for one stamped path (the token rides `token=`). */
function costUrl(path: string, token: string): string {
  const base = `${COST_ENDPOINT}?src=${encodeURIComponent(path)}`
  return token.length > 0 ? `${base}&token=${encodeURIComponent(token)}` : base
}

/** Collect the unique stamped paths in document order (the sweep's input). */
function stampedPaths(doc: Document): string[] {
  const paths: string[] = []
  const seen = new Set<string>()
  if (typeof doc.querySelectorAll !== 'function') return paths
  const nodes = doc.querySelectorAll(`[${OX_SRC_ATTR}]`)
  for (const node of Array.from(nodes)) {
    const ref = parseOxSrc(node.getAttribute?.(OX_SRC_ATTR))
    if (ref && !seen.has(ref.path)) {
      seen.add(ref.path)
      paths.push(ref.path)
    }
  }
  return paths
}

/** Append `line` to the panel's text (one render path for every arm). */
function renderPanel(state: OverlayState): void {
  if (!state.panel) return
  state.panel.textContent = state.text
}

/**
 * Mount the cost overlay on `doc`. Idempotent per document: a mount while
 * one is live returns the existing disposer (never a second panel or a
 * second listener set). Returns the disposer — removing the panel, clearing
 * the highlight, unbinding the listeners and re-arming the guard.
 *
 * Arms immediately when `?ox-cost` is in the location search; Alt+Shift+O
 * toggles at any time; Escape closes the panel. The hover attribution and
 * the panel's records only fetch while armed.
 */
export function mountCostOverlay(opts: CostOverlayOptions = {}): () => void {
  const doc =
    opts.doc ?? (globalThis as { document?: Document }).document
  if (!doc) return () => {}
  const live = mounted.get(doc)
  if (live) return live.dispose

  const w = globalThis as typeof globalThis & OxideGlobal & { fetch?: typeof fetch }
  const fetchFn = opts.fetchFn ?? w.fetch
  const state: OverlayState = {
    dispose: () => {},
    armed: false,
    panel: undefined,
    hovered: undefined,
    records: new Map(),
    inflight: new Map(),
    text: '',
  }

  const token = (): string => opts.devToken ?? w.__oxide?.devToken ?? ''

  const closePanel = (): void => {
    state.panel?.remove()
    state.panel = undefined
    if (state.hovered) {
      state.hovered.removeAttribute?.(HOVER_ATTR)
      state.hovered = undefined
    }
  }

  const arm = (): void => {
    state.armed = true
    const panel = doc.createElement('div')
    panel.setAttribute(PANEL_ATTR, '')
    state.panel = panel
    ;(doc.body ?? doc.documentElement)?.appendChild?.(panel)
    void sweep()
  }

  const disarm = (): void => {
    state.armed = false
    closePanel()
    state.text = ''
  }

  const fetchRecord = async (path: string): Promise<ComponentCostRecord | null> => {
    const inflight = state.inflight.get(path)
    if (inflight) return inflight
    if (typeof fetchFn !== 'function') return null
    const run = (async (): Promise<ComponentCostRecord | null> => {
      try {
        const res = await fetchFn(costUrl(path, token()))
        if (res.status === 403) {
          state.text = 'cost feed requires the dev token — copy it from the `oxide dev` banner into window.__oxide.devToken\n'
          return null
        }
        if (res.status === 404) {
          state.text = `no cost record for ${path} (the build did not produce one)\n`
          return null
        }
        if (!res.ok) {
          state.text = `cost endpoint answered ${res.status} for ${path}\n`
          return null
        }
        const body = (await res.json()) as ComponentCostRecord
        state.records.set(path, body)
        return body
      } catch (e) {
        state.text = `cost fetch failed for ${path} (see the console)\n`
        console.warn?.(`oxide: cost fetch failed for ${path}`, e)
        return null
      } finally {
        state.inflight.delete(path)
      }
    })()
    state.inflight.set(path, run)
    return run
  }

  const drawTop = async (): Promise<void> => {
    const pending = stampedPaths(doc)
    await Promise.all(pending.map((p) => fetchRecord(p)))
    // The notice a failed fetch left (403/404/network) is already in
    // `state.text` — render it even when the heaviest list stays empty.
    const top = heaviest(Array.from(state.records.values()), 3)
    if (top.length > 0) {
      const lines = top.map(
        (r, i) =>
          `${i + 1}. ${r.chunk} — ${formatBytes(r.bytes)}${r.renderMsP50 === null || r.renderMsP50 === undefined ? '' : ` — ${r.renderMsP50.toFixed(1)} ms`}`,
      )
      state.text += `\nthree heaviest:\n${lines.join('\n')}\n`
    }
    renderPanel(state)
  }

  const sweep = async (): Promise<void> => {
    await drawTop()
  }

  const onKeydown = (ev: Event): void => {
    const ke = ev as KeyboardEvent
    if (ke.altKey && ke.shiftKey && (ke.key === 'O' || ke.key === 'o')) {
      if (state.armed) disarm()
      else arm()
      return
    }
    if (ke.key === 'Escape' && state.armed) closePanel()
  }

  const onOver = (ev: Event): void => {
    if (!state.armed) return
    const target = (ev as { target?: unknown }).target as
      | (Element & { closest?: (sel: string) => Element | null })
      | undefined
    if (!target || typeof target.closest !== 'function') return
    const root = target.closest(`[${OX_SRC_ATTR}]`)
    if (!root) return
    if (state.hovered && state.hovered !== root) state.hovered.removeAttribute?.(HOVER_ATTR)
    state.hovered = root
    root.setAttribute?.(HOVER_ATTR, '')
    const ref = parseOxSrc(root.getAttribute?.(OX_SRC_ATTR))
    if (!ref) return
    void (async (): Promise<void> => {
      const record = await fetchRecord(ref.path)
      const lines = record
        ? costPanelPlan(record)
        : [state.text.trimEnd() || `no cost record for ${ref.path}`]
      state.text = `${ref.path}:${ref.line}\n${lines.join('\n')}\n`
      renderPanel(state)
    })()
  }

  doc.addEventListener?.('mouseover', onOver as EventListener)
  doc.addEventListener?.('keydown', onKeydown as EventListener)

  const search = opts.location?.search ?? (globalThis as { location?: { search?: string } }).location?.search ?? ''
  if (costOverlayActivatable(search)) arm()

  state.dispose = (): void => {
    doc.removeEventListener?.('mouseover', onOver as EventListener)
    doc.removeEventListener?.('keydown', onKeydown as EventListener)
    disarm()
    mounted.delete(doc)
  }
  mounted.set(doc, state)
  return state.dispose
}

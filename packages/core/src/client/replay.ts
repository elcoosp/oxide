// The client side of the interaction-replay contract (spec v0.2, §V2-F03).
//
// Clicks, input, change and focus that land on a not-yet-hydrated
// `<ox-island>` are lost today: the island's framework listeners attach
// only when its chunk resolves and `data-ox-hydrated` flips to `done`.
// This module captures those interactions in the capture phase (so the
// page's own handlers cannot have run) and replays them — exactly once,
// in order — when `hydrateIsland` marks the island done.
//
// Invariants (spec §V2-F03):
//   * An event is recorded only when the island was PENDING at capture
//     time — the exact window in which no handler could have run — so an
//     interaction that reached a live handler never replays twice.
//   * A record whose target no longer resolves to the captured element
//     (hydration or a defer swap replaced the content) is dropped, never
//     replayed onto a same-slot replacement.
//   * Content outside the island's subtree (e.g. `template[data-ox-b]`
//     fallback) is never recorded: the host-relative path would be
//     truncated garbage.
//   * The log is bounded (MAX = 100) and drops the OLDEST record first.
//
// Module-safety contract (same as `live.ts` / `resume.ts` / `boot.ts`):
// import-safe in the SSR / config-loader Node environment — no top-level
// side effects, everything DOM-shaped is structural and optional, and
// `startReplay()` without a document returns a no-op disposer instead of
// throwing.

/** The interaction kinds captured while an island is pending hydration. */
export type ReplayKind = 'click' | 'input' | 'change' | 'focus'

/** Synthetic event handed to `dispatchEvent` (a real `Event` in browsers). */
export interface ReplayEventLike {
  type: string
  bubbles?: boolean
}

/**
 * Minimal element surface the capture and replay paths need. Structural
 * and optional by design: real DOM elements satisfy it verbatim, and the
 * Node test runner passes minimal fakes (the `islands.ts` `IslandHost`
 * convention).
 */
export interface ReplayElement {
  getAttribute?(name: string): string | null
  parentElement?: ReplayElement | null
  children?: ArrayLike<ReplayElement>
  contains?(other: ReplayElement): boolean
  click?(): void
  focus?(): void
  value?: string
  checked?: boolean
  dispatchEvent?(event: ReplayEventLike): boolean
  closest?(selector: string): ReplayElement | null
}

/** The event object the capture listeners see (only `target` is read). */
export interface ReplayDomEvent {
  target?: ReplayElement | null
}

/** The document surface `startReplay` attaches to (structural, testable). */
export interface ReplayDoc {
  addEventListener(
    type: string,
    listener: (event: ReplayDomEvent) => void,
    capture?: boolean,
  ): void
  removeEventListener(
    type: string,
    listener: (event: ReplayDomEvent) => void,
    capture?: boolean,
  ): void
}

/** One captured interaction, keyed to its island by node identity. */
export interface ReplayRecord {
  type: ReplayKind
  /** The `<ox-island>` host the interaction landed in. */
  island: ReplayElement
  /** Host-relative child-index path to the target. */
  path: number[]
  /** The captured target, so a same-slot replacement is detected on replay. */
  target: ReplayElement
  /** Input value (`input` / `change` only). */
  value?: string
  /** Input checked state (`change` on a checkbox). */
  checked?: boolean
}

/** Capacity bound: the log holds at most this many records (drop-oldest). */
const MAX = 100

const KINDS: readonly ReplayKind[] = ['click', 'input', 'change', 'focus']

/** The DOM event each kind is captured on: `focus` rides `focusin` because
 *  `focus` does not bubble and capture-only listeners on the document miss
 *  it when the event retargets inside a shadow root. */
const CAPTURE_EVENT: Record<ReplayKind, string> = {
  click: 'click',
  input: 'input',
  change: 'change',
  focus: 'focusin',
}

// Module-level log (spec-verbatim). `boot.ts` starts at most one capture per
// document (startReplay is idempotent per document), and `resetReplay()`
// exists for the Node test runner — production boot never calls it.
const log: ReplayRecord[] = []

/** Records currently pending replay (devtools / diagnostics / tests). */
export function replayPending(): number {
  return log.length
}

/** Drop every pending record. A test/devtools hook; boot never calls it. */
export function resetReplay(): void {
  log.length = 0
}

function childIndex(parent: ReplayElement, child: ReplayElement): number {
  const kids = parent.children
  const len = kids?.length ?? 0
  for (let i = 0; i < len; i++) {
    if (kids?.[i] === child) return i
  }
  return -1
}

/**
 * Host-relative child-index path from `root` down to `el`, or `null` when
 * the ancestor chain does not terminate at `root` (detached node, template
 * fallback content). The spec's sketch walks `parentElement` unguarded and
 * records a truncated path for such nodes; refusing them here is what makes
 * the "unresolvable path is dropped" invariant hold at capture time.
 */
function pathOf(root: ReplayElement, el: ReplayElement): number[] | null {
  const path: number[] = []
  let node: ReplayElement | null | undefined = el
  while (node && node !== root) {
    const parent = node.parentElement
    if (!parent) return null
    const index = childIndex(parent, node)
    if (index < 0) return null
    path.unshift(index)
    node = parent
  }
  return node === root ? path : null
}

/** The island host `el`'s interaction belongs to, or `null` when the island
 *  is already hydrated (`data-ox-hydrated="done"` — its handlers are live). */
function pendingIsland(target: ReplayElement | null | undefined): ReplayElement | null {
  const host = target?.closest?.('ox-island')
  if (!host) return null
  return host.getAttribute?.('data-ox-hydrated') !== 'done' ? host : null
}

function captureFor(kind: ReplayKind): (event: ReplayDomEvent) => void {
  return (event: ReplayDomEvent): void => {
    const target = event.target
    if (!target) return
    const island = pendingIsland(target)
    if (!island) return
    const path = pathOf(island, target)
    if (path === null) return // template fallback / detached: never resolvable
    const record: ReplayRecord = { type: kind, island, path, target }
    if (kind === 'input' || kind === 'change') {
      if (target.value !== undefined) record.value = target.value
      if (target.checked !== undefined) record.checked = target.checked
    }
    log.push(record)
    if (log.length > MAX) log.shift()
  }
}

/**
 * Resolve a record's host-relative path against the island's CURRENT tree.
 * Returns the node the record still points at, or `null` when the path no
 * longer resolves (content replaced / removed).
 */
function resolve(root: ReplayElement, path: number[]): ReplayElement | null {
  let node: ReplayElement | null = root
  for (const index of path) {
    const next = node.children?.[index]
    if (!next) return null
    node = next
  }
  return node
}

type EventCtor = new (type: string, init?: { bubbles?: boolean }) => ReplayEventLike

/** A real bubbling `Event` where the platform provides one (browser, Node),
 *  a plain structural event for minimal fakes without one. */
function replayEvent(type: string): ReplayEventLike {
  const ctor = (globalThis as { Event?: EventCtor }).Event
  if (ctor) return new ctor(type, { bubbles: true })
  return { type, bubbles: true }
}

/**
 * Replay every record captured for `host`, in capture order, exactly once.
 * Called by `hydrateIsland` right after `data-ox-hydrated` becomes `done`.
 * Records for OTHER islands stay in the log; consumed or dropped records
 * are removed either way, so a later replay cannot double-fire.
 */
export function replayFor(host: ReplayElement): void {
  for (let i = 0; i < log.length; ) {
    const record = log[i]
    if (!record) break
    if (record.island !== host) {
      i++
      continue
    }
    log.splice(i, 1)
    const el = resolve(host, record.path)
    // Hydration or a defer swap replaced the content: drop, never replay
    // onto whatever now occupies the slot (node identity, not just reachability).
    if (!el || el !== record.target) continue
    if (host.contains && !host.contains(el)) continue
    if (record.type === 'click') {
      el.click?.()
      continue
    }
    if (record.type === 'focus') {
      el.focus?.()
      continue
    }
    if (record.value !== undefined) el.value = record.value
    if (record.checked !== undefined) el.checked = record.checked
    el.dispatchEvent?.(replayEvent(record.type))
  }
}

// Idempotency per document: `boot()` may run more than once across duplicate
// module evaluations (bundler dedup failure, SPA reimport), and two capture
// installs would record every interaction twice. The spec's sketch installs
// unconditionally; this guard is what makes the double-boot case replay once.
let activeDoc: ReplayDoc | undefined
let activeStop: (() => void) | undefined

/**
 * Start capturing interactions on pending islands (capture phase, before
 * any island hydrates). Defaults to the global `document`; without one
 * (SSR / config-loader Node) this is a no-op returning a no-op disposer.
 * Idempotent per document: a second start for the same document returns the
 * existing stop handle instead of installing duplicate listeners.
 *
 * Returns a disposer that removes the capture listeners. The boot runtime
 * intentionally does NOT dispose the capture on navigation — it is
 * page-lifetime glue, like `loadScripts` and the HMR hook.
 */
export function startReplay(doc?: ReplayDoc): () => void {
  const target = doc ?? (globalThis as { document?: ReplayDoc }).document
  if (!target) return () => {}
  if (activeDoc === target && activeStop) return activeStop

  const listeners = KINDS.map((kind) => ({
    type: CAPTURE_EVENT[kind],
    fn: captureFor(kind),
  }))
  for (const { type, fn } of listeners) {
    target.addEventListener(type, fn, true)
  }
  const stop = (): void => {
    for (const { type, fn } of listeners) {
      target.removeEventListener(type, fn, true)
    }
    if (activeDoc === target) {
      activeDoc = undefined
      activeStop = undefined
    }
  }
  activeDoc = target
  activeStop = stop
  return stop
}

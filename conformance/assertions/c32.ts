// §V2-R01 C32 — pre-hydration click is replayed exactly once (F03).
//
// Pins `packages/core/src/client/replay.ts`: a click landing on a
// not-yet-hydrated `<ox-island>` is captured in the capture phase and
// replayed exactly once when `replayFor(host)` runs (the call `hydrateIsland`
// makes right after `data-ox-hydrated` flips to `done`). The invariant: an
// interaction that reached a live handler never replays twice, and a record
// whose target no longer resolves is dropped.

import { replayFor, replayPending, resetReplay, startReplay, type ReplayDoc, type ReplayElement } from '../../packages/core/src/client/replay.ts'
import type { Assertion } from './types.ts'

/** A minimal fake element satisfying `ReplayElement` for the capture/replay
 *  paths: `closest`, `getAttribute`, `parentElement`, `children`, `click`,
 *  `contains`. The capture path walks `parentElement` + `children` index; the
 *  replay path resolves the path against the host's CURRENT tree and compares
 *  node identity. */
interface FakeEl {
  tag: string
  attrs: Record<string, string>
  children: FakeEl[]
  parentElement: FakeEl | null
  clicks: number
  getAttribute(n: string): string | null
  closest(sel: string): FakeEl | null
  contains(other: FakeEl): boolean
  click(): void
}
function el(tag: string, attrs: Record<string, string> = {}, children: FakeEl[] = []): FakeEl {
  const node: FakeEl = {
    tag,
    attrs,
    children,
    parentElement: null,
    clicks: 0,
    getAttribute(n) {
      return n in attrs ? attrs[n] ?? null : null
    },
    closest(sel) {
      let cur: FakeEl | null = this
      while (cur) {
        if (cur.tag === sel) return cur
        cur = cur.parentElement
      }
      return null
    },
    contains(other) {
      let cur: FakeEl | null = other
      while (cur) {
        if (cur === this) return true
        cur = cur.parentElement
      }
      return false
    },
    click() {
      this.clicks++
    },
  }
  for (const c of children) c.parentElement = node
  return node
}

const c32: Assertion = {
  id: 'C32',
  name: 'pre-hydration click is replayed exactly once (F03)',
  capability: null,
  adapters: ['react', 'vue', 'svelte', 'solid'],
  targets: ['binary', 'node'],
  surface: 'client',
  run() {
    resetReplay()
    // A pending island host (data-ox-hydrated is absent — not 'done') with a
    // button child: the click lands before hydration, so no handler ran.
    const button = el('button', { 'data-ox-target': 'b' })
    const host = el('ox-island', { 'data-ox-island': 'Counter', 'data-ox-chunk': '/islands/counter.ts' }, [button])
    const root = el('body', {}, [host])

    // The capture listeners attach to the document (capture phase).
    const listeners = new Map<string, Array<(e: { target?: FakeEl }) => void>>()
    const doc: ReplayDoc = {
      addEventListener(type, fn, _capture) {
        listeners.set(type, [...(listeners.get(type) ?? []), fn])
      },
      removeEventListener(type, fn, _capture) {
        listeners.set(type, (listeners.get(type) ?? []).filter((f) => f !== fn))
      },
    }
    const stop = startReplay(doc)
    // Dispatch a click on the button (capture phase): the capture listener
    // records it because the host is PENDING.
    for (const fn of listeners.get('click') ?? []) fn({ target: button })
    if (replayPending() !== 1) {
      stop()
      throw new Error(`the pending click was captured once (pending=${replayPending()}, expected 1)`)
    }

    // Replay: the button still resolves (same node identity) → click fires
    // exactly once, and the record is consumed.
    replayFor(host as unknown as ReplayElement)
    if (button.clicks !== 1) {
      stop()
      throw new Error(`the captured click replayed exactly once (clicks=${button.clicks}, expected 1)`)
    }
    if (replayPending() !== 0) {
      stop()
      throw new Error(`the replayed record was consumed (pending=${replayPending()}, expected 0)`)
    }

    // A second replay fires nothing — the record is gone, never double-fired.
    replayFor(host as unknown as ReplayElement)
    if (button.clicks !== 1) {
      stop()
      throw new Error(`a second replay never double-fires (clicks=${button.clicks}, expected 1)`)
    }
    stop()
    return { ok: true, detail: 'capture(1) → replay(1) → consume(0); second replay is a no-op' }
  },
}

export default c32

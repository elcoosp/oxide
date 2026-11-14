// Offline-wire micro-bench (V2-F14). Zero-dependency, Node-only (type
// stripping); NOT part of the CI test glob (`*.test.ts`) — run it
// directly: `node --experimental-strip-types test/enhance-offline.bench.ts`
// (Node ≥ 24 strips types without the flag).
//
// Reports ns/op (median of runs) for the two submit-failure paths the
// offline wire touches:
//   enqueue/path  — the offline probe → serialize → enqueue → event
//                   (the new path; the enqueue is a fake, so this
//                   measures the wire's own overhead, not IndexedDB)
//   error/path    — the unchanged failure path (no outbox configured):
//                   the baseline every wire addition must stay noise
//                   next to
// The wire adds one probe call and one Headers pass per failed submit —
// both must stay O(1) with no per-submit allocation beyond the record.
import { performance } from 'node:perf_hooks'
import { enhance, type EnhanceForm } from '../src/client/enhance.ts'

function fakeForm(): EnhanceForm & { dispatchLog: unknown[] } {
  const log: unknown[] = []
  return {
    addEventListener() {},
    removeEventListener() {},
    getAttribute(name: string) {
      return name === 'action' ? '/?/comment' : null
    },
    hasAttribute(name: string) {
      return name === 'action'
    },
    dispatchEvent(ev: { type: string; detail?: unknown }) {
      log.push(ev)
      return true
    },
    dispatchLog: log,
  } as EnhanceForm & { dispatchLog: unknown[] }
}

function urlEncodedForm(): FormData {
  const fd = new FormData()
  fd.set('body', 'nice post')
  return fd
}

const offlineFetch = (): Promise<Response> => Promise.reject(new TypeError('fetch failed'))

function median(xs: number[]): number {
  const s = [...xs].sort((a, b) => a - b)
  return s[Math.floor(s.length / 2)]!
}

const RUNS = 5
const ITERS = 2000

function bench(name: string, fn: () => void): void {
  const samples: number[] = []
  for (let r = 0; r < RUNS; r++) {
    const t0 = performance.now()
    for (let i = 0; i < ITERS; i++) fn()
    samples.push((performance.now() - t0) * 1e6 / ITERS)
  }
  console.log(`${name.padEnd(14)} ${median(samples).toFixed(0).padStart(6)} ns/op (median of ${RUNS} runs)`)
}

const queue = {
  enqueue: (it: unknown) => {
    void it
    return Promise.resolve('queued-id')
  },
}

// The wire's new path: probe says offline → serialize → enqueue →
// ox:form-queued. Each iteration is one full submit through `enhance`.
bench('enqueue/path', () => {
  const form = fakeForm()
  enhance(form, {
    fetch: offlineFetch as typeof fetch,
    getFormData: urlEncodedForm,
    outbox: queue,
    isOnline: () => false,
    onError: () => {},
  })
})

// The baseline: the same failed submit with no outbox armed — the path
// every previous enhance test pinned.
bench('error/path', () => {
  const form = fakeForm()
  enhance(form, {
    fetch: offlineFetch as typeof fetch,
    getFormData: urlEncodedForm,
    onError: () => {},
  })
})

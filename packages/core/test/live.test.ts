// §V2-F05 step 7 — the client live contract: connectLive opens the
// `/__oxide/live` EventSource with the route/url/build query (EventSource
// cannot set headers, so the build id travels in the query), applies
// decoded `data` snapshots in place, reloads on `stale`, and reconnects
// with jitter on transport errors — disposing cleanly.
//
// Written FIRST (TDD red): these pins failed against the missing
// `packages/core/src/client/live.ts` module. The router lifecycle wiring
// (connect after hydration, dispose on navigation) is deferred with the
// server endpoint wiring — see docs/deviations.md.

import { test, mock } from 'node:test'
import assert from 'node:assert/strict'
import { connectLive } from '../src/client/live.ts'
import { encodeLite } from './helpers/odf-lite.ts'

/** The bootstrap shape the render crate emits. */
function advertise(route: string, build?: string): () => void {
  const g = globalThis as { __oxide?: Record<string, unknown> }
  const prev = g.__oxide
  g.__oxide = { route, data: undefined, ...(build === undefined ? {} : { build }) }
  return () => {
    g.__oxide = prev
  }
}

/** A minimal EventSource double: records the URL, replays events on demand. */
class FakeEventSource {
  static instances: FakeEventSource[] = []
  static CLOSED = 2
  url: string
  closed = false
  onerror: ((ev?: unknown) => void) | null = null
  handlers = new Map<string, ((ev: { data?: string }) => void)[]>()
  constructor(url: string) {
    this.url = url
    FakeEventSource.instances.push(this)
  }
  addEventListener(type: string, cb: (ev: { data?: string }) => void): void {
    const list = this.handlers.get(type) ?? []
    list.push(cb)
    this.handlers.set(type, list)
  }
  removeEventListener(type: string, cb: (ev: { data?: string }) => void): void {
    this.handlers.set(type, (this.handlers.get(type) ?? []).filter((f) => f !== cb))
  }
  close(): void {
    this.closed = true
  }
  emit(type: string, data?: string): void {
    for (const cb of this.handlers.get(type) ?? []) cb({ data })
  }
  error(): void {
    this.onerror?.()
  }
}

function reset(): void {
  FakeEventSource.instances = []
}

test.afterEach(() => {
  reset()
  mock.timers.reset()
})

test('connectLive opens the spec URL with route, url and build in the query', () => {
  const restore = advertise('routes/blog/[slug]', 'deadbeef00000001')
  try {
    const g = globalThis as { location?: { pathname: string; search: string } }
    const prev = g.location
    g.location = { pathname: '/blog/rust', search: '?page=2' }
    try {
      connectLive('routes/blog/[slug]', () => {}, {
        EventSource: FakeEventSource as unknown as typeof EventSource,
      })
    } finally {
      g.location = prev
    }
    const es = FakeEventSource.instances[0]
    const url = new URL(es.url, 'https://oxide.test')
    assert.equal(url.pathname, '/__oxide/live')
    assert.equal(url.searchParams.get('route'), 'routes/blog/[slug]')
    assert.equal(url.searchParams.get('url'), '/blog/rust?page=2')
    assert.equal(url.searchParams.get('build'), 'deadbeef00000001')
  } finally {
    restore()
  }
})

test('data events decode the ODF snapshot and apply it in place', () => {
  const restore = advertise('routes/index')
  try {
    const seen: unknown[] = []
    connectLive('routes/index', (v) => seen.push(v), {
      EventSource: FakeEventSource as unknown as typeof EventSource,
    })
    const es = FakeEventSource.instances[0]
    const payload = Buffer.from(encodeLite({ views: 2 })).toString('base64')
    es.emit('data', payload)
    es.emit('data', Buffer.from(encodeLite({ views: 3 })).toString('base64'))
    assert.deepEqual(seen, [{ views: 2 }, { views: 3 }], 'each snapshot applies in order')
  } finally {
    restore()
  }
})

test('stale closes the source and reloads once', () => {
  const restore = advertise('routes/index', 'old')
  try {
    let reloads = 0
    connectLive('routes/index', () => {}, {
      EventSource: FakeEventSource as unknown as typeof EventSource,
      reload: () => {
        reloads += 1
      },
    })
    const es = FakeEventSource.instances[0]
    es.emit('stale', 'deadbeef00000002')
    assert.equal(es.closed, true, 'the stale event closes the stream')
    assert.equal(reloads, 1, 'one guarded reload (A04 client reloads once)')
  } finally {
    restore()
  }
})

test('errors close and reconnect with jitter inside 1-3s', () => {
  mock.timers.enable({ apis: ['setTimeout'] })
  const restore = advertise('routes/index')
  try {
    connectLive('routes/index', () => {}, {
      EventSource: FakeEventSource as unknown as typeof EventSource,
    })
    assert.equal(FakeEventSource.instances.length, 1)
    const es = FakeEventSource.instances[0]
    es.error()
    assert.equal(es.closed, true, 'the errored source is closed')
    mock.timers.tick(999)
    assert.equal(FakeEventSource.instances.length, 1, 'no reopen before the jitter floor (1s)')
    mock.timers.tick(2_001)
    assert.equal(FakeEventSource.instances.length, 2, 'a fresh source opens within the window')
    // The fresh connection carries the same query (the server replays a
    // full snapshot on connect — no client-side replay buffer needed).
    const fresh = FakeEventSource.instances[1]
    assert.equal(new URL(fresh.url, 'https://oxide.test').pathname, '/__oxide/live')
  } finally {
    restore()
  }
})

test('the disposer stops reconnection and closes the stream', () => {
  mock.timers.enable({ apis: ['setTimeout'] })
  const restore = advertise('routes/index')
  try {
    const dispose = connectLive('routes/index', () => {}, {
      EventSource: FakeEventSource as unknown as typeof EventSource,
    })
    const es = FakeEventSource.instances[0]
    dispose()
    assert.equal(es.closed, true, 'dispose closes the live source')
    es.error()
    mock.timers.tick(10_000)
    assert.equal(
      FakeEventSource.instances.length,
      1,
      'no reconnect after dispose',
    )
  } finally {
    restore()
  }
})

test('hidden for more than 30s closes the stream; visible reopens', () => {
  mock.timers.enable({ apis: ['setTimeout'] })
  const restore = advertise('routes/index')
  try {
    const listeners = new Map<string, () => void>()
    const fakeDoc = {
      visibilityState: 'visible',
      addEventListener: (type: string, cb: () => void) => void listeners.set(type, cb),
      removeEventListener: (type: string) => void listeners.delete(type),
    }
    connectLive('routes/index', () => {}, {
      EventSource: FakeEventSource as unknown as typeof EventSource,
      document: fakeDoc as unknown as Document,
    })
    assert.equal(FakeEventSource.instances.length, 1)
    const es = FakeEventSource.instances[0]

    fakeDoc.visibilityState = 'hidden'
    listeners.get('visibilitychange')?.()
    mock.timers.tick(30_000)
    assert.equal(es.closed, true, 'hidden for 30s closes the connection')

    fakeDoc.visibilityState = 'visible'
    listeners.get('visibilitychange')?.()
    assert.equal(FakeEventSource.instances.length, 2, 'visibility reconnects')
  } finally {
    restore()
  }
})

test('hidden briefly does not close the stream', () => {
  mock.timers.enable({ apis: ['setTimeout'] })
  const restore = advertise('routes/index')
  try {
    const listeners = new Map<string, () => void>()
    const fakeDoc = {
      visibilityState: 'visible',
      addEventListener: (type: string, cb: () => void) => void listeners.set(type, cb),
      removeEventListener: (type: string) => void listeners.delete(type),
    }
    connectLive('routes/index', () => {}, {
      EventSource: FakeEventSource as unknown as typeof EventSource,
      document: fakeDoc as unknown as Document,
    })
    const es = FakeEventSource.instances[0]
    fakeDoc.visibilityState = 'hidden'
    listeners.get('visibilitychange')?.()
    mock.timers.tick(5_000)
    fakeDoc.visibilityState = 'visible'
    listeners.get('visibilitychange')?.()
    mock.timers.tick(60_000)
    assert.equal(es.closed, false, 'a short hide never closes')
    assert.equal(FakeEventSource.instances.length, 1, 'no reconnect churn')
  } finally {
    restore()
  }
})

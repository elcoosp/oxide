// Mixed-framework island registry tests (§V2-F01): the mixed registry
// hydrates two fake adapters independently, the `adapter` shorthand
// keeps the one-adapter path working, and an unknown island marks the
// host with an error instead of hydrating the wrong module.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  hydrateMixedIsland,
  hydrateMixedIslands,
  type IslandHost,
  type MixedIslandRegistry,
} from '../src/client/islands.ts'
import { installBootRuntime } from '../src/client/boot.ts'
import { encodeLite } from './helpers/odf-lite.ts'

function islandEl(attrs: Record<string, string>) {
  return {
    attrs: { 'data-ox-strategy': 'load', ...attrs } as Record<string, string>,
    getAttribute(n: string): string | null {
      const v = this.attrs[n]
      return v === undefined ? null : v
    },
    hasAttribute(n: string) {
      return n in this.attrs
    },
    setAttribute(n: string, v: string) {
      this.attrs[n] = v
    },
  }
}

type FakeHost = IslandHost & Record<string, unknown>

function fakeAdapter(tag: string) {
  const calls: Array<{ el: unknown; mod: unknown; props: unknown }> = []
  return {
    calls,
    clientAdapter: {
      hydrateIsland(el: unknown, mod: unknown, props: unknown) {
        calls.push({ el, mod, props })
        return () => {}
      },
    },
    tag,
  }
}

const b64 = (v: unknown) => Buffer.from(encodeLite(v)).toString('base64')

function mixedRegistry(
  adapters: Record<string, ReturnType<typeof fakeAdapter>>,
  islands: Record<string, { fw: string; mod: unknown }>,
): MixedIslandRegistry {
  return {
    adapters: Object.fromEntries(
      Object.entries(adapters).map(([fw, a]) => [fw, async () => ({ clientAdapter: a.clientAdapter })]),
    ),
    islands: Object.fromEntries(
      Object.entries(islands).map(([name, e]) => [
        name,
        { fw: e.fw, load: async () => e.mod },
      ]),
    ),
  }
}

const NO_SCHED = {
  requestIdleCallback: (cb: () => void) => cb(),
  matchMedia: () => ({ matches: false, addEventListener() {}, removeEventListener() {} }),
}

test('mixed registry: two fake adapters hydrate independently (§V2-F01)', async () => {
  const react = fakeAdapter('react')
  const vue = fakeAdapter('vue')
  const reg = mixedRegistry(
    { react, vue },
    {
      Counter: { fw: 'react', mod: { default: 'counter-module' } },
      Price: { fw: 'vue', mod: { default: 'price-module' } },
    },
  )
  const counter = islandEl({
    'data-ox-island': 'Counter',
    'data-ox-props': b64({ label: 'hi', start: 1 }),
  }) as FakeHost
  const price = islandEl({
    'data-ox-island': 'Price',
    'data-ox-props': b64({ label: 'there', start: 2 }),
  }) as FakeHost

  await hydrateMixedIsland(counter, reg)
  await hydrateMixedIsland(price, reg)

  assert.equal(react.calls.length, 1, 'the react adapter hydrated once')
  assert.equal(vue.calls.length, 1, 'the vue adapter hydrated once')
  const counterCall = react.calls[0]
  const priceCall = vue.calls[0]
  if (!counterCall || !priceCall) throw new Error('both adapters must record a call')
  assert.deepEqual(counterCall.mod, { default: 'counter-module' })
  assert.deepEqual(counterCall.props, { label: 'hi', start: 1 })
  assert.deepEqual(priceCall.mod, { default: 'price-module' })
  assert.deepEqual(priceCall.props, { label: 'there', start: 2 })
  assert.equal(counter.getAttribute('data-ox-hydrated'), 'done')
  assert.equal(price.getAttribute('data-ox-hydrated'), 'done')
})

test('mixed registry: an unknown island marks an error, no adapter runs', async () => {
  const react = fakeAdapter('react')
  const reg = mixedRegistry({ react }, {})
  const el = islandEl({ 'data-ox-island': 'Ghost' }) as FakeHost
  await hydrateMixedIsland(el, reg)
  assert.equal(el.getAttribute('data-ox-hydrated'), 'error')
  assert.match(el.getAttribute('data-ox-error') ?? '', /unknown island Ghost/)
  assert.equal(react.calls.length, 0)
})

test('mixed registry: the scheduled scan dispatches by strategy', async () => {
  const react = fakeAdapter('react')
  const reg = mixedRegistry({ react }, { Counter: { fw: 'react', mod: {} } })
  const el = islandEl({ 'data-ox-island': 'Counter' }) as FakeHost
  const doc = { querySelectorAll: () => [el], addEventListener() {}, removeEventListener() {} }
  const dispose = hydrateMixedIslands(reg, { ...NO_SCHED, doc })
  await new Promise((r) => setTimeout(r, 1))
  assert.equal(el.getAttribute('data-ox-hydrated'), 'done')
  dispose()
})

test('installBootRuntime keeps the one-adapter path working (the adapter shorthand)', () => {
  // Non-browser environment: the call is a no-op (the boot window is
  // absent) — the pin is that the shorthand signature typechecks and
  // the no-argument legacy call still works.
  installBootRuntime()
  installBootRuntime({ adapter: { hydrate() { return () => {} } }, fw: 'react' })
  installBootRuntime({
    adapters: { react: async () => ({ clientAdapter: { hydrate() { return () => {} } } }) },
    islands: { A: { fw: 'react', load: async () => ({}) } },
  })
})

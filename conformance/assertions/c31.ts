// §V2-R01 C31 — four-framework mixed islands hydrate independently (F01).
//
// Pins `packages/core/src/client/islands.ts::hydrateMixedIslands`: a host
// page carrying four `<ox-island>` hosts — one per framework — hydrates each
// through its OWN framework adapter (the named `fw` picks the adapter), and
// an unknown island marks `data-ox-hydrated: error` instead of hydrating
// through a wrong adapter. The host page is react-rendered (the spec's
// "react host page"); the islands are the four frameworks.

import { hydrateMixedIslands, type IslandsAdapter } from '../../packages/core/src/client/islands.ts'
import { createFakeDom } from '../../packages/react/test/helpers/fake-dom.ts'
import type { Assertion } from './types.ts'

const c31: Assertion = {
  id: 'C31',
  name: 'four-framework mixed islands hydrate independently (F01)',
  capability: 'mixed-islands',
  adapters: ['react'],
  targets: ['binary', 'node'],
  surface: 'client',
  async run() {
    // A host page with one island per framework + a registry-unknown ghost.
    const dom = createFakeDom(
      `<body>` +
        `<ox-island data-ox-island="react-counter" data-ox-chunk="/r.js" data-ox-strategy="load"></ox-island>` +
        `<ox-island data-ox-island="vue-counter" data-ox-chunk="/v.js" data-ox-strategy="load"></ox-island>` +
        `<ox-island data-ox-island="svelte-counter" data-ox-chunk="/s.js" data-ox-strategy="load"></ox-island>` +
        `<ox-island data-ox-island="solid-counter" data-ox-chunk="/o.js" data-ox-strategy="load"></ox-island>` +
        `<ox-island data-ox-island="ghost" data-ox-chunk="/g.js" data-ox-strategy="load"></ox-island>` +
        `</body>`,
    )
    const hosts = dom.doc.querySelectorAll('ox-island')
    if (hosts.length !== 5) {
      throw new Error(`the host page carries four framework islands + one ghost (got ${hosts.length})`)
    }

    // Each framework's client adapter records the island it hydrated. The
    // mixed registry dispatches by `fw`, so each island reaches its OWN
    // framework's adapter — never a wrong one.
    const dispatched: Array<{ island: string; fw: string }> = []
    const adapterFor = (fw: string): IslandsAdapter => ({
      hydrateIsland(el) {
        const name = el.getAttribute?.('data-ox-island') ?? ''
        dispatched.push({ island: name, fw })
        return undefined
      },
    })

    const dispose = hydrateMixedIslands(
      {
        adapters: {
          react: async () => ({ clientAdapter: adapterFor('react') }),
          vue: async () => ({ clientAdapter: adapterFor('vue') }),
          svelte: async () => ({ clientAdapter: adapterFor('svelte') }),
          solid: async () => ({ clientAdapter: adapterFor('solid') }),
        },
        islands: {
          'react-counter': { fw: 'react', load: async () => ({}) },
          'vue-counter': { fw: 'vue', load: async () => ({}) },
          'svelte-counter': { fw: 'svelte', load: async () => ({}) },
          'solid-counter': { fw: 'solid', load: async () => ({}) },
        },
      },
      { doc: dom.doc },
    )
    // One microtask tick for the async adapter + chunk loads (4 concurrent
    // hydrations under the full registry run — give the event loop room).
    await new Promise((r) => setTimeout(r, 20))
    dispose()

    // Each framework island hydrated through its OWN adapter — four
    // dispatches, one per fw, matched.
    if (dispatched.length !== 4) {
      throw new Error(`four framework islands hydrated independently (dispatched=${dispatched.length}, expected 4)`)
    }
    const byIsland = new Map(dispatched.map((d) => [d.island, d.fw]))
    const expected: Record<string, string> = {
      'react-counter': 'react',
      'vue-counter': 'vue',
      'svelte-counter': 'svelte',
      'solid-counter': 'solid',
    }
    for (const [island, fw] of Object.entries(expected)) {
      if (byIsland.get(island) !== fw) {
        throw new Error(`${island} hydrated through its own adapter ${fw} (got ${byIsland.get(island)})`)
      }
    }
    // The ghost never hydrated through a wrong adapter — it marks an error.
    const ghost = hosts[4]
    if (!ghost || ghost.getAttribute('data-ox-hydrated') !== 'error') {
      throw new Error(`the unknown island marks data-ox-hydrated=error (got ${ghost?.getAttribute('data-ox-hydrated')})`)
    }
    if (dispatched.some((d) => d.island === 'ghost')) {
      throw new Error('an unknown island never reaches a framework adapter (wrong hydration)')
    }
    return { ok: true, detail: `4 frameworks × independent dispatch; ghost → error (no wrong hydration)` }
  },
}

export default c31

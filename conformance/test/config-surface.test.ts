// Config-surface conformance (spec §14 + §6.4 + §7). Every adapter must
// expose a callable default export that:
//   (a) with no options returns an adapter byte-identical to the named
//       `adapter` export on the home route (IR contract preserved),
//   (b) with `ssr: 'real'` and NO componentFor still selects the real
//       renderer — components come from the runtime resolver (§6.4), and
//   (c) the smoke apps' configs contain no component imports/closure: the
//       §14.1 literal shape is the only supported one.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import reactDefault, { adapter as react, createAdapter as reactCreate } from '../../packages/react/src/index.ts'
import vueDefault, { adapter as vue, createAdapter as vueCreate } from '../../packages/vue/src/index.ts'
import svelteDefault, { adapter as svelte, createAdapter as svelteCreate } from '../../packages/svelte/src/index.ts'
import solidDefault, { adapter as solid, createAdapter as solidCreate } from '../../packages/solid/src/index.ts'
import { atlasRoutes as atlas, asRenderInput, expect as E } from '../atlas/atlas.ts'
import type { FrameworkAdapter } from '../../packages/oxide/src/types.ts'

interface Bundle {
  name: string
  adapter: FrameworkAdapter
  createAdapter: (opts?: Record<string, unknown>) => FrameworkAdapter
  defaultFactory: (opts?: Record<string, unknown>) => FrameworkAdapter
}

const BUNDLES: Bundle[] = [
  { name: 'react', adapter: react, createAdapter: reactCreate as never, defaultFactory: reactDefault as never },
  { name: 'vue', adapter: vue, createAdapter: vueCreate as never, defaultFactory: vueDefault as never },
  { name: 'svelte', adapter: svelte, createAdapter: svelteCreate as never, defaultFactory: svelteDefault as never },
  { name: 'solid', adapter: solid, createAdapter: solidCreate as never, defaultFactory: solidDefault as never },
]

async function drain(stream: ReadableStream<Uint8Array>): Promise<string> {
  const chunks: string[] = []
  for await (const c of stream) chunks.push(new TextDecoder().decode(c))
  return chunks.join('')
}

for (const b of BUNDLES) {
  test(`config: ${b.name} default export is callable`, () => {
    const a = b.defaultFactory()
    assert.equal(a.contract, 1, 'contract 1')
    assert.equal(a.name, b.adapter.name, 'name matches the named `adapter` export')
  })

  test(`config: ${b.name} createAdapter() returns byte-identical IR output`, async () => {
    const a = b.createAdapter()
    const viaFactory = await drain(a.render(asRenderInput(atlas.home)))
    const viaNamed = await drain(b.adapter.render(asRenderInput(atlas.home)))
    assert.equal(viaFactory, viaNamed, 'factory with no opts == named adapter byte-for-byte')
  })

  test(`config: ${b.name} real SSR needs no componentFor (resolver supplies components)`, async () => {
    const a = b.createAdapter({ ssr: 'real' })
    const html = await drain(a.render(asRenderInput({ ...atlas.home, route: 'routes/missing' })))
    assert.ok(
      html.includes('no component resolved for route routes/missing'),
      `missing-page diagnostic emitted: ${html.slice(0, 160)}`,
    )
    assert.ok(
      html.includes('<!--oxide-status:500-->') || html.includes("data-ox-error='OX-500'"),
      `missing page is a 500 diagnostic, not a silent skip: ${html.slice(0, 160)}`,
    )
  })

  test(`config: ${b.name} createAdapter() with an explicit resolver renders that route`, async () => {
    const a = b.createAdapter({ ssr: 'real' })
    const calls: string[] = []
    const html = await drain(
      a.render(
        asRenderInput({
          ...atlas.home,
          route: 'routes/probe',
          // The runtime attaches `resolve`; a test supplies its own.
          resolve: (id) => {
            calls.push(id)
            return undefined
          },
        }),
      ),
    )
    assert.ok(calls.length > 0, 'adapter consulted input.resolve')
    assert.equal(typeof html, 'string')
  })

  test(`smoke configs: ${b.name} is literal (no component imports/closure)`, () => {
    const cfg = readFileSync(
      new URL(`../../examples/smoke-app-${b.name}/oxide.config.ts`, import.meta.url),
      'utf8',
    )
    assert.ok(!/\.(tsx|jsx|vue|svelte)['"]/.test(cfg), `${b.name} config imports no component files`)
    assert.ok(!/componentFor/.test(cfg), `${b.name} config has no componentFor closure`)
  })

  test(`config: ${b.name} default export accepts the smoke-app {compiler:true} shape`, () => {
    const a = b.defaultFactory({ compiler: true })
    assert.equal(a.contract, 1)
  })
}

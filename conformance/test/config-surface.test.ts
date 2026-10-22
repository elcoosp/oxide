// Config-surface conformance (spec §14 + §6.4 + §7). Every adapter must
// expose a callable default export that:
//   (a) with `ssr: 'ir'` returns an adapter byte-identical to the named
//       `adapter` export on the home route (the explicit IR escape hatch),
//   (b) with no `ssr` field defaults to real SSR (§14.1's literal example) —
//       a missing route must emit the real adapter's OX-500 diagnostic,
//   (c) with `ssr: 'real'` and NO componentFor still selects the real
//       renderer — components come from the runtime resolver (§6.4), and
//   (d) the smoke apps' configs contain no component imports/closure: the
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

  test(`config: ${b.name} createAdapter({ ssr: 'ir' }) returns byte-identical IR output`, async () => {
    // The explicit IR escape hatch — pre-§14.1 behavior, opt-in.
    const a = b.createAdapter({ ssr: 'ir' })
    const viaFactory = await drain(a.render(asRenderInput(atlas.home)))
    const viaNamed = await drain(b.adapter.render(asRenderInput(atlas.home)))
    assert.equal(viaFactory, viaNamed, 'ssr:ir == named adapter byte-for-byte')
  })

  test(`config: ${b.name} createAdapter() defaults to real SSR — §14.1 literal`, async () => {
    // §14.1's literal example is `react({ compiler: true })` with no `ssr`
    // field. Pre-fix this fell through to the IR adapter. Post-fix it
    // routes through the real adapter.
    //
    // Observable proof: only the real adapter consults `input.resolve`
    // — the runtime-supplied resolver. The IR adapter never calls it.
    // (In `auto` mode, real-ssr still falls back to IR when `resolve`
    // returns nothing and the framework deps are absent, so the bytes
    // are expected to match — that fallback is the spec's design.)
    const a = b.createAdapter({ compiler: true })
    const calls: string[] = []
    await drain(
      a.render(
        asRenderInput({
          ...atlas.home,
          // @ts-expect-error — test attaches the resolver the runtime would.
          resolve: (id: string) => {
            calls.push(id)
            return undefined
          },
        }),
      ),
    )
    assert.ok(
      calls.length > 0,
      `${b.name} default reached real SSR (resolve consulted: ${calls.length} call(s))`,
    )
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
    // Narrow to imports: an islands registry string ('src/islands/counter.tsx')
    // legitimately names a component file for the CLI to bundle — the
    // prohibition is against a config-side *import*, the pattern that made
    // the pre-§6.4 model evaluate the component graph on every render.
    assert.ok(
      !/^\s*import\b.*from\s+['"][^'"]+\.(tsx|jsx|vue|svelte)['"]/m.test(cfg),
      `${b.name} config imports no component files (islands strings are allowed)`,
    )
    assert.ok(!/componentFor/.test(cfg), `${b.name} config has no componentFor closure`)
  })

  test(`config: ${b.name} default export accepts the smoke-app {compiler:true} shape`, () => {
    const a = b.defaultFactory({ compiler: true })
    assert.equal(a.contract, 1)
  })
}

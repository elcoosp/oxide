// `<Island>` (svelte) — the Svelte adapter's island wrapper (§V2-F01): the
// twin of `packages/react/src/island.ts`, emitting the exact `<ox-island>`
// shape the render crate emits and the client boot script scans for, and the
// self-contained pending placeholder for a FOREIGN island (the worker's
// `island:render` op + the core `resolveForeign` transform resolve it with
// the island's own adapter).
//
// The package ships `src/Island.svelte` as source; a consumer's Svelte
// compiler builds it. This test does that compile itself (the same
// `svelte/compiler` path `defer-component.test.ts` uses) and asserts the SSR
// output. A wrapper is compiled alongside so the children *snippet* can be
// supplied from markup (Svelte 5 snippets cannot be constructed from plain
// TS). Both compiled modules live in one temp dir under the package so
// `svelte/internal/server` resolves — a `/tmp` path cannot see the package's
// node_modules. Assertions tolerate Svelte 5's SSR hydration comment markers.
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { compile } from 'svelte/compiler'
import type { Component } from 'svelte'
import { render } from 'svelte/server'
import { readFileSync, writeFileSync, mkdirSync, rmSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { encodePropsB64 } from '../../core/src/odf-encode.ts'

const here = dirname(fileURLToPath(import.meta.url))
const TMP = join(here, 'island-compiled')

process.on('exit', () => {
  try { rmSync(TMP, { recursive: true, force: true }) } catch { /* best-effort */ }
})

async function build(): Promise<Component<Record<string, unknown>, Record<string, unknown>, string>> {
  rmSync(TMP, { recursive: true, force: true })
  mkdirSync(TMP, { recursive: true })

  const islandSrc = readFileSync(join(here, '../src/Island.svelte'), 'utf8')
  const islandOut = compile(islandSrc, {
    generate: 'server',
    name: 'Island',
    filename: 'Island.svelte',
  })
  writeFileSync(join(TMP, 'Island.mjs'), islandOut.js.code)

  const wrapperSrc = `<script>
  import Island from './Island.mjs'
  let { islandProps = {}, children } = $props()
</script>

<Island {...islandProps}>{#snippet children()}<span class="child">ssr</span>{/snippet}</Island>
`
  const wrapperOut = compile(wrapperSrc, {
    generate: 'server',
    name: 'Wrapper',
    filename: 'Wrapper.svelte',
  })
  writeFileSync(join(TMP, 'Wrapper.mjs'), wrapperOut.js.code)

  const mod = (await import(pathToFileURL(join(TMP, 'Wrapper.mjs')).href)) as { default: Component<Record<string, unknown>, Record<string, unknown>, string> }
  return mod.default
}

const Wrapper = await build()

test('Island (svelte) emits the render-crate ox-island shape with children', () => {
  const { body } = render(Wrapper, {
    props: {
      islandProps: {
        component: 'Counter',
        chunk: '/islands/counter.js',
        strategy: 'load',
        props: { initial: 3 },
      },
    },
  })
  // The `{#if}` branch wrapper emits Svelte 5's hydration comment markers
  // before the element; the defer-component test tolerates them too.
  assert.match(body, /<ox-island/)
  assert.match(body, /data-ox-island="Counter"/)
  assert.match(body, /data-ox-chunk="\/islands\/counter\.js"/)
  assert.match(body, /data-ox-strategy="load"/)
  const want = encodePropsB64({ initial: 3 }).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  assert.match(body, new RegExp(`data-ox-props="${want}"`))
  assert.match(body, /<span[^>]*class="child"[^>]*>ssr<\/span>/)
})

test('Island (svelte) default strategy is "load"', () => {
  const { body } = render(Wrapper, {
    props: { islandProps: { component: 'X', chunk: '/x.js' } },
  })
  assert.match(body, /data-ox-strategy="load"/)
})

test('Island (svelte) media strategy serializes as media(...)', () => {
  const { body } = render(Wrapper, {
    props: {
      islandProps: { component: 'X', chunk: '/x.js', strategy: { media: '(min-width: 800px)' } },
    },
  })
  assert.match(body, /data-ox-strategy="media\(min-width: 800px\)"/)
})

test('Island (svelte) emits the pending placeholder for a foreign framework island', () => {
  const { body } = render(Wrapper, {
    props: {
      islandProps: {
        component: 'Price',
        chunk: '/islands/Price.vue',
        framework: 'vue',
        props: { a: 1 },
      },
    },
  })
  assert.match(body, /<ox-island/)
  assert.match(body, /data-ox-island="Price"/)
  assert.match(body, /data-ox-fw="vue"/)
  assert.match(body, /data-ox-pending="1"/)
  assert.doesNotMatch(body, /data-ox-chunk/, 'the foreign placeholder carries no chunk attr')
  assert.doesNotMatch(body, /data-ox-strategy/, 'the foreign placeholder carries no strategy attr')
  assert.doesNotMatch(body, /class="child"/, 'foreign children are not rendered by this adapter')
  const want = encodePropsB64({ a: 1 }).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  assert.match(body, new RegExp(`data-ox-props="${want}"`))
})

test('Island (svelte) framework matching the page framework renders the local shape', () => {
  const { body } = render(Wrapper, {
    props: {
      islandProps: {
        component: 'Counter',
        chunk: '/islands/counter.js',
        framework: 'svelte',
        props: { initial: 3 },
      },
    },
  })
  assert.match(body, /data-ox-chunk="\/islands\/counter\.js"/)
  assert.doesNotMatch(body, /data-ox-pending/, 'a local island is never pending')
  assert.match(body, /class="child"/)
})

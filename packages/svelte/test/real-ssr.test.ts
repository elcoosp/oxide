// @oxide/svelte real Svelte SSR tests (spec §7). Skipped when svelte/server
// is not resolvable.
//
// Svelte 5 SSR needs a COMPILED component. These tests compile a tiny inline
// SFC via `svelte/compiler`, evaluate the emitted module through a data: URL
// so no temp files are needed, then render through `svelte/server`.

import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  isSvelteAvailable, getSvelteServer, renderSvelteToString,
} from '../src/real-ssr.ts'

const available = await isSvelteAvailable()
const skip = !available

interface SvelteCompilerLike {
  compile(src: string, opts: { generate: string; filename: string }): { js: { code: string } }
}

async function compileInline(source: string, filename = 'Test.svelte'): Promise<unknown> {
  const compiler = (await import('svelte/compiler')) as unknown as SvelteCompilerLike
  const out = compiler.compile(source, { generate: 'server', filename })
  const b64 = Buffer.from(out.js.code, 'utf8').toString('base64')
  const mod = await import(`data:text/javascript;base64,${b64}`)
  return (mod as { default: unknown }).default
}

test('real SSR: svelte/server is resolvable', { skip }, async () => {
  assert.equal(await isSvelteAvailable(), true)
  const server = await getSvelteServer()
  assert.ok(server)
  assert.equal(typeof server.render, 'function')
})

test('real SSR: renders a compiled Svelte component with props', { skip }, async () => {
  const Component = await compileInline(
    `<script>let { name = 'world' } = $props();</script>\n<h1 class="greeting">Hello {name}</h1>\n`,
  )
  const html = await renderSvelteToString(Component, { name: 'Svelte' })
  assert.ok(html.includes('Hello Svelte'), html)
  assert.ok(html.includes('class="greeting"'), html)
})

test('real SSR: escapes interpolated text content', { skip }, async () => {
  const Component = await compileInline(`<p>{'<script>x</script>'}</p>\n`)
  const html = await renderSvelteToString(Component, {})
  assert.ok(html.includes('&lt;script&gt;'), html)
  assert.ok(!html.includes('<script>x</script>'), html)
})

test('real SSR: deterministic across identical renders', { skip }, async () => {
  const Component = await compileInline(`<div>stable</div>\n`)
  const a = await renderSvelteToString(Component, {})
  const b = await renderSvelteToString(Component, {})
  assert.equal(a, b)
})
